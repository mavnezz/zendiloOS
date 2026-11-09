//! Backups of Docker's volumes and restoring them (docs/architecture.md,
//! 4.9): the work the transient units do, as `backup-volume`,
//! `restore-volume` and `remove-volume-backup` of this binary. The system
//! agent checks what is asked and starts them outside its own sandbox,
//! which can write neither the shares nor Docker's data, each in a sandbox
//! of its own that may write only where its work lands.
//!
//! A backup reads the volume's folder as root, by descriptor, never
//! following a link or entering another file system, and writes one tar
//! archive (GNU headers: numeric owners, modes, times, symbolic and hard
//! links, extended attributes as pax records), compressed with zstd, into
//! the share, with a small manifest beside it. Both are made under hidden
//! names and renamed when complete, created so that the folder's inherited
//! ACL applies to them as to any new file.
//!
//! Restoring runs as root from a share that users may write. So the
//! archive is opened beneath the share's root without following a link,
//! and its SHA-256 is checked against the one recorded before anything is
//! extracted; it is extracted into a fresh folder beside the volume's data,
//! each entry beneath it (no absolute paths, no `..`, every folder on the
//! way opened with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, no device
//! nodes), while the same bytes are hashed again (`restore-volume`). Only
//! if they still match, and the system agent finds no running container on
//! the volume after that, does the fresh folder take the data's place,
//! swapped in one rename (`RENAME_EXCHANGE`) by a unit of its own
//! (`swap-volume`). Anything else leaves the old contents as they were.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rustix::fs::{
    AtFlags, Dir, FileType, Gid, Mode, OFlags, RenameFlags, ResolveFlags, Stat, Timespec, Timestamps, Uid,
    XattrFlags, chownat, fchmod, fchown, fgetxattr, flistxattr, fsetxattr, fstat, fsync, futimens, linkat,
    mkdirat, open, openat, openat2, readlinkat, renameat_with, statat, symlinkat, unlinkat, utimensat,
};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{ArchiveDigest, BackupStem, SharePath, VolumeName, WrittenFile};

/// Deepest nesting of folders backed up or restored.
const MAX_DEPTH: usize = 1024;
/// How hard zstd works: Debian's default, fast and already a good ratio.
const LEVEL: i32 = 3;
/// Folders on the way into a share or the fresh folder: beneath it, never
/// through a link, never onto another file system.
const BENEATH: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
/// Extended attributes that are restored: users' own, file capabilities and
/// POSIX ACLs. Others (`trusted.*`, SELinux's) belong to a machine, not to
/// the data.
const RESTORED_XATTRS: [&str; 4] =
    ["user.", "security.capability", "system.posix_acl_access", "system.posix_acl_default"];
/// The pax records extended attributes travel in, as GNU tar writes them.
const XATTR_RECORD: &str = "SCHILY.xattr.";
/// Where a manifest says which format it has.
const MANIFEST_FORMAT: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveError {
    /// A safety check refused it: an archive that is not the one recorded,
    /// an entry that would land outside, a link on the way.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Failed(String),
}

fn failed(what: impl std::fmt::Display, error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Failed(format!("{what}: {error}"))
}

fn refused(reason: impl Into<String>) -> ArchiveError {
    ArchiveError::Refused(reason.into())
}

/// A name as it is shown in a message.
fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What the system agent asks of a backup's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupJob {
    /// The volume's data: `<data root>/volumes/<volume>/_data`.
    pub source: PathBuf,
    /// The share's root, `/volume<N>/<share>`.
    pub share: PathBuf,
    /// The folder in the share the plan names; the archive goes into
    /// `<folder>/<volume>`.
    pub folder: SharePath,
    pub volume: VolumeName,
    pub name: BackupStem,
    /// As Docker inspected the volume, for the manifest.
    pub driver: String,
    pub labels: BTreeMap<String, String>,
    pub created: String,
}

/// What a backup wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackupDone {
    pub archive: WrittenFile,
    pub manifest: WrittenFile,
    pub sha256: ArchiveDigest,
    /// What the files in the volume hold.
    pub data_bytes: u64,
    pub manifest_json: String,
    /// Entries left out: sockets, pipes, devices.
    pub skipped: u64,
}

/// The manifest beside an archive: what the volume was, for people and for
/// restoring it on another machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub format: u32,
    pub volume: String,
    pub driver: String,
    pub labels: BTreeMap<String, String>,
    /// What the volume's files hold, in bytes.
    pub size_bytes: u64,
    /// When Docker made the volume, as it says it.
    pub created: String,
    /// Unix seconds.
    pub backed_up_at: i64,
    pub zendiloos_version: String,
    pub archive: String,
    pub sha256: String,
}

/// What the system agent asks of a restore's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreJob {
    /// The volume's folder, `<data root>/volumes/<volume>`: `_data` in it is
    /// replaced.
    pub volume_dir: PathBuf,
    /// The share's root, `/volume<N>/<share>`.
    pub share: PathBuf,
    pub archive: SharePath,
    pub sha256: ArchiveDigest,
}

/// What a restore's first unit made: the archive extracted into `fresh`,
/// a folder beside the data, for the second to swap in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extracted {
    pub entries: u64,
    pub fresh: String,
}

/// What the system agent asks of the unit that swaps a restore's fresh
/// folder in for the data, or, with `discard`, removes it instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwapJob {
    pub volume_dir: PathBuf,
    pub fresh: String,
    pub discard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Swapped {
    /// The old contents (or, discarding, what was extracted) could not all
    /// be removed: they are left in this folder beside the data.
    pub left_over: Option<String>,
}

/// What a restore's fresh folder is called: `_data.zendiloos-restore-`
/// and 16 hex digits.
const FRESH_PREFIX: &str = "_data.zendiloos-restore-";

