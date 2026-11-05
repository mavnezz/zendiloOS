//! Samba shares and NFS exports, and the permissions of share directories.
//!
//! The agent writes the configuration itself from validated names: no
//! option, path or host from zendiloosd reaches a configuration file as
//! text. Paths are always `/volume<N>/<share>`.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use zendiloos_agent_proto::{NfsExport, Principal, Response, ShareAccess, ShareName, SmbShare, VolumeNumber};
use zendiloos_exec::Program;

use crate::config::Tools;
use crate::operations::OperationError;
use crate::pools::{HostPaths, run};

pub use zendiloos_fileops::RECYCLE_DIRECTORY;
const HEADER: &str = "# Written by zendiloOS. Changes here are overwritten; use the web interface.\n";

#[derive(Debug)]
pub(crate) struct Sharing {
    paths: HostPaths,
    testparm: Program,
    exportfs: Program,
    systemctl: Program,
    setfacl: Program,
    serial: tokio::sync::Mutex<()>,
}

fn principal(principal: &Principal) -> String {
    match principal {
        Principal::User(user) => user.as_str().to_owned(),
        Principal::Group(group) => format!("@{}", group.as_str()),
    }
}

fn acl_entry(principal: &Principal) -> String {
    match principal {
        Principal::User(user) => format!("u:{}", user.as_str()),
        Principal::Group(group) => format!("g:{}", group.as_str()),
    }
}

/// `smb.conf` sections for `shares`. A share nobody may use is left out:
/// Samba would read an empty `valid users` as "everybody".
pub fn samba_config(shares: &[SmbShare]) -> String {
    let mut text = HEADER.to_owned();
    for share in shares.iter().filter(|share| !share.access.is_empty()) {
        let valid: Vec<String> = share.access.iter().map(|a| principal(&a.principal)).collect();
        let writers: Vec<String> =
            share.access.iter().filter(|a| a.write).map(|a| principal(&a.principal)).collect();
        let _ = write!(
            text,
            "\n[{name}]\n   path = /volume{volume}/{name}\n   valid users = {valid}\n",
            name = share.name.as_str(),
            volume = share.volume.get(),
            valid = valid.join(" "),
        );
        if let Some(comment) = &share.comment {
            let _ = writeln!(text, "   comment = {}", comment.as_str());
        }
        text.push_str("   read only = yes\n");
        if !writers.is_empty() {
            let _ = writeln!(text, "   write list = {}", writers.join(" "));
        }
        text.push_str("   browseable = yes\n   inherit acls = yes\n   map acl inherit = yes\n");
        if share.recycle_bin {
            text.push_str("   vfs objects = catia fruit streams_xattr recycle\n");
            let _ = writeln!(text, "   recycle:repository = {RECYCLE_DIRECTORY}/%U");
            // Each user's bin is private. `.recycle` itself exists with the
            // share's permissions (see `set_access`): created by Samba, it
            // would get this mode and keep everybody else out, and Samba
            // deletes files for good when it cannot move them.
            text.push_str(
                "   recycle:keeptree = yes\n   recycle:versions = yes\n   recycle:touch = yes\n   \
                 recycle:directory_mode = 0700\n",
            );
            // Temporary files and the lock files of office programs come and
            // go all the time; they would only fill the bins. Samba splits
            // the list at commas and spaces; the `|` of older examples
            // makes it one pattern that matches nothing.
            text.push_str("   recycle:exclude = *.tmp, *.temp, ~$*, .~lock.*#, *.swp\n");
        } else {
            text.push_str("   vfs objects = catia fruit streams_xattr\n");
        }
        // Hidden, not vetoed: users restore their files from it.
        let _ = writeln!(text, "   hide files = /{RECYCLE_DIRECTORY}/");
    }
    text
}

/// `/etc/exports` lines for `exports`; root is squashed everywhere.
pub fn nfs_exports(exports: &[NfsExport]) -> String {
    let mut text = HEADER.to_owned();
    for export in exports.iter().filter(|export| !export.clients.is_empty()) {
        let _ = write!(text, "/volume{}/{}", export.volume.get(), export.name.as_str());
        for client in &export.clients {
            let mode = if client.write { "rw" } else { "ro" };
            let _ = write!(text, " {}({mode},sync,no_subtree_check,root_squash,sec=sys)", client.network);
        }
        text.push('\n');
    }
    text
}

