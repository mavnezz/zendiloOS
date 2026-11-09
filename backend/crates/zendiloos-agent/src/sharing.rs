//! Samba shares and NFS exports, and what every share's folder has on disk.
//!
//! The agent writes the configuration itself from validated names: no
//! option, path or host from zendiloosd reaches a configuration file as
//! text. Paths are always `/volume<N>/<share>`.
//!
//! Access to a share is the share's as a whole (docs/architecture.md, 4.4):
//! Samba enforces the rules per section and works on the files as root,
//! zendiloosd enforces them for the file manager, NFS clients are named by
//! host and squashed to `zendiloos-files`. On disk every share is alike:
//! its folder `root:zendiloos-files`, mode 2770, and one ACL that lets the
//! group `zendiloos-files` read and write, inherited by what is made in it.

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
    FileServices, HOMES_DIRECTORY, HomesShare, NfsClient, NfsExport, Principal, Response,
    SNAPSHOT_LABEL_FORMAT, ShareName, SmbShare, UserName, VolumeNumber,
};
use zendiloos_exec::Program;

use crate::acl::{self, Acl, Managed};
use crate::config::Tools;
use crate::operations::OperationError;
use crate::pools::{HostPaths, run};

pub use zendiloos_fileops::RECYCLE_DIRECTORY;
/// How long a change of a share's own folder waits for the walk of the
/// same share: one that goes through all its files may take hours, and
/// whoever asked is told to come back rather than kept waiting.
const SHARE_WAIT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 15 });
const HEADER: &str = "# Written by zendiloOS. Changes here are overwritten; use the web interface.\n";
/// WS-Discovery, so Windows lists the NAS; recommended, not required.
const WSDD: &str = "wsdd2.service";
/// Bonjour, so Macs find the NAS and its Time Machine share; recommended too.
const AVAHI: &str = "avahi-daemon.service";
const AVAHI_SOCKET: &str = "avahi-daemon.socket";
/// A share's folder: owner and the group `zendiloos-files` read and write,
/// others nothing; what is made in it takes the group (setgid).
const ROOT_MODE: u32 = 0o2770;
/// The recycle bins' folder: as the share's, and sticky: nobody but root
/// renames or removes the bins in it, which Samba and the file manager
/// make as root.
const BINS_MODE: u32 = 0o3770;
/// How many entries the walk of a share goes through between two lines
/// saying how far it got.
const REPORT_EVERY: usize = if cfg!(test) { 2 } else { 10_000 };

#[derive(Debug)]
pub(crate) struct Sharing {
    paths: HostPaths,
    accounts: crate::accounts::AccountFiles,
    testparm: Program,
    exportfs: Program,
    systemctl: Program,
    smbcontrol: Program,
    /// One writing of the services' configuration at a time.
    config: tokio::sync::Mutex<()>,
    /// One change of a share's folder at a time, for each share: a walk
    /// going through a large share keeps only that share waiting.
    shares: std::sync::Mutex<HashMap<(u16, String), ShareLock>>,
}

type ShareLock = Arc<tokio::sync::Mutex<()>>;

fn principal(principal: &Principal) -> String {
    match principal {
        Principal::User(user) => user.as_str().to_owned(),
        Principal::Group(group) => format!("@{}", group.as_str()),
    }
}

/// Samba's settings for all shares, then a section for each share. A
/// share nobody may use is left out: Samba would read an empty `valid
/// users` as "everybody". Denied users and groups (and disabled accounts,
/// which zendiloosd sends as denied) are `invalid users`, which wins over
/// `valid users`, also for members of a group listed there.
///
/// Samba works on the files as root (`force user`), so the rules here are
/// the only ones: what is written takes the owner of the folder it lands
/// in (`inherit owner`) and the group `zendiloos-files`, and Windows'
/// security dialog changes nothing (`nt acl support = no`).
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
    let files = crate::accounts::FILES_GROUP;
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
        text.push_str("   browseable = yes\n");
        let _ = writeln!(
            text,
            "   force user = root\n   force group = {files}\n   inherit owner = yes\n   \
             create mask = 0660\n   directory mask = 2770\n   nt acl support = no"
        );
        // ZFS snapshots as "Previous Versions" in Windows: they are named
        // after their UTC time (see zendiloos-sharing's snapshot labels).
        let _ =
            writeln!(text, "   shadow:snapdir = .zfs/snapshot\n   shadow:format = {SNAPSHOT_LABEL_FORMAT}");
        text.push_str("   shadow:localtime = no\n   shadow:sort = desc\n");
        if share.recycle_bin {
            let _ = writeln!(text, "   vfs objects = shadow_copy2 {mac}streams_xattr recycle");
            // One bin per share; the folder of each user says who deleted
            // what, and everyone who may use the share sees all of it.
            let _ = writeln!(text, "   recycle:repository = {RECYCLE_DIRECTORY}/%U");
            text.push_str(
                "   recycle:keeptree = yes\n   recycle:versions = yes\n   recycle:touch = yes\n   \
                 recycle:directory_mode = 0770\n",
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
        // `%U` is the user who connected: each one sees their own folder,
        // as themselves; nobody else may enter it.
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

/// The user and group NFS clients are squashed to: `zendiloos-files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Squashed {
    pub uid: u32,
    pub gid: u32,
}

/// `/etc/exports` lines for `exports`: each client with what it may do
/// (`rw`/`ro`), when writes are answered (`sync`/`async`), from which
/// ports (`insecure` added only when wanted; `secure` is the default) and
/// how it proves who it is (`sec=sys`, the only way so far). Whoever is on
/// a client it names has the share's access as a whole: every request is
/// squashed to `zendiloos-files` (`all_squash`), whatever user the client
/// claims, as `sec=sys` lets a client claim any.
pub fn nfs_exports(exports: &[NfsExport], squashed: Squashed) -> String {
    let mut text = HEADER.to_owned();
    for export in exports.iter().filter(|export| !export.clients.is_empty()) {
        let _ = write!(text, "/volume{}/{}", export.volume.get(), export.name.as_str());
        for client in &export.clients {
            let _ = write!(text, " {}({})", client.network, export_options(client, squashed));
        }
        text.push('\n');
    }
    text
}

fn export_options(client: &NfsClient, squashed: Squashed) -> String {
    let mode = if client.write { "rw" } else { "ro" };
    let sync = if client.asynchronous { "async" } else { "sync" };
    let insecure = if client.insecure { "insecure," } else { "" };
    format!(
        "{mode},{sync},no_subtree_check,all_squash,anonuid={},anongid={},{insecure}sec=sys",
        squashed.uid, squashed.gid
    )
}

fn write_atomically(path: &Path, content: &str) -> Result<(), OperationError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
    }
    crate::durable::replace(path, content.as_bytes())
}

