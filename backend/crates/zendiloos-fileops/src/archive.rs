//! A folder as a tar archive (POSIX pax), for downloading it whole.
//!
//! The archive is planned first, as the user: every entry, with its size,
//! and so the archive's exact length. Planning fails if anything inside
//! cannot be read, so a download is refused rather than silently missing
//! files. Writing then follows the plan to the byte: a file that grew
//! since is cut, one that shrank is padded with zeros. Should writing fail
//! midway, the download ends short of its announced length, which browsers
//! report as a failed download.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{AtFlags, FileType, OFlags, fstat, readlinkat, statat};
use zendiloos_agent_proto::SharePath;

use super::{
    FileError, MAX_DEPTH, RECYCLE_DIRECTORY, ShareRoot, UPLOAD_PREFIX, device, kind, names,
    open_beneath_flags,
};

/// The most entries one archive holds.
pub const MAX_ARCHIVE_ENTRIES: usize = 100_000;

const BLOCK: u64 = 512;
/// The largest size and time the ustar header's octal fields hold.
const OCTAL_11: u64 = 0o77_777_777_777;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Directory,
    File { size: u64 },
    Symlink { target: Vec<u8> },
}

/// One entry: its own name and the index of its folder, so a deep tree
/// costs no more memory than a flat one.
#[derive(Debug, Clone)]
struct Entry {
    /// The folder it is in; `None` only for the archived folder itself.
    parent: Option<usize>,
    /// Empty for the archived folder itself.
    name: CString,
    kind: Kind,
    mode: u32,
    modified: i64,
}

/// What an archive of one folder will hold, and its length in bytes.
#[derive(Debug, Clone)]
pub struct ArchivePlan {
    /// The folder's name, the top of every path in the archive.
    root: Vec<u8>,
    entries: Vec<Entry>,
    length: u64,
}

impl ArchivePlan {
    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn entries(&self) -> usize {
        self.entries.len()
    }

    /// The path of entry `index` in the archive; folders end with `/`.
    fn path(&self, index: usize) -> Vec<u8> {
        let mut names = Vec::new();
        let mut at = Some(index);
        while let Some(i) = at {
            let entry = &self.entries[i];
            if entry.parent.is_some() {
                names.push(entry.name.to_bytes());
            }
            at = entry.parent;
        }
        let mut path = self.root.clone();
        for name in names.into_iter().rev() {
            path.push(b'/');
            path.extend_from_slice(name);
        }
        if self.entries[index].kind == Kind::Directory {
            path.push(b'/');
        }
        path
    }
}

