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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rustix::fs::Timespec;
use rustix::fs::{
    Access, AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, ResolveFlags, Stat, StatxFlags, Timestamps,
    accessat, fstat, futimens, mkdirat, open, openat, openat2, readlinkat, renameat_with, statat, statx,
    symlinkat, unlinkat,
};
use rustix::io::Errno;
use rustix::rand::{GetRandomFlags, getrandom};
use zendiloos_agent_proto::{
    CountStop, DirectoryEntry, EntryKind, FoundEntry, HOMES_DIRECTORY, MAX_COUNTED, MAX_DIRECTORY_ENTRIES,
    MAX_SEARCH_RESULTS, RemovableId, SearchStop, SearchText, ShareName, SharePath, SnapshotLabel, UserName,
    VolumeNumber, is_file_name,
};

mod archive;
mod copied;
mod stream;

pub use archive::{ArchivePlan, MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_NAME_BYTES};
pub use copied::{Copied, Kept, MAX_MOVED_ENTRIES, MAX_MOVED_NAME_BYTES};
pub use stream::{PIECES_IN_FLIGHT, Piece, Taken, copy_through, settle};

pub use zendiloos_agent_proto::RECYCLE_DIRECTORY;
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
    /// Two names of the source are one name to the target's file system
    /// (FAT, exFAT, NTFS), which does not tell upper from lower case.
    #[error("“{0}” and “{1}” differ only in case, which the target does not tell apart")]
    CaseClash(String, String),
    /// A move across file systems left part of its source; it is in the
    /// target too, as far as it was copied.
    #[error("moved in part: {0}")]
    PartlyMoved(Kept),
    /// The copy could not be written out to its file system: a move keeps
    /// its source.
    #[error("the copy could not be written out ({0}); nothing was removed")]
    NotDurable(#[source] std::io::Error),
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

/// Why `name` could not be made in `folder`, a folder of a copy that was
/// empty when it was made: another entry of the source of a name that
/// differs only in case, where the file system does not tell such names
/// apart (FAT, exFAT, NTFS); otherwise an entry somebody else made there
/// meanwhile. Reads the folder once, for the message.
fn clash(folder: BorrowedFd<'_>, name: &CStr) -> FileError {
    let wanted = String::from_utf8_lossy(name.to_bytes());
    let folded = wanted.to_lowercase();
    let mut alike = None;
    for entry in Dir::read_from(folder).into_iter().flatten().flatten() {
        let other = entry.file_name();
        if other.to_bytes() == name.to_bytes() {
            return nested(FileError::Exists);
        }
        let other = String::from_utf8_lossy(other.to_bytes());
        if alike.is_none() && other.to_lowercase() == folded {
            alike = Some(other.into_owned());
        }
    }
    match alike {
        Some(other) => FileError::CaseClash(other, wanted.into_owned()),
        None => nested(FileError::Exists),
    }
}

/// Whether `name` is an upload under way, which copies leave alone: it is
/// half written, and only where it began is a leftover ever removed.
fn upload_under_way(name: &CStr) -> bool {
    name.to_bytes().starts_with(UPLOAD_PREFIX.as_bytes())
}

/// What a search found, and why it ended early, if it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub entries: Vec<FoundEntry>,
    pub stopped: Option<SearchStop>,
}

/// When a search ends early: at `until`, or once `stop` is set (its client
/// hung up).
#[derive(Debug, Clone, Copy)]
pub struct SearchBounds<'a> {
    pub until: Instant,
    pub stop: &'a AtomicBool,
}

/// Entries looked at between two looks at the clock and the stop flag.
const SEARCH_CHECK_EVERY: usize = 64;

/// What a file or folder is, and for a folder what is in it, counted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub folder: bool,
    /// A file's size; for a folder, what the files counted in it hold.
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    /// Entries whose name starts with `.`: counted, not looked into.
    pub hidden: u64,
    pub stopped: Option<CountStop>,
    /// Unix seconds, where the file system keeps the time it was made.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub uid: u32,
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
        unreadable: std::str::from_utf8(name).is_err(),
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

/// Names read at once while a folder is emptied: removing while reading
/// can skip entries, and a folder of millions must not be held whole.
const BATCH: usize = 4096;

