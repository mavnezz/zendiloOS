//! Samba shares and NFS exports, and the permissions of share directories.
//!
//! The agent writes the configuration itself from validated names: no
//! option, path or host from zendiloosd reaches a configuration file as
//! text. Paths are always `/volume<N>/<share>`.

use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr};
use std::fmt::Write as _;
use std::fs;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use rustix::fs::{CWD, Mode, OFlags, openat};
use rustix::io::Errno;
use zendiloos_agent_proto::{
    FileServices, HOMES_DIRECTORY, HomesShare, NfsExport, Principal, Response, SNAPSHOT_LABEL_FORMAT,
    ShareAccess, ShareName, SharePermission, SmbShare, UserName, VolumeNumber,
};
use zendiloos_exec::{Limits, Program};

use crate::acl::{self, Acl, Owners, Rules, Who};
use crate::config::Tools;
use crate::operations::OperationError;
use crate::pools::{HostPaths, run};

const LIMITS: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 1024 * 1024 };
pub use zendiloos_fileops::RECYCLE_DIRECTORY;
/// How long a change of a share's own folder waits for another change of
/// the same share: one that goes through all its files may take hours, and
/// whoever asked is told to come back rather than kept waiting.
const SHARE_WAIT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 15 });
const HEADER: &str = "# Written by zendiloOS. Changes here are overwritten; use the web interface.\n";
/// WS-Discovery, so Windows lists the NAS; recommended, not required.
const WSDD: &str = "wsdd2.service";
/// Bonjour, so Macs find the NAS and its Time Machine share; recommended too.
const AVAHI: &str = "avahi-daemon.service";
const AVAHI_SOCKET: &str = "avahi-daemon.socket";

#[derive(Debug)]
pub(crate) struct Sharing {
    paths: HostPaths,
    accounts: crate::accounts::AccountFiles,
    testparm: Program,
    exportfs: Program,
    systemctl: Program,
    setfacl: Program,
    /// One writing of the services' configuration at a time.
    config: tokio::sync::Mutex<()>,
    /// One change of a share's permissions at a time, for each share: a
    /// change going through a large share keeps only that share waiting.
    shares: std::sync::Mutex<HashMap<(u16, String), ShareLock>>,
}

type ShareLock = Arc<tokio::sync::Mutex<()>>;

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

/// Samba's settings for all shares, then a section for each share. A
/// share nobody may use is left out: Samba would read an empty `valid
/// users` as "everybody". Denied users and groups are `invalid users`.
///
/// `/etc/samba/smb.conf` includes the file at the end of `[global]`: the
/// lines before the first section are global settings, and they win over
/// the same ones above the include.
pub fn samba_config(shares: &[SmbShare], services: &FileServices, homes: Option<&HomesShare>) -> String {
    let mut text = HEADER.to_owned();
    let yes = |on: bool| if on { "yes" } else { "no" };
    let _ = writeln!(
        text,
        "workgroup = {}\nserver min protocol = {}\nserver smb encrypt = {}\nserver signing = {}\n\
         server multi channel support = {}\nmulticast dns register = {}",
        services.workgroup.as_str(),
        services.smb_min_protocol.samba_name(),
        services.smb_encryption.samba_name(),
        services.smb_signing.samba_name(),
        yes(services.multichannel),
        yes(services.bonjour),
    );
    // Share settings here are the defaults of every share.
    let _ = writeln!(
        text,
        "access based share enum = {}\noplocks = {}\nsmb2 leases = {}\nfollow symlinks = {}",
        yes(services.hide_unreadable),
        yes(services.oplocks),
        yes(services.oplocks && services.leases),
        yes(services.follow_symlinks),
    );
    if services.durable_handles {
        // Durable handles need Samba's locks kept from the kernel; they then
        // do not hold against NFS or the file manager.
        text.push_str("durable handles = yes\nkernel share modes = no\nposix locking = no\n");
    } else {
        // Samba's default is yes; said here, so off is off.
        text.push_str("durable handles = no\n");
    }
    if let Some(veto) = &services.veto_files {
        let _ = writeln!(
            text,
            "veto files = {}\ndelete veto files = {}",
            veto.as_str(),
            yes(services.delete_veto_files)
        );
    }
    let mac = if services.mac_compat { "catia fruit " } else { "" };
    for share in shares.iter().filter(|share| share.access.iter().any(|a| !a.denied())) {
        let valid: Vec<String> =
            share.access.iter().filter(|a| !a.denied()).map(|a| principal(&a.principal)).collect();
        let writers: Vec<String> =
            share.access.iter().filter(|a| a.writes()).map(|a| principal(&a.principal)).collect();
        let invalid: Vec<String> =
            share.access.iter().filter(|a| a.denied()).map(|a| principal(&a.principal)).collect();
        let _ = write!(
            text,
            "\n[{name}]\n   path = /volume{volume}/{name}\n   valid users = {valid}\n",
            name = share.name.as_str(),
            volume = share.volume.get(),
            valid = valid.join(" "),
        );
        // Wins over `valid users`, also for members of a group listed there.
        if !invalid.is_empty() {
            let _ = writeln!(text, "   invalid users = {}", invalid.join(" "));
        }
        if let Some(comment) = &share.comment {
            let _ = writeln!(text, "   comment = {}", comment.as_str());
        }
        text.push_str("   read only = yes\n");
        if !writers.is_empty() {
            let _ = writeln!(text, "   write list = {}", writers.join(" "));
        }
        text.push_str("   browseable = yes\n   inherit acls = yes\n   map acl inherit = yes\n");
        // ZFS snapshots as "Previous Versions" in Windows: they are named
        // after their UTC time (see zendiloos-sharing's snapshot labels).
        let _ =
            writeln!(text, "   shadow:snapdir = .zfs/snapshot\n   shadow:format = {SNAPSHOT_LABEL_FORMAT}");
        text.push_str("   shadow:localtime = no\n   shadow:sort = desc\n");
        if share.recycle_bin {
            let _ = writeln!(text, "   vfs objects = shadow_copy2 {mac}streams_xattr recycle");
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
            let _ = writeln!(text, "   vfs objects = shadow_copy2 {mac}streams_xattr");
        }
        // Hidden, not vetoed: users restore their files from it.
        let _ = writeln!(text, "   hide files = /{RECYCLE_DIRECTORY}/");
        if let Some(limit) = share.time_machine {
            text.push_str("   fruit:time machine = yes\n");
            if let Some(bytes) = limit {
                let _ = writeln!(text, "   fruit:time machine max size = {}M", bytes / (1024 * 1024));
            }
        }
    }
    if let Some(homes) = homes.filter(|homes| !homes.users.is_empty()) {
        let users: Vec<&str> = homes.users.iter().map(UserName::as_str).collect();
        // `%U` is the user who connected: each one sees their own folder.
        let _ = write!(
            text,
            "\n[home]\n   comment = Personal folder\n   path = /volume{volume}/{HOMES_DIRECTORY}/%U\n   \
             valid users = {users}\n   read only = no\n   browseable = yes\n   create mask = 0600\n   \
             directory mask = 0700\n   vfs objects = {mac}streams_xattr recycle\n   \
             recycle:repository = {RECYCLE_DIRECTORY}/%U\n   recycle:keeptree = yes\n   \
             recycle:versions = yes\n   recycle:touch = yes\n   recycle:directory_mode = 0700\n   \
             recycle:exclude = *.tmp, *.temp, ~$*, .~lock.*#, *.swp\n   hide files = /{RECYCLE_DIRECTORY}/\n",
            volume = homes.volume.get(),
            users = users.join(" "),
        );
    }
    text
}

/// What smbd announces over Bonjour in a configuration it wrote: whether
/// it does, and its Time Machine shares.
fn announced(config: &str) -> (bool, Vec<&str>) {
    let mut section = "";
    let mut machines = Vec::new();
    let mut register = false;
    for line in config.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name;
        } else if line == "fruit:time machine = yes" {
            machines.push(section);
        } else if line == "multicast dns register = yes" {
            register = true;
        }
    }
    (register, machines)
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
    crate::durable::replace(path, content.as_bytes())
}