fn read(path: &Path) -> Result<String, OperationError> {
    fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
}

/// The user and group `zendiloos-files`, which the package makes.
fn files_ids(passwd: &str, group: &str) -> Result<Squashed, OperationError> {
    let name = crate::accounts::FILES_GROUP;
    let missing = || OperationError::Setup(format!("the user and group {name} are missing"));
    let (uid, _) = crate::passwd::lookup(passwd, name).ok_or_else(missing)?;
    let gid = crate::passwd::group_id(group, name).ok_or_else(missing)?;
    Ok(Squashed { uid, gid })
}

/// Whose named entries in a share's ACLs go (see [`Managed`]): the IDs
/// zendiloOS hands out and `zendiloos-users`, but not the apps' accounts
/// (`app-<id>`, with [`crate::accounts::APP_COMMENT`]) and their groups.
fn managed(passwd: &str, group: &str) -> Managed {
    let ids = crate::accounts::ID_MIN..=crate::accounts::ID_MAX;
    let (mut app_users, mut app_groups) = (Vec::new(), Vec::new());
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        let (Some(name), Some(uid), Some(gid), Some(comment)) =
            (fields.first(), fields.get(2), fields.get(3), fields.get(4))
        else {
            continue;
        };
        let (Ok(uid), Ok(gid)) = (uid.parse::<u32>(), gid.parse::<u32>()) else { continue };
        let app = name.starts_with(zendiloos_agent_proto::AppName::ACCOUNT_PREFIX)
            && *comment == crate::accounts::APP_COMMENT
            && ids.contains(&uid);
        if app {
            app_users.push(uid);
            if crate::passwd::group_id(group, name) == Some(gid) {
                app_groups.push(gid);
            }
        }
    }
    Managed {
        ids: Some(ids),
        groups: crate::passwd::group_id(group, crate::accounts::USERS_GROUP).into_iter().collect(),
        app_users,
        app_groups,
    }
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
            smbcontrol: Program::new(&tools.smbcontrol)?,
            config: tokio::sync::Mutex::new(()),
            shares: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Writes both configurations, Samba's only if `testparm` accepts it,
    /// and has the servers that are on run with them; those that are off
    /// stop and stay off after a restart. Then the open SMB connections of
    /// `close` end, so that clients connect again under the new rules.
    pub(crate) async fn apply(
        &self,
        smb: &[SmbShare],
        nfs: &[NfsExport],
        services: &FileServices,
        homes: Option<&HomesShare>,
        close: &[ShareName],
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
        let exported = nfs.iter().any(|export| !export.clients.is_empty());
        let squashed = if exported {
            files_ids(&read(&self.accounts.passwd)?, &read(&self.accounts.group)?)?
        } else {
            Squashed { uid: 0, gid: 0 }
        };
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
            if !close.is_empty() {
                // Every smbd reads the new rules first, then the shares'
                // connections end; a client connecting again gets the new
                // rules (a tree connection is checked when it is made).
                run(&self.smbcontrol, &["smbd", "reload-config"]).await?;
                for share in close {
                    run(&self.smbcontrol, &["smbd", "close-share", share.as_str()]).await?;
                }
            }
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

        let exports = nfs_exports(nfs, squashed);
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

    /// The share's folder, if its own dataset is mounted there; otherwise
    /// a change would go to the volume's directory instead.
    fn mounted_share(&self, volume: VolumeNumber, name: &ShareName) -> Result<PathBuf, OperationError> {
        let path = self.share_path(volume, name);
        let parent = path.parent().unwrap_or(&path).to_owned();
        let (own, above) = match (fs::metadata(&path), fs::metadata(&parent)) {
            (Ok(own), Ok(above)) => (own, above),
            _ => return Err(OperationError::Refused(format!("share {} is not present", name.as_str()))),
        };
        if !own.is_dir() || own.dev() == above.dev() {
            return Err(OperationError::Refused(format!("share {} is not mounted", name.as_str())));
        }
        Ok(path)
    }

    /// Who the share's folder is given to, and whose entries go there.
    fn ownership(&self) -> Result<(u32, Managed), OperationError> {
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        Ok((files_ids(&passwd, &group)?.gid, managed(&passwd, &group)))
    }

    /// The share's folder as every share has it: owner `root`, group
    /// `zendiloos-files`, mode 2770, the group's entry and default entry
    /// in its ACL; and the recycle bins' folder alike, sticky. A change of
    /// a share's folder does not wait long for the walk of the same share
    /// (the one a person waits for, after a rollback): it is refused, to be
    /// asked again once that has ended.
    pub(crate) async fn prepare_share(
        &self,
        volume: VolumeNumber,
        name: &ShareName,
    ) -> Result<Response, OperationError> {
        let lock = self.share_lock(volume, name);
        let _share =
            tokio::time::timeout(SHARE_WAIT, crate::waiting::lock(&lock)).await.map_err(|_| {
                OperationError::Refused(format!(
                    "share {} is being given share-level permissions; try again once that has ended",
                    name.as_str()
                ))
            })??;
        let path = self.mounted_share(volume, name)?;
        let (files, managed) = self.ownership()?;
        prepare_root(&path, name, Owner { uid: 0, files }, &managed)?;
        Ok(Response::Done)
    }

    /// The update to share-level access, once for each share: its folder as
    /// [`Self::prepare_share`] makes it, then every folder and file in it
    /// given the group's entries and rid of the named entries of the
    /// accounts and groups zendiloOS manages (see [`change_tree`]). Owners
    /// stay. Reports how many entries it went through.
    pub(crate) async fn migrate_share(
        &self,
        volume: VolumeNumber,
        name: &ShareName,
    ) -> Result<Response, OperationError> {
        let lock = self.share_lock(volume, name);
        let _share = crate::waiting::lock(&lock).await?;
        let path = self.mounted_share(volume, name)?;
        let (files, managed) = self.ownership()?;
        crate::progress::step("preparing", None);
        prepare_root(&path, name, Owner { uid: 0, files }, &managed)?;
        crate::progress::step("walking", None);
        let (sender, mut counts) = tokio::sync::mpsc::unbounded_channel();
        let walking = tokio::task::spawn_blocking(move || {
            change_tree(&path, files, &managed, &mut |count| {
                let _ = sender.send(count);
            })
        });
        // The walk's thread is not the request's: what it reports comes
        // here, and goes on from here.
        while let Some(count) = counts.recv().await {
            crate::progress::line(&format!("{count} entries"));
        }
        let walked =
            walking.await.map_err(|error| OperationError::Failed(format!("the walk stopped: {error}")))??;
        crate::progress::line(&format!("{walked} entries"));
        Ok(Response::Done)
    }
}

