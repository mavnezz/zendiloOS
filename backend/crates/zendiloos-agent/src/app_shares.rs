//! Folders and files in shares that apps and administrators' Compose
//! projects mount, pinned (docs/architecture.md, 4.9).
//!
//! A share's root is its dataset's mount point: nobody but root can move it.
//! Something in a share can be swapped for a link by anyone who may write
//! to the share (a user over SMB, an NFS client, another app), and Docker
//! follows links in a bind mount's source every time it starts a container,
//! also when it restarts one by itself. So containers never mount such a
//! folder or file by its path. It is walked to from the share's root name by
//! name, without following a link or leaving the share's file system, and
//! what was opened is mounted at `/run/zendiloos-app-shares/<app>/<id>` or
//! `/run/zendiloos-project-shares/<project>/<id>`, places only root can
//! change; the containers mount those.
//!
//! The mounts are made in the host's mount namespace, outside the agent's
//! sandbox: by a unit per app or project
//! (`zendiloos-app-<app>-shares.service`,
//! `zendiloos-project-<project>-shares.service`), which runs this binary as
//! `pin-shares <app>` or `pin-project <project>` at boot before Docker
//! starts, and again before each start; stopping it unmounts them. A folder
//! a project's bind names that does not exist yet is made by that unit, as
//! Docker would make it: the agent's own view of the volumes is read-only.

use std::fs;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{FileType, Mode, OFlags, fstat, mkdirat, open, openat};
use rustix::io::Errno;
use rustix::mount::{MoveMountFlags, OpenTreeFlags, UnmountFlags, move_mount, open_tree, unmount};
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::{AppFolder, AppName, ProjectName};
use zendiloos_exec::{Limits, Program};

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// Whose folders: an app's of a catalog, or an administrator's project.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Owner<'a> {
    App(&'a AppName),
    Project(&'a ProjectName),
}

impl Owner<'_> {
    fn name(&self) -> &str {
        match self {
            Self::App(app) => app.as_str(),
            Self::Project(project) => project.as_str(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::App(_) => "app",
            Self::Project(_) => "project",
        }
    }

    /// `zendiloos-app-<app>-shares.service`,
    /// `zendiloos-project-<project>-shares.service`.
    pub(crate) fn unit_name(&self) -> String {
        format!("zendiloos-{}-{}-shares.service", self.kind(), self.name())
    }

    /// Its normalized Compose file, kept by root.
    pub(crate) fn kept(&self, paths: &HostPaths) -> PathBuf {
        let folder = match self {
            Self::App(_) => &paths.apps,
            Self::Project(_) => &paths.projects,
        };
        folder.join(format!("{}.json", self.name()))
    }

    fn base<'p>(&self, paths: &'p HostPaths) -> &'p Path {
        match self {
            Self::App(_) => &paths.app_shares,
            Self::Project(_) => &paths.project_shares,
        }
    }

    /// Where its pins are mounted.
    fn places(&self, paths: &HostPaths) -> PathBuf {
        self.base(paths).join(self.name())
    }

    /// The commands of this program that pin and unpin.
    fn commands(&self) -> (&'static str, &'static str) {
        match self {
            Self::App(_) => ("pin-shares", "unpin-shares"),
            Self::Project(_) => ("pin-project", "unpin-project"),
        }
    }
}

/// One folder or file a container mounts in place of a path in a share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pin {
    /// The share's root, `/volume<N>/<share>`, or the volume's,
    /// `/volume<N>`: a mounted file system.
    pub share: String,
    /// The folder or file in it.
    pub folder: AppFolder,
    /// Where it is mounted for the containers.
    pub at: String,
    /// A file rather than a folder.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub file: bool,
    /// Made, as a folder, if it does not exist: what Docker does for a
    /// bind whose source is missing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub create: bool,
}