fn is_fresh_name(name: &[u8]) -> bool {
    name.strip_prefix(FRESH_PREFIX.as_bytes())
        .is_some_and(|hex| hex.len() == 16 && hex.iter().all(u8::is_ascii_hexdigit))
}

/// What the system agent asks of the unit that removes a backup's file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoveJob {
    pub share: PathBuf,
    pub path: SharePath,
    pub device: u64,
    pub inode: u64,
}

// Stat field types differ between architectures.
#[allow(clippy::useless_conversion)]
fn identity(stat: &Stat) -> (u64, u64) {
    (u64::from(stat.st_dev), u64::from(stat.st_ino))
}

fn kind(stat: &Stat) -> FileType {
    FileType::from_raw_mode(stat.st_mode)
}

fn c_name(bytes: &[u8]) -> Result<CString, ArchiveError> {
    CString::new(bytes).map_err(|_| refused(format!("{} is no file name", shown(bytes))))
}

/// A folder by its absolute path, for reading, never through a link at its
/// end; the places it is on are root's.
fn open_folder(path: &Path, flags: OFlags) -> Result<OwnedFd, ArchiveError> {
    open(path, flags | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| failed(path.display(), e))
}

/// `path` beneath `base`, as [`BENEATH`] allows: `.` for the base itself.
fn beneath(base: BorrowedFd<'_>, path: &[u8], flags: OFlags) -> Result<OwnedFd, Errno> {
    let relative: &[u8] = if path.is_empty() { b"." } else { path };
    openat2(
        base,
        OsStr::from_bytes(relative),
        flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        BENEATH,
    )
}

fn link_on_the_way(path: &str, error: Errno) -> ArchiveError {
    match error {
        Errno::LOOP | Errno::XDEV | Errno::NOTDIR => {
            refused(format!("{path} leads through a link or onto another file system"))
        }
        other => failed(path, other),
    }
}

/// The folder `path` of the share, made with everything on the way as far
/// as it is missing: mode 0777, which the folder's inherited ACL narrows,
/// as for any new folder.
fn make_folders(share: BorrowedFd<'_>, path: &SharePath) -> Result<OwnedFd, ArchiveError> {
    let mut folder =
        beneath(share, b"", OFlags::PATH | OFlags::DIRECTORY).map_err(|e| failed("the share", e))?;
    let mut walked = String::new();
    for name in path.names() {
        walked = if walked.is_empty() { name.to_owned() } else { format!("{walked}/{name}") };
        match mkdirat(&folder, name, Mode::from_raw_mode(0o777)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(error) => return Err(failed(&walked, error)),
        }
        folder = beneath(folder.as_fd(), name.as_bytes(), OFlags::PATH | OFlags::DIRECTORY)
            .map_err(|e| link_on_the_way(&walked, e))?;
    }
    Ok(folder)
}

/// A hidden file of its own in `folder`, for what becomes `name` once
/// complete: mode 0666, which the folder's inherited ACL narrows.
fn create_partial(folder: BorrowedFd<'_>, name: &str) -> Result<(OwnedFd, String), ArchiveError> {
    for _ in 0..8 {
        let mut random = [0u8; 8];
        rustix::rand::getrandom(&mut random, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| failed("no random name", e))?;
        let partial = format!(".{name}.partial-{:016x}", u64::from_le_bytes(random));
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match openat(folder, partial.as_str(), flags, Mode::from_raw_mode(0o666)) {
            Ok(file) => return Ok((file, partial)),
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(failed(name, error)),
        }
    }
    Err(ArchiveError::Failed(format!("no free name for {name}")))
}

/// Writes `content` into a new file `name` of `folder`, under a hidden name
/// first; refused if `name` exists.
fn write_new(
    folder: BorrowedFd<'_>,
    name: &str,
    content: impl FnOnce(&File) -> Result<(), ArchiveError>,
) -> Result<(u64, u64, u64), ArchiveError> {
    if statat(folder, name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err(refused(format!("{name} exists already")));
    }
    let (file, partial) = create_partial(folder, name)?;
    let file = File::from(file);
    let done = content(&file)
        .and_then(|()| fsync(&file).map_err(|e| failed(name, e)))
        .and_then(|()| {
            renameat_with(folder, partial.as_str(), folder, name, RenameFlags::NOREPLACE)
                .map_err(|e| failed(name, e))
        })
        .and_then(|()| fstat(&file).map_err(|e| failed(name, e)));
    match done {
        Ok(stat) => {
            let (device, inode) = identity(&stat);
            Ok((u64::try_from(stat.st_size).unwrap_or(0), device, inode))
        }
        Err(error) => {
            // Only the hidden name of this run: whoever may write the
            // folder could have swapped it, which removes no more than a name.
            let _ = unlinkat(folder, partial.as_str(), AtFlags::empty());
            Err(error)
        }
    }
}

/// Bytes through, hashed and counted.
struct Hashing<T> {
    inner: T,
    hash: Sha256,
    bytes: u64,
}

impl<T> Hashing<T> {
    fn new(inner: T) -> Self {
        Self { inner, hash: Sha256::new(), bytes: 0 }
    }

