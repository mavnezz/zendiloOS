//! Debian packages through apt, and zendiloOS's own releases
//! (docs/architecture.md, 4.10).
//!
//! apt writes all over the system and fetches over the network; the agent
//! has neither. So it starts a unit of its own, `zendiloos-packages.service`
//! (one at a time), which runs this binary as `run-packages <id> <task>`
//! outside the sandbox. The runner records each step in
//! `/var/lib/zendiloos-packages/run.json`, which the agent reads back; a run
//! outlives a restart of the agent or of zendiloosd, which matters when it
//! installs zendiloOS itself.
//!
//! A release of zendiloOS is installed only after the runner copied its
//! package, `SHA256SUMS` and `SHA256SUMS.minisig` out of the folder
//! zendiloosd downloaded them to, checked the signature with the release key
//! the package brought along (`/usr/share/zendiloos/release-signing.pub`,
//! or one in `/etc/zendiloos`), the package's checksum, name, architecture
//! and version, and only a newer version than the installed one: an old
//! signed release cannot be brought back.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    PackageReport, PackageRun, PackageRunState, PackageTask, ReleasePackage, Response, UpgradablePackage,
};
use zendiloos_exec::{Limits, Program};

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// The unit a run of apt goes in.
const UNIT: &str = "zendiloos-packages.service";
const STATE_FILE: &str = "run.json";
const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
/// Lines of apt's output kept for a failure.
const TAIL: usize = 15;
/// The files of a release beside its package.
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.minisig";
/// Largest files of a release taken in.
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;

#[derive(Debug)]
pub(crate) struct Packages {
    systemctl: Program,
    systemd_run: Program,
    paths: HostPaths,
}

impl Packages {
    pub(crate) fn new(systemctl: Program, systemd_run: Program, paths: HostPaths) -> Self {
        Self { systemctl, systemd_run, paths }
    }

    pub(crate) async fn start(
        &self,
        task: PackageTask,
        release: Option<&ReleasePackage>,
    ) -> Result<Response, OperationError> {
        if release.is_some() && task != PackageTask::Update {
            return Err(OperationError::Refused("only an update installs a release".into()));
        }
        if let Some(release) = release {
            // Checked here too, for a refusal before anything runs; the
            // runner checks its own copy again.
            let folder = tempfile_folder()?;
            let taken = take_release(&self.paths, release, &folder);
            let _ = fs::remove_dir_all(&folder);
            taken.map_err(OperationError::Refused)?;
        }
        if self.active().await {
            return Err(OperationError::Refused("apt is busy with another run".into()));
        }
        let mut bytes = [0u8; 8];
        rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| OperationError::Failed(format!("no random id: {e}")))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let agent = self.paths.agent.to_string_lossy().into_owned();
        let unit = format!("--unit={UNIT}");
        let description = format!("--description=zendiloOS: Debian packages ({})", task.as_str());
        let mut args = vec![
            unit.as_str(),
            description.as_str(),
            "--collect",
            "--quiet",
            "--property=Type=exec",
            agent.as_str(),
            "run-packages",
            id.as_str(),
            task.as_str(),
        ];
        if let Some(release) = release {
            args.push(release.as_str());
        }
        let output = zendiloos_exec::run(&self.systemd_run, &args, LIMITS).await?;
        if !output.status.success() {
            let complaint = output.stderr.trim().to_owned();
            return Err(OperationError::Failed(format!("systemd-run: {complaint}")));
        }
        Ok(Response::PackagesStarted { id })
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let path = self.paths.packages.join(STATE_FILE);
        let mut run: PackageRun = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|_| OperationError::Refused(format!("{} cannot be read", path.display())))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Response::Packages { run: None });
            }
            Err(error) => return Err(OperationError::Read(path, error)),
        };
        if run.state == PackageRunState::Running && !self.active().await {
            run.state = PackageRunState::Failed;
            run.error = Some("the run stopped before it finished (was the NAS restarted?)".into());
        }
        Ok(Response::Packages { run: Some(run) })
    }

    async fn active(&self) -> bool {
        zendiloos_exec::run(&self.systemctl, ["is-active", "--quiet", UNIT], LIMITS)
            .await
            .is_ok_and(|output| output.status.success())
    }
}

/// A folder of the agent's own in its private `/tmp`, for a first look at a
/// release.
fn tempfile_folder() -> Result<PathBuf, OperationError> {
    let mut bytes = [0u8; 8];
    rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
        .map_err(|e| OperationError::Failed(format!("no random name: {e}")))?;
    let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(std::env::temp_dir().join(format!("zendiloos-release-{name}")))
}