/// The unit that pins `owner`'s folders, after the shares' own mounts and
/// before Docker. It runs outside any mount namespace of its own, or the
/// host would not see what it mounts; what it may do is narrowed
/// otherwise. A project's unit may make missing folders in shares, as
/// Docker would, so it may write where the shares' permissions say no.
pub(crate) fn unit(agent: &Path, owner: Owner<'_>, shares: &[&str]) -> String {
    let (pin, unpin) = owner.commands();
    let (what, capabilities) = match owner {
        Owner::App(_) => ("the folders of shares it was given", "CAP_SYS_ADMIN CAP_DAC_READ_SEARCH"),
        Owner::Project(_) => (
            "the folders and files in shares its binds name",
            "CAP_SYS_ADMIN CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE",
        ),
    };
    format!(
        "# Created by zendiloOS for the {kind} {name}; removed with the {kind}.\n\
         [Unit]\n\
         Description=zendiloOS {kind} {name}: {what}\n\
         RequiresMountsFor={mounts}\n\
         Before=docker.service\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart={agent} {pin} {name}\n\
         ExecStop={agent} {unpin} {name}\n\
         NoNewPrivileges=yes\n\
         CapabilityBoundingSet={capabilities}\n\
         RestrictAddressFamilies=AF_UNIX\n\
         LockPersonality=yes\n\
         MemoryDenyWriteExecute=yes\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        kind = owner.kind(),
        name = owner.name(),
        agent = agent.display(),
        mounts = shares.join(" "),
    )
}

const SYSTEMCTL: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 1024 * 1024 };

async fn systemctl(program: &Program, args: &[&str]) -> Result<(), OperationError> {
    let output = zendiloos_exec::run(program, args, SYSTEMCTL).await?;
    if output.status.success() {
        return Ok(());
    }
    let lines: Vec<&str> = output.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Err(OperationError::Failed(format!("systemctl {}: {tail}", args.join(" "))))
}

/// Writes and enables the unit pinning `owner`'s folders when there are
/// any; stops and removes it when there are none. Starting it is the
/// caller's.
pub(crate) async fn place_unit(
    program: &Program,
    paths: &HostPaths,
    owner: Owner<'_>,
    pins: &[Pin],
) -> Result<(), OperationError> {
    if pins.is_empty() {
        return remove_unit(program, paths, owner).await;
    }
    let name = owner.unit_name();
    let path = paths.units.join(&name);
    let mut shares: Vec<&str> = pins.iter().map(|pin| pin.share.as_str()).collect();
    shares.sort_unstable();
    shares.dedup();
    let content = unit(&paths.agent, owner, &shares);
    crate::durable::replace(&path, content.as_bytes())?;
    systemctl(program, &["daemon-reload"]).await?;
    systemctl(program, &["enable", &name]).await
}

/// Stops (and so unpins) and removes `owner`'s unit, if there is one.
pub(crate) async fn remove_unit(
    program: &Program,
    paths: &HostPaths,
    owner: Owner<'_>,
) -> Result<(), OperationError> {
    let name = owner.unit_name();
    let path = paths.units.join(&name);
    if path.exists() {
        systemctl(program, &["disable", "--now", &name]).await?;
        fs::remove_file(&path).map_err(|e| OperationError::Write(path.clone(), e))?;
        systemctl(program, &["daemon-reload"]).await?;
    }
    Ok(())
}

/// Pins `owner`'s folders now (again): a folder that was missing at boot
/// may be back, and what was pinned stays the same otherwise.
pub(crate) async fn repin(program: &Program, owner: Owner<'_>) -> Result<(), OperationError> {
    systemctl(program, &["restart", &owner.unit_name()]).await
}

/// Whether `path` is a volume, `/volume<N>`, or lies in one: where Docker
/// must never make a missing folder, for if the pool was not mounted yet it
/// would make it on the system's disk and the containers fill that.
pub(crate) fn on_a_volume(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/volume") else { return false };
    let number = rest.split('/').next().unwrap_or_default();
    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
}