    fn digest(&self) -> String {
        self.hash.clone().finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hash.update(&buf[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Reports how far it got, in percent of `total`, each time that changes.
struct Progress<'a> {
    total: u64,
    done: u64,
    told: Option<u8>,
    report: &'a mut dyn FnMut(u8),
}

impl<'a> Progress<'a> {
    fn new(total: u64, report: &'a mut dyn FnMut(u8)) -> Self {
        Self { total, done: 0, told: None, report }
    }

    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        let percent = (self.done.min(self.total) * 100).checked_div(self.total).map_or(100, |p| p as u8);
        if self.told != Some(percent) {
            self.told = Some(percent);
            (self.report)(percent);
        }
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hash.update(&buf[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

/// A reader that tells `progress` what it read.
struct Told<'a, 'b, R> {
    inner: R,
    progress: &'b mut Progress<'a>,
}

impl<R: Read> Read for Told<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress.add(read as u64);
        Ok(read)
    }
}

/// The extended attributes of an open file or folder, as far as it has
/// any and they can be read.
fn xattrs(fd: BorrowedFd<'_>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut names = vec![0u8; 64 * 1024];
    let Ok(length) = flistxattr(fd, &mut names[..]) else { return Vec::new() };
    names.truncate(length);
    let mut out = Vec::new();
    for name in names.split(|&b| b == 0).filter(|name| !name.is_empty()) {
        let Ok(text) = std::str::from_utf8(name) else { continue };
        if !RESTORED_XATTRS.iter().any(|prefix| text.starts_with(prefix)) {
            continue;
        }
        let mut value = vec![0u8; 64 * 1024];
        if let Ok(length) = fgetxattr(fd, text, &mut value[..]) {
            value.truncate(length);
            out.push((name.to_vec(), value));
        }
    }
    out
}

/// What a walk of a folder found to count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counted {
    pub bytes: u64,
    pub entries: u64,
    pub complete: bool,
}

/// Counts what the files below `folder` hold, without following a link or
/// entering another file system: at most `max_entries`, until `until`.
pub(crate) fn count(folder: &OwnedFd, max_entries: u64, until: Option<Instant>) -> Counted {
    let mut counted = Counted { complete: true, ..Counted::default() };
    let device = fstat(folder).map(|s| identity(&s).0).unwrap_or(0);
    let mut linked = std::collections::HashSet::new();
    let Ok(top) = folder.try_clone() else { return Counted::default() };
    let mut stack = vec![(top, 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        let Ok(entries) = Dir::read_from(&directory) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            counted.entries += 1;
            if counted.entries > max_entries || until.is_some_and(|until| Instant::now() > until) {
                counted.complete = false;
                return counted;
            }
            let Ok(stat) = statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) else { continue };
            match kind(&stat) {
                FileType::RegularFile if stat.st_nlink < 2 || linked.insert(identity(&stat)) => {
                    counted.bytes += u64::try_from(stat.st_size).unwrap_or(0);
                }
                FileType::Directory if identity(&stat).0 == device && depth < MAX_DEPTH => {
                    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                    if let Ok(inner) = openat(&directory, name, flags, Mode::empty()) {
                        stack.push((inner, depth + 1));
                    }
                }
                _ => {}
            }
        }
    }
    counted
}

/// The header of an entry, with what `stat` says of it.
fn header(stat: &Stat, kind: tar::EntryType, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(stat.st_mode & 0o7777);
    header.set_uid(u64::from(stat.st_uid));
    header.set_gid(u64::from(stat.st_gid));
    #[allow(clippy::useless_conversion)]
    header.set_mtime(u64::try_from(i64::from(stat.st_mtime)).unwrap_or(0));
    header.set_size(size);
    header
}

/// The archive's path of a name below the volume's root.
fn archive_path(parts: &[Vec<u8>]) -> PathBuf {
    if parts.is_empty() {
        return PathBuf::from("./");
    }
    PathBuf::from(OsStr::from_bytes(&parts.join(&b'/')))
}

/// Writes the archive of one volume's folder.
struct Writer<'p, W: Write> {
    builder: tar::Builder<W>,
    device: u64,
    /// Files with more than one name: their first path.
    links: HashMap<(u64, u64), PathBuf>,
    progress: Progress<'p>,
    bytes: u64,
    skipped: u64,
}

impl<W: Write> Writer<'_, W> {
    fn attributes(&mut self, fd: BorrowedFd<'_>) -> Result<(), ArchiveError> {
        let found = xattrs(fd);
        if found.is_empty() {
            return Ok(());
        }
        let records: Vec<(String, Vec<u8>)> = found
            .into_iter()
            .map(|(name, value)| (format!("{XATTR_RECORD}{}", shown(&name)), value))
            .collect();
        self.builder
            .append_pax_extensions(records.iter().map(|(key, value)| (key.as_str(), value.as_slice())))
            .map_err(|e| failed("the archive", e))
    }

    fn folder(&mut self, folder: &OwnedFd, parts: &mut Vec<Vec<u8>>) -> Result<(), ArchiveError> {
        if parts.len() > MAX_DEPTH {
            return Err(ArchiveError::Failed("folders are nested too deeply".into()));
        }
        let stat = fstat(folder).map_err(|e| failed("a folder", e))?;
        self.attributes(folder.as_fd())?;
        let mut header = header(&stat, tar::EntryType::Directory, 0);
        let mut path = archive_path(parts);
        if !parts.is_empty() {
            path.push("");
        }
        self.builder.append_data(&mut header, &path, io::empty()).map_err(|e| failed(path.display(), e))?;
        let mut names = Vec::new();
        for entry in Dir::read_from(folder).map_err(|e| failed(path.display(), e))? {
            let entry = entry.map_err(|e| failed(path.display(), e))?;
            let name = entry.file_name();
            if name != c"." && name != c".." {
                names.push(name.to_owned());
            }
        }
        names.sort();
        for name in names {
            parts.push(name.to_bytes().to_vec());
            let done = self.entry(folder.as_fd(), &name, parts);
            parts.pop();
            done?;
        }
        Ok(())
    }

