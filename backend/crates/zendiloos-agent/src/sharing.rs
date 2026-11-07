//! Samba shares and NFS exports, and the permissions of share directories.
//!
//! The agent writes the configuration itself from validated names: no
//! option, path or host from zendiloosd reaches a configuration file as
//! text. Paths are always `/volume<N>/<share>`.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{CWD, Mode, OFlags, openat};
use zendiloos_agent_proto::{
    FileServices, HOMES_DIRECTORY, HomesShare, NfsExport, Principal, Response, SNAPSHOT_LABEL_FORMAT,
    ShareAccess, ShareName, SharePermission, SmbShare, UserName, VolumeNumber,
};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::pools::{HostPaths, run};

const LIMITS: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 1024 * 1024 };
/// Permissions of a whole tree: large shares take a while, and a change of
/// access must not die halfway.
pub use zendiloos_fileops::RECYCLE_DIRECTORY;
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
            serial: tokio::sync::Mutex::new(()),
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
        let _serial = self.serial.lock().await;
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
        match fs::create_dir(&home) {
            Ok(()) => {
                std::os::unix::fs::chown(&home, Some(uid), Some(gid))
                    .map_err(|e| OperationError::Write(home.clone(), e))?;
                fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                    .map_err(|e| OperationError::Write(home.clone(), e))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = fs::symlink_metadata(&home).map_err(|e| OperationError::Read(home.clone(), e))?;
                if !meta.is_dir() {
                    return Err(OperationError::Refused(format!("{} is not a folder", home.display())));
                }
            }
            Err(error) => return Err(OperationError::Write(home, error)),
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
        match fs::create_dir(&bin) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(OperationError::Write(bin, error));
            }
            _ => {}
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let directory = openat(CWD, &bin, flags, Mode::empty()).map_err(|_| {
            OperationError::Refused(format!("{RECYCLE_DIRECTORY} in {} is not a folder", name.as_str()))
        })?;
        let directory = fs::File::from(directory);
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

        // Each writer's own bin, made here: otherwise one writer could make
        // another's first and read what lands in it, which Samba does not
        // check.
        for (user, uid, gid) in self.writers(access)? {
            if let Err(error) = prepare_bin(&directory, &user, uid, gid) {
                tracing::warn!(%error, share = name.as_str(), user, "cannot prepare a recycle bin");
            }
        }

        if recursive {
            let mut modify = entries(false, true);
            modify.extend(entries(true, false));
            let removals: Vec<String> =
                removed.iter().flat_map(|p| [acl_entry(p), format!("d:{}", acl_entry(p))]).collect();
            self.change_tree(&path, &removals, &modify).await?;
        }
        Ok(Response::Done)
    }
}