/// Where the pin `id` of `owner` is mounted.
pub(crate) fn pin_path(paths: &HostPaths, owner: Owner<'_>, id: &str) -> Result<PathBuf, OperationError> {
    let plain = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if !plain {
        return Err(refused(format!("the share id {id:?} cannot name a folder")));
    }
    Ok(owner.places(paths).join(id))
}

/// Opens `folder` in the share at `share`, which must be a mounted file
/// system's root: name by name, never through a link, never onto another
/// file system.
pub(crate) fn walk(share: &Path, folder: &AppFolder) -> Result<OwnedFd, OperationError> {
    descend(&open_mounted(share)?, share, folder)
}

/// Opens `folder` below `root` (the share at `share`), as [`walk`] does.
fn descend(root: &OwnedFd, share: &Path, folder: &AppFolder) -> Result<OwnedFd, OperationError> {
    let file_system = device(root, share, folder)?;
    let mut current = openat(root, ".", PATH_FLAGS, Mode::empty())
        .map_err(|_| not_reachable(share, folder, "cannot be opened"))?;
    for name in folder.names() {
        current = openat(&current, name, PATH_FLAGS, Mode::empty())
            .map_err(|_| not_reachable(share, folder, "is missing, a link or no folder"))?;
        if device(&current, share, folder)? != file_system {
            return Err(not_reachable(share, folder, "has another file system mounted on the way"));
        }
    }
    Ok(current)
}

fn device(fd: &OwnedFd, share: &Path, folder: &AppFolder) -> Result<u64, OperationError> {
    Ok(fstat(fd).map_err(|_| not_reachable(share, folder, "cannot be opened"))?.st_dev)
}

/// What a bind finds at `folder` of the share at `share`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    Folder,
    File,
    /// It, or a folder on the way, does not exist.
    Missing,
}

/// Looks for `folder` (a folder or a file) as [`walk`] goes: a link or
/// another file system on the way is refused, a missing name is
/// [`Found::Missing`].
pub(crate) fn find(share: &Path, folder: &AppFolder) -> Result<Found, OperationError> {
    let root = open_mounted(share)?;
    match last(&root, share, folder, false)? {
        Some(fd) => kind(&fd, share, folder),
        None => Ok(Found::Missing),
    }
}

/// Opens the last name of `folder` below `root` without following it,
/// folder or file; `None` if something on the way is missing (unless
/// `make`, which makes the folders on the way).
fn last(
    root: &OwnedFd,
    share: &Path,
    folder: &AppFolder,
    make: bool,
) -> Result<Option<OwnedFd>, OperationError> {
    let file_system = device(root, share, folder)?;
    let names: Vec<&str> = folder.names().collect();
    let Some((last, way)) = names.split_last() else {
        return Ok(Some(
            openat(root, ".", PATH_FLAGS, Mode::empty())
                .map_err(|_| not_reachable(share, folder, "cannot be opened"))?,
        ));
    };
    let mut current = openat(root, ".", PATH_FLAGS, Mode::empty())
        .map_err(|_| not_reachable(share, folder, "cannot be opened"))?;
    for name in way {
        let Some(next) = step(&current, name, make, share, folder)? else { return Ok(None) };
        current = next;
        if device(&current, share, folder)? != file_system {
            return Err(not_reachable(share, folder, "has another file system mounted on the way"));
        }
    }
    let end = match openat(&current, *last, ANY_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::NOENT) if make => step(&current, last, true, share, folder)?
            .ok_or_else(|| not_reachable(share, folder, "cannot be made"))?,
        Err(Errno::NOENT) => return Ok(None),
        Err(_) => return Err(not_reachable(share, folder, "cannot be opened")),
    };
    if device(&end, share, folder)? != file_system {
        return Err(not_reachable(share, folder, "has another file system mounted on the way"));
    }
    Ok(Some(end))
}