/// Owner, group, mode and ACL of a share's folder, and of its recycle
/// bins' folder, made if missing; both reached by descriptor. Whoever may
/// write the share can rename what is in its root: `.recycle` is opened
/// without following a link and changed through that descriptor, and
/// something else in its place is moved aside.
fn prepare_root(
    path: &Path,
    name: &ShareName,
    owner: Owner,
    managed: &Managed,
) -> Result<(), OperationError> {
    let failed = |path: &Path| {
        let path = path.to_owned();
        move |e: Errno| OperationError::Write(path.clone(), e.into())
    };
    let root = openat(CWD, path, FOLDER, Mode::empty()).map_err(failed(path))?;
    give_folder(&root, owner, ROOT_MODE, managed).map_err(failed(path))?;
    let bins = path.join(RECYCLE_DIRECTORY);
    let bin = open_bin(path, name)?;
    give_folder(&bin, owner, BINS_MODE, managed).map_err(failed(&bins))?;
    Ok(())
}

/// Who a share's folder belongs to: root, and the group `zendiloos-files`
/// (`files`), which its ACL lets read and write too.
#[derive(Debug, Clone, Copy)]
struct Owner {
    uid: u32,
    files: u32,
}

/// `folder` given to `owner`, with `mode` and the share's ACL (see
/// [`acl::root`]), as access and default ACL.
fn give_folder(folder: &OwnedFd, owner: Owner, mode: u32, managed: &Managed) -> Result<(), Errno> {
    let files = owner.files;
    rustix::fs::fchown(
        folder,
        Some(rustix::fs::Uid::from_raw(owner.uid)),
        Some(rustix::fs::Gid::from_raw(files)),
    )?;
    rustix::fs::fchmod(folder, Mode::from_raw_mode(mode))?;
    let path = format!("/proc/self/fd/{}", folder.as_raw_fd());
    let access = acl::root(read_acl(&path, acl::ACCESS)?, mode, files, managed);
    rustix::fs::setxattr(&path, acl::ACCESS, &access.to_bytes(), rustix::fs::XattrFlags::empty())?;
    let default = acl::root(read_acl(&path, acl::DEFAULT)?, mode, files, managed);
    rustix::fs::setxattr(&path, acl::DEFAULT, &default.to_bytes(), rustix::fs::XattrFlags::empty())?;
    Ok(())
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
    let make = || match fs::create_dir(&bin) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            Err(OperationError::Write(bin.clone(), error))
        }
        _ => Ok(()),
    };
    make()?;
    if let Ok(directory) = openat(CWD, &bin, FOLDER, Mode::empty()) {
        return Ok(directory);
    }
    let since =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let aside = share.join(format!("{RECYCLE_DIRECTORY}.not-a-folder-{since}-{}", std::process::id()));
    rustix::fs::renameat_with(CWD, &bin, CWD, &aside, rustix::fs::RenameFlags::NOREPLACE)
        .map_err(|e| OperationError::Write(bin.clone(), e.into()))?;
    tracing::warn!(share = name.as_str(), aside = %aside.display(), "what took the recycle bin's place was moved aside");
    make()?;
    openat(CWD, &bin, FOLDER, Mode::empty()).map_err(|_| {
        OperationError::Refused(format!("{RECYCLE_DIRECTORY} in {} is not a folder", name.as_str()))
    })
}

/// Folders deeper than this are not looked into; what is below counts as
/// not changed. The folder at the limit itself is changed.
const MAX_DEPTH: usize = if cfg!(test) { 8 } else { 256 };

/// What a change of a tree could not change: entries, and folders too deep
/// to look into. Counted, never a reason to stop: a walk goes on to the
/// end whatever fails on the way, so a writer cannot keep it from the rest
/// of the share (with folders nested too deeply, say).
#[derive(Debug, Default)]
struct Failures {
    count: usize,
    first: Option<String>,
    too_deep: usize,
    first_too_deep: Option<String>,
}

