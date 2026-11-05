//! File operations beneath the root of a share, with the credentials of the
//! calling thread. The files agent calls them on a thread that took on a
//! user's credentials, so the file system's permissions decide, exactly as
//! for SMB and NFS; the simulator calls them in a scratch directory.
//!
//! Paths from requests are resolved with `openat2` beneath the share's root
//! (`RESOLVE_BENEATH | RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS`): neither
//! `..` nor a symbolic link leads out of the share, and no file system
//! mounted inside it is entered. Deeper work (copying and removing trees)
//! goes name by name from directory descriptors and never follows a
//! symbolic link.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

use rustix::fs::Timespec;
use rustix::fs::{
    Access, AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, ResolveFlags, Stat, Timestamps, accessat,
    fstat, futimens, mkdirat, open, openat, openat2, readlinkat, renameat_with, statat, symlinkat, unlinkat,
};
use rustix::io::Errno;
use rustix::rand::{GetRandomFlags, getrandom};
use zendiloos_agent_proto::{
    DirectoryEntry, EntryKind, MAX_DIRECTORY_ENTRIES, ShareName, SharePath, SnapshotLabel, VolumeNumber,
    is_file_name,
};

mod archive;

pub use archive::{ArchivePlan, MAX_ARCHIVE_ENTRIES};

/// The recycle bin at a share's root: `.recycle/<user>` per user, as
/// Samba's `vfs_recycle` keeps it.
pub const RECYCLE_DIRECTORY: &str = ".recycle";
/// Uploads are written to hidden files with this prefix, then renamed.
pub const UPLOAD_PREFIX: &str = ".zendiloos-upload-";
/// Deepest directory nesting copied or removed.
const MAX_DEPTH: u32 = 256;
/// Most numbered names tried for a copy or a recycled entry.
const MAX_NUMBERED: u32 = 1000;
const RESOLVE: ResolveFlags =
    ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV).union(ResolveFlags::NO_MAGICLINKS);

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("no such file or directory")]
    NotFound,
    #[error("permission denied")]
    Denied,
    #[error("the name is taken")]
    Exists,
    #[error("{0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<Errno> for FileError {
    fn from(errno: Errno) -> Self {
        match errno {
            Errno::NOENT => FileError::NotFound,
            Errno::ACCESS | Errno::PERM => FileError::Denied,
            Errno::EXIST | Errno::NOTEMPTY => FileError::Exists,
            Errno::NOTDIR => FileError::Invalid("not a folder"),
            Errno::ISDIR => FileError::Invalid("a folder"),
            other => FileError::Io(other.into()),
        }
    }
}

/// A name taken inside a copy is not a reason to pick another top-level name.
fn nested(error: FileError) -> FileError {
    match error {
        FileError::Exists => FileError::Io(std::io::Error::other("a name in the copy was taken meanwhile")),
        other => other,
    }
}

/// A directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
    /// The caller may create and remove entries in it.
    pub writable: bool,
}

// Stat field types differ between architectures.
#[allow(clippy::useless_conversion)]
fn device(stat: &Stat) -> u64 {
    u64::from(stat.st_dev)
}

#[allow(clippy::useless_conversion)]
fn inode(stat: &Stat) -> u64 {
    u64::from(stat.st_ino)
}

fn kind(stat: &Stat) -> FileType {
    FileType::from_raw_mode(stat.st_mode)
}

#[allow(clippy::useless_conversion)]
fn timestamps(stat: &Stat) -> Timestamps {
    Timestamps {
        last_access: Timespec {
            tv_sec: i64::from(stat.st_atime),
            tv_nsec: i64::try_from(stat.st_atime_nsec).unwrap_or(0),
        },
        last_modification: Timespec {
            tv_sec: i64::from(stat.st_mtime),
            tv_nsec: i64::try_from(stat.st_mtime_nsec).unwrap_or(0),
        },
    }
}

#[allow(clippy::useless_conversion)]
fn entry(name: &[u8], stat: &Stat) -> DirectoryEntry {
    DirectoryEntry {
        name: String::from_utf8_lossy(name).into_owned(),
        kind: match kind(stat) {
            FileType::RegularFile => EntryKind::File,
            FileType::Directory => EntryKind::Directory,
            FileType::Symlink => EntryKind::Symlink,
            _ => EntryKind::Other,
        },
        size: u64::try_from(stat.st_size).unwrap_or(0),
        modified: i64::from(stat.st_mtime),
    }
}