    fn entry(
        &mut self,
        folder: BorrowedFd<'_>,
        name: &CStr,
        parts: &mut Vec<Vec<u8>>,
    ) -> Result<(), ArchiveError> {
        let path = archive_path(parts);
        let what = || path.display().to_string();
        let stat = match statat(folder, name, AtFlags::SYMLINK_NOFOLLOW) {
            // Gone since the folder was read: a container still writes.
            Err(Errno::NOENT) => return Ok(()),
            other => other.map_err(|e| failed(what(), e))?,
        };
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match kind(&stat) {
            FileType::Directory => {
                if identity(&stat).0 != self.device {
                    self.skipped += 1;
                    return Ok(());
                }
                let inner = openat(folder, name, flags | OFlags::DIRECTORY, Mode::empty())
                    .map_err(|e| failed(what(), e))?;
                self.folder(&inner, parts)
            }
            FileType::RegularFile => {
                let id = identity(&stat);
                if stat.st_nlink > 1 {
                    if let Some(first) = self.links.get(&id) {
                        let mut header = header(&stat, tar::EntryType::Link, 0);
                        let first = first.clone();
                        return self
                            .builder
                            .append_link(&mut header, &path, &first)
                            .map_err(|e| failed(what(), e));
                    }
                    self.links.insert(id, path.clone());
                }
                let file = openat(folder, name, flags | OFlags::NONBLOCK, Mode::empty())
                    .map_err(|e| failed(what(), e))?;
                let opened = fstat(&file).map_err(|e| failed(what(), e))?;
                if kind(&opened) != FileType::RegularFile || identity(&opened) != id {
                    // Replaced since it was looked at: what is there now
                    // comes in the next backup.
                    self.skipped += 1;
                    return Ok(());
                }
                self.attributes(file.as_fd())?;
                let size = u64::try_from(opened.st_size).unwrap_or(0);
                let mut header = header(&opened, tar::EntryType::Regular, size);
                // Exactly `size` bytes: a file that grew meanwhile is cut,
                // one that shrank padded with zeros.
                let content = Told { inner: File::from(file).take(size), progress: &mut self.progress }
                    .chain(io::repeat(0))
                    .take(size);
                self.builder.append_data(&mut header, &path, content).map_err(|e| failed(what(), e))?;
                self.bytes += size;
                Ok(())
            }
            FileType::Symlink => {
                let target = readlinkat(folder, name, Vec::new()).map_err(|e| failed(what(), e))?;
                let mut header = header(&stat, tar::EntryType::Symlink, 0);
                let target = PathBuf::from(OsStr::from_bytes(target.as_bytes()));
                self.builder.append_link(&mut header, &path, &target).map_err(|e| failed(what(), e))
            }
            // Sockets, pipes and devices are no data.
            _ => {
                self.skipped += 1;
                Ok(())
            }
        }
    }
}

/// Writes the archive of `source` into `out`; its SHA-256, its length, what
/// the files held and what was left out.
fn write_archive(
    source: &OwnedFd,
    out: &File,
    total: u64,
    report: &mut dyn FnMut(u8),
) -> Result<(String, u64, u64, u64), ArchiveError> {
    let hashing = Hashing::new(BufWriter::with_capacity(1024 * 1024, out));
    let encoder = zstd::stream::write::Encoder::new(hashing, LEVEL).map_err(|e| failed("zstd", e))?;
    let device = fstat(source).map(|s| identity(&s).0).map_err(|e| failed("the volume", e))?;
    let mut writer = Writer {
        builder: tar::Builder::new(encoder),
        device,
        links: HashMap::new(),
        progress: Progress::new(total, report),
        bytes: 0,
        skipped: 0,
    };
    writer.folder(source, &mut Vec::new())?;
    // Files that shrank or went meanwhile: done all the same.
    writer.progress.add(writer.progress.total.saturating_sub(writer.progress.done));
    let (bytes, skipped) = (writer.bytes, writer.skipped);
    let encoder = writer.builder.into_inner().map_err(|e| failed("the archive", e))?;
    let mut hashing = encoder.finish().map_err(|e| failed("zstd", e))?;
    hashing.flush().map_err(|e| failed("the archive", e))?;
    Ok((hashing.digest(), hashing.bytes, bytes, skipped))
}

/// Backs up one volume as `job` says; `report` hears how far it got.
pub(crate) fn back_up(
    job: &BackupJob,
    now: i64,
    report: &mut dyn FnMut(u8),
) -> Result<BackupDone, ArchiveError> {
    let source = open_folder(&job.source, OFlags::RDONLY)?;
    let share = open_folder(&job.share, OFlags::PATH)?;
    let folder =
        job.folder.join(job.volume.as_str()).map_err(|_| refused("the folder's path is too long"))?;
    let target = make_folders(share.as_fd(), &folder)?;
    let total = count(&source, u64::MAX, None).bytes;
    let archive_name = format!("{}.tar.zst", job.name.as_str());
    let manifest_name = format!("{}.json", job.name.as_str());
    if statat(&target, manifest_name.as_str(), AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err(refused(format!("{manifest_name} exists already")));
    }
    let mut written = None;
    let (archive_bytes, archive_device, archive_inode) = write_new(target.as_fd(), &archive_name, |file| {
        written = Some(write_archive(&source, file, total, report)?);
        Ok(())
    })?;
    let (sha256, _, data_bytes, skipped) =
        written.ok_or_else(|| ArchiveError::Failed("nothing written".into()))?;
    let manifest = Manifest {
        format: MANIFEST_FORMAT,
        volume: job.volume.as_str().to_owned(),
        driver: job.driver.clone(),
        labels: job.labels.clone(),
        size_bytes: data_bytes,
        created: job.created.clone(),
        backed_up_at: now,
        zendiloos_version: env!("CARGO_PKG_VERSION").to_owned(),
        archive: archive_name.clone(),
        sha256: sha256.clone(),
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| failed("the manifest", e))? + "\n";
    let (manifest_bytes, manifest_device, manifest_inode) =
        write_new(target.as_fd(), &manifest_name, |mut file| {
            file.write_all(manifest_json.as_bytes()).map_err(|e| failed(&manifest_name, e))
        })?;
    let path = |name: &str| folder.join(name).map_err(|_| refused("the folder's path is too long"));
    Ok(BackupDone {
        archive: WrittenFile {
            path: path(&archive_name)?,
            bytes: archive_bytes,
            device: archive_device,
            inode: archive_inode,
        },
        manifest: WrittenFile {
            path: path(&manifest_name)?,
            bytes: manifest_bytes,
            device: manifest_device,
            inode: manifest_inode,
        },
        sha256: ArchiveDigest::parse(&sha256).map_err(|e| failed("the digest", e))?,
        data_bytes,
        manifest_json,
        skipped,
    })
}

