//! Folders in shares that apps were given, pinned (docs/architecture.md,
//! 4.9).
//!
//! A share's root is its dataset's mount point: nobody but root can move it.
//! A folder in a share can be swapped for a link by anyone who may write to
//! the share (an NFS client, another app), and Docker follows links in a
//! bind mount's source every time it starts a container, also when it
//! restarts one by itself. So an app never mounts such a folder by its
//! path. The folder is walked to from the share's root name by name,
//! without following a link or leaving the share's file system, and what
//! was opened is mounted at `/run/zendiloos-app-shares/<app>/<share id>`,
//! a place only root can change; the containers mount that.
//!
//! The mounts are made in the host's mount namespace, outside the agent's
//! sandbox: by a unit per app, `zendiloos-app-<app>-shares.service`, which
//! runs this binary as `pin-shares <app>` at boot before Docker starts, and
//! again before each start of the app; stopping it unmounts them.

use std::fs;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags, fstat, open, openat};
use rustix::mount::{MoveMountFlags, OpenTreeFlags, UnmountFlags, move_mount, open_tree, unmount};
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::{AppFolder, AppName};

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// One folder an app mounts in place of a share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pin {
    /// The share's root, `/volume<N>/<share>`.
    pub share: String,
    /// The folder in it.
    pub folder: AppFolder,
    /// Where it is mounted for the containers.
    pub at: String,
}

/// `zendiloos-app-<app>-shares.service`.
pub(crate) fn unit_name(app: &AppName) -> String {
    format!("zendiloos-app-{}-shares.service", app.as_str())
}

/// The unit that pins `app`'s folders, after the shares' own mounts and
/// before Docker. It runs outside any mount namespace of its own, or the
/// host would not see what it mounts; what it may do is narrowed
/// otherwise.
pub(crate) fn unit(agent: &Path, app: &AppName, shares: &[&str]) -> String {
    let agent = agent.display();
    format!(
        "# Created by zendiloOS for the app {app}; removed with the app.\n\
         [Unit]\n\
         Description=zendiloOS app {app}: the folders of shares it was given\n\
         RequiresMountsFor={mounts}\n\
         Before=docker.service\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart={agent} pin-shares {app}\n\
         ExecStop={agent} unpin-shares {app}\n\
         NoNewPrivileges=yes\n\
         CapabilityBoundingSet=CAP_SYS_ADMIN CAP_DAC_READ_SEARCH\n\
         RestrictAddressFamilies=AF_UNIX\n\
         LockPersonality=yes\n\
         MemoryDenyWriteExecute=yes\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        app = app.as_str(),
        mounts = shares.join(" "),
    )
}

/// Where the folder for the share `id` of `app` is mounted.
pub(crate) fn pin_path(paths: &HostPaths, app: &AppName, id: &str) -> Result<PathBuf, OperationError> {
    let plain = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if !plain {
        return Err(refused(format!("the share id {id:?} cannot name a folder")));
    }
    Ok(paths.app_shares.join(app.as_str()).join(id))
}

/// Opens `folder` in the share at `share`, which must be a mounted file
/// system's root: name by name, never through a link, never onto another
/// file system.
pub(crate) fn walk(share: &Path, folder: &AppFolder) -> Result<OwnedFd, OperationError> {
    descend(&open_mounted(share)?, share, folder)
}

/// Opens `folder` below `root` (the share at `share`), as [`walk`] does.
fn descend(root: &OwnedFd, share: &Path, folder: &AppFolder) -> Result<OwnedFd, OperationError> {
    let file_system = fstat(root).map_err(|_| not_reachable(share, folder, "cannot be opened"))?.st_dev;
    let mut current = openat(root, ".", PATH_FLAGS, Mode::empty())
        .map_err(|_| not_reachable(share, folder, "cannot be opened"))?;
    for name in folder.names() {
        current = openat(&current, name, PATH_FLAGS, Mode::empty())
            .map_err(|_| not_reachable(share, folder, "is missing, a link or no folder"))?;
        let device = fstat(&current).map_err(|_| not_reachable(share, folder, "cannot be opened"))?.st_dev;
        if device != file_system {
            return Err(not_reachable(share, folder, "has another file system mounted on the way"));
        }
    }
    Ok(current)
}

/// Whether `at` is the pin of `folder` in `share` now: a mount of the very
/// folder the walk reaches.
pub(crate) fn is_pinned(at: &Path, share: &Path, folder: &AppFolder) -> Result<(), OperationError> {
    let pinned = open_mounted(at)?;
    let folder_now = walk(share, folder)?;
    let (pinned, folder_now) = (
        fstat(&pinned).map_err(|_| refused(format!("{} cannot be opened", at.display())))?,
        fstat(&folder_now).map_err(|_| refused(format!("{} cannot be opened", share.display())))?,
    );
    if (pinned.st_dev, pinned.st_ino) == (folder_now.st_dev, folder_now.st_ino) {
        Ok(())
    } else {
        Err(refused(format!("{} is not the folder {} of {}", at.display(), folder.as_str(), share.display())))
    }
}