impl Sharing {
    pub(crate) fn new(
        tools: &Tools,
        paths: HostPaths,
        accounts: crate::accounts::AccountFiles,
    ) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            paths,
            accounts,
            testparm: Program::new(&tools.testparm)?,
            exportfs: Program::new(&tools.exportfs)?,
            systemctl: Program::new(&tools.systemctl)?,
            setfacl: Program::new(&tools.setfacl)?,
            config: tokio::sync::Mutex::new(()),
            shares: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Writes both configurations, Samba's only if `testparm` accepts it,
    /// and has the servers that are on run with them; those that are off
    /// stop and stay off after a restart.
    pub(crate) async fn apply(
        &self,
        smb: &[SmbShare],
        nfs: &[NfsExport],
        services: &FileServices,
        homes: Option<&HomesShare>,
    ) -> Result<Response, OperationError> {
        let _config = crate::waiting::lock(&self.config).await?;
        if !services.mac_compat && smb.iter().any(|share| share.time_machine.is_some()) {
            return Err(OperationError::Refused("Time Machine needs the macOS compatibility of SMB".into()));
        }
        if homes.is_some() && smb.iter().any(|share| share.name.as_str() == "home") {
            return Err(OperationError::Refused(
                "a share is called home, the name of the personal folders' share".into(),
            ));
        }
        let samba = samba_config(smb, services, homes);
        let candidate = self.paths.samba_config.with_extension("check");
        write_atomically(&candidate, &samba)?;
        let checked = run(&self.testparm, &["--suppress-prompt", &candidate.to_string_lossy()]).await;
        let _ = fs::remove_file(&candidate);
        checked?;
        let before = fs::read_to_string(&self.paths.samba_config).unwrap_or_default();
        write_atomically(&self.paths.samba_config, &samba)?;
        if services.smb {
            run(&self.systemctl, &["enable", "--now", "smbd.service"]).await?;
            // smbd announces itself and its Time Machine shares over Bonjour
            // when it starts, not when it reloads: it restarts when that
            // changes, which ends open connections; otherwise it reloads.
            let verb =
                if announced(&before) == announced(&samba) { "try-reload-or-restart" } else { "try-restart" };
            run(&self.systemctl, &[verb, "smbd.service"]).await?;
        } else {
            run(&self.systemctl, &["disable", "--now", "smbd.service"]).await?;
        }
        if services.smb && services.wsdd {
            self.optional(&["enable", "--now", WSDD]).await;
        } else {
            self.optional(&["disable", "--now", WSDD]).await;
        }
        if services.bonjour {
            self.optional(&["enable", "--now", AVAHI_SOCKET, AVAHI]).await;
        } else {
            self.optional(&["disable", "--now", AVAHI, AVAHI_SOCKET]).await;
        }

        let exports = nfs_exports(nfs);
        let exported = nfs.iter().any(|export| !export.clients.is_empty());
        let nfs_installed = self.exportfs.path().exists();
        if !nfs_installed && services.nfs && exported {
            return Err(OperationError::Setup("the NFS server is not installed".to_owned()));
        }
        write_atomically(&self.paths.nfs_exports, &exports)?;
        if nfs_installed {
            // Through systemd: the kernel's NFS server keeps its export
            // cache per network namespace, and this agent has its own.
            if services.nfs {
                run(&self.systemctl, &["enable", "--now", "nfs-server.service"]).await?;
                run(&self.systemctl, &["reload-or-restart", "nfs-server.service"]).await?;
            } else {
                run(&self.systemctl, &["disable", "--now", "nfs-server.service"]).await?;
            }
        }
        Ok(Response::Done)
    }

    /// For a service that may not be installed: a failure is only noted.
    async fn optional(&self, args: &[&str]) {
        if let Err(error) = run(&self.systemctl, args).await {
            tracing::info!(%error, "an optional service was not changed");
        }
    }

    /// Makes `user`'s personal folder if missing: owner the user, mode
    /// 0700. Only in the homes' dataset, mounted at `@homes`, which only
    /// root may write to, so nothing can be swapped in meanwhile. Run by
    /// the files agent, the one that may write to the volumes.
    pub(crate) fn make_home(
        &self,
        volume: VolumeNumber,
        user: &UserName,
    ) -> Result<Response, OperationError> {
        let volume_directory = self.paths.volumes.join(format!("volume{}", volume.get()));
        let homes = volume_directory.join(HOMES_DIRECTORY);
        let (homes_meta, volume_meta) = match (fs::metadata(&homes), fs::metadata(&volume_directory)) {
            (Ok(homes), Ok(volume)) => (homes, volume),
            _ => {
                return Err(OperationError::Refused(
                    "the personal folders are not set up on this volume".into(),
                ));
            }
        };
        if homes_meta.dev() == volume_meta.dev() {
            return Err(OperationError::Refused("the personal folders' dataset is not mounted".into()));
        }
        // Users reach their own folder through it; nobody lists the others,
        // and only root makes folders in it.
        std::os::unix::fs::chown(&homes, Some(0), Some(0))
            .map_err(|e| OperationError::Write(homes.clone(), e))?;
        fs::set_permissions(&homes, std::os::unix::fs::PermissionsExt::from_mode(0o711))
            .map_err(|e| OperationError::Write(homes.clone(), e))?;
        let read =
            |path: &Path| fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e));
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let (uid, gid) = crate::passwd::lookup(&passwd, user.as_str())
            .ok_or_else(|| OperationError::NotManaged(user.as_str().to_owned()))?;
        let managed = (crate::accounts::ID_MIN..=crate::accounts::ID_MAX).contains(&uid)
            && crate::passwd::group_id(&group, crate::accounts::USERS_GROUP) == Some(gid);
        if !managed {
            return Err(OperationError::NotManaged(user.as_str().to_owned()));
        }
        let home = homes.join(user.as_str());
        if fresh_home(&homes, user.as_str(), uid)? {
            std::os::unix::fs::chown(&home, Some(uid), Some(gid))
                .map_err(|e| OperationError::Write(home.clone(), e))?;
            fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .map_err(|e| OperationError::Write(home.clone(), e))?;
        }
        Ok(Response::Done)
    }

    fn share_path(&self, volume: VolumeNumber, name: &ShareName) -> PathBuf {
        self.paths.volumes.join(format!("volume{}", volume.get())).join(name.as_str())
    }

    fn share_lock(&self, volume: VolumeNumber, name: &ShareName) -> ShareLock {
        let mut shares = self.shares.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(shares.entry((volume.get(), name.as_str().to_owned())).or_default())
    }

    /// Owner `root`, mode 0770 and an ACL for each principal, inherited by
    /// new files. With `recursive`, existing contents follow, except the
    /// users' recycle bins and other private folders (see `change_tree`).
    ///
    /// A change of the share's folder alone (the one a person waits for)
    /// does not wait long for one going through its files: it is refused,
    /// to be asked again once that has ended.
    pub(crate) async fn set_access(
        &self,
        volume: VolumeNumber,
        name: &ShareName,
        access: &[ShareAccess],
        removed: &[Principal],
        recursive: bool,
    ) -> Result<Response, OperationError> {
        let lock = self.share_lock(volume, name);
        let _share = if recursive {
            crate::waiting::lock(&lock).await?
        } else {
            tokio::time::timeout(SHARE_WAIT, crate::waiting::lock(&lock)).await.map_err(|_| {
                OperationError::Refused(format!(
                    "the permissions of share {} are being applied to its files; try again once that has ended",
                    name.as_str()
                ))
            })??
        };
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
                    let permissions = match (a.permission, files) {
                        (SharePermission::Deny, _) => "---",
                        (SharePermission::ReadWrite, true) => "rwX",
                        (SharePermission::Read, true) => "rX",
                        (SharePermission::ReadWrite, false) => "rwx",
                        (SharePermission::Read, false) => "r-x",
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
        // every writer can create their own bin in it. Writers can rename
        // what is in the share's root, so it is opened without following a
        // link and changed through that descriptor: a link swapped in
        // meanwhile changes nothing.
        let bin = path.join(RECYCLE_DIRECTORY);
        let directory = fs::File::from(open_bin(&path, name)?);
        std::os::unix::fs::fchown(&directory, Some(0), Some(0))
            .map_err(|e| OperationError::Write(bin.clone(), e))?;
        let for_acl = directory.try_clone().map_err(|e| OperationError::Write(bin.clone(), e))?;
        zendiloos_exec::run_on(
            &self.setfacl,
            ["--set", &set.join(","), "--", "/proc/self/fd/0"],
            for_acl,
            LIMITS,
        )
        .await?
        .success(&self.setfacl)?;
        sticky(&directory).map_err(|e| OperationError::Write(bin.clone(), e.into()))?;

        // Each writer's own bin, made here: otherwise one writer could make
        // another's first and read what lands in it, which Samba does not
        // check.
        for (user, uid, gid) in self.writers(access)? {
            if let Err(error) = prepare_bin(&directory, &user, uid, gid) {
                tracing::warn!(%error, share = name.as_str(), user, "cannot prepare a recycle bin");
            }
        }

        if recursive {
            let rules = self.rules(access, removed)?;
            tokio::task::spawn_blocking(move || change_tree(&path, &rules))
                .await
                .map_err(|error| OperationError::Failed(format!("the change stopped: {error}")))??;
        }
        Ok(Response::Done)
    }
}