/// The archive `path` of the share, opened for reading: beneath the share's
/// root, through no link, a regular file.
fn open_archive(share: BorrowedFd<'_>, path: &SharePath) -> Result<(File, u64), ArchiveError> {
    if path.is_root() {
        return Err(refused("no archive named"));
    }
    let file = beneath(share, path.as_str().as_bytes(), OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY)
        .map_err(|error| match error {
        Errno::NOENT => refused(format!("{} is not there any more", path.as_str())),
        other => link_on_the_way(path.as_str(), other),
    })?;
    let stat = fstat(&file).map_err(|e| failed(path.as_str(), e))?;
    if kind(&stat) != FileType::RegularFile {
        return Err(refused(format!("{} is no file", path.as_str())));
    }
    Ok((File::from(file), u64::try_from(stat.st_size).unwrap_or(0)))
}

/// The SHA-256 of what `file` holds from its start.
fn hash_file(file: &mut File, size: u64, report: &mut dyn FnMut(u8)) -> Result<String, ArchiveError> {
    file.seek(io::SeekFrom::Start(0)).map_err(|e| failed("the archive", e))?;
    let mut progress = Progress::new(size, report);
    let mut hashing = Hashing::new(io::sink());
    io::copy(&mut Told { inner: &mut *file, progress: &mut progress }, &mut hashing)
        .map_err(|e| failed("the archive", e))?;
    Ok(hashing.digest())
}

/// An entry's path as names beneath the target: refused if it is absolute
/// or goes up with `..`; `.` and empty names are dropped, so `./` is the
/// target itself.
pub(crate) fn names(path: &[u8]) -> Result<Vec<Vec<u8>>, ArchiveError> {
    if path.starts_with(b"/") {
        return Err(refused(format!("the archive holds an absolute path: {}", shown(path))));
    }
    let mut names = Vec::new();
    for name in path.split(|&b| b == b'/') {
        match name {
            b"" | b"." => {}
            b".." => return Err(refused(format!("the archive holds a path that leads up: {}", shown(path)))),
            name if name.len() > 255 || name.contains(&0) => {
                return Err(refused(format!("the archive holds an odd name: {}", shown(path))));
            }
            name => names.push(name.to_vec()),
        }
    }
    if names.len() > MAX_DEPTH {
        return Err(refused("the archive's folders are nested too deeply"));
    }
    Ok(names)
}

/// Owner, mode, time and extended attributes an entry brings.
#[derive(Debug, Clone)]
struct Meta {
    uid: u32,
    gid: u32,
    mode: u32,
    mtime: i64,
    xattrs: Vec<(String, Vec<u8>)>,
}

impl Meta {
    fn of<R: Read>(entry: &mut tar::Entry<'_, R>, what: &str) -> Result<Self, ArchiveError> {
        let bad = |e: io::Error| refused(format!("{what}: {e}"));
        let header = entry.header();
        let (uid, gid, mode, mtime) = (
            header.uid().map_err(bad)?,
            header.gid().map_err(bad)?,
            header.mode().map_err(bad)?,
            header.mtime().map_err(bad)?,
        );
        let mut xattrs = Vec::new();
        if let Some(records) = entry.pax_extensions().map_err(bad)? {
            for record in records {
                let record = record.map_err(bad)?;
                let Ok(key) = record.key() else { continue };
                if let Some(name) = key.strip_prefix(XATTR_RECORD)
                    && RESTORED_XATTRS.iter().any(|prefix| name.starts_with(prefix))
                {
                    xattrs.push((name.to_owned(), record.value_bytes().to_vec()));
                }
            }
        }
        let id =
            |value: u64| u32::try_from(value).map_err(|_| refused(format!("{what}: an owner out of range")));
        Ok(Self {
            uid: id(uid)?,
            gid: id(gid)?,
            mode: mode & 0o7777,
            mtime: i64::try_from(mtime).unwrap_or(0),
            xattrs,
        })
    }

    fn times(&self) -> Timestamps {
        let time = Timespec { tv_sec: self.mtime, tv_nsec: 0 };
        Timestamps { last_access: time, last_modification: time }
    }

    /// Onto an opened file or folder: its owner first (which clears setuid
    /// bits and file capabilities), then its mode, attributes and time.
    fn apply(&self, fd: BorrowedFd<'_>, what: &str) -> Result<(), ArchiveError> {
        fchown(fd, Some(Uid::from_raw(self.uid)), Some(Gid::from_raw(self.gid)))
            .map_err(|e| failed(format!("{what}: its owner"), e))?;
        fchmod(fd, Mode::from_raw_mode(self.mode)).map_err(|e| failed(format!("{what}: its mode"), e))?;
        for (name, value) in &self.xattrs {
            // Where the file system takes them.
            if let Err(error) = fsetxattr(fd, name.as_str(), value, XattrFlags::empty()) {
                tracing::debug!(%error, what, name, "an extended attribute is not restored");
            }
        }
        futimens(fd, &self.times()).map_err(|e| failed(format!("{what}: its time"), e))
    }
}

/// Extracts an archive beneath one fresh folder.
struct Extractor<'a> {
    root: BorrowedFd<'a>,
    /// Folders and what they bring, applied once their contents are in.
    folders: Vec<(Vec<Vec<u8>>, Meta)>,
    top: Option<Meta>,
    entries: u64,
}