impl Sharing {
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

/// Folders deeper than this are not looked into.
const MAX_DEPTH: usize = 256;

impl Sharing {
    /// Changes the permissions of everything below a share's root (not the
    /// root, which is set on its own): access taken away first, so it is
    /// gone even if giving fails. Each folder and file is reached by
    /// descriptor and changed through it (`/proc/self/fd/0`), never by its
    /// path: whoever may write the share could swap a folder for a link
    /// while the change goes on, and setfacl's own walk (`-R`) would follow
    /// it, onto other shares, the bins or `@docker`. Links, other file
    /// systems and the recycle bins are left alone.
    async fn change_tree(
        &self,
        root: &Path,
        removals: &[String],
        modify: &[String],
    ) -> Result<(), OperationError> {
        let folder_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let unreadable = |e: rustix::io::Errno| OperationError::Read(root.to_owned(), e.into());
        let top = openat(CWD, root, folder_flags, Mode::empty()).map_err(unreadable)?;
        let device = rustix::fs::fstat(&top).map_err(unreadable)?.st_dev;
        let mut folders = vec![(top, 0)];
        while let Some((folder, depth)) = folders.pop() {
            let mut names = Vec::new();
            for entry in rustix::fs::Dir::read_from(&folder).map_err(unreadable)? {
                let name = entry.map_err(unreadable)?.file_name().to_owned();
                let own = name.as_c_str() == c"." || name.as_c_str() == c"..";
                let bin = depth == 0 && name.as_bytes() == RECYCLE_DIRECTORY.as_bytes();
                if !own && !bin {
                    names.push(name);
                }
            }
            for name in names {
                // Gone, or swapped for a link, since it was listed: skipped.
                let Ok(stat) = rustix::fs::statat(&folder, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                else {
                    continue;
                };
                let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
                if kind == rustix::fs::FileType::Symlink || stat.st_dev != device {
                    continue;
                }
                let is_folder = kind == rustix::fs::FileType::Directory;
                let flags =
                    if is_folder { folder_flags } else { OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC };
                let Ok(entry) = openat(&folder, &name, flags, Mode::empty()) else { continue };
                let opened = rustix::fs::fstat(&entry).map_err(unreadable)?;
                let still = rustix::fs::FileType::from_raw_mode(opened.st_mode);
                if still == rustix::fs::FileType::Symlink
                    || opened.st_dev != device
                    || (still == rustix::fs::FileType::Directory) != is_folder
                {
                    continue;
                }
                self.change_one(&entry, is_folder, removals, modify).await?;
                if is_folder && depth + 1 < MAX_DEPTH {
                    folders.push((entry, depth + 1));
                }
            }
        }
        Ok(())
    }

    /// `setfacl -x` then `-m` on the entry behind `entry`; default entries
    /// (`d:`) for folders only.
    async fn change_one(
        &self,
        entry: &std::os::fd::OwnedFd,
        folder: bool,
        removals: &[String],
        modify: &[String],
    ) -> Result<(), OperationError> {
        for (option, list) in [("-x", removals), ("-m", modify)] {
            let kept: Vec<&str> =
                list.iter().map(String::as_str).filter(|entry| folder || !entry.starts_with("d:")).collect();
            if kept.is_empty() {
                continue;
            }
            let file =
                fs::File::from(entry.try_clone().map_err(|e| OperationError::Write(PathBuf::from("."), e))?);
            let args = [option, &kept.join(","), "--", "/proc/self/fd/0"];
            zendiloos_exec::run_on(&self.setfacl, args, file, LIMITS).await?.success(&self.setfacl)?;
        }
        Ok(())
    }
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

    #[tokio::test]
    async fn a_tree_is_changed_entry_by_entry_through_descriptors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        fs::create_dir_all(root.join("album/2024")).unwrap();
        fs::write(root.join("album/2024/a.jpg"), "x").unwrap();
        fs::write(root.join("notes.txt"), "x").unwrap();
        fs::create_dir_all(root.join(RECYCLE_DIRECTORY).join("anna")).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("escape")).unwrap();
        // setfacl as it is called: its arguments and what descriptor 0 is.
        let log = dir.path().join("setfacl.log");
        let setfacl = dir.path().join("setfacl");
        fs::write(
            &setfacl,
            format!("#!/bin/sh\necho \"$* $(readlink /proc/self/fd/0)\" >> {}\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&setfacl, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let tools = Tools { setfacl, ..Tools::default() };
        let accounts = crate::accounts::AccountFiles::default();
        let sharing = Sharing::new(&tools, HostPaths::default(), accounts).unwrap();
        let removals = ["u:bob".to_owned(), "d:u:bob".to_owned()];
        let modify = ["u:anna:rwX".to_owned(), "d:u:anna:rwX".to_owned()];

        sharing.change_tree(&root, &removals, &modify).await.unwrap();

        let shown = |path: &str| root.join(path).display().to_string();
        let mut calls: Vec<String> = fs::read_to_string(&log).unwrap().lines().map(str::to_owned).collect();
        calls.sort();
        let mut expected = vec![
            format!("-m u:anna:rwX,d:u:anna:rwX -- /proc/self/fd/0 {}", shown("album")),
            format!("-m u:anna:rwX,d:u:anna:rwX -- /proc/self/fd/0 {}", shown("album/2024")),
            format!("-m u:anna:rwX -- /proc/self/fd/0 {}", shown("album/2024/a.jpg")),
            format!("-m u:anna:rwX -- /proc/self/fd/0 {}", shown("notes.txt")),
            format!("-x u:bob,d:u:bob -- /proc/self/fd/0 {}", shown("album")),
            format!("-x u:bob,d:u:bob -- /proc/self/fd/0 {}", shown("album/2024")),
            format!("-x u:bob -- /proc/self/fd/0 {}", shown("album/2024/a.jpg")),
            format!("-x u:bob -- /proc/self/fd/0 {}", shown("notes.txt")),
        ];
        expected.sort();
        assert_eq!(calls, expected, "neither the link nor the bins, default entries on folders only");
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