/// `pin-shares <app>`: mounts each folder of the app's kept compose file at
/// its place, and nothing else stays there.
pub fn pin_shares(paths: &HostPaths, app: &AppName) -> Result<(), String> {
    pin(paths, app).map_err(|error| error.to_string())
}

/// `unpin-shares <app>`: unmounts the app's folders and removes their
/// places.
pub fn unpin_shares(paths: &HostPaths, app: &AppName) -> Result<(), String> {
    unpin(paths, app).map_err(|error| error.to_string())
}

fn pin(paths: &HostPaths, app: &AppName) -> Result<(), OperationError> {
    let pins = kept_pins(paths, app)?;
    let base = own_folder(&paths.app_shares)?;
    let folder = own_folder(&paths.app_shares.join(app.as_str()))?;
    drop((base, folder));
    let wanted: Vec<PathBuf> = pins.iter().map(|pin| PathBuf::from(&pin.at)).collect();
    clear(&paths.app_shares.join(app.as_str()), |path| !wanted.contains(&path.to_path_buf()))?;
    for pin in &pins {
        let at = PathBuf::from(&pin.at);
        let id = at.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if pin_path(paths, app, id)? != at {
            return Err(refused(format!("{} is no place for a folder of {}", pin.at, app.as_str())));
        }
        let source = walk(Path::new(&pin.share), &pin.folder)?;
        release(&at)?;
        match fs::DirBuilder::new().mode(0o700).create(&at) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(OperationError::Write(at.clone(), error)),
        }
        let target =
            open(&at, PATH_FLAGS, Mode::empty()).map_err(|e| OperationError::Write(at.clone(), e.into()))?;
        let tree = open_tree(
            &source,
            "",
            OpenTreeFlags::OPEN_TREE_CLONE | OpenTreeFlags::OPEN_TREE_CLOEXEC | OpenTreeFlags::AT_EMPTY_PATH,
        )
        .map_err(|e| OperationError::Write(at.clone(), e.into()))?;
        move_mount(
            tree.as_fd(),
            "",
            target.as_fd(),
            "",
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH | MoveMountFlags::MOVE_MOUNT_T_EMPTY_PATH,
        )
        .map_err(|e| OperationError::Write(at.clone(), e.into()))?;
    }
    Ok(())
}

fn unpin(paths: &HostPaths, app: &AppName) -> Result<(), OperationError> {
    let folder = paths.app_shares.join(app.as_str());
    clear(&folder, |_| true)?;
    match fs::remove_dir(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(OperationError::Write(folder, error)),
    }
}

/// The pins the kept compose file of `app` lists.
fn kept_pins(paths: &HostPaths, app: &AppName) -> Result<Vec<Pin>, OperationError> {
    let path = paths.apps.join(format!("{}.json", app.as_str()));
    let text = fs::read_to_string(&path).map_err(|e| OperationError::Read(path.clone(), e))?;
    let normalized: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| refused(format!("{} cannot be read", path.display())))?;
    pins_of(&normalized)
}

/// The pins in a normalized compose file's extension.
pub(crate) fn pins_of(normalized: &serde_json::Value) -> Result<Vec<Pin>, OperationError> {
    let Some(pins) = normalized.get(crate::apps::EXTENSION).and_then(|e| e.get("pins")) else {
        return Ok(Vec::new());
    };
    serde_json::from_value(pins.clone()).map_err(|_| refused("the kept compose file lists odd folders"))
}

/// A folder only root may change, made if missing; never through a link.
fn own_folder(path: &Path) -> Result<OwnedFd, OperationError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(OperationError::Write(path.to_owned(), error)),
    }
    let folder = open(path, PATH_FLAGS, Mode::empty())
        .map_err(|e| OperationError::Write(path.to_owned(), e.into()))?;
    let stat = fstat(&folder).map_err(|e| OperationError::Write(path.to_owned(), e.into()))?;
    if stat.st_uid != 0 || stat.st_mode & 0o022 != 0 {
        return Err(refused(format!("{} may be changed by others than root", path.display())));
    }
    Ok(folder)
}

/// Unmounts and removes each place in `folder` that `remove` picks.
fn clear(folder: &Path, remove: impl Fn(&Path) -> bool) -> Result<(), OperationError> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(OperationError::Read(folder.to_owned(), error)),
    };
    for entry in entries {
        let path = entry.map_err(|e| OperationError::Read(folder.to_owned(), e))?.path();
        if remove(&path) {
            release(&path)?;
            match fs::remove_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(OperationError::Write(path, error)),
            }
        }
    }
    Ok(())
}

/// Unmounts whatever is mounted at `at`, also several mounts on top of
/// each other; containers that mount it keep their own.
fn release(at: &Path) -> Result<(), OperationError> {
    while is_mount_point(at) {
        unmount(at, UnmountFlags::DETACH | UnmountFlags::NOFOLLOW)
            .map_err(|e| OperationError::Write(at.to_owned(), e.into()))?;
    }
    Ok(())
}

fn is_mount_point(path: &Path) -> bool {
    open_mounted(path).is_ok()
}