fn c_name(name: &str) -> Result<CString, FileError> {
    CString::new(name).map_err(|_| FileError::Invalid("not a valid name"))
}

/// `name (2).ext` for `name.ext`.
fn numbered(name: &str, number: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({number}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({number})"),
    }
}

/// A name in the recycle bin without the `Copy #N of ` it got there.
fn original_name(name: &str) -> &str {
    name.strip_prefix("Copy #")
        .and_then(|rest| rest.split_once(" of "))
        .filter(|(number, original)| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) && !original.is_empty()
        })
        .map_or(name, |(_, original)| original)
}

/// Names in a directory opened for reading, without `.` and `..`.
fn names(directory: impl AsFd) -> Result<Vec<CString>, FileError> {
    let mut names = Vec::new();
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn open_beneath_flags(base: BorrowedFd<'_>, name: &CStr, flags: OFlags) -> Result<OwnedFd, FileError> {
    Ok(openat(base, name, flags | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?)
}

/// Copies `name` from `source` to a new `target_name` in `target`, which
/// must not exist yet. On failure, what was created is removed again.
fn copy_new(
    source: BorrowedFd<'_>,
    name: &CStr,
    target: BorrowedFd<'_>,
    target_name: &CStr,
    file_system: u64,
    depth: u32,
) -> Result<(), FileError> {
    let stat = statat(source, name, AtFlags::SYMLINK_NOFOLLOW)?;
    match kind(&stat) {
        FileType::Directory => {
            if device(&stat) != file_system {
                return Ok(()); // a file system mounted inside: not entered
            }
            if depth >= MAX_DEPTH {
                return Err(FileError::Invalid("folders are nested too deeply"));
            }
            let from = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            mkdirat(target, target_name, Mode::from_raw_mode(0o777))?;
            let filled = (|| {
                let to = open_beneath_flags(target, target_name, OFlags::PATH | OFlags::DIRECTORY)?;
                for child in names(&from)? {
                    copy_new(from.as_fd(), &child, to.as_fd(), &child, file_system, depth + 1)
                        .map_err(nested)?;
                }
                Ok(())
            })();
            if let Err(error) = filled {
                let _ = remove_tree(target, target_name, file_system, 0);
                return Err(nested(error));
            }
        }
        FileType::RegularFile => {
            let from = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::NONBLOCK)?;
            let to = openat(
                target,
                target_name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o666),
            )?;
            let copied = (|| {
                let (mut from, mut to) = (File::from(from), File::from(to));
                // copy_file_range: on ZFS, blocks are cloned instead of copied.
                std::io::copy(&mut from, &mut to)?;
                futimens(&to, &timestamps(&stat))?;
                Ok::<_, FileError>(())
            })();
            if let Err(error) = copied {
                let _ = unlinkat(target, target_name, AtFlags::empty());
                return Err(nested(error));
            }
        }
        FileType::Symlink => {
            let link = readlinkat(source, name, Vec::new())?;
            symlinkat(link.as_c_str(), target, target_name)?;
        }
        // Devices, pipes and sockets are not copied.
        _ => {}
    }
    Ok(())
}

/// Removes `name` from `directory`, with everything in it.
fn remove_tree(
    directory: BorrowedFd<'_>,
    name: &CStr,
    file_system: u64,
    depth: u32,
) -> Result<(), FileError> {
    let stat = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) if depth > 0 => return Ok(()),
        other => other?,
    };
    if kind(&stat) == FileType::Directory {
        if device(&stat) != file_system {
            return Err(FileError::Invalid("a file system is mounted inside"));
        }
        if depth >= MAX_DEPTH {
            return Err(FileError::Invalid("folders are nested too deeply"));
        }
        let inner = open_beneath_flags(directory, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
        // All names first: removing while reading can skip entries.
        for child in names(&inner)? {
            remove_tree(inner.as_fd(), &child, file_system, depth + 1)?;
        }
        unlinkat(directory, name, AtFlags::REMOVEDIR)?;
    } else {
        unlinkat(directory, name, AtFlags::empty())?;
    }
    Ok(())
}