/// Up to `limit` names in a directory opened for reading, from its start,
/// without `.` and `..`.
fn some_names(directory: impl AsFd, limit: usize) -> Result<Vec<CString>, FileError> {
    let mut names = Vec::new();
    for entry in Dir::read_from(directory)? {
        if names.len() == limit {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// What a file or folder is, whatever its name is by now.
fn identity(stat: &Stat) -> (u64, u64) {
    (device(stat), inode(stat))
}

fn open_beneath_flags(base: BorrowedFd<'_>, name: &CStr, flags: OFlags) -> Result<OwnedFd, FileError> {
    Ok(openat(base, name, flags | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?)
}

/// A symbolic link `name` in `directory`, opened as itself, its facts and
/// its target: all of the one link, whatever has the name by now. None if
/// it is no link (any more).
fn open_link(directory: BorrowedFd<'_>, name: &CStr) -> Result<Option<(Stat, CString)>, FileError> {
    let link = open_beneath_flags(directory, name, OFlags::PATH)?;
    let stat = fstat(&link)?;
    if kind(&stat) != FileType::Symlink {
        return Ok(None);
    }
    Ok(Some((stat, readlinkat(&link, c"", Vec::new())?)))
}

/// Copies `name` from `source` to a new `target_name` in `target`, which
/// must not exist yet, and records in `copied` what it copied, each entry
/// as the descriptor it was read through says it was before it was read.
/// On failure, what was created is removed again. Uploads under way are
/// left out (the top refused), and so are devices, pipes, sockets and
/// file systems mounted inside.
fn copy_new(
    source: BorrowedFd<'_>,
    name: &CStr,
    target: BorrowedFd<'_>,
    target_name: &CStr,
    file_system: u64,
    depth: u32,
    copied: &mut Copied,
) -> Result<(), FileError> {
    if upload_under_way(name) {
        return if depth == 0 {
            Err(FileError::Invalid("an upload under way is not copied"))
        } else {
            Ok(())
        };
    }
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
            let opened = fstat(&from)?;
            if device(&opened) != file_system {
                return Ok(()); // mounted on meanwhile
            }
            mkdirat(target, target_name, Mode::from_raw_mode(0o777))?;
            // Whoever may write the target could put a folder of their own
            // (or of the user's) in the new one's place: only an empty
            // folder is filled, and only the folder opened here is removed
            // again after a failure, never whatever has the name by then.
            let to = open_beneath_flags(target, target_name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            let made = fstat(&to)?;
            if !some_names(&to, 1)?.is_empty() {
                return Err(nested(FileError::Exists));
            }
            let folder = copied.folder(name, &opened);
            let filled = (|| {
                // Read as it goes: what is copied lands in another folder.
                for entry in Dir::read_from(&from)? {
                    let entry = entry?;
                    let child = entry.file_name();
                    if child == c"." || child == c".." {
                        continue;
                    }
                    match copy_new(from.as_fd(), child, to.as_fd(), child, file_system, depth + 1, copied) {
                        Ok(()) => {}
                        Err(FileError::Exists) => return Err(clash(to.as_fd(), child)),
                        Err(error) => return Err(nested(error)),
                    }
                }
                Ok(())
            })();
            copied.leave(folder);
            if let Err(error) = filled {
                // A copy into another share lies on another file system.
                let _ = remove_entry(target, target_name, device(&made), 0, Some(identity(&made)));
                return Err(nested(error));
            }
        }
        FileType::RegularFile => {
            let from = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::NONBLOCK)?;
            // Before reading: whatever changes it from now on, the move sees.
            let opened = fstat(&from)?;
            if kind(&opened) != FileType::RegularFile {
                return Ok(()); // something else in its place meanwhile
            }
            let to = openat(
                target,
                target_name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o666),
            )?;
            let made = fstat(&to)?;
            let written = (|| {
                let (mut from, mut to) = (File::from(from), File::from(to));
                // copy_file_range: on ZFS, blocks are cloned instead of copied.
                std::io::copy(&mut from, &mut to)?;
                futimens(&to, &timestamps(&opened))?;
                Ok::<_, FileError>(())
            })();
            if let Err(error) = written {
                let _ = remove_entry(target, target_name, device(&made), 0, Some(identity(&made)));
                return Err(nested(error));
            }
            copied.file(name, &opened);
        }
        FileType::Symlink => {
            let Some((opened, link)) = open_link(source, name)? else { return Ok(()) };
            symlinkat(link.as_c_str(), target, target_name)?;
            copied.link(name, &opened);
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
    remove_entry(directory, name, file_system, depth, None)
}

/// Removes `name` from `directory`, with everything in it; with
/// `expected`, only if it still is the file or folder of that identity
/// (device and inode). Whoever may write `directory` can swap names: what
/// is emptied is the folder opened and checked, and only its emptied self
/// goes by name at the end, which removes nothing but an empty folder.
fn remove_entry(
    directory: BorrowedFd<'_>,
    name: &CStr,
    file_system: u64,
    depth: u32,
    expected: Option<(u64, u64)>,
) -> Result<(), FileError> {
    let replaced = || FileError::Invalid("it was replaced meanwhile; nothing else was removed");
    let stat = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) if depth > 0 => return Ok(()),
        other => other?,
    };
    if expected.is_some_and(|expected| expected != identity(&stat)) {
        return Err(replaced());
    }
    if kind(&stat) == FileType::Directory {
        if device(&stat) != file_system {
            return Err(FileError::Invalid("a file system is mounted inside"));
        }
        if depth >= MAX_DEPTH {
            return Err(FileError::Invalid("folders are nested too deeply"));
        }
        let inner = open_beneath_flags(directory, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let opened = identity(&fstat(&inner)?);
        if expected.is_some_and(|expected| expected != opened) {
            return Err(replaced());
        }
        // In batches, from the start each time: removing while reading can
        // skip entries.
        loop {
            let batch = some_names(&inner, BATCH)?;
            if batch.is_empty() {
                break;
            }
            for child in batch {
                remove_entry(inner.as_fd(), &child, file_system, depth + 1, None)?;
            }
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

    /// The personal folder of `user`, `<volumes>/volume<N>/@homes/<user>`,
    /// as a root: only with the homes' dataset mounted at `@homes`, and only
    /// a folder in that dataset itself.
    pub fn open_home(volumes: &Path, volume: VolumeNumber, user: &UserName) -> Result<Self, FileError> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let base = open(volumes, flags, Mode::empty())?;
        let volume_directory = openat(&base, format!("volume{}", volume.get()), flags, Mode::empty())?;
        let homes = openat(&volume_directory, HOMES_DIRECTORY, flags, Mode::empty())?;
        let homes_device = device(&fstat(&homes)?);
        if homes_device == device(&fstat(&volume_directory)?) {
            return Err(FileError::Invalid("the personal folders' dataset is not mounted"));
        }
        let directory = openat(&homes, user.as_str(), flags, Mode::empty())?;
        let stat = fstat(&directory)?;
        if device(&stat) != homes_device {
            return Err(FileError::Invalid("the personal folder is not a folder of the homes' dataset"));
        }
        Ok(Self { directory, file_system: device(&stat), inode: inode(&stat) })
    }

    /// A removable drive's root, `<places>/<id>`: with `require_mount`, only
    /// with the drive's file system mounted there, never the empty folder
    /// beneath it (on the tmpfs of `/run`).
    pub fn open_removable(places: &Path, id: &RemovableId, require_mount: bool) -> Result<Self, FileError> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let base = open(places, flags, Mode::empty())?;
        let directory = openat(&base, id.as_str(), flags, Mode::empty())?;
        let stat = fstat(&directory)?;
        if require_mount && device(&stat) == device(&fstat(&base)?) {
            return Err(FileError::Invalid("the drive is not mounted"));
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

    /// Looks through the folder `path` and everything below it for names
    /// containing `text`, whatever the case, as the calling thread may: a
    /// folder it may not read is passed by. Walks by descriptor, name by
    /// name, like the other walks: no link is followed, no other file system
    /// entered, nothing deeper than 256 folders looked into. Names starting
    /// with `.` are neither found nor entered unless `hidden`; the recycle
    /// bins only when searched from inside one, and uploads under way never.
    /// A name that is not UTF-8 is never found, nor what is in it: it cannot
    /// be told from a name that looks alike. At most
    /// [`MAX_SEARCH_RESULTS`], ending early at `bounds`.
    pub fn search(
        &self,
        path: &SharePath,
        text: &SearchText,
        hidden: bool,
        bounds: SearchBounds<'_>,
    ) -> Result<Found, FileError> {
        struct Level {
            folder: Dir,
            path: SharePath,
            depth: u32,
        }
        let needle = text.needle();
        let start = self.resolve(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let mut stack = vec![Level { folder: Dir::new(start)?, path: path.clone(), depth: 0 }];
        let mut entries = Vec::new();
        let mut looked = 0usize;
        let stopped = loop {
            let Some(level) = stack.last_mut() else { break None };
            looked += 1;
            if looked.is_multiple_of(SEARCH_CHECK_EVERY)
                && (Instant::now() >= bounds.until || bounds.stop.load(Ordering::Relaxed))
            {
                break Some(SearchStop::Time);
            }
            let item = match level.folder.next() {
                Some(Ok(item)) => item,
                // What cannot be read further is passed by.
                Some(Err(_)) | None => {
                    stack.pop();
                    continue;
                }
            };
            let raw = item.file_name();
            if raw == c"." || raw == c".." {
                continue;
            }
            let Ok(name) = std::str::from_utf8(raw.to_bytes()) else { continue };
            let skipped = (!hidden && name.starts_with('.'))
                || (path.is_root() && level.depth == 0 && name == RECYCLE_DIRECTORY)
                || name.starts_with(UPLOAD_PREFIX);
            if skipped {
                continue;
            }
            let Ok(found_path) = level.path.join(name) else { continue };
            let folder = level.folder.fd()?;
            let Ok(stat) = statat(folder, raw, AtFlags::SYMLINK_NOFOLLOW) else { continue };
            if name.to_lowercase().contains(&needle) {
                entries.push(FoundEntry { path: found_path.clone(), entry: entry(raw.to_bytes(), &stat) });
                if entries.len() == MAX_SEARCH_RESULTS {
                    break Some(SearchStop::Results);
                }
            }
            let enter = kind(&stat) == FileType::Directory
                && device(&stat) == self.file_system
                && level.depth + 1 < MAX_DEPTH;
            if !enter {
                continue;
            }
            // Only the folder looked at: whoever may write here could swap it
            // for another meanwhile.
            let Ok(inner) = open_beneath_flags(folder, raw, OFlags::RDONLY | OFlags::DIRECTORY) else {
                continue;
            };
            if fstat(&inner).map(|opened| identity(&opened)).ok() != Some(identity(&stat)) {
                continue;
            }
            let depth = level.depth + 1;
            stack.push(Level { folder: Dir::new(inner)?, path: found_path, depth });
        };
        Ok(Found { entries, stopped })
    }

    /// What `path` is: a file or a folder, when it was made (where the file
    /// system keeps that) and last changed, whose it is; for a folder, its
    /// files, folders and hidden entries below it counted as the calling
    /// thread may see them, and what the files hold. Walks as
    /// [`ShareRoot::search`] does: by descriptor, no link followed, no other
    /// file system entered, no deeper than 256 folders; a folder it may not
    /// read is passed by. Hidden entries are counted, not looked into, so
    /// the recycle bins and uploads under way are not counted through. At
    /// most [`MAX_COUNTED`] entries, ending early at `bounds`.
    pub fn facts(&self, path: &SharePath, bounds: SearchBounds<'_>) -> Result<Facts, FileError> {
        let at = self.resolve(path, OFlags::PATH)?;
        let stat = fstat(&at)?;
        let times = statx(&at, "", AtFlags::EMPTY_PATH, StatxFlags::BASIC_STATS | StatxFlags::BTIME)?;
        let created = times.stx_mask & StatxFlags::BTIME.bits() != 0;
        let mut facts = Facts {
            folder: kind(&stat) == FileType::Directory,
            created: created.then_some(times.stx_btime.tv_sec),
            modified: Some(times.stx_mtime.tv_sec),
            uid: times.stx_uid,
            ..Facts::default()
        };
        if !facts.folder {
            facts.bytes = if kind(&stat) == FileType::RegularFile {
                u64::try_from(stat.st_size).unwrap_or(0)
            } else {
                0
            };
            return Ok(facts);
        }
        let folder = self.resolve(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
        if identity(&fstat(&folder)?) != identity(&stat) {
            return Err(FileError::Invalid("the folder was replaced meanwhile"));
        }
        let mut stack = vec![(Dir::new(folder)?, 0u32)];
        let mut looked = 0usize;
        facts.stopped = loop {
            let Some((dir, depth)) = stack.last_mut() else { break None };
            looked += 1;
            if looked.is_multiple_of(SEARCH_CHECK_EVERY)
                && (Instant::now() >= bounds.until || bounds.stop.load(Ordering::Relaxed))
            {
                break Some(CountStop::Time);
            }
            let item = match dir.next() {
                Some(Ok(item)) => item,
                Some(Err(_)) | None => {
                    stack.pop();
                    continue;
                }
            };
            let depth = *depth;
            let raw = item.file_name();
            if raw == c"." || raw == c".." || raw.to_bytes().starts_with(UPLOAD_PREFIX.as_bytes()) {
                continue;
            }
            if raw.to_bytes().starts_with(b".") {
                facts.hidden += 1;
            } else {
                let base = dir.fd()?;
                let Ok(entry) = statat(base, raw, AtFlags::SYMLINK_NOFOLLOW) else { continue };
                if kind(&entry) != FileType::Directory {
                    facts.files += 1;
                    if kind(&entry) == FileType::RegularFile {
                        facts.bytes = facts.bytes.saturating_add(u64::try_from(entry.st_size).unwrap_or(0));
                    }
                } else {
                    facts.folders += 1;
                    let enter = device(&entry) == self.file_system && depth + 1 < MAX_DEPTH;
                    // Only the folder looked at: whoever may write here could
                    // swap it for another meanwhile.
                    let opened = enter
                        .then(|| open_beneath_flags(base, raw, OFlags::RDONLY | OFlags::DIRECTORY).ok())
                        .flatten()
                        .filter(|inner| fstat(inner).map(|o| identity(&o)).ok() == Some(identity(&entry)));
                    if let Some(inner) = opened {
                        stack.push((Dir::new(inner)?, depth + 1));
                    }
                }
            }
            if facts.files + facts.folders + facts.hidden >= MAX_COUNTED {
                break Some(CountStop::Entries);
            }
        };
        Ok(facts)
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

    /// Removes an upload's hidden file that was given up. Only a file of
    /// such a name goes, never a folder: whoever may write the folder can
    /// rename one of the user's folders to the upload's name, and removing
    /// it with everything in it would delete it with the user's rights.
    pub fn discard_upload(&self, path: &SharePath) -> Result<(), FileError> {
        let (parent, name) = self.parent(path)?;
        if !name.starts_with(UPLOAD_PREFIX) {
            return Err(FileError::Invalid("not an upload"));
        }
        // A folder gives EISDIR and stays.
        Ok(unlinkat(&parent, name, AtFlags::empty())?)
    }

    pub fn make_directory(&self, path: &SharePath) -> Result<(), FileError> {
        let (parent, name) = self.parent(path)?;
        Ok(mkdirat(&parent, name, Mode::from_raw_mode(0o777))?)
    }

    /// Renames or moves `from` to `to` in `target`, which may be another
    /// share: then by copying, writing the copy out and removing what was
    /// copied (see [`ShareRoot::remove_copied`]), which may leave part of
    /// the source ([`FileError::PartlyMoved`]). With `replace`, a file
    /// replaces a file; a folder is never replaced.
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
            // Within one root a move is a rename, never a copy.
            Err(Errno::XDEV) if self.same_as(target) => {
                Err(FileError::Invalid("not a rename within one place"))
            }
            // Another share is another file system.
            Err(Errno::XDEV) => {
                let (from_name, to_name) = (c_name(from_name)?, c_name(to_name)?);
                // Only what was copied goes after it, and only as it was
                // copied: whoever may write the folder could change or add
                // something meanwhile, or put something else in its place,
                // which the user's rights would then delete.
                let mut copied = Copied::recording();
                copy_new(
                    from_parent.as_fd(),
                    &from_name,
                    to_parent.as_fd(),
                    &to_name,
                    self.file_system,
                    0,
                    &mut copied,
                )?;
                target.sync()?;
                self.remove_copied(from, &copied)
            }
            other => Ok(other?),
        }
    }

    /// Writes out what the root's file system holds in memory (`syncfs`),
    /// and says whether that worked: a move removes its source only once
    /// the copy is on the target for sure, which a drive pulled out right
    /// after it would otherwise lose. Whoever may use the root may read it.
    pub fn sync(&self) -> Result<(), FileError> {
        let root = self.resolve(&SharePath::root(), OFlags::RDONLY | OFlags::DIRECTORY)?;
        rustix::fs::syncfs(&root).map_err(|error| FileError::NotDurable(error.into()))
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
                &mut Copied::off(),
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
    /// at any depth, without the `Copy #N of ` a second deletion of a name
    /// got there, of the entry or of a folder on its way (something inside
    /// a folder deleted twice goes back into that folder). Folders on the
    /// way that are gone are made again; if the name is taken, the first
    /// free `name (N).ext`. Returns where it went.
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
            let folder = original_name(folder);
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
        // Another writer could have made a folder of this name first, to
        // read what lands in it.
        let own = fstat(&bin)?;
        if own.st_uid != rustix::process::geteuid().as_raw() || own.st_mode & 0o077 != 0 {
            return Err(FileError::Invalid("the recycle bin is not yours alone; ask an administrator"));
        }
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