impl Extractor<'_> {
    /// The folder an entry of `names` lies in, made as far as it is
    /// missing (an archive need not list every folder).
    fn parent(&self, names: &[Vec<u8>], what: &str) -> Result<OwnedFd, ArchiveError> {
        let parents = &names[..names.len().saturating_sub(1)];
        match beneath(self.root, &parents.join(&b'/'), OFlags::PATH | OFlags::DIRECTORY) {
            Ok(folder) => return Ok(folder),
            Err(Errno::NOENT) => {}
            Err(error) => return Err(link_on_the_way(what, error)),
        }
        let mut folder =
            beneath(self.root, b"", OFlags::PATH | OFlags::DIRECTORY).map_err(|e| failed(what, e))?;
        for name in parents {
            match mkdirat(&folder, OsStr::from_bytes(name), Mode::from_raw_mode(0o755)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(error) => return Err(failed(what, error)),
            }
            folder = beneath(folder.as_fd(), name, OFlags::PATH | OFlags::DIRECTORY)
                .map_err(|e| link_on_the_way(what, e))?;
        }
        Ok(folder)
    }

    fn entry<R: Read>(&mut self, entry: &mut tar::Entry<'_, R>) -> Result<(), ArchiveError> {
        let path = entry.path_bytes().into_owned();
        let what = shown(&path);
        let names = names(&path)?;
        let meta = Meta::of(entry, &what)?;
        let kind = entry.header().entry_type();
        self.entries += 1;
        let Some(last) = names.last().cloned() else {
            return match kind {
                tar::EntryType::Directory => {
                    self.top = Some(meta);
                    Ok(())
                }
                _ => Err(refused(format!("the archive holds a file without a name: {what}"))),
            };
        };
        let name = c_name(&last)?;
        let parent = self.parent(&names, &what)?;
        match kind {
            tar::EntryType::Directory => {
                match mkdirat(&parent, name.as_c_str(), Mode::from_raw_mode(0o700)) {
                    Ok(()) => {}
                    Err(Errno::EXIST) => {
                        let stat = statat(&parent, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(|e| failed(&what, e))?;
                        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                            return Err(refused(format!("the archive holds {what} twice")));
                        }
                    }
                    Err(error) => return Err(failed(&what, error)),
                }
                self.folders.push((names, meta));
                Ok(())
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let flags =
                    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                let file =
                    openat(&parent, name.as_c_str(), flags, Mode::from_raw_mode(0o600)).map_err(|error| {
                        if error == Errno::EXIST {
                            refused(format!("the archive holds {what} twice"))
                        } else {
                            failed(&what, error)
                        }
                    })?;
                let mut file = File::from(file);
                io::copy(entry, &mut file).map_err(|e| failed(&what, e))?;
                meta.apply(file.as_fd(), &what)
            }
            tar::EntryType::Symlink => {
                let target =
                    entry.link_name_bytes().ok_or_else(|| refused(format!("{what}: a link to nothing")))?;
                let target = c_name(&target)?;
                symlinkat(target.as_c_str(), &parent, name.as_c_str()).map_err(|error| {
                    if error == Errno::EXIST {
                        refused(format!("the archive holds {what} twice"))
                    } else {
                        failed(&what, error)
                    }
                })?;
                chownat(
                    &parent,
                    name.as_c_str(),
                    Some(Uid::from_raw(meta.uid)),
                    Some(Gid::from_raw(meta.gid)),
                    AtFlags::SYMLINK_NOFOLLOW,
                )
                .map_err(|e| failed(format!("{what}: its owner"), e))?;
                utimensat(&parent, name.as_c_str(), &meta.times(), AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|e| failed(format!("{what}: its time"), e))
            }
            tar::EntryType::Link => {
                let target =
                    entry.link_name_bytes().ok_or_else(|| refused(format!("{what}: a link to nothing")))?;
                let target_names = self::names(&target)?;
                let Some(target_last) = target_names.last() else {
                    return Err(refused(format!("{what} links to the volume itself")));
                };
                let target_parent = self.parent(&target_names, &what)?;
                let target_name = c_name(target_last)?;
                // Without AT_SYMLINK_FOLLOW: a link names the entry itself,
                // never what a symbolic link there points to.
                linkat(&target_parent, target_name.as_c_str(), &parent, name.as_c_str(), AtFlags::empty())
                    .map_err(|error| match error {
                        Errno::EXIST => refused(format!("the archive holds {what} twice")),
                        Errno::NOENT => {
                            refused(format!("{what} links to {} before it is there", shown(&target)))
                        }
                        other => failed(&what, other),
                    })
            }
            tar::EntryType::Char | tar::EntryType::Block => {
                Err(refused(format!("the archive holds a device node: {what}")))
            }
            _ => Err(refused(format!("the archive holds an entry zendiloOS does not restore: {what}"))),
        }
    }

    /// The folders' owners, modes and times, the deepest first, the target
    /// last: once nothing more goes into them.
    fn finish(mut self) -> Result<u64, ArchiveError> {
        self.folders.sort_by_key(|folder| std::cmp::Reverse(folder.0.len()));
        let flags = OFlags::RDONLY | OFlags::DIRECTORY;
        for (names, meta) in &self.folders {
            let what = shown(&names.join(&b'/'));
            let folder =
                beneath(self.root, &names.join(&b'/'), flags).map_err(|e| link_on_the_way(&what, e))?;
            meta.apply(folder.as_fd(), &what)?;
        }
        if let Some(meta) = &self.top {
            let folder = beneath(self.root, b"", flags).map_err(|e| failed("the volume", e))?;
            meta.apply(folder.as_fd(), "the volume")?;
        }
        Ok(self.entries)
    }
}