/// Opens directory `name` in `directory`, creating it with `mode` first if
/// it does not exist.
fn enter(directory: &OwnedFd, name: &str, mode: u32) -> Result<OwnedFd, FileError> {
    match mkdirat(directory, name, Mode::from_raw_mode(mode)) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    open_beneath_flags(directory.as_fd(), &c_name(name)?, OFlags::PATH | OFlags::DIRECTORY)
}

/// The root directory of one share.
#[derive(Debug)]
pub struct ShareRoot {
    directory: OwnedFd,
    file_system: u64,
    inode: u64,
}

impl ShareRoot {
    /// `<volumes>/volume<N>/<share>`. With `require_mount`, only if a file
    /// system of its own is mounted there: otherwise files would land in
    /// the volume's directory underneath.
    pub fn open(
        volumes: &Path,
        volume: VolumeNumber,
        share: &ShareName,
        require_mount: bool,
    ) -> Result<Self, FileError> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let base = open(volumes, flags, Mode::empty())?;
        let volume_directory = openat(&base, format!("volume{}", volume.get()), flags, Mode::empty())?;
        let directory = openat(&volume_directory, share.as_str(), flags, Mode::empty())?;
        let stat = fstat(&directory)?;
        if require_mount && device(&stat) == device(&fstat(&volume_directory)?) {
            return Err(FileError::Invalid("the share is not mounted"));
        }
        Ok(Self { directory, file_system: device(&stat), inode: inode(&stat) })
    }

    /// The snapshot `label` of a mounted share, as a root of its own: ZFS
    /// mounts it when `.zfs/snapshot/<label>` is entered (behind
    /// `RESOLVE_NO_XDEV` from the share), and it is read only.
    pub fn open_snapshot(
        volumes: &Path,
        volume: VolumeNumber,
        share: &ShareName,
        label: &SnapshotLabel,
    ) -> Result<Self, FileError> {
        let live = Self::open(volumes, volume, share, true)?;
        let path = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let zfs = openat(&live.directory, ".zfs", path, Mode::empty())?;
        let snapshots = openat(&zfs, "snapshot", path, Mode::empty())?;
        // Opened for reading, which mounts the snapshot; a path-only open
        // would stay on the empty directory beneath.
        let reading = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let directory = openat(&snapshots, label.as_str(), reading, Mode::empty())?;
        let stat = fstat(&directory)?;
        if device(&stat) == live.file_system {
            return Err(FileError::Invalid("the snapshot is not mounted"));
        }
        Ok(Self { directory, file_system: device(&stat), inode: inode(&stat) })
    }

    fn same_as(&self, other: &ShareRoot) -> bool {
        self.file_system == other.file_system && self.inode == other.inode
    }

    fn resolve(&self, path: &SharePath, flags: OFlags) -> Result<OwnedFd, FileError> {
        let relative = if path.is_root() { "." } else { path.as_str() };
        Ok(openat2(&self.directory, relative, flags | OFlags::CLOEXEC, Mode::empty(), RESOLVE)?)
    }

    /// A directory, as a base for `*at` calls.
    fn directory(&self, path: &SharePath) -> Result<OwnedFd, FileError> {
        self.resolve(path, OFlags::PATH | OFlags::DIRECTORY)
    }

    /// The directory containing `path`, and the last name of `path`.
    fn parent<'p>(&self, path: &'p SharePath) -> Result<(OwnedFd, &'p str), FileError> {
        let name = path.file_name().ok_or(FileError::Invalid("not possible with the share itself"))?;
        Ok((self.directory(&path.parent().unwrap_or_default())?, name))
    }

    pub fn list(&self, path: &SharePath) -> Result<Listing, FileError> {
        let directory = self.resolve(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for item in Dir::read_from(&directory)? {
            let item = item?;
            let name = item.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            if entries.len() == MAX_DIRECTORY_ENTRIES {
                truncated = true;
                break;
            }
            match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => entries.push(entry(name.to_bytes(), &stat)),
                Err(Errno::NOENT) => {} // removed meanwhile
                Err(error) => return Err(error.into()),
            }
        }
        let writable =
            accessat(&directory, ".", Access::WRITE_OK | Access::EXEC_OK, AtFlags::empty()).is_ok();
        Ok(Listing { entries, truncated, writable })
    }

    /// A regular file, opened for reading, and its size.
    pub fn open_file(&self, path: &SharePath) -> Result<(OwnedFd, u64), FileError> {
        // O_NONBLOCK keeps a pipe from blocking the open; it means nothing
        // for regular files.
        let file = self.resolve(path, OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY)?;
        let stat = fstat(&file)?;
        if kind(&stat) != FileType::RegularFile {
            return Err(FileError::Invalid("not a file"));
        }
        Ok((file, u64::try_from(stat.st_size).unwrap_or(0)))
    }

    /// A new, empty hidden file in `directory`, opened for writing, and its name.
    pub fn create_upload(&self, directory: &SharePath) -> Result<(OwnedFd, String), FileError> {
        let parent = self.directory(directory)?;
        for _ in 0..8 {
            let mut random = [0; 8];
            getrandom(&mut random, GetRandomFlags::empty())?;
            let name = format!("{UPLOAD_PREFIX}{:016x}", u64::from_le_bytes(random));
            let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match openat(&parent, name.as_str(), flags, Mode::from_raw_mode(0o666)) {
                Ok(file) => return Ok((file, name)),
                Err(Errno::EXIST) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(FileError::Exists)
    }

    pub fn make_directory(&self, path: &SharePath) -> Result<(), FileError> {
        let (parent, name) = self.parent(path)?;
        Ok(mkdirat(&parent, name, Mode::from_raw_mode(0o777))?)
    }

    /// Renames or moves `from` to `to` in `target`, which may be another
    /// share: then by copying and removing. With `replace`, a file replaces
    /// a file; a folder is never replaced.
    pub fn move_entry(
        &self,
        from: &SharePath,
        target: &ShareRoot,
        to: &SharePath,
        replace: bool,
    ) -> Result<(), FileError> {
        if self.same_as(target) && to.parent().unwrap_or_default().starts_with(from) {
            return Err(FileError::Invalid("a folder cannot be moved into itself"));
        }
        let (from_parent, from_name) = self.parent(from)?;
        let (to_parent, to_name) = target.parent(to)?;
        if replace {
            let source = statat(&from_parent, from_name, AtFlags::SYMLINK_NOFOLLOW)?;
            match statat(&to_parent, to_name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(existing)
                    if kind(&existing) == FileType::Directory || kind(&source) == FileType::Directory =>
                {
                    return Err(FileError::Exists);
                }
                Ok(_) | Err(Errno::NOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let flags = if replace { RenameFlags::empty() } else { RenameFlags::NOREPLACE };
        match renameat_with(&from_parent, from_name, &to_parent, to_name, flags) {
            // Another share is another file system.
            Err(Errno::XDEV) => {
                let (from_name, to_name) = (c_name(from_name)?, c_name(to_name)?);
                copy_new(from_parent.as_fd(), &from_name, to_parent.as_fd(), &to_name, self.file_system, 0)?;
                remove_tree(from_parent.as_fd(), &from_name, self.file_system, 0)
            }
            other => Ok(other?),
        }
    }

    /// Copies `from` to `to` in `target`; if `to` exists, to the first free
    /// `name (N).ext` next to it. Returns where the copy went.
    pub fn copy_entry(
        &self,
        from: &SharePath,
        target: &ShareRoot,
        to: &SharePath,
    ) -> Result<SharePath, FileError> {
        if self.same_as(target) && to.parent().unwrap_or_default().starts_with(from) {
            return Err(FileError::Invalid("a folder cannot be copied into itself"));
        }
        let (from_parent, from_name) = self.parent(from)?;
        let (to_parent, to_name) = target.parent(to)?;
        let from_name = c_name(from_name)?;
        let directory = to.parent().unwrap_or_default();
        for number in 1..=MAX_NUMBERED {
            let name = if number == 1 { to_name.to_owned() } else { numbered(to_name, number) };
            if !is_file_name(&name) {
                return Err(FileError::Invalid("the name is too long"));
            }
            match copy_new(
                from_parent.as_fd(),
                &from_name,
                to_parent.as_fd(),
                &c_name(&name)?,
                self.file_system,
                0,
            ) {
                Ok(()) => {
                    return directory.join(&name).map_err(|_| FileError::Invalid("the path is too long"));
                }
                Err(FileError::Exists) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(FileError::Exists)
    }

    /// Moves `path` into `user`'s recycle bin, or deletes it for good if
    /// `user` is none or `path` is in the recycle bin already.
    pub fn remove(&self, path: &SharePath, recycle_for: Option<&str>) -> Result<(), FileError> {
        let in_recycle_bin = path.names().next() == Some(RECYCLE_DIRECTORY);
        match recycle_for {
            Some(user) if !in_recycle_bin => self.recycle(path, user),
            _ => {
                let (parent, name) = self.parent(path)?;
                remove_tree(parent.as_fd(), &c_name(name)?, self.file_system, 0)
            }
        }
    }

    /// Moves an entry out of `user`'s recycle bin back to where it was
    /// deleted from: `.recycle/<user>/<folders>/<name>` to `<folders>/<name>`,
    /// without the `Copy #N of ` a second deletion of a name got there.
    /// Folders on the way that are gone are made again; if the name is
    /// taken, the first free `name (N).ext`. Returns where it went.
    pub fn restore(&self, path: &SharePath, user: &str) -> Result<SharePath, FileError> {
        let mut names = path.names();
        if names.next() != Some(RECYCLE_DIRECTORY) || names.next() != Some(user) {
            return Err(FileError::Invalid("only what is in your own recycle bin can be restored"));
        }
        let rest: Vec<&str> = names.collect();
        let Some((name, folders)) = rest.split_last() else {
            return Err(FileError::Invalid("the recycle bin itself cannot be restored"));
        };
        let too_long = |_| FileError::Invalid("the path is too long");
        let (from_parent, from_name) = self.parent(path)?;
        let mut directory = self.directory(&SharePath::root())?;
        let mut target = SharePath::root();
        for folder in folders {
            directory = enter(&directory, folder, 0o777)?;
            target = target.join(folder).map_err(too_long)?;
        }
        let original = original_name(name);
        for number in 1..=MAX_NUMBERED {
            let candidate = if number == 1 { original.to_owned() } else { numbered(original, number) };
            if !is_file_name(&candidate) {
                return Err(FileError::Invalid("the name is too long"));
            }
            match renameat_with(
                &from_parent,
                from_name,
                &directory,
                candidate.as_str(),
                RenameFlags::NOREPLACE,
            ) {
                Err(Errno::EXIST) => continue,
                Err(error) => return Err(error.into()),
                Ok(()) => return target.join(&candidate).map_err(too_long),
            }
        }
        Err(FileError::Exists)
    }

    /// As Samba's `vfs_recycle` does with `keeptree` and `versions`:
    /// `.recycle/<user>/<folders>/<name>`, or `Copy #N of <name>` there if
    /// taken. The user's directory is private to them.
    fn recycle(&self, path: &SharePath, user: &str) -> Result<(), FileError> {
        if !is_file_name(user) {
            return Err(FileError::Invalid("not a valid user name"));
        }
        let (parent, name) = self.parent(path)?;
        // The agent creates `.recycle` with the share's permissions; if it is
        // missing, it inherits them from the share's root.
        let mut bin = enter(&self.directory(&SharePath::root())?, RECYCLE_DIRECTORY, 0o777)?;
        bin = enter(&bin, user, 0o700)?;
        for folder in path.parent().unwrap_or_default().names() {
            bin = enter(&bin, folder, 0o700)?;
        }
        for version in 0..MAX_NUMBERED {
            let target = if version == 0 { name.to_owned() } else { format!("Copy #{version} of {name}") };
            if !is_file_name(&target) {
                return Err(FileError::Invalid("the name is too long"));
            }
            match renameat_with(&parent, name, &bin, target.as_str(), RenameFlags::NOREPLACE) {
                Err(Errno::EXIST) => continue,
                other => return Ok(other?),
            }
        }
        Err(FileError::Exists)
    }
}

#[cfg(test)]
mod tests;