/// The folder `name` in `current`, made first if `make` and missing; never
/// a link.
fn step(
    current: &OwnedFd,
    name: &str,
    make: bool,
    share: &Path,
    folder: &AppFolder,
) -> Result<Option<OwnedFd>, OperationError> {
    match openat(current, name, PATH_FLAGS, Mode::empty()) {
        Ok(fd) => return Ok(Some(fd)),
        Err(Errno::NOENT) if make => {}
        Err(Errno::NOENT) => return Ok(None),
        Err(_) => return Err(not_reachable(share, folder, "is a link or no folder")),
    }
    match mkdirat(current, name, Mode::from_raw_mode(0o755)) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(_) => return Err(not_reachable(share, folder, "cannot be made")),
    }
    openat(current, name, PATH_FLAGS, Mode::empty())
        .map(Some)
        .map_err(|_| not_reachable(share, folder, "is a link or no folder"))
}

fn kind(fd: &OwnedFd, share: &Path, folder: &AppFolder) -> Result<Found, OperationError> {
    let stat = fstat(fd).map_err(|_| not_reachable(share, folder, "cannot be opened"))?;
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::Directory => Ok(Found::Folder),
        FileType::RegularFile => Ok(Found::File),
        FileType::Symlink => Err(not_reachable(share, folder, "is a link")),
        _ => Err(not_reachable(share, folder, "is neither a folder nor a file")),
    }
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
    pin(paths, Owner::App(app)).map_err(|error| error.to_string())
}

/// `unpin-shares <app>`: unmounts the app's folders and removes their
/// places.
pub fn unpin_shares(paths: &HostPaths, app: &AppName) -> Result<(), String> {
    unpin(paths, Owner::App(app)).map_err(|error| error.to_string())
}

/// `pin-project <project>`: as [`pin_shares`], for an administrator's
/// project.
pub fn pin_project(paths: &HostPaths, project: &ProjectName) -> Result<(), String> {
    pin(paths, Owner::Project(project)).map_err(|error| error.to_string())
}

/// `unpin-project <project>`.
pub fn unpin_project(paths: &HostPaths, project: &ProjectName) -> Result<(), String> {
    unpin(paths, Owner::Project(project)).map_err(|error| error.to_string())
}

fn pin(paths: &HostPaths, owner: Owner<'_>) -> Result<(), OperationError> {
    let pins = kept_pins(paths, owner)?;
    let places = owner.places(paths);
    drop((own_folder(owner.base(paths))?, own_folder(&places)?));
    let wanted: Vec<PathBuf> = pins.iter().map(|pin| PathBuf::from(&pin.at)).collect();
    clear(&places, |path| !wanted.contains(&path.to_path_buf()))?;
    for pin in &pins {
        let at = PathBuf::from(&pin.at);
        let id = at.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if pin_path(paths, owner, id)? != at {
            return Err(refused(format!("{} is no place for a folder of {}", pin.at, owner.name())));
        }
        let (share, folder) = (Path::new(&pin.share), &pin.folder);
        let root = open_mounted(share)?;
        let source = last(&root, share, folder, pin.create)?
            .ok_or_else(|| not_reachable(share, folder, "is missing"))?;
        let found = kind(&source, share, folder)?;
        if (found == Found::File) != pin.file {
            let what = if pin.file { "no file" } else { "no folder" };
            return Err(not_reachable(share, folder, &format!("is {what} any more")));
        }
        release(&at)?;
        let made = if pin.file {
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc_nofollow())
                .open(&at)
                .map(drop)
        } else {
            match fs::DirBuilder::new().mode(0o700).create(&at) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                other => other,
            }
        };
        made.map_err(|e| OperationError::Write(at.clone(), e))?;
        let flags = if pin.file { ANY_FLAGS } else { PATH_FLAGS };
        let target =
            open(&at, flags, Mode::empty()).map_err(|e| OperationError::Write(at.clone(), e.into()))?;
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