const PATH_FLAGS: OFlags =
    OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

/// Opens a directory that is a file system's root (a dataset or a pin
/// mounted there), without following a link to it.
pub(crate) fn open_mounted(path: &Path) -> Result<OwnedFd, OperationError> {
    let not_mounted = || refused(format!("{} is not mounted", path.display()));
    let directory = open(path, PATH_FLAGS, Mode::empty()).map_err(|_| not_mounted())?;
    let parent = open(path.parent().unwrap_or(Path::new("/")), PATH_FLAGS, Mode::empty())
        .map_err(|_| not_mounted())?;
    let (own, above) =
        (fstat(&directory).map_err(|_| not_mounted())?, fstat(&parent).map_err(|_| not_mounted())?);
    // A folder bound onto a folder of the same file system has the same
    // device; its mount ID tells it apart.
    if own.st_dev == above.st_dev && mount_id(&directory) == mount_id(&parent) {
        return Err(not_mounted());
    }
    Ok(directory)
}

/// The ID of the mount `fd` is on, from `/proc/self/fdinfo`.
fn mount_id(fd: &OwnedFd) -> Option<u64> {
    use std::os::fd::AsRawFd;
    let info = fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).ok()?;
    info.lines().find_map(|line| line.strip_prefix("mnt_id:")).and_then(|id| id.trim().parse().ok())
}

fn not_reachable(share: &Path, folder: &AppFolder, reason: &str) -> OperationError {
    refused(format!("the folder {} of {} {reason}", folder.as_str(), share.display()))
}

fn refused(reason: impl Into<String>) -> OperationError {
    OperationError::Refused(reason.into())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn app() -> AppName {
        AppName::parse("jellyfin").unwrap()
    }

    #[test]
    fn a_folder_is_reached_only_name_by_name() {
        let share = tempfile::tempdir().unwrap();
        fs::create_dir_all(share.path().join("Films/New")).unwrap();
        std::os::unix::fs::symlink("/", share.path().join("escape")).unwrap();
        std::os::unix::fs::symlink("Films", share.path().join("inside")).unwrap();
        fs::write(share.path().join("file"), "").unwrap();
        let root = open(share.path(), PATH_FLAGS, Mode::empty()).unwrap();
        let reach = |folder: &str| descend(&root, share.path(), &AppFolder::parse(folder).unwrap());
        let reached = reach("Films/New").unwrap();
        let expected = fs::metadata(share.path().join("Films/New")).unwrap();
        assert_eq!(fstat(&reached).unwrap().st_ino, std::os::unix::fs::MetadataExt::ino(&expected));
        for refused in ["escape", "inside", "inside/New", "file", "missing"] {
            assert!(reach(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn only_a_mounted_share_is_walked() {
        let share = tempfile::tempdir().unwrap();
        let error = walk(share.path(), &AppFolder::whole()).unwrap_err();
        assert!(error.to_string().contains("is not mounted"), "{error}");
    }

    #[test]
    fn pins_have_a_place_of_their_own() {
        let paths = HostPaths { app_shares: PathBuf::from("/run/pins"), ..HostPaths::default() };
        assert_eq!(pin_path(&paths, &app(), "media").unwrap(), Path::new("/run/pins/jellyfin/media"));
        for odd in ["", "..", "a/b", "a b", &"x".repeat(65)] {
            assert!(pin_path(&paths, &app(), odd).is_err(), "{odd:?}");
        }
    }

    #[test]
    fn the_unit_comes_after_the_shares_and_before_docker() {
        let unit = unit(Path::new("/usr/libexec/zendiloos/zendiloos-agent"), &app(), &["/volume1/media"]);
        assert!(unit.contains("RequiresMountsFor=/volume1/media\n"));
        assert!(unit.contains("Before=docker.service\n"));
        assert!(unit.contains("ExecStart=/usr/libexec/zendiloos/zendiloos-agent pin-shares jellyfin\n"));
        assert!(unit.contains("ExecStop=/usr/libexec/zendiloos/zendiloos-agent unpin-shares jellyfin\n"));
        assert!(!unit.contains("ProtectSystem"), "a mount namespace of its own would hide the mounts");
        assert_eq!(unit_name(&app()), "zendiloos-app-jellyfin-shares.service");
    }

    #[test]
    fn pins_are_read_from_the_kept_file() {
        let none = json!({ "services": {}, "x-zendiloos": { "app_data": "/volume1/@apps/x", "shares": [] } });
        assert!(pins_of(&none).unwrap().is_empty());
        let pinned = json!({ "x-zendiloos": { "pins": [
            { "share": "/volume1/media", "folder": "Films", "at": "/run/zendiloos-app-shares/jellyfin/media" }
        ] } });
        let pins = pins_of(&pinned).unwrap();
        assert_eq!(pins[0].folder.as_str(), "Films");
        let odd = json!({ "x-zendiloos": { "pins": [{ "share": "/volume1/media", "folder": "../etc", "at": "/x" }] } });
        assert!(pins_of(&odd).is_err());
    }
}