/// `run-packages <id> <task> [release]`, in `zendiloos-packages.service`:
/// does the task, each step recorded.
pub fn run_packages(paths: &HostPaths, id: &str, task: &str, release: Option<&str>) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{id:?} is no run id"));
    }
    let task = PackageTask::parse(task).ok_or_else(|| format!("{task:?} is no task"))?;
    let release = release
        .map(|name| ReleasePackage::parse(name).map_err(|_| format!("{name:?} is no release")))
        .transpose()?;
    if release.is_some() && task != PackageTask::Update {
        return Err("only an update installs a release".into());
    }
    // A run started once is never run again under its id, should its unit
    // be started anew: its record stays as it is.
    if fs::read_to_string(paths.packages.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<PackageRun>(&text).ok())
        .is_some_and(|recorded| recorded.id == id)
    {
        return Err(format!("the run {id} was started before"));
    }
    let mut run = PackageRun {
        id: id.to_owned(),
        task,
        state: PackageRunState::Running,
        step: "lists".into(),
        started_at: now(),
        finished_at: None,
        error: None,
        release: release.as_ref().map(|release| release.version().to_owned()),
        report: None,
    };
    record(&paths.packages, &run)?;
    let mut report = None;
    let done = steps(paths, task, release.as_ref(), &mut report, |step| {
        run.step = step.to_owned();
        record(&paths.packages, &run)
    });
    run.finished_at = Some(now());
    run.state = if done.is_ok() { PackageRunState::Succeeded } else { PackageRunState::Failed };
    run.error = done.as_ref().err().cloned();
    run.report = report;
    record(&paths.packages, &run)?;
    done
}

/// The steps of `task`; `step` records each as it begins.
fn steps(
    paths: &HostPaths,
    task: PackageTask,
    release: Option<&ReleasePackage>,
    report: &mut Option<PackageReport>,
    mut step: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    step("lists")?;
    apt(&["update"])?;
    match task {
        PackageTask::InstallDocker => {
            step("install")?;
            apt(&["install", "--yes", "docker.io", "docker-compose"])
        }
        PackageTask::Check => {
            *report = Some(host_report()?);
            Ok(())
        }
        PackageTask::Update => {
            step("debian")?;
            apt(&["--with-new-pkgs", "--yes", "upgrade"])?;
            if let Some(release) = release {
                step("zendiloos")?;
                install_release(paths, release)?;
            }
            step("modules")?;
            let checked = host_report()?;
            let zfs_ready = checked.zfs_ready;
            let kernel = checked.newest_kernel.clone().unwrap_or_default();
            *report = Some(checked);
            if zfs_ready {
                Ok(())
            } else {
                Err(format!(
                    "the ZFS module is missing for kernel {kernel}: do not restart; the pools would not \
                     be found. `dkms autoinstall -k {kernel}` builds it (the kernel's headers are needed)."
                ))
            }
        }
    }
}