impl ShareRoot {
    /// Plans an archive of the folder `path`, its top named `name`. At the
    /// share's top the recycle bins are left out; uploads in progress
    /// everywhere; file systems mounted inside are not entered.
    pub fn plan_archive(&self, path: &SharePath, name: &str) -> Result<ArchivePlan, FileError> {
        let folder = self.resolve(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let stat = fstat(&folder)?;
        let mut entries = vec![Entry {
            parent: None,
            name: CString::default(),
            kind: Kind::Directory,
            mode: stat.st_mode,
            modified: stat.st_mtime,
        }];
        self.plan_folder(folder.as_fd(), 0, 0, path.is_root(), &mut entries)?;
        let mut plan = ArchivePlan { root: name.as_bytes().to_vec(), entries, length: 2 * BLOCK };
        let length: u64 = (0..plan.entries.len()).map(|index| entry_length(&plan, index)).sum();
        plan.length += length;
        Ok(plan)
    }

    fn plan_folder(
        &self,
        folder: BorrowedFd<'_>,
        index: usize,
        depth: u32,
        share_top: bool,
        entries: &mut Vec<Entry>,
    ) -> Result<(), FileError> {
        if depth >= MAX_DEPTH {
            return Err(FileError::Invalid("folders are nested too deeply"));
        }
        let mut children = names(folder)?;
        children.sort();
        for child in children {
            let bytes = child.to_bytes();
            if bytes.starts_with(UPLOAD_PREFIX.as_bytes())
                || (share_top && bytes == RECYCLE_DIRECTORY.as_bytes())
            {
                continue;
            }
            if entries.len() >= MAX_ARCHIVE_ENTRIES {
                return Err(FileError::Invalid("too many files for one download"));
            }
            let stat = statat(folder, child.as_c_str(), AtFlags::SYMLINK_NOFOLLOW)?;
            let (mode, modified, parent) = (stat.st_mode, stat.st_mtime, Some(index));
            match kind(&stat) {
                FileType::Directory if device(&stat) == self.file_system => {
                    let inner = open_beneath_flags(folder, &child, OFlags::RDONLY | OFlags::DIRECTORY)?;
                    entries.push(Entry { parent, name: child, kind: Kind::Directory, mode, modified });
                    let own = entries.len() - 1;
                    self.plan_folder(inner.as_fd(), own, depth + 1, false, entries)?;
                }
                FileType::RegularFile => {
                    // Opened once now: what cannot be read refuses the archive.
                    drop(open_beneath_flags(folder, &child, OFlags::RDONLY | OFlags::NONBLOCK)?);
                    let size = u64::try_from(stat.st_size).unwrap_or(0);
                    entries.push(Entry { parent, name: child, kind: Kind::File { size }, mode, modified });
                }
                FileType::Symlink => {
                    let target = readlinkat(folder, child.as_c_str(), Vec::new())?.into_bytes();
                    entries.push(Entry {
                        parent,
                        name: child,
                        kind: Kind::Symlink { target },
                        mode,
                        modified,
                    });
                }
                // Mounted file systems, devices, pipes and sockets.
                _ => {}
            }
        }
        Ok(())
    }

    /// Writes the archive `plan` describes of the folder `path`: exactly
    /// [`ArchivePlan::length`] bytes, unless it fails.
    pub fn write_archive(
        &self,
        path: &SharePath,
        plan: &ArchivePlan,
        out: &mut impl Write,
    ) -> Result<(), FileError> {
        // The folders on the way to the current entry, opened: entries come
        // folder by folder, each folder before what is in it.
        let mut open: Vec<(usize, OwnedFd)> =
            vec![(0, self.resolve(path, OFlags::PATH | OFlags::DIRECTORY)?)];
        for (index, entry) in plan.entries.iter().enumerate() {
            out.write_all(&header(plan, index))?;
            let Some(parent) = entry.parent else { continue };
            while open.last().is_some_and(|(i, _)| *i != parent) {
                open.pop();
            }
            let folder =
                open.last().map(|(_, fd)| fd.as_fd()).ok_or(FileError::Invalid("the plan is broken"))?;
            match entry.kind {
                Kind::Directory => {
                    let inner = open_beneath_flags(folder, &entry.name, OFlags::PATH | OFlags::DIRECTORY)?;
                    open.push((index, inner));
                }
                Kind::File { size } => {
                    let file = open_beneath_flags(folder, &entry.name, OFlags::RDONLY | OFlags::NONBLOCK)?;
                    copy_exactly(file, size, out)?;
                    out.write_all(&padding(size))?;
                }
                Kind::Symlink { .. } => {}
            }
        }
        out.write_all(&[0; 2 * BLOCK as usize])?;
        out.flush()?;
        Ok(())
    }
}

/// `size` bytes of `file`: cut if it grew, padded with zeros if it shrank
/// or is no longer a regular file.
fn copy_exactly(file: OwnedFd, size: u64, out: &mut impl Write) -> Result<(), FileError> {
    let copied = if kind(&fstat(&file)?) == FileType::RegularFile {
        std::io::copy(&mut File::from(file).take(size), out)?
    } else {
        0
    };
    let mut missing = size - copied;
    let zeros = [0u8; 64 * 1024];
    while missing > 0 {
        let chunk = usize::try_from(missing.min(zeros.len() as u64)).unwrap_or(zeros.len());
        out.write_all(&zeros[..chunk])?;
        missing -= chunk as u64;
    }
    Ok(())
}

fn padding(size: u64) -> Vec<u8> {
    vec![0; usize::try_from((BLOCK - size % BLOCK) % BLOCK).unwrap_or(0)]
}

/// Bytes entry `index` takes in the archive: its header(s) and content.
fn entry_length(plan: &ArchivePlan, index: usize) -> u64 {
    let content = match plan.entries[index].kind {
        Kind::File { size } => size.div_ceil(BLOCK) * BLOCK,
        _ => 0,
    };
    header(plan, index).len() as u64 + content
}

/// The ustar header of entry `index`, after a pax header if a value does
/// not fit.
fn header(plan: &ArchivePlan, index: usize) -> Vec<u8> {
    let entry = &plan.entries[index];
    let path = plan.path(index);
    let (typeflag, size, link) = match &entry.kind {
        Kind::Directory => (b'5', 0, &[][..]),
        Kind::File { size } => (b'0', *size, &[][..]),
        Kind::Symlink { target } => (b'2', 0, target.as_slice()),
    };
    let mut records = Vec::new();
    if path.len() > 100 {
        records.extend(pax_record("path", &path));
    }
    if link.len() > 100 {
        records.extend(pax_record("linkpath", link));
    }
    if size > OCTAL_11 {
        records.extend(pax_record("size", size.to_string().as_bytes()));
    }
    let modified = u64::try_from(entry.modified).unwrap_or(0).min(OCTAL_11);
    let mut out = Vec::new();
    if !records.is_empty() {
        out.extend(ustar(b"././@PaxHeader", b'x', 0o644, records.len() as u64, modified, &[]));
        let length = records.len() as u64;
        out.extend(records);
        out.extend(padding(length));
    }
    let size = if size > OCTAL_11 { 0 } else { size };
    out.extend(ustar(&path, typeflag, entry.mode & 0o1777, size, modified, link));
    out
}

/// One pax record: `<length> <key>=<value>\n`, the length counting itself.
fn pax_record(key: &str, value: &[u8]) -> Vec<u8> {
    let body = [b" ", key.as_bytes(), b"=", value, b"\n"].concat();
    let mut length = body.len() + 1;
    loop {
        let total = length.to_string().len() + body.len();
        if total == length {
            break;
        }
        length = total;
    }
    [length.to_string().as_bytes(), &body].concat()
}

/// A 512-byte ustar header. Names and links longer than their fields are
/// cut here; the pax header before it holds them whole.
fn ustar(name: &[u8], typeflag: u8, mode: u32, size: u64, modified: u64, link: &[u8]) -> [u8; 512] {
    let mut block = [0u8; 512];
    let name = &name[..name.len().min(100)];
    block[..name.len()].copy_from_slice(name);
    octal(&mut block[100..108], u64::from(mode));
    octal(&mut block[108..116], 0); // uid
    octal(&mut block[116..124], 0); // gid
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], modified);
    block[156] = typeflag;
    let link = &link[..link.len().min(100)];
    block[157..157 + link.len()].copy_from_slice(link);
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    // The checksum counts its own field as spaces.
    block[148..156].copy_from_slice(b"        ");
    let sum: u32 = block.iter().map(|&b| u32::from(b)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

/// Zero-padded octal digits, then a NUL.
fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&text.as_bytes()[text.len() - digits..]);
    field[digits] = 0;
}