/// Extracts the archive in `file` (from its start) beneath `root`, hashing
/// what is read; refused unless the hash is `expected` and the file was
/// read to its end at `size` bytes.
fn extract(
    file: &mut File,
    root: BorrowedFd<'_>,
    size: u64,
    expected: &str,
    report: &mut dyn FnMut(u8),
) -> Result<u64, ArchiveError> {
    file.seek(io::SeekFrom::Start(0)).map_err(|e| failed("the archive", e))?;
    let mut progress = Progress::new(size, report);
    let hashing = Hashing::new(Told { inner: &mut *file, progress: &mut progress });
    let decoder =
        zstd::stream::read::Decoder::new(hashing).map_err(|e| refused(format!("not a zstd archive: {e}")))?;
    let mut archive = tar::Archive::new(decoder);
    let mut extractor = Extractor { root, folders: Vec::new(), top: None, entries: 0 };
    for entry in archive.entries().map_err(|e| refused(format!("not a tar archive: {e}")))? {
        let mut entry = entry.map_err(|e| refused(format!("the archive cannot be read: {e}")))?;
        extractor.entry(&mut entry)?;
    }
    // The rest of the file, so that the hash covers all of it.
    let mut hashing = archive.into_inner().finish().into_inner();
    io::copy(&mut hashing, &mut io::sink()).map_err(|e| failed("the archive", e))?;
    if hashing.digest() != expected || hashing.bytes != size {
        return Err(refused("the archive changed while it was read; nothing was restored"));
    }
    extractor.finish()
}

/// Removes `name` from `folder` with everything in it, never following a
/// link or entering another file system.
fn remove_tree(folder: BorrowedFd<'_>, name: &CStr, device: u64, depth: usize) -> Result<(), Errno> {
    let stat = statat(folder, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if kind(&stat) != FileType::Directory {
        return unlinkat(folder, name, AtFlags::empty());
    }
    if identity(&stat).0 != device || depth > MAX_DEPTH {
        return Err(Errno::XDEV);
    }
    let inner = openat(
        folder,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    loop {
        let mut batch = Vec::new();
        for entry in Dir::read_from(&inner)? {
            let entry = entry?;
            let child = entry.file_name();
            if child != c"." && child != c".." {
                batch.push(child.to_owned());
            }
            if batch.len() >= 4096 {
                break;
            }
        }
        if batch.is_empty() {
            break;
        }
        for child in batch {
            remove_tree(inner.as_fd(), &child, device, depth + 1)?;
        }
    }
    unlinkat(folder, name, AtFlags::REMOVEDIR)
}

/// The volume's folder in Docker's data root, and its file system.
fn volume_folder(path: &Path) -> Result<(OwnedFd, u64), ArchiveError> {
    let volume = open_folder(path, OFlags::PATH)?;
    let device = identity(&fstat(&volume).map_err(|e| failed("the volume", e))?).0;
    Ok((volume, device))
}

/// The folder `name` beside the data: on the volume's file system, no link.
fn folder_beside(volume: BorrowedFd<'_>, name: &str, device: u64) -> Result<(), ArchiveError> {
    let stat = statat(volume, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| failed(name, e))?;
    if kind(&stat) != FileType::Directory || identity(&stat).0 != device {
        return Err(refused(format!("{name} is no folder of Docker's data root")));
    }
    Ok(())
}

/// Extracts the archive `job` names into a fresh folder beside the
/// volume's data, for [`swap`] to swap in; `report` hears each step and how
/// far. A failure removes what it made.
pub(crate) fn extract_volume(
    job: &RestoreJob,
    report: &mut dyn FnMut(&'static str, u8),
) -> Result<Extracted, ArchiveError> {
    let share = open_folder(&job.share, OFlags::PATH)?;
    let (mut file, size) = open_archive(share.as_fd(), &job.archive)?;
    let digest = hash_file(&mut file, size, &mut |percent| report("verifying", percent))?;
    if digest != job.sha256.as_str() {
        return Err(refused(format!(
            "{} is not the archive that was backed up: its SHA-256 differs; nothing was restored",
            job.archive.as_str()
        )));
    }
    let (volume, device) = volume_folder(&job.volume_dir)?;
    folder_beside(volume.as_fd(), "_data", device)?;
    let mut random = [0u8; 8];
    rustix::rand::getrandom(&mut random, rustix::rand::GetRandomFlags::empty())
        .map_err(|e| failed("no random name", e))?;
    let fresh = format!("{FRESH_PREFIX}{:016x}", u64::from_le_bytes(random));
    let fresh_c = CString::new(fresh.as_str()).map_err(|e| failed("a name", e))?;
    mkdirat(&volume, fresh_c.as_c_str(), Mode::from_raw_mode(0o700))
        .map_err(|e| failed("a fresh folder", e))?;
    let extracted = beneath(volume.as_fd(), fresh.as_bytes(), OFlags::PATH | OFlags::DIRECTORY)
        .map_err(|e| failed("the fresh folder", e))
        .and_then(|root| {
            extract(&mut file, root.as_fd(), size, &digest, &mut |percent| report("restoring", percent))
        });
    match extracted {
        Ok(entries) => Ok(Extracted { entries, fresh }),
        Err(error) => {
            if let Err(cleanup) = remove_tree(volume.as_fd(), &fresh_c, device, 0) {
                tracing::warn!(%cleanup, "what was extracted could not all be removed");
            }
            Err(error)
        }
    }
}

/// Swaps the fresh folder `job` names in for the volume's data, in one
/// rename, and removes the old contents; with `discard`, removes the fresh
/// folder instead.
pub(crate) fn swap(job: &SwapJob) -> Result<Swapped, ArchiveError> {
    if !is_fresh_name(job.fresh.as_bytes()) {
        return Err(refused(format!("{} is no folder a restore made", job.fresh)));
    }
    let (volume, device) = volume_folder(&job.volume_dir)?;
    folder_beside(volume.as_fd(), &job.fresh, device)?;
    let fresh = CString::new(job.fresh.as_str()).map_err(|e| failed("a name", e))?;
    if !job.discard {
        folder_beside(volume.as_fd(), "_data", device)?;
        renameat_with(&volume, "_data", &volume, fresh.as_c_str(), RenameFlags::EXCHANGE)
            .map_err(|e| failed("swapping the data", e))?;
    }
    // The old contents are under the fresh name now, or what was extracted.
    let left_over = remove_tree(volume.as_fd(), &fresh, device, 0).err().map(|error| {
        tracing::warn!(%error, "a folder beside the volume's data could not all be removed");
        job.fresh.clone()
    });
    Ok(Swapped { left_over })
}

/// What a whole restore did: extracted, then swapped in.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RestoreDone {
    pub entries: u64,
    pub left_over: Option<String>,
}

/// Restores a volume as `job` says, both units' work in one.
#[cfg(test)]
pub(crate) fn restore(
    job: &RestoreJob,
    report: &mut dyn FnMut(&'static str, u8),
) -> Result<RestoreDone, ArchiveError> {
    let extracted = extract_volume(job, report)?;
    let swap = SwapJob { volume_dir: job.volume_dir.clone(), fresh: extracted.fresh, discard: false };
    let swapped = self::swap(&swap)?;
    Ok(RestoreDone { entries: extracted.entries, left_over: swapped.left_over })
}

/// Removes a backup's file if it still is the one written; `false` if it
/// is gone or another file now.
pub(crate) fn remove_backup(job: &RemoveJob) -> Result<bool, ArchiveError> {
    let share = open_folder(&job.share, OFlags::PATH)?;
    let Some(name) = job.path.file_name() else { return Err(refused("no file named")) };
    let parent = job.path.parent().unwrap_or_default();
    let folder = match beneath(share.as_fd(), parent.as_str().as_bytes(), OFlags::PATH | OFlags::DIRECTORY) {
        Ok(folder) => folder,
        Err(Errno::NOENT) => return Ok(false),
        Err(error) => return Err(link_on_the_way(parent.as_str(), error)),
    };
    let stat = match statat(&folder, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(false),
        Err(error) => return Err(failed(job.path.as_str(), error)),
    };
    if kind(&stat) != FileType::RegularFile || identity(&stat) != (job.device, job.inode) {
        return Ok(false);
    }
    unlinkat(&folder, name, AtFlags::empty()).map_err(|e| failed(job.path.as_str(), e))?;
    Ok(true)
}

/// What a unit prints, one JSON object per line: how far it is, then what
/// it did or why it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnitLine {
    Progress { step: String, percent: u8 },
    Done(serde_json::Value),
    Failed { message: String, refused: bool },
}

/// A job as it travels on a unit's command line: hex digits of its JSON,
/// which neither systemd nor a shell reads anything into.
pub(crate) fn encode(job: &impl Serialize) -> String {
    serde_json::to_vec(job).unwrap_or_default().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(hex: &str) -> Result<T, String> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the job is not hex digits".into());
    }
    let bytes: Vec<u8> =
        (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0)).collect();
    serde_json::from_slice(&bytes).map_err(|e| format!("the job cannot be read: {e}"))
}