/// Makes the bins' folder sticky: every writer may make their bin in it,
/// but only a bin's owner may rename or remove it, so nobody takes away
/// another's bin and puts one of their own in its place to get what lands
/// there (Samba does not check whose a bin is). With an ACL the group bits
/// are its mask, so they are kept as they are.
fn sticky(directory: &fs::File) -> Result<(), Errno> {
    let mode = rustix::fs::fstat(directory)?.st_mode;
    rustix::fs::fchmod(directory, Mode::from_raw_mode((mode & 0o777) | 0o1000))
}

/// Makes `user`'s folder in `homes` unless the user (`uid`) has it already;
/// says whether it was made (and so still needs its owner and mode). A
/// folder of that name of another UID is left by an earlier account of the
/// same name: moved aside (`<user>.previous-<uid>`), its files kept for an
/// administrator, so the new account gets a folder it can use instead of
/// one it is locked out of. Only root writes `homes`.
fn fresh_home(homes: &Path, user: &str, uid: u32) -> Result<bool, OperationError> {
    let home = homes.join(user);
    for _ in 0..2 {
        match fs::create_dir(&home) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = fs::symlink_metadata(&home).map_err(|e| OperationError::Read(home.clone(), e))?;
                if !meta.is_dir() {
                    return Err(OperationError::Refused(format!("{} is not a folder", home.display())));
                }
                if meta.uid() == uid {
                    return Ok(false);
                }
                let aside = homes.join(format!("{user}.previous-{}", meta.uid()));
                rustix::fs::renameat_with(CWD, &home, CWD, &aside, rustix::fs::RenameFlags::NOREPLACE)
                    .map_err(|e| OperationError::Write(home.clone(), e.into()))?;
                tracing::warn!(user, aside = %aside.display(), "a personal folder of an earlier account was moved aside");
            }
            Err(error) => return Err(OperationError::Write(home, error)),
        }
    }
    Err(OperationError::Refused(format!("{} keeps being taken", home.display())))
}

/// The share's `.recycle`, made if missing, opened without following a
/// link. Something else of that name (a writer may make a file or link
/// there) is moved aside, so the bins work again.
fn open_bin(share: &Path, name: &ShareName) -> Result<OwnedFd, OperationError> {
    let bin = share.join(RECYCLE_DIRECTORY);
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let make = || match fs::create_dir(&bin) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            Err(OperationError::Write(bin.clone(), error))
        }
        _ => Ok(()),
    };
    make()?;
    if let Ok(directory) = openat(CWD, &bin, flags, Mode::empty()) {
        return Ok(directory);
    }
    let since =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let aside = share.join(format!("{RECYCLE_DIRECTORY}.not-a-folder-{since}-{}", std::process::id()));
    rustix::fs::renameat_with(CWD, &bin, CWD, &aside, rustix::fs::RenameFlags::NOREPLACE)
        .map_err(|e| OperationError::Write(bin.clone(), e.into()))?;
    tracing::warn!(share = name.as_str(), aside = %aside.display(), "what took the recycle bin's place was moved aside");
    make()?;
    openat(CWD, &bin, flags, Mode::empty()).map_err(|_| {
        OperationError::Refused(format!("{RECYCLE_DIRECTORY} in {} is not a folder", name.as_str()))
    })
}

impl Sharing {
    /// The rules for what is in a share, by ID: what setfacl would look up
    /// by name. Who is removed but no longer known has no entries to find.
    fn rules(&self, access: &[ShareAccess], removed: &[Principal]) -> Result<Rules, OperationError> {
        let read = |path: &std::path::Path| {
            fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
        };
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let id = |subject: &Principal| match subject {
            Principal::User(user) => {
                crate::passwd::lookup(&passwd, user.as_str()).map(|(uid, _)| Who::User(uid))
            }
            Principal::Group(name) => crate::passwd::group_id(&group, name.as_str()).map(Who::Group),
        };
        let mut grant = Vec::new();
        for entry in access {
            let who = id(&entry.principal)
                .ok_or_else(|| OperationError::NotManaged(principal(&entry.principal)))?;
            grant.push((who, entry.permission));
        }
        let mut remove = Vec::new();
        for subject in removed {
            match id(subject) {
                Some(who) if !grant.iter().any(|(granted, _)| *granted == who) => remove.push(who),
                Some(_) => {}
                None => tracing::info!(
                    subject = principal(subject),
                    "no longer known: nothing of theirs to remove"
                ),
            }
        }
        Ok(Rules { remove, grant, owners: owners(access, &passwd, &group) })
    }

    /// The managed users who may write, directly or through a group, with
    /// their UID and primary GID.
    fn writers(&self, access: &[ShareAccess]) -> Result<Vec<(String, u32, u32)>, OperationError> {
        let read = |path: &std::path::Path| {
            fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
        };
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let mut names: Vec<String> = Vec::new();
        for entry in access.iter().filter(|a| a.writes()) {
            match &entry.principal {
                Principal::User(user) => names.push(user.as_str().to_owned()),
                Principal::Group(name) => {
                    names
                        .extend(crate::passwd::members(&group, name.as_str()).into_iter().map(str::to_owned));
                }
            }
        }
        names.sort();
        names.dedup();
        // A denied user gets no bin, even as a member of a group that writes.
        let denied: Vec<&str> = access
            .iter()
            .filter(|a| a.denied())
            .filter_map(|a| match &a.principal {
                Principal::User(user) => Some(user.as_str()),
                Principal::Group(_) => None,
            })
            .collect();
        names.retain(|name| !denied.contains(&name.as_str()));
        let users_group = crate::passwd::group_id(&group, crate::accounts::USERS_GROUP);
        Ok(names
            .into_iter()
            .filter(|name| zendiloos_agent_proto::is_file_name(name))
            .filter_map(|name| {
                let (uid, gid) = crate::passwd::lookup(&passwd, &name)?;
                let managed = (crate::accounts::ID_MIN..=crate::accounts::ID_MAX).contains(&uid)
                    && users_group == Some(gid);
                managed.then_some((name, uid, gid))
            })
            .collect())
    }
}

/// The managed accounts that may write by `access`, as the kernel decides
/// it: by an entry of the account's own if it has one, otherwise by the
/// entry of any group it is in (its primary group or a listed one).
fn owners(access: &[ShareAccess], passwd: &str, group: &str) -> Owners {
    let managed = crate::accounts::ID_MIN..=crate::accounts::ID_MAX;
    let writing_groups: Vec<u32> = access
        .iter()
        .filter(|entry| entry.permission == SharePermission::ReadWrite)
        .filter_map(|entry| match &entry.principal {
            Principal::Group(name) => crate::passwd::group_id(group, name.as_str()),
            Principal::User(_) => None,
        })
        .collect();
    let writes = |name: &str, gid: u32| {
        let own = access
            .iter()
            .find(|entry| matches!(&entry.principal, Principal::User(user) if user.as_str() == name));
        match own {
            Some(entry) => entry.permission == SharePermission::ReadWrite,
            None => {
                writing_groups.contains(&gid)
                    || crate::passwd::memberships(group, name).iter().any(|id| writing_groups.contains(id))
            }
        }
    };
    let writers = crate::passwd::accounts(passwd)
        .filter(|(name, uid, gid)| managed.contains(uid) && writes(name, *gid))
        .map(|(_, uid, _)| uid)
        .collect();
    Owners { managed: Some(managed), writers }
}

