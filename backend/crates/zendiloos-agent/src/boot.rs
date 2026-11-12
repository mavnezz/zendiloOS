//! Which kernel GRUB starts (docs/architecture.md, 4.10).
//!
//! DKMS builds ZFS for each kernel. Should that fail for a new one, a
//! restart into it would come up without the pools. So GRUB's default is
//! the entry zendiloOS saves in GRUB's environment block (`GRUB_DEFAULT=saved`
//! from `/etc/default/grub.d/zendiloos.cfg`): the newest kernel that has
//! its ZFS module, and a newer one only once it has its module too. This
//! binary sets it as `choose-boot-kernel`, outside the agent's sandbox
//! (which keeps `/boot` read only): from the package's hooks after a
//! kernel or its headers were installed or a kernel removed
//! (`/etc/kernel/*.d/zz-zendiloos`, which run after DKMS's and GRUB's
//! own), and in every run of the package runner, a failed one too.
//!
//! The entry is pinned by its id as Debian's `10_linux` names it,
//! `gnulinux-advanced-<uuid>>gnulinux-<kernel>-advanced-<uuid>`, never the
//! first one ("Debian GNU/Linux"), which starts whatever kernel is newest
//! the next time GRUB's configuration is written. A kernel removed takes
//! its entry along, and GRUB would then start the first; so the hook after
//! a removal chooses again.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zendiloos_agent_proto::{BootKernel, BootReason};

/// Where GRUB and the kernels' modules are, and the tools: Debian's own,
/// or stand-ins in tests.
#[derive(Debug, Clone)]
pub(crate) struct Boot {
    /// `/boot`, with GRUB's configuration and environment block in `grub/`.
    pub(crate) boot: PathBuf,
    /// The kernels' modules, `/lib/modules`.
    pub(crate) modules: PathBuf,
    pub(crate) grub_editenv: PathBuf,
    pub(crate) dpkg: PathBuf,
}

impl Default for Boot {
    fn default() -> Self {
        Self {
            boot: "/boot".into(),
            modules: "/lib/modules".into(),
            grub_editenv: "/usr/bin/grub-editenv".into(),
            dpkg: "/usr/bin/dpkg".into(),
        }
    }
}

/// A kernel GRUB's configuration can start, and the path of its entry as
/// `saved_entry` takes it: the ids of the submenus it is in and its own,
/// joined by `>`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kernel: String,
    path: String,
}

impl Boot {
    /// Sets GRUB's default to the newest kernel that has its ZFS module (or
    /// the newest, when none has), and says which and why.
    pub(crate) fn choose(&self) -> BootKernel {
        let not = |reason, detail: String| BootKernel { kernel: None, reason, detail: Some(detail) };
        let config = self.boot.join("grub/grub.cfg");
        let text = match fs::read_to_string(&config) {
            Ok(text) => text,
            Err(error) => return not(BootReason::NotGrub, format!("{}: {error}", config.display())),
        };
        if !self.grub_editenv.exists() {
            return not(BootReason::NotGrub, format!("{} is not installed", self.grub_editenv.display()));
        }
        let entries = entries(&text);
        let newest = match self.newest(entries.iter()) {
            Ok(Some(newest)) => newest,
            Ok(None) => {
                return not(BootReason::NotGrub, format!("{} names no kernel of Debian's", config.display()));
            }
            Err(error) => return not(BootReason::Failed, error),
        };
        let with_zfs = self.newest(entries.iter().filter(|entry| has_zfs(&self.modules.join(&entry.kernel))));
        let (entry, reason) = match with_zfs {
            Ok(Some(entry)) if entry.kernel == newest.kernel => (entry, BootReason::Newest),
            Ok(Some(entry)) => (entry, BootReason::NewestLacksZfs),
            Ok(None) => (newest, BootReason::NoZfs),
            Err(error) => return not(BootReason::Failed, error),
        };
        if let Err(error) = self.save(&entry.path) {
            return not(BootReason::Failed, error);
        }
        if !follows_saved_entry(&text) {
            return not(
                BootReason::NotSaved,
                format!("{} does not start the saved entry; `update-grub` writes it anew", config.display()),
            );
        }
        BootKernel { kernel: Some(entry.kernel.clone()), reason, detail: None }
    }

    /// The newest of `entries`, by dpkg's order of versions.
    fn newest<'a>(&self, entries: impl Iterator<Item = &'a Entry>) -> Result<Option<&'a Entry>, String> {
        let mut newest: Option<&Entry> = None;
        for entry in entries {
            newest = match newest {
                Some(current) if !newer(&self.dpkg, &entry.kernel, &current.kernel)? => Some(current),
                _ => Some(entry),
            };
        }
        Ok(newest)
    }

    /// Saves `path` as GRUB's default, unless it is already.
    fn save(&self, path: &str) -> Result<(), String> {
        let environment = self.boot.join("grub/grubenv");
        if saved_entry(&fs::read_to_string(&environment).unwrap_or_default()).as_deref() == Some(path) {
            return Ok(());
        }
        let output = Command::new(&self.grub_editenv)
            .arg(&environment)
            .arg("set")
            .arg(format!("saved_entry={path}"))
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("grub-editenv cannot be started: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let complaint = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(format!("grub-editenv failed ({}): {complaint}", output.status))
        }
    }
}