/// Which job a unit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeJob {
    BackUp,
    /// A restore's first part: the archive extracted beside the data.
    Restore,
    /// Its second: swapped in.
    Swap,
    RemoveBackup,
}

impl VolumeJob {
    /// The command of this binary that runs it.
    pub(crate) fn command(self) -> &'static str {
        match self {
            Self::BackUp => "backup-volume",
            Self::Restore => "restore-volume",
            Self::Swap => "swap-volume",
            Self::RemoveBackup => "remove-volume-backup",
        }
    }

    /// The word for it in its unit's name.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::BackUp => "backup",
            Self::Restore => "restore",
            Self::Swap => "swap",
            Self::RemoveBackup => "remove",
        }
    }
}

fn say(out: &mut dyn Write, line: &UnitLine) {
    let _ = writeln!(out, "{}", serde_json::to_string(line).unwrap_or_default());
    let _ = out.flush();
}

/// Runs a unit's job, printing [`UnitLine`]s to `out`; whether it was done.
pub fn run_volume_job(kind: VolumeJob, hex: &str, now: i64, out: &mut dyn Write) -> bool {
    let outcome: Result<serde_json::Value, ArchiveError> = {
        let mut progress = |step: &'static str, percent: u8| {
            say(out, &UnitLine::Progress { step: step.to_owned(), percent });
        };
        match kind {
            VolumeJob::BackUp => decode::<BackupJob>(hex).map_err(ArchiveError::Refused).and_then(|job| {
                back_up(&job, now, &mut |percent| progress("backing_up", percent))
                    .and_then(|done| serde_json::to_value(done).map_err(|e| failed("the result", e)))
            }),
            VolumeJob::Restore => decode::<RestoreJob>(hex).map_err(ArchiveError::Refused).and_then(|job| {
                extract_volume(&job, &mut progress)
                    .and_then(|done| serde_json::to_value(done).map_err(|e| failed("the result", e)))
            }),
            VolumeJob::Swap => decode::<SwapJob>(hex).map_err(ArchiveError::Refused).and_then(|job| {
                progress("restoring", 100);
                swap(&job).and_then(|done| serde_json::to_value(done).map_err(|e| failed("the result", e)))
            }),
            VolumeJob::RemoveBackup => decode::<RemoveJob>(hex)
                .map_err(ArchiveError::Refused)
                .and_then(|job| remove_backup(&job).map(serde_json::Value::Bool)),
        }
    };
    match outcome {
        Ok(value) => {
            say(out, &UnitLine::Done(value));
            true
        }
        Err(error) => {
            let refused = matches!(error, ArchiveError::Refused(_));
            say(out, &UnitLine::Failed { message: error.to_string(), refused });
            false
        }
    }
}

#[cfg(test)]
mod tests;