/// Makes `user`'s bin in the share's `.recycle` (opened as `bin`): theirs,
/// mode 0700, without ACL entries. One somebody else made first is moved
/// aside, so it never receives the user's deletions.
fn prepare_bin(bin: &fs::File, user: &str, uid: u32, gid: u32) -> Result<(), OperationError> {
    use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with, statat};
    let failed = |e: rustix::io::Errno| OperationError::Write(PathBuf::from(user), e.into());
    match statat(bin, user, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if stat.st_uid == uid && rustix::fs::FileType::from_raw_mode(stat.st_mode).is_dir() => {
            return Ok(());
        }
        Ok(_) => {
            let aside = format!(".{user}.not-theirs-{}", std::process::id());
            renameat_with(bin, user, bin, aside.as_str(), RenameFlags::NOREPLACE).map_err(failed)?;
            tracing::warn!(user, aside, "a recycle bin somebody else made was moved aside");
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(failed(error)),
    }
    mkdirat(bin, user, Mode::from_raw_mode(0o700)).map_err(failed)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let own = fs::File::from(openat(bin, user, flags, Mode::empty()).map_err(failed)?);
    std::os::unix::fs::fchown(&own, Some(uid), Some(gid))
        .map_err(|e| OperationError::Write(PathBuf::from(user), e))?;
    // The share's default ACL was inherited; the bin is private.
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        match rustix::fs::fremovexattr(&own, name) {
            Ok(()) | Err(rustix::io::Errno::NODATA) | Err(rustix::io::Errno::OPNOTSUPP) => {}
            Err(error) => return Err(failed(error)),
        }
    }
    rustix::fs::fchmod(&own, Mode::from_raw_mode(0o700)).map_err(failed)?;
    Ok(())
}

/// Folders deeper than this are not looked into; what is below counts as
/// not changed.
const MAX_DEPTH: usize = if cfg!(test) { 8 } else { 256 };
/// Entries that may fail before a change of a whole tree gives up.
const MAX_FAILURES: usize = 100;

/// The entries a change of a tree could not change.
#[derive(Debug, Default)]
struct Failures {
    count: usize,
    first: Option<String>,
}

impl Failures {
    /// Notes one; too many end the change.
    fn note(&mut self, path: &Path, error: Errno) -> Result<(), OperationError> {
        self.note_why(path, std::io::Error::from(error))
    }

    fn note_why(&mut self, path: &Path, why: impl std::fmt::Display) -> Result<(), OperationError> {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some(format!("{}: {why}", path.display()));
        }
        if self.count > MAX_FAILURES { Err(self.error()) } else { Ok(()) }
    }

    fn error(&self) -> OperationError {
        OperationError::Failed(format!(
            "the permissions of {} entries could not be changed, among them {}",
            self.count,
            self.first.as_deref().unwrap_or("?")
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Entries removed and what is owned handed over.
    Remove,
    Grant,
}

/// Changes the permissions of everything below a share's root (not the
/// root, which is set on its own): access taken away from the whole tree
/// first, entries and what those who may not write own (see [`Owners`]),
/// so it is gone even if giving fails. Each folder and file is
/// reached by descriptor and its ACL changed through it, never by its path:
/// whoever may write the share could swap a folder for a link while the
/// change goes on, and a walk by path would follow it, onto other shares,
/// the bins or `@docker`.
///
/// Left alone: links, other file systems, what is neither folder nor file,
/// and the entries of the recycle bins and of every other private folder
/// with what is in it (a folder only its owner may enter: renamed bins
/// among them, which nobody else could reach anyway, and those passed to
/// root from someone who may no longer write, which stay private). What is
/// in them is still handed over. Only the folders on the way
/// down are open at a time, each read as it goes, never held whole: a
/// folder of millions of entries must not take the agent's memory. An entry
/// that cannot be changed, or a folder too deep to look into, does not stop
/// the others; the change fails at the end, naming one of them.
fn change_tree(root: &Path, rules: &Rules) -> Result<(), OperationError> {
    let mut failures = Failures::default();
    if !rules.remove.is_empty() || rules.owners.managed.is_some() {
        walk(root, rules, Pass::Remove, &mut failures)?;
    }
    if !rules.grant.is_empty() {
        walk(root, rules, Pass::Grant, &mut failures)?;
    }
    if failures.count > 0 { Err(failures.error()) } else { Ok(()) }
}

const FOLDER: OFlags = OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

/// A folder on the way down, read as the walk goes: changing permissions
/// adds and removes no names, so reading while changing misses nothing.
struct Level {
    folder: rustix::fs::Dir,
    path: PathBuf,
    /// The share's root, where the recycle bins' folder is (`set_access`
    /// sets it on its own).
    top: bool,
    /// In the bins or a private folder: only owners change.
    private: bool,
}

impl Level {
    /// The next name to look at, and whether it is the bins' folder;
    /// `None` once the folder is read.
    fn next_name(&mut self) -> Option<Result<(CString, bool), Errno>> {
        loop {
            let entry = match self.folder.next()? {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error)),
            };
            let name = entry.file_name();
            if name != c"." && name != c".." {
                let bin = self.top && name.to_bytes() == RECYCLE_DIRECTORY.as_bytes();
                return Some(Ok((name.to_owned(), bin)));
            }
        }
    }
}

/// An entry of a share as opened.
struct Found {
    entry: OwnedFd,
    folder: bool,
    mode: u32,
    owner: u32,
}

impl Found {
    /// Only its owner may enter it.
    fn is_private(&self) -> bool {
        self.folder && self.mode & 0o077 == 0
    }
}

fn walk(root: &Path, rules: &Rules, pass: Pass, failures: &mut Failures) -> Result<(), OperationError> {
    let unreadable = |e: Errno| OperationError::Read(root.to_owned(), e.into());
    let top = openat(CWD, root, FOLDER, Mode::empty()).map_err(unreadable)?;
    let device = rustix::fs::fstat(&top).map_err(unreadable)?.st_dev;
    let top = rustix::fs::Dir::new(top).map_err(unreadable)?;
    let mut stack = vec![Level { folder: top, path: root.to_owned(), top: true, private: false }];
    while let Some(level) = stack.last_mut() {
        let (name, bin) = match level.next_name() {
            Some(Ok(named)) => named,
            Some(Err(error)) => {
                // What is left of this folder cannot be read.
                let path = level.path.clone();
                stack.pop();
                failures.note(&path, error)?;
                continue;
            }
            None => {
                stack.pop();
                continue;
            }
        };
        let path = level.path.join(OsStr::from_bytes(name.as_bytes()));
        let within = level.private;
        if pass == Pass::Grant && (within || bin) {
            continue;
        }
        let opened = level.folder.fd().and_then(|fd| open_entry(fd, &name, device));
        let found = match opened {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                failures.note(&path, error)?;
                continue;
            }
        };
        let private = within || bin || found.is_private();
        if pass == Pass::Grant && private {
            continue;
        }
        if let Err(error) = change_entry(&found, private, rules, pass) {
            failures.note(&path, error)?;
        }
        if !found.folder {
            continue;
        }
        if stack.len() >= MAX_DEPTH {
            failures.note_why(&path, "folders are nested too deeply to look into")?;
            continue;
        }
        match rustix::fs::Dir::new(found.entry) {
            Ok(inner) => stack.push(Level { folder: inner, path, top: false, private }),
            Err(error) => failures.note(&path, error)?,
        }
    }
    Ok(())
}