impl Failures {
    fn note(&mut self, path: &Path, error: Errno) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some(format!("{}: {}", path.display(), std::io::Error::from(error)));
        }
    }

    fn note_too_deep(&mut self, path: &Path) {
        self.too_deep += 1;
        if self.first_too_deep.is_none() {
            self.first_too_deep = Some(path.display().to_string());
        }
    }

    fn any(&self) -> bool {
        self.count > 0 || self.too_deep > 0
    }

    fn error(&self) -> OperationError {
        let mut why = Vec::new();
        if let Some(first) = &self.first {
            why.push(format!(
                "the permissions of {} entries could not be changed, among them {first}",
                self.count
            ));
        }
        if let Some(first) = &self.first_too_deep {
            why.push(format!(
                "{} folders are nested too deeply to look into, among them {first}",
                self.too_deep
            ));
        }
        OperationError::Failed(why.join("; "))
    }
}

/// Gives everything below a share's root (not the root, which is set on
/// its own) the share's entries: the group `files` reads and writes (see
/// [`acl::shared`]), folders pass that on to what is made in them, and
/// the named entries `managed` holds go. Owners and other entries stay.
/// Returns how many entries it went through, telling `report` every so
/// often on the way.
///
/// Each folder and file is reached by descriptor and its ACL changed
/// through it, never by its path: whoever may write the share could swap a
/// folder for a link while the walk goes on, and a walk by path would
/// follow it, onto other shares or `@docker`. Left alone: links, other
/// file systems, what is neither folder nor file. Only the folders on the
/// way down are open at a time, each read as it goes, never held whole: a
/// folder of millions of entries must not take the agent's memory. An entry
/// that cannot be changed, or a folder too deep to look into, does not stop
/// the others; the walk fails at the end, naming one of them.
fn change_tree(
    root: &Path,
    files: u32,
    managed: &Managed,
    report: &mut dyn FnMut(usize),
) -> Result<usize, OperationError> {
    let mut failures = Failures::default();
    let unreadable = |e: Errno| OperationError::Read(root.to_owned(), e.into());
    let top = openat(CWD, root, FOLDER, Mode::empty()).map_err(unreadable)?;
    let device = rustix::fs::fstat(&top).map_err(unreadable)?.st_dev;
    let top = rustix::fs::Dir::new(top).map_err(unreadable)?;
    let mut stack = vec![Level { folder: top, path: root.to_owned() }];
    let mut walked = 0;
    while let Some(level) = stack.last_mut() {
        let name = match level.next_name() {
            Some(Ok(name)) => name,
            Some(Err(error)) => {
                // What is left of this folder cannot be read.
                let path = level.path.clone();
                stack.pop();
                failures.note(&path, error);
                continue;
            }
            None => {
                stack.pop();
                continue;
            }
        };
        let path = level.path.join(OsStr::from_bytes(name.as_bytes()));
        let opened = level.folder.fd().and_then(|fd| open_entry(fd, &name, device));
        let found = match opened {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                failures.note(&path, error);
                continue;
            }
        };
        walked += 1;
        if walked % REPORT_EVERY == 0 {
            report(walked);
        }
        // At the depth limit too: only what is below is not looked into.
        if let Err(error) = change_acl(&found, files, managed) {
            failures.note(&path, error);
        }
        if !found.folder {
            continue;
        }
        if stack.len() >= MAX_DEPTH {
            failures.note_too_deep(&path);
            continue;
        }
        match rustix::fs::Dir::new(found.entry) {
            Ok(inner) => stack.push(Level { folder: inner, path }),
            Err(error) => failures.note(&path, error),
        }
    }
    if failures.any() { Err(failures.error()) } else { Ok(walked) }
}

const FOLDER: OFlags = OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

/// A folder on the way down, read as the walk goes: changing permissions
/// adds and removes no names, so reading while changing misses nothing.
struct Level {
    folder: rustix::fs::Dir,
    path: PathBuf,
}

impl Level {
    /// The next name to look at; `None` once the folder is read.
    fn next_name(&mut self) -> Option<Result<CString, Errno>> {
        loop {
            let entry = match self.folder.next()? {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error)),
            };
            let name = entry.file_name();
            if name != c"." && name != c".." {
                return Some(Ok(name.to_owned()));
            }
        }
    }
}

/// An entry of a share as opened.
struct Found {
    entry: OwnedFd,
    folder: bool,
    mode: u32,
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
    Ok(Some(Found { entry, folder: folder_kind, mode: opened.st_mode }))
}