/// `O_NOFOLLOW` for [`std::fs::OpenOptions`].
fn libc_nofollow() -> i32 {
    OFlags::NOFOLLOW.bits() as i32
}

fn unpin(paths: &HostPaths, owner: Owner<'_>) -> Result<(), OperationError> {
    let folder = owner.places(paths);
    clear(&folder, |_| true)?;
    match fs::remove_dir(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(OperationError::Write(folder, error)),
    }
}

/// The pins `owner`'s kept compose file lists.
fn kept_pins(paths: &HostPaths, owner: Owner<'_>) -> Result<Vec<Pin>, OperationError> {
    let path = owner.kept(paths);
    let text = fs::read_to_string(&path).map_err(|e| OperationError::Read(path.clone(), e))?;
    let normalized: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| refused(format!("{} cannot be read", path.display())))?;
    pins_of(&normalized)
}

/// Who mounts something of the share at `share` (`/volume<N>/<name>`):
/// `app <name>` and `project <name>`, from their kept files. A file that
/// cannot be read counts as one that does: nothing is pulled from under it.
pub(crate) fn pinned_from(paths: &HostPaths, share: &Path) -> Vec<String> {
    let mut users = Vec::new();
    for (kind, folder) in [("app", &paths.apps), ("project", &paths.projects)] {
        let Ok(entries) = fs::read_dir(folder) else { continue };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
            .collect();
        names.sort();
        for name in names {
            let path = folder.join(format!("{name}.json"));
            let pins = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .map(|normalized| pins_of(&normalized));
            let uses = match pins {
                Some(Ok(pins)) => pins.iter().any(|pin| Path::new(&pin.share) == share),
                _ => true,
            };
            if uses {
                users.push(format!("{kind} {name}"));
            }
        }
    }
    users
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
            let removed = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir(&path),
                Ok(_) => fs::remove_file(&path),
                Err(error) => Err(error),
            };
            match removed {
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

/// Whether something (a folder or a file) is mounted at `path`.
fn is_mount_point(path: &Path) -> bool {
    let (Ok(own), Ok(parent)) = (
        open(path, ANY_FLAGS, Mode::empty()),
        open(path.parent().unwrap_or(Path::new("/")), PATH_FLAGS, Mode::empty()),
    ) else {
        return false;
    };
    let (Ok(stat), Ok(above)) = (fstat(&own), fstat(&parent)) else { return false };
    stat.st_dev != above.st_dev || mount_id(&own) != mount_id(&parent)
}

const PATH_FLAGS: OFlags =
    OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
/// A folder or a file, not followed if it is a link.
const ANY_FLAGS: OFlags = OFlags::PATH.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

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

    fn project() -> ProjectName {
        ProjectName::parse("teslamate").unwrap()
    }

    #[test]
    fn who_mounts_a_share_is_read_from_the_kept_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HostPaths {
            apps: dir.path().join("apps"),
            projects: dir.path().join("projects"),
            ..HostPaths::default()
        };
        fs::create_dir_all(&paths.apps).unwrap();
        fs::create_dir_all(&paths.projects).unwrap();
        let kept = |share: &str| {
            json!({ "services": {}, crate::apps::EXTENSION: { "pins": [
                { "share": share, "folder": "Films", "at": "/run/zendiloos-app-shares/x/1" }
            ] } })
            .to_string()
        };
        fs::write(paths.apps.join("jellyfin.json"), kept("/volume1/media")).unwrap();
        fs::write(paths.apps.join("immich.json"), kept("/volume1/photos")).unwrap();
        fs::write(paths.projects.join("teslamate.json"), kept("/volume1/media")).unwrap();
        fs::write(paths.projects.join("broken.json"), "not json").unwrap();

        assert_eq!(
            pinned_from(&paths, Path::new("/volume1/media")),
            ["app jellyfin", "project broken", "project teslamate"]
        );
        assert_eq!(
            pinned_from(&paths, Path::new("/volume1/backup")),
            ["project broken"],
            "unread: kept safe"
        );
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
    fn a_bind_finds_folders_and_files_but_never_a_link() {
        let share = tempfile::tempdir().unwrap();
        fs::create_dir_all(share.path().join("app/data")).unwrap();
        fs::write(share.path().join("app/nginx.conf"), "").unwrap();
        std::os::unix::fs::symlink("/", share.path().join("app/escape")).unwrap();
        std::os::unix::fs::symlink("/etc/shadow", share.path().join("app/secret")).unwrap();
        std::os::unix::fs::symlink("app", share.path().join("inside")).unwrap();
        let root = open(share.path(), PATH_FLAGS, Mode::empty()).unwrap();
        let look = |folder: &str| {
            let folder = AppFolder::parse(folder).unwrap();
            last(&root, share.path(), &folder, false)
                .and_then(|fd| fd.map_or(Ok(Found::Missing), |fd| kind(&fd, share.path(), &folder)))
        };
        assert_eq!(look("app/data").unwrap(), Found::Folder);
        assert_eq!(look("app/nginx.conf").unwrap(), Found::File);
        assert_eq!(look("app/new").unwrap(), Found::Missing);
        assert_eq!(look("app/new/deeper").unwrap(), Found::Missing);
        for refused in ["app/escape", "app/secret", "inside/data", "app/nginx.conf/x"] {
            assert!(look(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn missing_folders_are_made_on_the_way_but_not_through_a_link() {
        let share = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", share.path().join("link")).unwrap();
        let root = open(share.path(), PATH_FLAGS, Mode::empty()).unwrap();
        let folder = AppFolder::parse("docker/app/data").unwrap();
        let made = last(&root, share.path(), &folder, true).unwrap().unwrap();
        assert_eq!(kind(&made, share.path(), &folder).unwrap(), Found::Folder);
        assert!(share.path().join("docker/app/data").is_dir());
        let through = AppFolder::parse("link/data").unwrap();
        assert!(last(&root, share.path(), &through, true).is_err(), "made through a link");
    }

    #[test]
    fn only_a_mounted_share_is_walked() {
        let share = tempfile::tempdir().unwrap();
        let error = walk(share.path(), &AppFolder::whole()).unwrap_err();
        assert!(error.to_string().contains("is not mounted"), "{error}");
        let error = find(share.path(), &AppFolder::parse("x").unwrap()).unwrap_err();
        assert!(error.to_string().contains("is not mounted"), "{error}");
    }

    #[test]
    fn pins_have_a_place_of_their_own() {
        let paths = HostPaths {
            app_shares: PathBuf::from("/run/pins"),
            project_shares: PathBuf::from("/run/project-pins"),
            ..HostPaths::default()
        };
        assert_eq!(
            pin_path(&paths, Owner::App(&app()), "media").unwrap(),
            Path::new("/run/pins/jellyfin/media")
        );
        assert_eq!(
            pin_path(&paths, Owner::Project(&project()), "1").unwrap(),
            Path::new("/run/project-pins/teslamate/1")
        );
        for odd in ["", "..", "a/b", "a b", &"x".repeat(65)] {
            assert!(pin_path(&paths, Owner::App(&app()), odd).is_err(), "{odd:?}");
        }
    }

    #[test]
    fn the_unit_comes_after_the_shares_and_before_docker() {
        let agent = Path::new("/usr/libexec/zendiloos/zendiloos-agent");
        let unit = unit(agent, Owner::App(&app()), &["/volume1/media"]);
        assert!(unit.contains("RequiresMountsFor=/volume1/media\n"));
        assert!(unit.contains("Before=docker.service\n"));
        assert!(unit.contains("ExecStart=/usr/libexec/zendiloos/zendiloos-agent pin-shares jellyfin\n"));
        assert!(unit.contains("ExecStop=/usr/libexec/zendiloos/zendiloos-agent unpin-shares jellyfin\n"));
        assert!(!unit.contains("ProtectSystem"), "a mount namespace of its own would hide the mounts");
        assert!(!unit.contains("CAP_DAC_OVERRIDE"), "an app's unit makes nothing");
        assert_eq!(Owner::App(&app()).unit_name(), "zendiloos-app-jellyfin-shares.service");

        let unit = super::unit(agent, Owner::Project(&project()), &["/volume1/docker"]);
        assert!(unit.contains("ExecStart=/usr/libexec/zendiloos/zendiloos-agent pin-project teslamate\n"));
        assert!(unit.contains("ExecStop=/usr/libexec/zendiloos/zendiloos-agent unpin-project teslamate\n"));
        assert_eq!(Owner::Project(&project()).unit_name(), "zendiloos-project-teslamate-shares.service");
    }

    #[test]
    fn pins_are_read_from_the_kept_file() {
        let none = json!({ "services": {}, "x-zendiloos": { "app_data": "/volume1/@apps/x", "shares": [] } });
        assert!(pins_of(&none).unwrap().is_empty());
        let pinned = json!({ "x-zendiloos": { "pins": [
            { "share": "/volume1/media", "folder": "Films", "at": "/run/zendiloos-app-shares/jellyfin/media" },
            { "share": "/volume1/docker", "folder": "app/nginx.conf", "at": "/run/x/app/2", "file": true },
        ] } });
        let pins = pins_of(&pinned).unwrap();
        assert_eq!(pins[0].folder.as_str(), "Films");
        assert!(!pins[0].file && !pins[0].create);
        assert!(pins[1].file);
        let odd = json!({ "x-zendiloos": { "pins": [{ "share": "/volume1/media", "folder": "../etc", "at": "/x" }] } });
        assert!(pins_of(&odd).is_err());
        assert_eq!(
            serde_json::to_value(&pins[0]).unwrap(),
            json!({ "share": "/volume1/media", "folder": "Films", "at": "/run/zendiloos-app-shares/jellyfin/media" }),
            "apps' kept files stay as they were"
        );
    }

    /// A folder of the test's own in `/dev/shm`, a mounted file system's
    /// root below a real folder on every Linux: what a share is to the
    /// agent. Removed when dropped.
    struct Shm(PathBuf);

    impl Shm {
        fn new(tag: &str) -> Self {
            let path = Path::new("/dev/shm").join(format!("zendiloos-shares-{}-{tag}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// The folder's name, as a folder of the "share" `/dev/shm`.
        fn folder(&self, below: &str) -> AppFolder {
            let name = self.0.file_name().unwrap().to_str().unwrap();
            AppFolder::parse(&if below.is_empty() { name.to_owned() } else { format!("{name}/{below}") })
                .unwrap()
        }
    }

    impl Drop for Shm {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_mounted_share_is_walked_and_its_folders_found() {
        let shm = Shm::new("walk");
        fs::create_dir_all(shm.0.join("Films")).unwrap();
        fs::write(shm.0.join("notes.txt"), "").unwrap();
        std::os::unix::fs::symlink("/etc", shm.0.join("escape")).unwrap();
        let share = Path::new("/dev/shm");

        assert!(walk(share, &shm.folder("Films")).is_ok());
        assert!(walk(share, &shm.folder("escape")).is_err(), "never through a link");
        assert_eq!(find(share, &shm.folder("Films")).unwrap(), Found::Folder);
        assert_eq!(find(share, &shm.folder("notes.txt")).unwrap(), Found::File);
        assert_eq!(find(share, &shm.folder("missing/deeper")).unwrap(), Found::Missing);
        assert!(find(share, &shm.folder("escape")).is_err());
    }

    #[test]
    fn a_pin_is_the_very_folder_the_walk_reaches() {
        let shm = Shm::new("pinned");
        fs::create_dir_all(shm.0.join("Films")).unwrap();
        let share = Path::new("/dev/shm");
        // `/dev/shm` itself stands for a pin of the share's root.
        assert!(is_pinned(share, share, &AppFolder::whole()).is_ok());
        let other = is_pinned(share, share, &shm.folder("Films")).unwrap_err();
        assert!(other.to_string().contains("is not the folder"), "{other}");
        let unmounted = is_pinned(&shm.0, share, &AppFolder::whole()).unwrap_err();
        assert!(unmounted.to_string().contains("is not mounted"), "{unmounted}");
    }

    /// systemctl as a script recording its calls; it fails while `fail`
    /// is there.
    fn systemctl(dir: &Path) -> Program {
        let path = dir.join("systemctl");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\n[ -f {} ] && {{ echo 'Unit not found.' >&2; exit 5; }}\nexit 0\n",
                dir.join("calls.log").display(),
                dir.join("fail").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    #[tokio::test]
    async fn the_pins_unit_is_placed_enabled_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let program = systemctl(dir.path());
        let paths = HostPaths { units: dir.path().to_owned(), ..HostPaths::default() };
        let owner = Owner::App(&app());
        let pin = Pin {
            share: "/volume1/media".into(),
            folder: AppFolder::parse("Films").unwrap(),
            at: "/run/zendiloos-app-shares/jellyfin/media".into(),
            file: false,
            create: false,
        };
        let unit = dir.path().join(owner.unit_name());

        place_unit(&program, &paths, owner, &[pin.clone(), pin]).await.unwrap();
        let text = fs::read_to_string(&unit).unwrap();
        assert!(text.contains("RequiresMountsFor=/volume1/media\n"), "each share once: {text}");
        repin(&program, owner).await.unwrap();
        // No pins any more: the unit goes.
        place_unit(&program, &paths, owner, &[]).await.unwrap();
        assert!(!unit.exists());
        remove_unit(&program, &paths, owner).await.unwrap();
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert_eq!(
            calls,
            "daemon-reload\nenable zendiloos-app-jellyfin-shares.service\n\
             restart zendiloos-app-jellyfin-shares.service\n\
             disable --now zendiloos-app-jellyfin-shares.service\ndaemon-reload\n",
            "nothing to remove the second time"
        );

        fs::write(dir.path().join("fail"), "").unwrap();
        let failed = repin(&program, owner).await.unwrap_err();
        assert!(failed.to_string().contains("Unit not found."), "{failed}");
    }

    #[test]
    fn unpinning_clears_the_places_and_pinning_wants_roots_folders() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HostPaths {
            apps: dir.path().join("apps"),
            app_shares: dir.path().join("app-shares"),
            project_shares: dir.path().join("project-shares"),
            ..HostPaths::default()
        };
        let places = dir.path().join("app-shares/jellyfin");
        fs::create_dir_all(places.join("media")).unwrap();
        fs::write(places.join("nginx.conf"), "").unwrap();

        unpin_shares(&paths, &app()).unwrap();
        assert!(!places.exists(), "every place goes");
        unpin_shares(&paths, &app()).unwrap();
        unpin_project(&paths, &project()).unwrap();

        // Nothing kept: nothing to pin.
        assert!(pin_shares(&paths, &app()).unwrap_err().contains("cannot read"));
        fs::create_dir_all(&paths.apps).unwrap();
        fs::write(paths.apps.join("jellyfin.json"), json!({ "services": {} }).to_string()).unwrap();
        let theirs = pin_shares(&paths, &app());
        if rustix::process::geteuid().is_root() {
            assert!(theirs.is_ok(), "{theirs:?}");
        } else {
            // The places' folders are the test's, not root's.
            assert!(theirs.unwrap_err().contains("others than root"));
        }
        fs::write(paths.apps.join("jellyfin.json"), "not json").unwrap();
        assert!(pin_shares(&paths, &app()).unwrap_err().contains("cannot be read"));
        assert!(pin_project(&paths, &project()).is_err(), "no kept project");
    }
}