/// `name` in `folder` opened; `None` for what is left alone (links, other
/// file systems, what is neither folder nor file), or went or changed
/// since it was listed.
fn open_entry(folder: impl AsFd, name: &CStr, device: u64) -> Result<Option<Found>, Errno> {
    let folder = folder.as_fd();
    use rustix::fs::FileType;
    let stat = match rustix::fs::statat(folder, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    let kind = FileType::from_raw_mode(stat.st_mode);
    let folder_kind = match kind {
        FileType::Directory => true,
        FileType::RegularFile => false,
        _ => return Ok(None),
    };
    if stat.st_dev != device {
        return Ok(None);
    }
    let flags = if folder_kind { FOLDER } else { OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC };
    let entry = match openat(folder, name, flags, Mode::empty()) {
        Ok(entry) => entry,
        // Gone, or swapped for a link or something else, since.
        Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR) => return Ok(None),
        Err(error) => return Err(error),
    };
    let opened = rustix::fs::fstat(&entry)?;
    if FileType::from_raw_mode(opened.st_mode) != kind || opened.st_dev != device {
        return Ok(None);
    }
    Ok(Some(Found { entry, folder: folder_kind, mode: opened.st_mode, owner: opened.st_uid }))
}

/// One pass's change of `found`, through `/proc/self/fd`: the attributes
/// of what was opened, whatever its path is by now. Default entries for
/// folders only; of a `private` one only its owner, in the taking pass.
fn change_entry(found: &Found, private: bool, rules: &Rules, pass: Pass) -> Result<(), Errno> {
    if !private {
        change_acl(&found.entry, found.folder, found.mode, rules, pass)?;
    }
    if pass == Pass::Remove && rules.owners.hand_over(found.owner) {
        // Its group stays: the share's folder gives that group nothing.
        rustix::fs::chownat(
            &found.entry,
            c"",
            Some(rustix::fs::Uid::ROOT),
            None,
            rustix::fs::AtFlags::EMPTY_PATH,
        )?;
    }
    Ok(())
}

fn change_acl(entry: &OwnedFd, folder: bool, mode: u32, rules: &Rules, pass: Pass) -> Result<(), Errno> {
    let path = format!("/proc/self/fd/{}", entry.as_raw_fd());
    let access = read_acl(&path, acl::ACCESS)?;
    let changed = match pass {
        Pass::Remove => acl::without(access.clone(), mode, rules),
        Pass::Grant => acl::with(access.clone(), mode, folder, rules),
    };
    if let Some(acl) = &changed {
        rustix::fs::setxattr(&path, acl::ACCESS, &acl.to_bytes(), rustix::fs::XattrFlags::empty())?;
    }
    if folder {
        let default = read_acl(&path, acl::DEFAULT)?;
        let changed_default = match pass {
            Pass::Remove => acl::default_without(default, rules),
            Pass::Grant => {
                let now = changed.or(access).unwrap_or_else(|| Acl::from_mode(mode));
                acl::default_with(default, &now, rules)
            }
        };
        if let Some(acl) = changed_default {
            rustix::fs::setxattr(&path, acl::DEFAULT, &acl.to_bytes(), rustix::fs::XattrFlags::empty())?;
        }
    }
    Ok(())
}