/// `choose-boot-kernel`, from the package's kernel hooks and its
/// maintainer scripts: chooses, and says what; fails when the choice is
/// not in force.
pub fn choose_boot_kernel() -> Result<String, String> {
    let chosen = Boot::default().choose();
    match chosen.reason {
        BootReason::NotSaved | BootReason::Failed => Err(said(&chosen)),
        _ => Ok(said(&chosen)),
    }
}

/// What was chosen, in a line for the journal.
pub(crate) fn said(chosen: &BootKernel) -> String {
    let kernel = chosen.kernel.as_deref().unwrap_or_default();
    let detail = chosen.detail.as_deref().unwrap_or_default();
    match chosen.reason {
        BootReason::Newest => format!("GRUB starts {kernel}, the newest kernel, which has its ZFS module"),
        BootReason::NewestLacksZfs => {
            format!(
                "GRUB keeps starting {kernel}: a newer kernel has no ZFS module yet (`dkms autoinstall` builds it)"
            )
        }
        BootReason::NoZfs => format!("GRUB starts {kernel}, the newest kernel; no kernel has a ZFS module"),
        BootReason::NotGrub => format!("no kernel chosen: {detail}"),
        BootReason::NotSaved | BootReason::Failed => format!("GRUB's default is not set: {detail}"),
    }
}

/// `a` is a newer version than `b`, as dpkg orders them.
pub(crate) fn newer(dpkg: &Path, a: &str, b: &str) -> Result<bool, String> {
    let status = Command::new(dpkg)
        .args(["--compare-versions", a, "gt", b])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("dpkg: {e}"))?;
    Ok(status.success())
}

/// A `zfs.ko*` somewhere below the kernel's modules (DKMS puts it in
/// `updates/dkms`).
pub(crate) fn has_zfs(kernel: &Path) -> bool {
    fn walk(folder: &Path, depth: usize) -> bool {
        let Ok(entries) = fs::read_dir(folder) else { return false };
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "zfs.ko" || name.starts_with("zfs.ko.") {
                return true;
            }
            depth > 0 && entry.file_type().is_ok_and(|kind| kind.is_dir()) && walk(&entry.path(), depth - 1)
        })
    }
    walk(kernel, 6)
}

/// A block of GRUB's configuration, open at a line.
enum Block {
    Submenu(String),
    Entry {
        path: String,
        kernel: Option<String>,
    },
    /// A function's, say.
    Other,
}

/// The entries of Debian's own kernels in GRUB's configuration, each in
/// the one form that starts that very kernel, whatever is newest: the
/// entry `10_linux` makes for it among the advanced options (and on the
/// top level with `GRUB_DISABLE_SUBMENU`), not its recovery mode, not the
/// first entry, not another system's that os-prober found.
fn entries(config: &str) -> Vec<Entry> {
    let mut found = Vec::new();
    let mut open: Vec<Block> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let words = words(line);
        let Some(first) = words.first() else { continue };
        if line == "}" {
            if let Some(Block::Entry { path, kernel: Some(kernel) }) = open.pop() {
                let id = path.rsplit('>').next().unwrap_or_default();
                if id.starts_with(&format!("gnulinux-{kernel}-advanced-")) {
                    found.push(Entry { kernel, path });
                }
            }
            continue;
        }
        if !line.ends_with('{') {
            if let (Some(Block::Entry { kernel, .. }), "linux" | "linuxefi") =
                (open.last_mut(), first.as_str())
                && let Some(image) = words.get(1)
            {
                let file = image.rsplit('/').next().unwrap_or_default();
                // A name of a kernel's, which `/lib/modules` is looked into by.
                *kernel = file.strip_prefix("vmlinuz-").filter(|name| kernel_name(name)).map(str::to_owned);
            }
            continue;
        }
        let id = words
            .windows(2)
            .find(|pair| pair[0] == "$menuentry_id_option")
            .map(|pair| pair[1].clone())
            // Without ids, the titles name the entries.
            .or_else(|| words.get(1).cloned())
            .unwrap_or_default();
        let within: Vec<&str> = open
            .iter()
            .filter_map(|block| match block {
                Block::Submenu(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let path = within.iter().copied().chain([id.as_str()]).collect::<Vec<_>>().join(">");
        open.push(match first.as_str() {
            "submenu" => Block::Submenu(id),
            "menuentry" => Block::Entry { path, kernel: None },
            _ => Block::Other,
        });
    }
    found
}

/// Whether `name` is the version of a kernel: no folder of its own, no
/// way out of one.
fn kernel_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+~-".contains(&b))
}

/// Whether GRUB's configuration starts the saved entry, as `00_header`
/// writes it for `GRUB_DEFAULT=saved`.
fn follows_saved_entry(config: &str) -> bool {
    config.lines().any(|line| line.trim() == "set default=\"${saved_entry}\"")
}

/// `saved_entry` in GRUB's environment block.
fn saved_entry(environment: &str) -> Option<String> {
    environment.lines().find_map(|line| line.strip_prefix("saved_entry=")).map(str::to_owned)
}

/// A line of GRUB's configuration split into words as GRUB's shell does:
/// quotes taken away, a comment left out.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.extend(chars.next());
            }
            '#' if !in_word => break,
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

#[cfg(test)]
pub(crate) mod tests;