fn write_atomically(path: &Path, content: &str) -> Result<(), OperationError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
    }
    let temporary = path.with_extension("new");
    fs::write(&temporary, content).map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))
}

impl Sharing {
    pub(crate) fn new(tools: &Tools, paths: HostPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            paths,
            testparm: Program::new(&tools.testparm)?,
            exportfs: Program::new(&tools.exportfs)?,
            systemctl: Program::new(&tools.systemctl)?,
            setfacl: Program::new(&tools.setfacl)?,
            serial: tokio::sync::Mutex::new(()),
        })
    }

    /// Writes both configurations; Samba's only if `testparm` accepts it.
    pub(crate) async fn apply(
        &self,
        smb: &[SmbShare],
        nfs: &[NfsExport],
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let samba = samba_config(smb);
        let candidate = self.paths.samba_config.with_extension("check");
        write_atomically(&candidate, &samba)?;
        let checked = run(&self.testparm, &["--suppress-prompt", &candidate.to_string_lossy()]).await;
        let _ = fs::remove_file(&candidate);
        checked?;
        write_atomically(&self.paths.samba_config, &samba)?;
        run(&self.systemctl, &["try-reload-or-restart", "smbd.service"]).await?;

        let exports = nfs_exports(nfs);
        let nfs_installed = self.exportfs.path().exists();
        if !nfs_installed && nfs.iter().any(|export| !export.clients.is_empty()) {
            return Err(OperationError::Setup("the NFS server is not installed".to_owned()));
        }
        write_atomically(&self.paths.nfs_exports, &exports)?;
        if nfs_installed {
            // Through systemd: the kernel's NFS server keeps its export
            // cache per network namespace, and this agent has its own.
            let reload = if nfs.iter().any(|export| !export.clients.is_empty()) {
                "reload-or-restart"
            } else {
                "try-reload-or-restart"
            };
            run(&self.systemctl, &[reload, "nfs-server.service"]).await?;
        }
        Ok(Response::Done)
    }

    fn share_path(&self, volume: VolumeNumber, name: &ShareName) -> PathBuf {
        self.paths.volumes.join(format!("volume{}", volume.get())).join(name.as_str())
    }

    /// Owner `root`, mode 0770 and an ACL for each principal, inherited by
    /// new files. With `recursive`, existing contents follow, except the
    /// users' recycle bins.
    pub(crate) async fn set_access(
        &self,
        volume: VolumeNumber,
        name: &ShareName,
        access: &[ShareAccess],
        removed: &[Principal],
        recursive: bool,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let path = self.share_path(volume, name);
        let parent = path.parent().unwrap_or(&path).to_owned();
        let (own, above) = match (fs::metadata(&path), fs::metadata(&parent)) {
            (Ok(own), Ok(above)) => (own, above),
            _ => return Err(OperationError::Refused(format!("share {} is not present", name.as_str()))),
        };
        // Its own dataset is mounted there; otherwise this would change
        // the volume's directory instead.
        if !own.is_dir() || own.dev() == above.dev() {
            return Err(OperationError::Refused(format!("share {} is not mounted", name.as_str())));
        }
        std::os::unix::fs::chown(&path, Some(0), Some(0))
            .map_err(|e| OperationError::Write(path.clone(), e))?;

        let entries = |default: bool, files: bool| -> Vec<String> {
            access
                .iter()
                .map(|a| {
                    let permissions = match (a.write, files) {
                        (true, true) => "rwX",
                        (false, true) => "rX",
                        (true, false) => "rwx",
                        (false, false) => "r-x",
                    };
                    let prefix = if default { "d:" } else { "" };
                    format!("{prefix}{}:{permissions}", acl_entry(&a.principal))
                })
                .collect()
        };
        let mut set = vec!["u::rwx".to_owned(), "g::---".to_owned(), "o::---".to_owned()];
        set.extend(entries(false, false));
        set.push("m::rwx".to_owned());
        set.extend(["d:u::rwx", "d:g::---", "d:o::---"].map(str::to_owned));
        set.extend(entries(true, false));
        set.push("d:m::rwx".to_owned());
        let path_text = path.to_string_lossy().into_owned();
        run(&self.setfacl, &["--set", &set.join(","), "--", &path_text]).await?;

        // The recycle bin's top directory, with the share's permissions, so
        // every writer can create their own bin in it.
        let bin = path.join(RECYCLE_DIRECTORY);
        match fs::create_dir(&bin) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(OperationError::Write(bin, error));
            }
            _ => {}
        }
        if !fs::symlink_metadata(&bin).map_err(|e| OperationError::Read(bin.clone(), e))?.is_dir() {
            return Err(OperationError::Refused(format!(
                "{RECYCLE_DIRECTORY} in {} is not a folder",
                name.as_str()
            )));
        }
        std::os::unix::fs::lchown(&bin, Some(0), Some(0))
            .map_err(|e| OperationError::Write(bin.clone(), e))?;
        run(&self.setfacl, &["--set", &set.join(","), "--", &bin.to_string_lossy()]).await?;

        if recursive {
            let contents: Vec<String> = fs::read_dir(&path)
                .map_err(|e| OperationError::Read(path.clone(), e))?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() != RECYCLE_DIRECTORY)
                .map(|entry| entry.path().to_string_lossy().into_owned())
                .collect();
            let mut modify = entries(false, true);
            modify.extend(entries(true, false));
            let removals: Vec<String> =
                removed.iter().flat_map(|p| [acl_entry(p), format!("d:{}", acl_entry(p))]).collect();
            for chunk in contents.chunks(256) {
                if !modify.is_empty() {
                    let mut args = vec!["-R".to_owned(), "-m".to_owned(), modify.join(","), "--".to_owned()];
                    args.extend(chunk.iter().cloned());
                    run(&self.setfacl, &args).await?;
                }
                if !removals.is_empty() {
                    let mut args =
                        vec!["-R".to_owned(), "-x".to_owned(), removals.join(","), "--".to_owned()];
                    args.extend(chunk.iter().cloned());
                    run(&self.setfacl, &args).await?;
                }
            }
        }
        Ok(Response::Done)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{GroupName, Network, NfsClient, ShareComment, UserName};

    use super::*;

    fn access(principal: Principal, write: bool) -> ShareAccess {
        ShareAccess { principal, write }
    }

    #[test]
    fn writes_samba_shares_from_names_only() {
        let shares = [
            SmbShare {
                name: ShareName::parse("photos").unwrap(),
                volume: VolumeNumber::new(2).unwrap(),
                access: vec![
                    access(Principal::Group(GroupName::parse("family").unwrap()), true),
                    access(Principal::User(UserName::parse("guest-bob").unwrap()), false),
                ],
                recycle_bin: true,
                comment: Some(ShareComment::parse("Family photos").unwrap()),
            },
            SmbShare {
                name: ShareName::parse("nobody").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                access: vec![],
                recycle_bin: false,
                comment: None,
            },
        ];
        let text = samba_config(&shares);
        assert!(
            text.contains(
                "[photos]\n   path = /volume2/photos\n   valid users = @family guest-bob\n   comment = Family photos\n"
            ),
            "{text}"
        );
        assert!(text.contains("   read only = yes\n   write list = @family\n"), "{text}");
        assert!(text.contains("recycle:repository = .recycle/%U"), "{text}");
        assert!(text.contains("recycle:exclude = *.tmp, *.temp, ~$*, .~lock.*#, *.swp\n"), "{text}");
        assert!(!text.contains("[nobody]"), "a share without access is not offered: {text}");
    }

    #[test]
    fn writes_exports_with_root_squashed() {
        let exports = [NfsExport {
            name: ShareName::parse("media").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            clients: vec![
                NfsClient { network: Network::parse("192.168.1.0/24").unwrap(), write: true },
                NfsClient { network: Network::parse("10.0.0.5").unwrap(), write: false },
            ],
        }];
        assert_eq!(
            nfs_exports(&exports).lines().nth(1).unwrap(),
            "/volume1/media 192.168.1.0/24(rw,sync,no_subtree_check,root_squash,sec=sys) \
             10.0.0.5(ro,sync,no_subtree_check,root_squash,sec=sys)"
        );
    }
}