/// An ACL attribute; `None` if there is none. One the kernel would not
/// write is not guessed at.
fn read_acl(path: &str, name: &str) -> Result<Option<Acl>, Errno> {
    let mut buffer = vec![0u8; 4 + 8 * 64];
    let length = match rustix::fs::getxattr(path, name, &mut buffer[..]) {
        Ok(length) => length,
        Err(Errno::NODATA) => return Ok(None),
        Err(Errno::RANGE) => {
            let size = rustix::fs::getxattr(path, name, &mut [0u8; 0][..])?;
            buffer = vec![0u8; size];
            rustix::fs::getxattr(path, name, &mut buffer[..])?
        }
        Err(error) => return Err(error),
    };
    Acl::parse(&buffer[..length]).map(Some).ok_or(Errno::INVAL)
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{
        GroupName, Network, NfsClient, ShareComment, SmbProtocol, UserName, Workgroup,
    };

    use super::*;

    /// SMB on, NFS off, everything else as a fresh install has it.
    fn base_services() -> FileServices {
        FileServices {
            smb: true,
            nfs: false,
            workgroup: Workgroup::parse("WORKGROUP").unwrap(),
            smb_min_protocol: SmbProtocol::Smb2_1,
            smb_encryption: zendiloos_agent_proto::SmbEncryption::Default,
            smb_signing: zendiloos_agent_proto::SmbSigning::Default,
            hide_unreadable: true,
            multichannel: true,
            oplocks: true,
            leases: true,
            durable_handles: false,
            mac_compat: true,
            follow_symlinks: true,
            veto_files: None,
            delete_veto_files: false,
            wsdd: true,
            bonjour: true,
        }
    }

    #[test]
    fn each_writer_gets_a_private_bin_and_nothing_else_takes_its_name() {
        use std::os::unix::fs::PermissionsExt;
        let recycle = tempfile::tempdir().unwrap();
        let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
        let bin = fs::File::open(recycle.path()).unwrap();
        // Something of that name made first, not a folder of the user's.
        fs::write(recycle.path().join("anna"), "planted").unwrap();

        prepare_bin(&bin, "anna", uid, gid).unwrap();
        prepare_bin(&bin, "anna", uid, gid).unwrap();

        let made = fs::metadata(recycle.path().join("anna")).unwrap();
        assert!(made.is_dir());
        assert_eq!((made.uid(), made.permissions().mode() & 0o777), (uid, 0o700));
        let names: Vec<String> = fs::read_dir(recycle.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "the planted one moved aside: {names:?}");
        assert!(names.iter().any(|n| n.starts_with(".anna.not-theirs-")), "{names:?}");
    }

    fn access(principal: Principal, write: bool) -> ShareAccess {
        let permission = if write { SharePermission::ReadWrite } else { SharePermission::Read };
        ShareAccess { principal, permission }
    }

    /// The sharing of a host whose accounts are anna (2001) and ben (2002),
    /// both in `family`, and the owner's own user (1000), unmanaged.
    fn with_accounts(dir: &Path) -> Sharing {
        fs::write(
            dir.join("passwd"),
            "root:x:0:0::/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n\
             anna:x:2001:990::/nonexistent:/usr/sbin/nologin\nben:x:2002:990::/nonexistent:/usr/sbin/nologin\n",
        )
        .unwrap();
        fs::write(dir.join("group"), "root:x:0:\nzendiloos-users:x:990:\nfamily:x:2100:anna,ben,debian\n")
            .unwrap();
        let accounts = crate::accounts::AccountFiles {
            passwd: dir.join("passwd"),
            group: dir.join("group"),
            high_water: dir.join("high-water"),
        };
        Sharing::new(&Tools::default(), HostPaths::default(), accounts).unwrap()
    }

    fn user(name: &str) -> Principal {
        Principal::User(UserName::parse(name).unwrap())
    }

    fn group(name: &str) -> Principal {
        Principal::Group(GroupName::parse(name).unwrap())
    }

    #[test]
    fn writers_are_the_managed_users_who_may_write_and_are_not_denied() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = with_accounts(dir.path());
        let denied = ShareAccess { principal: user("ben"), permission: SharePermission::Deny };
        let writers =
            sharing.writers(&[access(group("family"), true), denied, access(user("debian"), true)]).unwrap();
        assert_eq!(writers, [("anna".to_owned(), 2001, 990)], "ben is denied, debian is no managed user");
        assert!(sharing.writers(&[access(user("anna"), false)]).unwrap().is_empty(), "reading makes no bin");
    }

    #[test]
    fn rules_name_their_subjects_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = with_accounts(dir.path());
        let rules = sharing
            .rules(
                &[access(user("anna"), true), access(group("family"), false)],
                &[user("ben"), user("anna"), user("gone")],
            )
            .unwrap();
        assert_eq!(
            rules.grant,
            [(Who::User(2001), SharePermission::ReadWrite), (Who::Group(2100), SharePermission::Read)]
        );
        assert_eq!(rules.remove, [Who::User(2002)], "anna keeps hers, a gone user has none");
        let unknown = sharing.rules(&[access(user("nobody"), true)], &[]).unwrap_err();
        assert!(unknown.to_string().contains("nobody"), "{unknown}");
        fs::remove_file(dir.path().join("group")).unwrap();
        assert!(sharing.rules(&[], &[]).is_err(), "the accounts must be readable");
        assert!(sharing.writers(&[]).is_err());
    }

    #[test]
    fn a_personal_folder_of_an_earlier_account_is_moved_aside() {
        let homes = tempfile::tempdir().unwrap();
        let me = rustix::process::getuid().as_raw();
        assert!(fresh_home(homes.path(), "anna", me).unwrap(), "made");
        assert!(!fresh_home(homes.path(), "anna", me).unwrap(), "hers already");

        // The same name, another account now.
        assert!(fresh_home(homes.path(), "anna", me + 1).unwrap(), "made anew");
        let aside = homes.path().join(format!("anna.previous-{me}"));
        assert!(aside.is_dir(), "the earlier one's files are kept");

        fs::write(homes.path().join("ben"), "not a folder").unwrap();
        let refused = fresh_home(homes.path(), "ben", me).unwrap_err();
        assert!(refused.to_string().contains("not a folder"), "{refused}");
    }

    #[test]
    fn a_home_needs_the_personal_folders_dataset_mounted() {
        let dir = tempfile::tempdir().unwrap();
        let accounts = crate::accounts::AccountFiles::default();
        let paths = HostPaths { volumes: dir.path().to_owned(), ..HostPaths::default() };
        let sharing = Sharing::new(&Tools::default(), paths, accounts).unwrap();
        let (volume, anna) = (VolumeNumber::new(1).unwrap(), UserName::parse("anna").unwrap());
        let missing = sharing.make_home(volume, &anna).unwrap_err();
        assert!(missing.to_string().contains("not set up"), "{missing}");
        fs::create_dir_all(dir.path().join("volume1").join(HOMES_DIRECTORY)).unwrap();
        let unmounted = sharing.make_home(volume, &anna).unwrap_err();
        assert!(unmounted.to_string().contains("not mounted"), "{unmounted}");
    }

    /// Sharing with testparm and systemctl as scripts that record what they
    /// are asked; testparm refuses while `refuse` is there.
    fn applying(dir: &Path, exportfs: bool) -> Sharing {
        let log = dir.join("calls.log");
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            path
        };
        let tools = Tools {
            testparm: script(
                "testparm",
                &format!("[ -f {} ] && exit 1; exit 0", dir.join("refuse").display()),
            ),
            systemctl: script("systemctl", ""),
            exportfs: if exportfs { script("exportfs", "") } else { dir.join("no-exportfs") },
            ..Tools::default()
        };
        let paths = HostPaths {
            samba_config: dir.join("samba/zendiloos.conf"),
            nfs_exports: dir.join("exports.d/zendiloos.exports"),
            ..HostPaths::default()
        };
        Sharing::new(&tools, paths, crate::accounts::AccountFiles::default()).unwrap()
    }

    #[tokio::test]
    async fn what_samba_would_not_take_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = applying(dir.path(), true);
        let share = |name: &str, time_machine: bool| SmbShare {
            name: ShareName::parse(name).unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            comment: None,
            access: vec![access(user("anna"), true)],
            recycle_bin: false,
            time_machine: time_machine.then_some(None),
        };
        let mut services = base_services();
        services.mac_compat = false;
        let refused = sharing.apply(&[share("backup", true)], &[], &services, None).await.unwrap_err();
        assert!(refused.to_string().contains("macOS compatibility"), "{refused}");
        let homes = HomesShare { volume: VolumeNumber::new(1).unwrap(), users: vec![] };
        let refused =
            sharing.apply(&[share("home", false)], &[], &base_services(), Some(&homes)).await.unwrap_err();
        assert!(refused.to_string().contains("called home"), "{refused}");

        fs::write(dir.path().join("refuse"), "").unwrap();
        assert!(sharing.apply(&[share("media", false)], &[], &base_services(), None).await.is_err());
        assert!(!dir.path().join("samba/zendiloos.conf").exists(), "not written");
        assert!(!dir.path().join("samba/zendiloos.check").exists(), "the candidate goes");
        fs::remove_file(dir.path().join("refuse")).unwrap();

        // Everything off: the servers stop and stay off.
        let mut off = base_services();
        (off.smb, off.nfs, off.wsdd, off.bonjour) = (false, false, false, false);
        assert_eq!(sharing.apply(&[], &[], &off, None).await.unwrap(), Response::Done);
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        for stopped in
            ["disable --now smbd.service", "disable --now wsdd2.service", "disable --now nfs-server.service"]
        {
            assert!(calls.contains(stopped), "{stopped}: {calls}");
        }
    }

    #[tokio::test]
    async fn exports_need_the_nfs_server() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = applying(dir.path(), false);
        let export = NfsExport {
            name: ShareName::parse("media").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            clients: vec![NfsClient { network: Network::parse("192.168.1.0/24").unwrap(), write: false }],
        };
        let mut services = base_services();
        services.nfs = true;
        let refused = sharing.apply(&[], std::slice::from_ref(&export), &services, None).await.unwrap_err();
        assert!(refused.to_string().contains("not installed"), "{refused}");
        // Without exports nothing is missing.
        assert_eq!(sharing.apply(&[], &[], &services, None).await.unwrap(), Response::Done);

        let installed = applying(dir.path(), true);
        assert_eq!(installed.apply(&[], &[export], &services, None).await.unwrap(), Response::Done);
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert!(
            calls.contains("enable --now nfs-server.service")
                && calls.contains("reload-or-restart nfs-server")
        );
    }

    fn acl_of(path: &Path, name: &str) -> Option<Acl> {
        read_acl(path.to_str().unwrap(), name).unwrap()
    }

    #[test]
    fn a_tree_is_changed_through_descriptors_and_private_folders_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        fs::create_dir_all(root.join("album/2024")).unwrap();
        fs::write(root.join("album/2024/a.jpg"), "x").unwrap();
        fs::write(root.join("notes.txt"), "x").unwrap();
        fs::write(root.join("run.sh"), "x").unwrap();
        fs::set_permissions(root.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        fs::create_dir_all(root.join(RECYCLE_DIRECTORY).join("anna")).unwrap();
        // A bin a writer renamed: private, so nobody else's entries go there.
        let renamed = root.join("old-bin/bob");
        fs::create_dir_all(&renamed).unwrap();
        fs::write(renamed.join("deleted.txt"), "x").unwrap();
        fs::set_permissions(&renamed, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("escape")).unwrap();
        let (bob, anna, staff) = (Who::User(4242), Who::User(4243), Who::Group(4244));

        let before = Rules { grant: vec![(bob, SharePermission::ReadWrite)], ..Rules::default() };
        change_tree(&root, &before).unwrap();
        let access = |path: &str| acl_of(&root.join(path), acl::ACCESS).unwrap();
        assert_eq!(access("album").named(bob), Some(7));
        assert_eq!(acl_of(&root.join("album"), acl::DEFAULT).unwrap().named(bob), Some(7));
        assert_eq!(access("album/2024/a.jpg").named(bob), Some(6), "no execute on a plain file");
        assert_eq!(access("run.sh").named(bob), Some(7), "a program stays one");

        let after = Rules {
            remove: vec![bob],
            grant: vec![(anna, SharePermission::Read), (staff, SharePermission::ReadWrite)],
            ..Rules::default()
        };
        change_tree(&root, &after).unwrap();
        for folder in ["album", "album/2024", "old-bin"] {
            let acl = access(folder);
            assert_eq!(
                (acl.named(bob), acl.named(anna), acl.named(staff)),
                (None, Some(5), Some(7)),
                "{folder}"
            );
            let default = acl_of(&root.join(folder), acl::DEFAULT).unwrap();
            assert_eq!((default.named(bob), default.named(anna)), (None, Some(5)), "{folder}");
        }
        assert_eq!(access("notes.txt").named(anna), Some(4));
        assert_eq!(access("notes.txt").named(bob), None);
        assert_eq!(access("run.sh").named(anna), Some(5));
        assert!(acl_of(&elsewhere, acl::ACCESS).is_none(), "not through the link");
        assert!(acl_of(&root.join(RECYCLE_DIRECTORY).join("anna"), acl::ACCESS).is_none(), "not the bins");
        assert!(acl_of(&renamed, acl::ACCESS).is_none(), "not a private folder");
        assert!(acl_of(&renamed.join("deleted.txt"), acl::ACCESS).is_none(), "nor what is in it");
    }

    #[test]
    fn a_wide_tree_keeps_only_the_folders_on_the_way_down_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        for n in 0..1500 {
            fs::create_dir_all(root.join(format!("{n}/inner"))).unwrap();
        }
        let anna = Who::User(4243);
        change_tree(&root, &Rules { grant: vec![(anna, SharePermission::Read)], ..Rules::default() })
            .unwrap();
        for n in [0, 749, 1499] {
            assert_eq!(acl_of(&root.join(format!("{n}/inner")), acl::ACCESS).unwrap().named(anna), Some(5));
        }
    }

    #[test]
    fn a_files_owner_writes_it_whatever_its_own_named_entry_says() {
        // Why what a reader or a denied account owns passes to root: the
        // kernel asks the owner's entry first, and the owner may change it.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mine.txt");
        fs::write(&file, "x").unwrap();
        let me = rustix::process::getuid().as_raw();
        let mut acl = Acl::from_mode(0o640);
        acl.set(Who::User(me), 0);
        acl.fix_mask();
        rustix::fs::setxattr(&file, acl::ACCESS, &acl.to_bytes(), rustix::fs::XattrFlags::empty()).unwrap();
        assert_eq!(acl_of(&file, acl::ACCESS).unwrap().named(Who::User(me)), Some(0));

        assert!(fs::OpenOptions::new().append(true).open(&file).is_ok(), "written despite user:{me}:---");
        fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o440)).unwrap();
        fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();
    }

    #[test]
    fn what_those_who_may_not_write_own_is_handed_to_root_bins_and_private_folders_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        fs::create_dir_all(root.join("album")).unwrap();
        fs::write(root.join("album/a.jpg"), "x").unwrap();
        fs::create_dir_all(root.join(RECYCLE_DIRECTORY).join("ben")).unwrap();
        fs::write(root.join(RECYCLE_DIRECTORY).join("ben/old.txt"), "x").unwrap();
        let private = root.join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("secret.txt"), "x").unwrap();
        fs::set_permissions(&private, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("escape")).unwrap();
        let me = rustix::process::getuid().as_raw();
        let anna = Who::User(4243);
        let writing = |writers: Vec<u32>| Rules {
            grant: vec![(anna, SharePermission::Read)],
            owners: Owners { managed: Some(me..=me), writers },
            ..Rules::default()
        };

        change_tree(&root, &writing(vec![me])).unwrap();
        assert_eq!(fs::metadata(root.join("album/a.jpg")).unwrap().uid(), me, "a writer keeps theirs");

        let changed = change_tree(&root, &writing(vec![]));
        // Seven entries: the album and its photo, the bins' folder, a bin
        // and what is in it, the private folder and its file; not the link.
        if rustix::process::geteuid().is_root() {
            changed.unwrap();
            for path in ["album", "album/a.jpg", ".recycle", ".recycle/ben/old.txt", "private/secret.txt"] {
                assert_eq!(fs::metadata(root.join(path)).unwrap().uid(), 0, "{path}");
            }
        } else {
            let error = changed.unwrap_err().to_string();
            assert!(error.contains("of 7 entries") && error.contains("Operation not permitted"), "{error}");
        }
        assert!(acl_of(&private, acl::ACCESS).is_none(), "a private folder gets no entries");
        assert!(acl_of(&root.join(RECYCLE_DIRECTORY).join("ben"), acl::ACCESS).is_none(), "nor a bin");
        assert_eq!(acl_of(&root.join("album"), acl::ACCESS).unwrap().named(anna), Some(5));
    }

    #[test]
    fn the_owners_are_the_managed_accounts_that_may_write() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = with_accounts(dir.path());
        let owners = |access: &[ShareAccess]| sharing.rules(access, &[]).unwrap().owners;
        let reads = |principal| ShareAccess { principal, permission: SharePermission::Read };
        let denied = |principal| ShareAccess { principal, permission: SharePermission::Deny };

        let family = owners(&[access(group("family"), true), reads(user("ben"))]);
        assert_eq!(family.writers, [2001], "ben's own rule wins over his group's");
        assert_eq!(family.managed, Some(crate::accounts::ID_MIN..=crate::accounts::ID_MAX));
        assert!(!family.writers.contains(&1000), "debian is in family but no managed account");
        assert_eq!(owners(&[access(group("family"), true), denied(user("anna"))]).writers, [2002]);
        assert_eq!(owners(&[access(user("ben"), true), reads(group("family"))]).writers, [2002]);
        assert_eq!(
            owners(&[access(group("zendiloos-users"), true)]).writers,
            [2001, 2002],
            "a primary group"
        );
        assert!(owners(&[reads(group("family"))]).writers.is_empty());
    }

    #[test]
    fn what_lies_too_deep_to_look_into_fails_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        let mut deep = root.clone();
        for n in 0..MAX_DEPTH + 2 {
            deep = deep.join(format!("d{n}"));
        }
        fs::create_dir_all(&deep).unwrap();
        let anna = Who::User(4243);

        let failed =
            change_tree(&root, &Rules { grant: vec![(anna, SharePermission::Read)], ..Rules::default() });

        let error = failed.unwrap_err().to_string();
        assert!(error.contains("nested too deeply") && error.contains("1 entries"), "{error}");
        assert_eq!(acl_of(&root.join("d0"), acl::ACCESS).unwrap().named(anna), Some(5), "above it: changed");
    }

    #[test]
    fn the_bins_folder_is_sticky_and_keeps_its_mask() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(RECYCLE_DIRECTORY);
        fs::create_dir(&bin).unwrap();
        let mut acl = Acl::from_mode(0o770);
        acl.set(Who::User(4243), 7);
        acl.fix_mask();
        rustix::fs::setxattr(&bin, acl::ACCESS, &acl.to_bytes(), rustix::fs::XattrFlags::empty()).unwrap();
        let folder = fs::File::open(&bin).unwrap();

        sticky(&folder).unwrap();

        let mode = fs::metadata(&bin).unwrap().mode();
        assert_eq!(mode & 0o7777, 0o1770, "{mode:o}");
        assert_eq!(acl_of(&bin, acl::ACCESS).unwrap().named(Who::User(4243)), Some(7));
    }

    #[test]
    fn a_personal_folder_of_an_earlier_account_of_the_name_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let homes = dir.path();
        let me = fs::metadata(homes).unwrap().uid();
        assert!(fresh_home(homes, "alice", me).unwrap(), "made");
        fs::write(homes.join("alice/diary.txt"), "old").unwrap();
        assert!(!fresh_home(homes, "alice", me).unwrap(), "hers already");

        // A new account of the name has another UID.
        assert!(fresh_home(homes, "alice", me + 1).unwrap(), "made anew");
        assert_eq!(fs::read_dir(homes.join("alice")).unwrap().count(), 0, "fresh");
        let aside = homes.join(format!("alice.previous-{me}"));
        assert_eq!(fs::read_to_string(aside.join("diary.txt")).unwrap(), "old", "the old files stay");

        fs::write(homes.join("bob"), "").unwrap();
        assert!(fresh_home(homes, "bob", me).is_err(), "not a folder");
    }

    #[test]
    fn failures_are_counted_and_too_many_end_the_change() {
        let mut failures = Failures::default();
        for _ in 0..MAX_FAILURES {
            failures.note(Path::new("/volume1/media/a"), Errno::IO).unwrap();
        }
        let error = failures.note(Path::new("/volume1/media/b"), Errno::IO).unwrap_err();
        assert!(error.to_string().contains("101 entries"), "{error}");
        assert!(error.to_string().contains("/volume1/media/a"), "{error}");
    }

    #[test]
    fn what_takes_the_bins_place_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let name = ShareName::parse("media").unwrap();
        for (share, odd) in [("file", true), ("link", false)] {
            let root = dir.path().join(share);
            fs::create_dir(&root).unwrap();
            if odd {
                fs::write(root.join(RECYCLE_DIRECTORY), "not a folder").unwrap();
            } else {
                std::os::unix::fs::symlink(dir.path(), root.join(RECYCLE_DIRECTORY)).unwrap();
            }
            let bin = open_bin(&root, &name).unwrap();
            assert!(rustix::fs::fstat(&bin).map(|s| s.st_mode & 0o170000 == 0o040000).unwrap());
            assert!(fs::symlink_metadata(root.join(RECYCLE_DIRECTORY)).unwrap().is_dir());
            let aside: Vec<String> = fs::read_dir(&root)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n.starts_with(".recycle.not-a-folder-"))
                .collect();
            assert_eq!(aside.len(), 1, "{share}: {aside:?}");
        }
    }

    #[tokio::test]
    async fn a_change_of_a_shares_folder_does_not_wait_long_for_one_through_its_files() {
        let sharing =
            Sharing::new(&Tools::default(), HostPaths::default(), crate::accounts::AccountFiles::default())
                .unwrap();
        let volume = VolumeNumber::new(1).unwrap();
        let (media, photos) = (ShareName::parse("media").unwrap(), ShareName::parse("photos").unwrap());
        let walking = sharing.share_lock(volume, &media);
        let _walking = walking.lock().await;

        let refused = sharing.set_access(volume, &media, &[], &[], false).await.unwrap_err();
        assert!(refused.to_string().contains("being applied to its files"), "{refused}");
        // Another share is not held up: it gets as far as finding its folder.
        let other = sharing.set_access(volume, &photos, &[], &[], false).await.unwrap_err();
        assert!(other.to_string().contains("not present"), "{other}");
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
                time_machine: None,
            },
            SmbShare {
                name: ShareName::parse("nobody").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                access: vec![],
                recycle_bin: false,
                comment: None,
                time_machine: None,
            },
        ];
        let services = FileServices {
            smb: true,
            nfs: false,
            workgroup: Workgroup::parse("HOME").unwrap(),
            smb_min_protocol: SmbProtocol::Smb3,
            ..base_services()
        };
        let text = samba_config(&shares, &services, None);
        let globals = &text[..text.find('[').unwrap()];
        assert!(
            globals.contains("use the web interface.\nworkgroup = HOME\nserver min protocol = SMB3\n"),
            "global settings come before the first section: {text}"
        );
        assert!(
            text.contains(
                "[photos]\n   path = /volume2/photos\n   valid users = @family guest-bob\n   comment = Family photos\n"
            ),
            "{text}"
        );
        assert!(text.contains("   read only = yes\n   write list = @family\n"), "{text}");
        assert!(text.contains("recycle:repository = .recycle/%U"), "{text}");
        assert!(text.contains("vfs objects = shadow_copy2 catia fruit streams_xattr recycle\n"), "{text}");
        assert!(text.contains("shadow:format = zdl-%Y-%m-%d_%H.%M.%S\n"), "{text}");
        assert!(text.contains("recycle:exclude = *.tmp, *.temp, ~$*, .~lock.*#, *.swp\n"), "{text}");
        assert!(!text.contains("[nobody]"), "a share without access is not offered: {text}");
    }

    #[test]
    fn denied_users_and_groups_are_invalid_users_and_a_share_only_denying_is_not_offered() {
        let user = |name: &str| Principal::User(UserName::parse(name).unwrap());
        let group = |name: &str| Principal::Group(GroupName::parse(name).unwrap());
        let rule = |principal, permission| ShareAccess { principal, permission };
        let services = FileServices {
            smb: true,
            nfs: false,
            workgroup: Workgroup::parse("WORKGROUP").unwrap(),
            smb_min_protocol: SmbProtocol::Smb2_1,
            ..base_services()
        };
        let shares = [
            SmbShare {
                name: ShareName::parse("photos").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                access: vec![
                    rule(group("family"), SharePermission::ReadWrite),
                    rule(user("guest-bob"), SharePermission::Deny),
                    rule(group("kids"), SharePermission::Deny),
                ],
                recycle_bin: false,
                comment: None,
                time_machine: None,
            },
            SmbShare {
                name: ShareName::parse("closed").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                access: vec![rule(group("kids"), SharePermission::Deny)],
                recycle_bin: false,
                comment: None,
                time_machine: None,
            },
        ];

        let text = samba_config(&shares, &services, None);

        assert!(text.contains("   valid users = @family\n   invalid users = guest-bob @kids\n"), "{text}");
        assert!(text.contains("   write list = @family\n"), "{text}");
        assert!(!text.contains("[closed]"), "nobody may use it: {text}");
    }

    #[test]
    fn options_become_defaults_of_every_share_and_time_machine_one_share_s() {
        let services = FileServices {
            smb_encryption: zendiloos_agent_proto::SmbEncryption::Required,
            smb_signing: zendiloos_agent_proto::SmbSigning::Mandatory,
            hide_unreadable: false,
            oplocks: false,
            durable_handles: true,
            veto_files: Some(zendiloos_agent_proto::VetoFiles::parse("/*.tmp/Thumbs.db/").unwrap()),
            delete_veto_files: true,
            bonjour: false,
            ..base_services()
        };
        let backups = SmbShare {
            name: ShareName::parse("backups").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            access: vec![ShareAccess {
                principal: Principal::User(UserName::parse("anna").unwrap()),
                permission: SharePermission::ReadWrite,
            }],
            recycle_bin: false,
            comment: None,
            time_machine: Some(Some(500 << 30)),
        };

        let text = samba_config(std::slice::from_ref(&backups), &services, None);

        for line in [
            "server smb encrypt = required\n",
            "server signing = mandatory\n",
            "multicast dns register = no\n",
            "access based share enum = no\n",
            "oplocks = no\nsmb2 leases = no\n",
            "durable handles = yes\nkernel share modes = no\nposix locking = no\n",
            "veto files = /*.tmp/Thumbs.db/\ndelete veto files = yes\n",
            "   vfs objects = shadow_copy2 catia fruit streams_xattr\n",
            "   fruit:time machine = yes\n   fruit:time machine max size = 512000M\n",
        ] {
            assert!(text.contains(line), "{line:?} missing: {text}");
        }
        let windows_only = FileServices { mac_compat: false, ..base_services() };
        let text = samba_config(&[SmbShare { time_machine: None, ..backups }], &windows_only, None);
        assert!(text.contains("   vfs objects = shadow_copy2 streams_xattr\n"), "{text}");
    }

    #[test]
    fn a_new_time_machine_share_is_a_new_announcement() {
        let plain = "multicast dns register = yes\n\n[photos]\n   path = /volume1/photos\n";
        let machine = format!("{plain}\n[backups]\n   fruit:time machine = yes\n");
        assert_eq!(announced(plain), announced(&plain.replace("photos\n", "pictures\n")));
        assert_ne!(announced(plain), announced(&machine));
        assert_ne!(announced(plain), announced(&plain.replace("register = yes", "register = no")));
    }

    #[test]
    fn the_home_share_shows_each_user_their_own_folder() {
        let services = FileServices {
            smb: true,
            nfs: false,
            workgroup: Workgroup::parse("WORKGROUP").unwrap(),
            smb_min_protocol: SmbProtocol::Smb2_1,
            ..base_services()
        };
        let homes = HomesShare {
            volume: VolumeNumber::new(2).unwrap(),
            users: vec![UserName::parse("anna").unwrap(), UserName::parse("ben").unwrap()],
        };

        let text = samba_config(&[], &services, Some(&homes));

        assert!(text.contains("[home]\n"), "{text}");
        assert!(text.contains("   path = /volume2/@homes/%U\n   valid users = anna ben\n"), "{text}");
        assert!(text.contains("   create mask = 0600\n   directory mask = 0700\n"), "{text}");
        let nobody = HomesShare { users: vec![], ..homes };
        assert!(!samba_config(&[], &services, Some(&nobody)).contains("[home]"));
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