/// Copies the release out of the download folder, checks it and installs it.
fn install_release(paths: &HostPaths, release: &ReleasePackage) -> Result<(), String> {
    let folder = paths.packages.join("release");
    let package = take_release(paths, release, &folder)?;
    let field = |name: &str| -> Result<String, String> {
        let output = Command::new("/usr/bin/dpkg-deb")
            .args(["--field"])
            .arg(&package)
            .arg(name)
            .output()
            .map_err(|e| format!("dpkg-deb: {e}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    if field("Package")? != "zendiloos" {
        return Err("the release's package is not zendiloos".into());
    }
    if field("Version")? != release.version() {
        return Err("the release's package has another version than its name says".into());
    }
    let arch = run_text("/usr/bin/dpkg", &["--print-architecture"])?;
    if field("Architecture")? != arch {
        return Err(format!("the release's package is not for {arch}"));
    }
    let installed = run_text("/usr/bin/dpkg-query", &["--show", "--showformat=${Version}", "zendiloos"])
        .unwrap_or_default();
    if !installed.is_empty() && !newer(release.version(), &installed)? {
        return Err(format!("{} is not newer than {installed}, which is installed", release.version()));
    }
    apt(&["install", "--yes", &package.to_string_lossy()])
}

/// Copies the release's three files from the download folder into `folder`
/// (made anew, root's only), never through a link, and checks the
/// signature and the package's checksum. Returns the copied package.
fn take_release(paths: &HostPaths, release: &ReleasePackage, folder: &Path) -> Result<PathBuf, String> {
    let key = release_key(paths)?;
    let _ = fs::remove_dir_all(folder);
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let spool = &paths.update_spool;
    let sums = copy_plain(&spool.join(SUMS), &folder.join(SUMS), MAX_SMALL)?;
    let signature = copy_plain(&spool.join(SIGNATURE), &folder.join(SIGNATURE), MAX_SMALL)?;
    let package = folder.join(release.as_str());
    copy_plain(&spool.join(release.as_str()), &package, MAX_PACKAGE)?;
    let signature = minisign_verify::Signature::decode(&String::from_utf8_lossy(&signature))
        .map_err(|_| "the release's signature cannot be read".to_owned())?;
    key.verify(&sums, &signature, false)
        .map_err(|_| "the release's signature does not match its key".to_owned())?;
    let expected = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|line| {
            let (sum, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start().trim_start_matches('*') == release.as_str()).then(|| sum.to_ascii_lowercase())
        })
        .ok_or_else(|| format!("{SUMS} does not list {}", release.as_str()))?;
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(&package).map_err(|e| format!("{}: {e}", package.display()))?;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("{}: {e}", package.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(format!("the checksum of {} does not match", release.as_str()));
    }
    Ok(package)
}

/// The key releases are signed with: one in `/etc/zendiloos` replaces the
/// one the package brought along.
fn release_key(paths: &HostPaths) -> Result<minisign_verify::PublicKey, String> {
    let file = paths
        .release_keys
        .iter()
        .find(|path| path.exists())
        .ok_or("there is no key to check releases with; this build of zendiloOS takes no updates")?;
    let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    minisign_verify::PublicKey::decode(&text)
        .or_else(|_| minisign_verify::PublicKey::from_base64(text.trim()))
        .map_err(|_| format!("{} holds no minisign key", file.display()))
}

/// Copies a regular file, never through a link, of at most `max` bytes;
/// returns the content of small ones.
fn copy_plain(from: &Path, to: &Path, max: u64) -> Result<Vec<u8>, String> {
    let mut source = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(from)
        .map_err(|e| format!("{}: {e}", from.display()))?;
    let metadata = source.metadata().map_err(|e| format!("{}: {e}", from.display()))?;
    if !metadata.is_file() || metadata.len() > max {
        return Err(format!("{} is no file of the release", from.display()));
    }
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    let mut kept = Vec::new();
    let copied = std::io::copy(&mut (&mut source).take(max + 1), &mut target)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    if copied > max {
        return Err(format!("{} is too large", from.display()));
    }
    if max <= MAX_SMALL {
        kept = fs::read(to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    Ok(kept)
}

/// What an upgrade would bring now, which kernel runs, which is newest, and
/// whether ZFS is built for it.
fn host_report() -> Result<PackageReport, String> {
    let output = Command::new("/usr/bin/apt-get")
        .args(["--simulate", "--quiet", "-o", "Debug::NoLocking=1", "--with-new-pkgs", "upgrade"])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apt-get cannot be started: {e}"))?;
    if !output.status.success() {
        return Err(format!("apt-get --simulate upgrade failed ({})", output.status));
    }
    let upgradable = String::from_utf8_lossy(&output.stdout).lines().filter_map(upgradable).collect();
    let running_kernel = rustix::system::uname().release().to_string_lossy().into_owned();
    let newest_kernel = newest_kernel(Path::new("/lib/modules"))?;
    let zfs_ready =
        newest_kernel.as_deref().is_none_or(|kernel| has_zfs(&Path::new("/lib/modules").join(kernel)));
    Ok(PackageReport { upgradable, running_kernel, newest_kernel, zfs_ready })
}

/// One line of `apt-get --simulate`: `Inst openssl [3.5.1-1] (3.5.4-1
/// Debian-Security:13/stable-security [amd64])`.
fn upgradable(line: &str) -> Option<UpgradablePackage> {
    let rest = line.strip_prefix("Inst ")?;
    let (name, rest) = rest.split_once(' ')?;
    let (from, rest) = match rest.strip_prefix('[') {
        Some(rest) => {
            let (from, rest) = rest.split_once("] ")?;
            (Some(from.to_owned()), rest)
        }
        None => (None, rest),
    };
    let inside = rest.strip_prefix('(')?;
    let to = inside.split_whitespace().next()?.to_owned();
    let security = inside.to_ascii_lowercase().contains("security");
    Some(UpgradablePackage { name: name.to_owned(), from, to, security })
}

/// The newest kernel with modules, by dpkg's order of versions.
fn newest_kernel(modules: &Path) -> Result<Option<String>, String> {
    let entries = match fs::read_dir(modules) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", modules.display())),
    };
    let mut newest: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().join("modules.dep").exists() && !entry.path().join("kernel").is_dir() {
            continue;
        }
        newest = match newest {
            Some(current) if !newer(&name, &current)? => Some(current),
            _ => Some(name),
        };
    }
    Ok(newest)
}

/// A `zfs.ko*` somewhere below the kernel's modules (DKMS puts it in
/// `updates/dkms`).
fn has_zfs(kernel: &Path) -> bool {
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

/// `a` is a newer version than `b`, as dpkg orders them.
fn newer(a: &str, b: &str) -> Result<bool, String> {
    let status = Command::new("/usr/bin/dpkg")
        .args(["--compare-versions", a, "gt", b])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("dpkg: {e}"))?;
    Ok(status.success())
}

fn run_text(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!("{program} failed ({})", output.status))
    }
}