/// The share's entries for `found`, through `/proc/self/fd`: the
/// attributes of what was opened, whatever its path is by now. Default
/// entries for folders only.
fn change_acl(found: &Found, files: u32, managed: &Managed) -> Result<(), Errno> {
    let path = format!("/proc/self/fd/{}", found.entry.as_raw_fd());
    let access = read_acl(&path, acl::ACCESS)?;
    let changed = acl::shared(access.clone(), found.mode, found.folder, files, managed);
    if let Some(acl) = &changed {
        rustix::fs::setxattr(&path, acl::ACCESS, &acl.to_bytes(), rustix::fs::XattrFlags::empty())?;
    }
    if found.folder {
        let now = changed.or(access).unwrap_or_else(|| Acl::from_mode(found.mode));
        if let Some(default) = acl::shared_default(read_acl(&path, acl::DEFAULT)?, &now, files, managed) {
            rustix::fs::setxattr(&path, acl::DEFAULT, &default.to_bytes(), rustix::fs::XattrFlags::empty())?;
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
        GroupName, NfsClient, NfsHost, ShareAccess, ShareComment, SharePermission, SmbProtocol, UserName,
        Workgroup,
    };

    use super::*;
    use crate::acl::Who;

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

    fn access(principal: Principal, write: bool) -> ShareAccess {
        let permission = if write { SharePermission::ReadWrite } else { SharePermission::Read };
        ShareAccess { principal, permission }
    }

    fn user(name: &str) -> Principal {
        Principal::User(UserName::parse(name).unwrap())
    }

    fn group(name: &str) -> Principal {
        Principal::Group(GroupName::parse(name).unwrap())
    }

    /// The account files of a host with the accounts anna (2001) and ben
    /// (2002), the app immich (2500, its group 2501), the owner's own user
    /// (1000), zendiloos-users (990) and zendiloos-files (995).
    fn accounts(dir: &Path) -> crate::accounts::AccountFiles {
        fs::write(
            dir.join("passwd"),
            "root:x:0:0::/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n\
             zendiloos-files:x:995:995:zendiloOS files:/:/usr/sbin/nologin\n\
             anna:x:2001:990::/nonexistent:/usr/sbin/nologin\nben:x:2002:990::/nonexistent:/usr/sbin/nologin\n\
             app-immich:x:2500:2501:zendiloOS app:/nonexistent:/usr/sbin/nologin\n\
             app-fake:x:2600:2601:not an app:/nonexistent:/usr/sbin/nologin\n",
        )
        .unwrap();
        fs::write(
            dir.join("group"),
            "root:x:0:\nzendiloos-users:x:990:\nzendiloos-files:x:995:\nfamily:x:2100:anna,ben,debian\n\
             app-immich:x:2501:\napp-fake:x:2601:\n",
        )
        .unwrap();
        crate::accounts::AccountFiles {
            passwd: dir.join("passwd"),
            group: dir.join("group"),
            high_water: dir.join("high-water"),
        }
    }

    #[test]
    fn the_files_group_and_whose_entries_go_are_read_from_the_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let files = accounts(dir.path());
        let (passwd, group) = (read(&files.passwd).unwrap(), read(&files.group).unwrap());
        assert_eq!(files_ids(&passwd, &group).unwrap(), Squashed { uid: 995, gid: 995 });
        let missing = files_ids("root:x:0:0::/root:/bin/bash\n", &group).unwrap_err();
        assert!(missing.to_string().contains("zendiloos-files are missing"), "{missing}");

        let managed = managed(&passwd, &group);
        assert_eq!(managed.ids, Some(crate::accounts::ID_MIN..=crate::accounts::ID_MAX));
        assert_eq!(managed.groups, [990], "everyone's primary group");
        assert_eq!((managed.app_users, managed.app_groups), (vec![2500], vec![2501]), "only a real app's");
    }

    #[test]
    fn writes_samba_shares_from_names_only() {
        let shares = [
            SmbShare {
                name: ShareName::parse("photos").unwrap(),
                volume: VolumeNumber::new(2).unwrap(),
                access: vec![access(group("family"), true), access(user("guest-bob"), false)],
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
        assert!(
            text.contains(
                "   force user = root\n   force group = zendiloos-files\n   inherit owner = yes\n   \
                 create mask = 0660\n   directory mask = 2770\n   nt acl support = no\n"
            ),
            "Samba works on the files as root, its rules are the only ones: {text}"
        );
        assert!(!text.contains("inherit acls"), "{text}");
        assert!(text.contains("recycle:repository = .recycle/%U"), "{text}");
        assert!(text.contains("recycle:directory_mode = 0770\n"), "one bin everyone sees: {text}");
        assert!(text.contains("vfs objects = shadow_copy2 catia fruit streams_xattr recycle\n"), "{text}");
        assert!(text.contains("shadow:format = zdl-%Y-%m-%d_%H.%M.%S\n"), "{text}");
        assert!(text.contains("recycle:exclude = *.tmp, *.temp, ~$*, .~lock.*#, *.swp\n"), "{text}");
        assert!(!text.contains("[nobody]"), "a share without access is not offered: {text}");
    }

    #[test]
    fn denied_users_and_groups_are_invalid_users_and_a_share_only_denying_is_not_offered() {
        let rule = |principal, permission| ShareAccess { principal, permission };
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

        let text = samba_config(&shares, &base_services(), None);

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
            access: vec![access(user("anna"), true)],
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
    fn the_home_share_shows_each_user_their_own_folder_as_themselves() {
        let homes = HomesShare {
            volume: VolumeNumber::new(2).unwrap(),
            users: vec![UserName::parse("anna").unwrap(), UserName::parse("ben").unwrap()],
        };

        let text = samba_config(&[], &base_services(), Some(&homes));

        assert!(text.contains("[home]\n"), "{text}");
        assert!(text.contains("   path = /volume2/@homes/%U\n   valid users = anna ben\n"), "{text}");
        assert!(text.contains("   create mask = 0600\n   directory mask = 0700\n"), "{text}");
        assert!(!text.contains("force user"), "each user is themselves there: {text}");
        let nobody = HomesShare { users: vec![], ..homes };
        assert!(!samba_config(&[], &base_services(), Some(&nobody)).contains("[home]"));
    }

    fn client(host: &str, write: bool) -> NfsClient {
        NfsClient { network: NfsHost::parse(host).unwrap(), write, asynchronous: false, insecure: false }
    }

    #[test]
    fn every_export_squashes_everyone_to_the_files_group() {
        let exports = [
            NfsExport {
                name: ShareName::parse("media").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                clients: vec![client("192.168.1.0/24", true), client("10.0.0.5", false)],
            },
            NfsExport {
                name: ShareName::parse("none").unwrap(),
                volume: VolumeNumber::new(1).unwrap(),
                clients: vec![],
            },
        ];
        let text = nfs_exports(&exports, Squashed { uid: 995, gid: 996 });
        assert_eq!(text.lines().count(), 2, "the header and one export: {text}");
        assert_eq!(
            text.lines().nth(1).unwrap(),
            "/volume1/media 192.168.1.0/24(rw,sync,no_subtree_check,all_squash,anonuid=995,anongid=996,sec=sys) \
             10.0.0.5(ro,sync,no_subtree_check,all_squash,anonuid=995,anongid=996,sec=sys)"
        );
    }

    #[test]
    fn writes_each_clients_own_options() {
        let exports = [NfsExport {
            name: ShareName::parse("media").unwrap(),
            volume: VolumeNumber::new(2).unwrap(),
            clients: vec![
                NfsClient { asynchronous: true, ..client("nas.lan", true) },
                NfsClient { insecure: true, ..client("*.example.com", false) },
            ],
        }];
        assert_eq!(
            nfs_exports(&exports, Squashed { uid: 995, gid: 995 }).lines().nth(1).unwrap(),
            "/volume2/media nas.lan(rw,async,no_subtree_check,all_squash,anonuid=995,anongid=995,sec=sys) \
             *.example.com(ro,sync,no_subtree_check,all_squash,anonuid=995,anongid=995,insecure,sec=sys)"
        );
    }

    /// Sharing with testparm, systemctl and smbcontrol as scripts that
    /// record what they are asked; testparm refuses while `refuse` is there.
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
            smbcontrol: script("smbcontrol", ""),
            exportfs: if exportfs { script("exportfs", "") } else { dir.join("no-exportfs") },
            ..Tools::default()
        };
        let paths = HostPaths {
            samba_config: dir.join("samba/zendiloos.conf"),
            nfs_exports: dir.join("exports.d/zendiloos.exports"),
            ..HostPaths::default()
        };
        Sharing::new(&tools, paths, accounts(dir)).unwrap()
    }

    fn smb(name: &str, time_machine: bool) -> SmbShare {
        SmbShare {
            name: ShareName::parse(name).unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            comment: None,
            access: vec![access(user("anna"), true)],
            recycle_bin: false,
            time_machine: time_machine.then_some(None),
        }
    }

    #[tokio::test]
    async fn what_samba_would_not_take_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = applying(dir.path(), true);
        let mut services = base_services();
        services.mac_compat = false;
        let refused = sharing.apply(&[smb("backup", true)], &[], &services, None, &[]).await.unwrap_err();
        assert!(refused.to_string().contains("macOS compatibility"), "{refused}");
        let homes = HomesShare { volume: VolumeNumber::new(1).unwrap(), users: vec![] };
        let refused =
            sharing.apply(&[smb("home", false)], &[], &base_services(), Some(&homes), &[]).await.unwrap_err();
        assert!(refused.to_string().contains("called home"), "{refused}");

        fs::write(dir.path().join("refuse"), "").unwrap();
        assert!(sharing.apply(&[smb("media", false)], &[], &base_services(), None, &[]).await.is_err());
        assert!(!dir.path().join("samba/zendiloos.conf").exists(), "not written");
        assert!(!dir.path().join("samba/zendiloos.check").exists(), "the candidate goes");
        fs::remove_file(dir.path().join("refuse")).unwrap();

        // Everything off: the servers stop and stay off, and no connection
        // is closed (there are none).
        let mut off = base_services();
        (off.smb, off.nfs, off.wsdd, off.bonjour) = (false, false, false, false);
        let media = ShareName::parse("media").unwrap();
        assert_eq!(
            sharing.apply(&[], &[], &off, None, std::slice::from_ref(&media)).await.unwrap(),
            Response::Done
        );
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        for stopped in
            ["disable --now smbd.service", "disable --now wsdd2.service", "disable --now nfs-server.service"]
        {
            assert!(calls.contains(stopped), "{stopped}: {calls}");
        }
        assert!(!calls.contains("smbcontrol"), "{calls}");
    }

    #[tokio::test]
    async fn a_change_that_takes_access_away_closes_the_shares_connections_after_the_reload() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = applying(dir.path(), true);
        let (media, photos) = (ShareName::parse("media").unwrap(), ShareName::parse("photos").unwrap());

        sharing.apply(&[smb("media", false)], &[], &base_services(), None, &[]).await.unwrap();
        let quiet = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert!(!quiet.contains("smbcontrol"), "nothing taken away, nothing closed: {quiet}");
        fs::remove_file(dir.path().join("calls.log")).unwrap();

        sharing.apply(&[smb("media", false)], &[], &base_services(), None, &[media, photos]).await.unwrap();

        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        let reloaded = lines.iter().position(|l| l.contains("reload-or-restart smbd.service")).unwrap();
        let config = lines.iter().position(|l| *l == "smbcontrol smbd reload-config").unwrap();
        let closed: Vec<usize> =
            lines.iter().enumerate().filter(|(_, l)| l.contains("close-share")).map(|(n, _)| n).collect();
        assert!(reloaded < config && closed.iter().all(|n| config < *n), "{calls}");
        assert!(calls.contains("smbcontrol smbd close-share media\nsmbcontrol smbd close-share photos\n"));
    }

    #[tokio::test]
    async fn exports_need_the_nfs_server_and_the_files_group() {
        let dir = tempfile::tempdir().unwrap();
        let sharing = applying(dir.path(), false);
        let export = NfsExport {
            name: ShareName::parse("media").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            clients: vec![client("192.168.1.0/24", false)],
        };
        let mut services = base_services();
        services.nfs = true;
        let refused =
            sharing.apply(&[], std::slice::from_ref(&export), &services, None, &[]).await.unwrap_err();
        assert!(refused.to_string().contains("not installed"), "{refused}");
        // Without exports nothing is missing.
        assert_eq!(sharing.apply(&[], &[], &services, None, &[]).await.unwrap(), Response::Done);

        let installed = applying(dir.path(), true);
        assert_eq!(
            installed.apply(&[], std::slice::from_ref(&export), &services, None, &[]).await.unwrap(),
            Response::Done
        );
        let exports = fs::read_to_string(dir.path().join("exports.d/zendiloos.exports")).unwrap();
        assert!(exports.contains("all_squash,anonuid=995,anongid=995"), "{exports}");
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert!(
            calls.contains("enable --now nfs-server.service")
                && calls.contains("reload-or-restart nfs-server")
        );

        fs::write(dir.path().join("group"), "root:x:0:\n").unwrap();
        let without = installed.apply(&[], &[export], &services, None, &[]).await.unwrap_err();
        assert!(without.to_string().contains("zendiloos-files are missing"), "{without}");
    }

    #[test]
    fn a_personal_folder_of_an_earlier_account_is_moved_aside() {
        let homes = tempfile::tempdir().unwrap();
        let me = rustix::process::getuid().as_raw();
        assert!(fresh_home(homes.path(), "anna", me).unwrap(), "made");
        fs::write(homes.path().join("anna/diary.txt"), "old").unwrap();
        assert!(!fresh_home(homes.path(), "anna", me).unwrap(), "hers already");

        // The same name, another account now.
        assert!(fresh_home(homes.path(), "anna", me + 1).unwrap(), "made anew");
        assert_eq!(fs::read_dir(homes.path().join("anna")).unwrap().count(), 0, "fresh");
        let aside = homes.path().join(format!("anna.previous-{me}"));
        assert_eq!(fs::read_to_string(aside.join("diary.txt")).unwrap(), "old", "the old files stay");

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

    fn acl_of(path: &Path, name: &str) -> Option<Acl> {
        read_acl(path.to_str().unwrap(), name).unwrap()
    }

    fn named_of(path: &Path, who: Who) -> Option<u16> {
        acl_of(path, acl::ACCESS).and_then(|acl| acl.named(who))
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    /// Sets `entries` on `path`'s access ACL (and default ACL, for a
    /// folder), from `mode`.
    fn set_named(path: &Path, mode: u32, entries: &[(Who, u16)]) {
        let mut acl = Acl::from_mode(mode);
        for (who, perm) in entries {
            acl.set(*who, *perm);
        }
        acl.fix_mask();
        rustix::fs::setxattr(path, acl::ACCESS, &acl.to_bytes(), rustix::fs::XattrFlags::empty()).unwrap();
        if path.is_dir() {
            rustix::fs::setxattr(path, acl::DEFAULT, &acl.to_bytes(), rustix::fs::XattrFlags::empty())
                .unwrap();
        }
    }

    /// The test's own user and group: what a share's folder is given to
    /// here, with the group standing in for `zendiloos-files`.
    fn me() -> Owner {
        Owner { uid: rustix::process::getuid().as_raw(), files: rustix::process::getgid().as_raw() }
    }

    fn people() -> Managed {
        Managed { ids: Some(2000..=59999), groups: vec![990], app_users: vec![2500], app_groups: vec![2501] }
    }

    #[test]
    fn a_shares_folder_and_its_bins_get_the_one_acl() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("photos");
        fs::create_dir(&root).unwrap();
        let name = ShareName::parse("photos").unwrap();
        // The folder as walks of before left it: 0770, an entry per person,
        // one app's.
        fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o770)).unwrap();
        set_named(&root, 0o770, &[(Who::User(2001), 7), (Who::Group(2100), 5), (Who::User(2500), 5)]);
        let owner = me();

        prepare_root(&root, &name, owner, &people()).unwrap();
        prepare_root(&root, &name, owner, &people()).unwrap();

        let bins = root.join(RECYCLE_DIRECTORY);
        assert_eq!(mode_of(&root), ROOT_MODE, "the group takes over what is made");
        assert_eq!(mode_of(&bins), BINS_MODE, "only root renames the bins");
        // The bins were made in the folder of before: they took over its
        // default entries, the app's too.
        for path in [&root, &bins] {
            let meta = fs::metadata(path).unwrap();
            assert_eq!((meta.uid(), meta.gid()), (owner.uid, owner.files));
            for kind in [acl::ACCESS, acl::DEFAULT] {
                let acl = acl_of(path, kind).unwrap();
                assert_eq!(acl.named(Who::Group(owner.files)), Some(7), "{path:?} {kind}");
                assert_eq!((acl.named(Who::User(2001)), acl.named(Who::Group(2100))), (None, None));
                assert_eq!(acl.mask(), Some(7));
            }
            assert_eq!(named_of(path, Who::User(2500)), Some(5), "an app's entry stays");
        }
    }

    #[test]
    fn a_new_shares_folder_has_exactly_the_one_acl() {
        let dir = tempfile::tempdir().unwrap();
        let name = ShareName::parse("media").unwrap();
        let owner = me();
        prepare_root(dir.path(), &name, owner, &people()).unwrap();
        let mut expected = Acl::from_mode(0o770);
        expected.set(Who::Group(owner.files), 7);
        expected.fix_mask();
        let bytes = |kind| acl_of(dir.path(), kind).map(|acl| acl.to_bytes());
        assert_eq!(bytes(acl::ACCESS), Some(expected.to_bytes()));
        assert_eq!(bytes(acl::DEFAULT), Some(expected.to_bytes()));
    }

    #[test]
    fn the_walk_gives_every_entry_the_group_and_takes_the_peoples_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        fs::create_dir_all(root.join("album/2024")).unwrap();
        fs::write(root.join("album/2024/a.jpg"), "x").unwrap();
        fs::write(root.join("run.sh"), "x").unwrap();
        fs::set_permissions(root.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // A private bin of before, and what is in it.
        let bin = root.join(RECYCLE_DIRECTORY).join("anna");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("old.txt"), "x").unwrap();
        fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("escape")).unwrap();
        let (anna, family, users, app, debian) =
            (Who::User(2001), Who::Group(2100), Who::Group(990), Who::User(2500), Who::User(1000));
        for folder in ["album", "album/2024"] {
            set_named(
                &root.join(folder),
                0o770,
                &[(anna, 7), (family, 5), (users, 7), (app, 7), (debian, 5)],
            );
        }
        set_named(&root.join("album/2024/a.jpg"), 0o660, &[(anna, 6), (app, 6)]);
        let files = me().files;
        let before = fs::metadata(root.join("album/2024/a.jpg")).unwrap().uid();
        let mut reported = Vec::new();

        let walked = change_tree(&root, files, &people(), &mut |n| reported.push(n)).unwrap();

        assert_eq!(walked, 7, "two folders, a photo, a program, the bins, a bin and what is in it");
        assert_eq!(reported, [2, 4, 6], "every few entries");
        for folder in ["album", "album/2024", ".recycle", ".recycle/anna"] {
            let path = root.join(folder);
            for kind in [acl::ACCESS, acl::DEFAULT] {
                let acl = acl_of(&path, kind).unwrap();
                assert_eq!(acl.named(Who::Group(files)), Some(7), "{folder} {kind}");
                assert_eq!((acl.named(anna), acl.named(family), acl.named(users)), (None, None, None));
            }
        }
        assert_eq!(mode_of(&bin) & 0o070, 0o070, "a private bin of before is the share's now");
        let photo = root.join("album/2024/a.jpg");
        assert_eq!(named_of(&photo, Who::Group(files)), Some(6), "no execute on a plain file");
        assert_eq!((named_of(&photo, anna), named_of(&photo, app)), (None, Some(6)), "the app keeps working");
        assert_eq!(named_of(&root.join("album"), debian), Some(5), "an ID zendiloOS does not hand out stays");
        assert_eq!(named_of(&root.join("run.sh"), Who::Group(files)), Some(7), "a program stays one");
        assert_eq!(named_of(&bin.join("old.txt"), Who::Group(files)), Some(6));
        assert!(acl_of(&elsewhere, acl::ACCESS).is_none(), "not through the link");
        assert_eq!(fs::metadata(&photo).unwrap().uid(), before, "owners stay");
        assert_eq!(fs::read_to_string(&photo).unwrap(), "x");

        // A second walk finds everything so already.
        assert_eq!(change_tree(&root, files, &people(), &mut |_| {}).unwrap(), 7);
    }

    #[test]
    fn a_wide_tree_keeps_only_the_folders_on_the_way_down_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        for n in 0..1500 {
            fs::create_dir_all(root.join(format!("{n}/inner"))).unwrap();
        }
        let files = me().files;
        assert_eq!(change_tree(&root, files, &Managed::default(), &mut |_| {}).unwrap(), 3000);
        for n in [0, 749, 1499] {
            assert_eq!(named_of(&root.join(format!("{n}/inner")), Who::Group(files)), Some(7));
        }
    }

    #[test]
    fn what_lies_too_deep_to_look_into_fails_the_walk_after_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("share");
        let mut deep = root.clone();
        for n in 0..MAX_DEPTH + 2 {
            deep = deep.join(format!("d{n}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(root.join("beside.txt"), "x").unwrap();
        let files = me().files;

        let error = change_tree(&root, files, &people(), &mut |_| {}).unwrap_err().to_string();

        assert!(error.contains("1 folders are nested too deeply"), "{error}");
        assert!(!error.contains("could not be changed"), "{error}");
        assert_eq!(named_of(&root.join("d0"), Who::Group(files)), Some(7), "above it: changed");
        assert_eq!(named_of(&root.join("beside.txt"), Who::Group(files)), Some(6), "the rest too");
        let mut limit = root.clone();
        for n in 0..MAX_DEPTH {
            limit = limit.join(format!("d{n}"));
        }
        assert_eq!(named_of(&limit, Who::Group(files)), Some(7), "the folder at the limit itself: changed");
        assert_eq!(named_of(&limit.join(format!("d{MAX_DEPTH}")), Who::Group(files)), None);
    }

    #[test]
    fn failures_are_counted_with_the_first_of_each_kind() {
        let mut failures = Failures::default();
        assert!(!failures.any());
        for _ in 0..1000 {
            failures.note(Path::new("/volume1/media/a"), Errno::IO);
        }
        failures.note(Path::new("/volume1/media/b"), Errno::IO);
        failures.note_too_deep(Path::new("/volume1/media/deep"));
        assert!(failures.any());
        let error = failures.error().to_string();
        assert!(error.contains("1001 entries could not be changed, among them /volume1/media/a"), "{error}");
        assert!(
            error.contains("1 folders are nested too deeply to look into, among them /volume1/media/deep")
        );
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
    async fn a_change_of_a_shares_folder_does_not_wait_long_for_its_walk() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HostPaths { volumes: dir.path().to_owned(), ..HostPaths::default() };
        let sharing = Sharing::new(&Tools::default(), paths, accounts(dir.path())).unwrap();
        let volume = VolumeNumber::new(1).unwrap();
        let (media, photos) = (ShareName::parse("media").unwrap(), ShareName::parse("photos").unwrap());
        let walking = sharing.share_lock(volume, &media);
        let _walking = walking.lock().await;

        let refused = sharing.prepare_share(volume, &media).await.unwrap_err();
        assert!(refused.to_string().contains("try again once that has ended"), "{refused}");
        // Another share is not held up: it gets as far as finding its folder.
        let other = sharing.prepare_share(volume, &photos).await.unwrap_err();
        assert!(other.to_string().contains("not present"), "{other}");
        fs::create_dir_all(dir.path().join("volume1/photos")).unwrap();
        let unmounted = sharing.migrate_share(volume, &photos).await.unwrap_err();
        assert!(unmounted.to_string().contains("not mounted"), "nothing lands on the volume: {unmounted}");
    }
}