/// `apt-get` without questions, keeping changed configuration files, waiting
/// up to ten minutes for another apt to finish. Its output goes to the
/// journal; the last lines of a failure into the error.
///
/// needrestart (which `docker.io` recommends) stays out of it: it would
/// restart this very unit once an update replaces this binary, and Docker
/// with every app after a library's update. Services restart as their own
/// packages say, as on a Debian without needrestart.
fn apt(args: &[&str]) -> Result<(), String> {
    let output = Command::new("/usr/bin/apt-get")
        .args(["--quiet", "-o", "DPkg::Lock::Timeout=600"])
        .args(["-o", "Dpkg::Options::=--force-confdef", "-o", "Dpkg::Options::=--force-confold"])
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("APT_LISTCHANGES_FRONTEND", "none")
        .env("NEEDRESTART_SUSPEND", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apt-get cannot be started: {e}"))?;
    let text =
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{text}");
    if output.status.success() {
        return Ok(());
    }
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL)..].join("\n");
    Err(format!("apt-get {} failed ({}):\n{tail}", args[0], output.status))
}

/// Writes the run's state, readable by everyone, replacing the last.
fn record(folder: &Path, run: &PackageRun) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o755).create(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("{}: {error}", folder.display())),
    }
    let path = folder.join(STATE_FILE);
    let temporary = folder.join(format!("{STATE_FILE}.new"));
    let text = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    let written = (|| {
        let mut file =
            fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    written.map_err(|e| format!("{}: {e}", path.display()))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().try_into().unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(task: PackageTask) -> PackageRun {
        PackageRun {
            id: "ab12".into(),
            task,
            state: PackageRunState::Running,
            step: "install".into(),
            started_at: 1,
            finished_at: None,
            error: None,
            release: None,
            report: None,
        }
    }

    #[test]
    fn a_run_is_recorded_for_the_agent_to_read() {
        let folder = tempfile::tempdir().unwrap();
        let run = run(PackageTask::InstallDocker);
        record(folder.path(), &run).unwrap();
        let read: PackageRun =
            serde_json::from_str(&fs::read_to_string(folder.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(read, run);
    }

    #[test]
    fn a_run_is_never_run_twice() {
        let folder = tempfile::tempdir().unwrap();
        let paths = HostPaths { packages: folder.path().to_owned(), ..HostPaths::default() };
        let mut recorded = run(PackageTask::Update);
        recorded.state = PackageRunState::Succeeded;
        record(folder.path(), &recorded).unwrap();
        let again = run_packages(&paths, "ab12", "update", None).unwrap_err();
        assert!(again.contains("started before"), "{again}");
        let kept: PackageRun =
            serde_json::from_str(&fs::read_to_string(folder.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(kept.state, PackageRunState::Succeeded);
    }

    #[test]
    fn only_hex_ids_known_tasks_and_release_names_run() {
        let paths =
            HostPaths { packages: std::env::temp_dir().join("zendiloos-never"), ..HostPaths::default() };
        assert!(run_packages(&paths, "../x", "install-docker", None).unwrap_err().contains("no run id"));
        assert!(run_packages(&paths, "ab12", "rm-rf", None).unwrap_err().contains("no task"));
        assert!(
            run_packages(&paths, "ab12", "update", Some("../etc/shadow")).unwrap_err().contains("no release")
        );
        let docker = run_packages(&paths, "ab12", "install-docker", Some("zendiloos_1.0_amd64.deb"));
        assert!(docker.unwrap_err().contains("only an update"));
    }

    #[test]
    fn apt_tells_what_an_upgrade_brings() {
        let lines = "NOTE: This is only a simulation!\n\
            Inst openssl [3.5.1-1] (3.5.4-1~deb13u1 Debian-Security:13/stable-security [amd64])\n\
            Inst linux-image-6.12.57+deb13-amd64 (6.12.57-1 Debian:13.2/stable [amd64])\n\
            Conf openssl (3.5.4-1~deb13u1 Debian-Security:13/stable-security [amd64])\n";
        let found: Vec<UpgradablePackage> = lines.lines().filter_map(upgradable).collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].from.as_deref(), Some("3.5.1-1"));
        assert_eq!(found[0].to, "3.5.4-1~deb13u1");
        assert!(found[0].security);
        assert_eq!(found[1].name, "linux-image-6.12.57+deb13-amd64");
        assert_eq!(found[1].from, None);
        assert!(!found[1].security);
    }

    #[test]
    fn zfs_is_found_where_dkms_puts_it() {
        let kernel = tempfile::tempdir().unwrap();
        assert!(!has_zfs(kernel.path()));
        fs::create_dir_all(kernel.path().join("updates/dkms")).unwrap();
        fs::write(kernel.path().join("updates/dkms/zfs.ko.xz"), "").unwrap();
        assert!(has_zfs(kernel.path()));
    }

    /// A key made with `minisign -G -W` (its secret one is gone), and
    /// `SHA256SUMS` of a "package" signed with it.
    const KEY: &str = "untrusted comment: minisign public key DF2398FD1421641E\n\
        RWQeZCEU/Zgj30ypMHVOUazGHOS4zrR2comO1Cr6nNACcycOZJl0Qv3z\n";
    const PACKAGE: &str = "test package\n";
    const SIGNED_SUMS: &str =
        "d400a577a056c0530d6acfc0e0a2f937be15323bffe02dfc81a8ca66b332a907  zendiloos_9.9.9_amd64.deb\n";
    const SIGNED: &str = "untrusted comment: signature from minisign secret key\n\
        RUQeZCEU/Zgj3w/shOlDbRzCkd75l/5mJ9ZiScpj4yMyq5thsyekWQS5toBgDunkEebNhxIsqz0ubaxSW+AdqscpXf13Q3Q6oAI=\n\
        trusted comment: zendiloos 9.9.9\n\
        xRAvEXYdiCSNSWJHQ2JmtSHjKY61Uz+J7Bn22qhUpJXTEAKzlSUEBlv/lwtADWtkFb5kPh2dk+6Xo2IKSl13BQ==\n";

    #[test]
    fn a_release_is_taken_only_with_its_signature_and_checksum() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        let key = root.path().join("release.pub");
        fs::write(&key, KEY).unwrap();
        let paths = HostPaths {
            update_spool: spool.clone(),
            release_keys: vec![root.path().join("not-there.pub"), key],
            packages: root.path().join("packages"),
            ..HostPaths::default()
        };
        let release = ReleasePackage::parse("zendiloos_9.9.9_amd64.deb").unwrap();
        let taken = root.path().join("taken");
        let put = |name: &str, content: &str| {
            let _ = fs::remove_file(spool.join(name));
            fs::write(spool.join(name), content).unwrap();
        };
        put(release.as_str(), PACKAGE);
        put(SUMS, SIGNED_SUMS);
        put(SIGNATURE, SIGNED);
        let package = take_release(&paths, &release, &taken).unwrap();
        assert_eq!(fs::read_to_string(package).unwrap(), PACKAGE);

        put(release.as_str(), "another package\n");
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains("checksum"), "{refused}");

        put(release.as_str(), PACKAGE);
        put(SUMS, &SIGNED_SUMS.replace("d400", "0000"));
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains("signature does not match"), "{refused}");

        // A link in the download folder is never followed.
        fs::remove_file(spool.join(SUMS)).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", spool.join(SUMS)).unwrap();
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains(SUMS), "{refused}");

        let without_key = HostPaths { release_keys: vec![root.path().join("none")], ..paths };
        let refused = take_release(&without_key, &release, &taken).unwrap_err();
        assert!(refused.contains("no key"), "{refused}");
    }
}
