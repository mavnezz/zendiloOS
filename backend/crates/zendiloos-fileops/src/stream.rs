//! Copies between roots that are worked on with different credentials: a
//! removable drive, read and written with root's access to files, and a
//! folder on a volume, read and written as the person. One thread reads
//! the source with its credentials and sends what it finds, piece by piece
//! and in order, files already opened; another creates the copy with its
//! own: so what lands in a folder on a volume belongs to the person and
//! takes the folder's inherited ACL, exactly as an upload does, and only
//! what the person may read there reaches a drive. The source walks as a
//! copy does (by descriptor, no link followed, no other file system
//! entered), and the target is filled as a copy is: only a folder made
//! empty is filled, and what was made goes again when anything fails.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::mpsc::{Receiver, SyncSender};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, Timestamps, futimens, mkdirat, openat, readlinkat, statat,
    symlinkat,
};
use zendiloos_agent_proto::{SharePath, is_file_name};

use super::{
    FileError, MAX_DEPTH, MAX_NUMBERED, ShareRoot, c_name, device, identity, kind, nested, numbered,
    open_beneath_flags, remove_entry, some_names, timestamps,
};

/// One entry of what is copied, as the source's side read it.
#[derive(Debug)]
pub enum Piece {
    /// A folder begins; what is in it follows, up to its [`Piece::Leave`].
    Folder(CString),
    /// A regular file, opened for reading by the source's side.
    File {
        name: CString,
        file: OwnedFd,
        times: Timestamps,
    },
    Link {
        name: CString,
        target: CString,
    },
    /// The folder begun last ends.
    Leave,
    /// Everything was read.
    Done,
}

/// The most pieces on their way at once: a file each, so a few descriptors.
pub const PIECES_IN_FLIGHT: usize = 32;

/// What was sent, by device and inode: a move removes only that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent(pub(crate) u64, pub(crate) u64);

/// Where the copy should go if its own name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Beside it, as `name (2).ext`: a copy.
    Beside,
    /// Nowhere: a move fails.
    Refuse,
}

fn put(out: &SyncSender<Piece>, piece: Piece) -> Result<(), FileError> {
    out.send(piece).map_err(|_| FileError::Invalid("the copy's target stopped"))
}

fn next(pieces: &Receiver<Piece>) -> Result<Piece, FileError> {
    pieces.recv().map_err(|_| FileError::Invalid("the copy's source stopped"))
}

fn out_of_order() -> FileError {
    FileError::Invalid("the copy came out of order")
}

/// Sends `name` in `source`, and what is in it.
fn send(
    source: BorrowedFd<'_>,
    name: &CStr,
    file_system: u64,
    depth: u32,
    out: &SyncSender<Piece>,
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
            let folder = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            put(out, Piece::Folder(name.to_owned()))?;
            for entry in Dir::read_from(&folder)? {
                let entry = entry?;
                let child = entry.file_name();
                if child != c"." && child != c".." {
                    send(folder.as_fd(), child, file_system, depth + 1, out)?;
                }
            }
            put(out, Piece::Leave)?;
        }
        FileType::RegularFile => {
            let file = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::NONBLOCK)?;
            put(out, Piece::File { name: name.to_owned(), file, times: timestamps(&stat) })?;
        }
        FileType::Symlink => {
            let target = readlinkat(source, name, Vec::new())?;
            put(out, Piece::Link { name: name.to_owned(), target })?;
        }
        // Devices, pipes and sockets are not copied.
        _ => {}
    }
    Ok(())
}

fn write_file(from: OwnedFd, to: OwnedFd, times: &Timestamps) -> Result<(), FileError> {
    let (mut from, mut to) = (File::from(from), File::from(to));
    std::io::copy(&mut from, &mut to)?;
    futimens(&to, times)?;
    Ok(())
}

fn new_file(directory: BorrowedFd<'_>, name: &CStr) -> Result<OwnedFd, rustix::io::Errno> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    openat(directory, name, flags, Mode::from_raw_mode(0o666))
}

/// A new folder `name` in `directory`, opened, and empty: whoever may write
/// `directory` could put one of their own in its place.
fn new_folder(directory: BorrowedFd<'_>, name: &CStr) -> Result<OwnedFd, FileError> {
    mkdirat(directory, name, Mode::from_raw_mode(0o777))?;
    let folder = open_beneath_flags(directory, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
    if !some_names(&folder, 1)?.is_empty() {
        return Err(nested(FileError::Exists));
    }
    Ok(folder)
}

/// Fills `directory` with what comes, up to the [`Piece::Leave`] of the
/// folder it is.
fn fill(directory: BorrowedFd<'_>, pieces: &Receiver<Piece>, depth: u32) -> Result<(), FileError> {
    loop {
        match next(pieces)? {
            Piece::Folder(name) => {
                if depth >= MAX_DEPTH {
                    return Err(FileError::Invalid("folders are nested too deeply"));
                }
                let folder = new_folder(directory, &name).map_err(nested)?;
                fill(folder.as_fd(), pieces, depth + 1)?;
            }
            Piece::File { name, file, times } => {
                let to = new_file(directory, &name).map_err(|e| nested(e.into()))?;
                write_file(file, to, &times)?;
            }
            Piece::Link { name, target } => {
                symlinkat(target.as_c_str(), directory, &name).map_err(|e| nested(e.into()))?;
            }
            Piece::Leave => return Ok(()),
            Piece::Done => return Err(out_of_order()),
        }
    }
}

/// The top of the copy, made under `name`; the rest of it is not read yet.
enum Made {
    Folder(OwnedFd),
    File(OwnedFd),
    Link,
}

fn make_top(parent: BorrowedFd<'_>, name: &CStr, first: &Piece) -> Result<Made, FileError> {
    match first {
        Piece::Folder(_) => Ok(Made::Folder(new_folder(parent, name)?)),
        Piece::File { .. } => Ok(Made::File(new_file(parent, name)?)),
        Piece::Link { target, .. } => {
            symlinkat(target.as_c_str(), parent, name)?;
            Ok(Made::Link)
        }
        Piece::Leave | Piece::Done => Err(out_of_order()),
    }
}

impl ShareRoot {
    /// Sends `from`, with everything in it, as the calling thread may read
    /// it, ending with [`Piece::Done`]. Says what it sent, for a move.
    pub fn send_entry(&self, from: &SharePath, out: &SyncSender<Piece>) -> Result<Sent, FileError> {
        let (parent, name) = self.parent(from)?;
        let name = c_name(name)?;
        let (device, inode) = identity(&statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW)?);
        send(parent.as_fd(), &name, self.file_system, 0, out)?;
        put(out, Piece::Done)?;
        Ok(Sent(device, inode))
    }

    /// Makes `to` of what `pieces` brings, with the calling thread's
    /// credentials. If the name is taken, beside it or not at all
    /// (`taken`). If the pieces stop before [`Piece::Done`] or anything
    /// fails, what was made goes again. Returns where the copy went.
    pub fn receive_entry(
        &self,
        to: &SharePath,
        pieces: &Receiver<Piece>,
        taken: Taken,
    ) -> Result<SharePath, FileError> {
        let (parent, to_name) = self.parent(to)?;
        let directory = to.parent().unwrap_or_default();
        let first = next(pieces)?;
        if matches!(first, Piece::Done) {
            return Ok(to.clone()); // nothing that is copied: a pipe, say
        }
        let tries = if taken == Taken::Beside { MAX_NUMBERED } else { 1 };
        for number in 1..=tries {
            let name = if number == 1 { to_name.to_owned() } else { numbered(to_name, number) };
            if !is_file_name(&name) {
                return Err(FileError::Invalid("the name is too long"));
            }
            let c = c_name(&name)?;
            let made = match make_top(parent.as_fd(), &c, &first) {
                Ok(made) => made,
                Err(FileError::Exists) => continue,
                Err(error) => return Err(error),
            };
            let stat = statat(&parent, &c, AtFlags::SYMLINK_NOFOLLOW)?;
            let filled = match (made, first) {
                (Made::Folder(folder), Piece::Folder(_)) => fill(folder.as_fd(), pieces, 1),
                (Made::File(file), Piece::File { file: from, times, .. }) => write_file(from, file, &times),
                (Made::Link, Piece::Link { .. }) => Ok(()),
                _ => Err(out_of_order()),
            }
            .and_then(|()| match next(pieces)? {
                Piece::Done => Ok(()),
                _ => Err(out_of_order()),
            });
            if let Err(error) = filled {
                // Only what was made here, whatever has the name by now.
                let _ = remove_entry(parent.as_fd(), &c, device(&stat), 0, Some(identity(&stat)));
                return Err(nested(error));
            }
            return directory.join(&name).map_err(|_| FileError::Invalid("the path is too long"));
        }
        Err(FileError::Exists)
    }

    /// Removes `from` if it still is what was sent: a move across
    /// credentials, once its copy is complete.
    pub fn remove_sent(&self, from: &SharePath, sent: Sent) -> Result<(), FileError> {
        let (parent, name) = self.parent(from)?;
        remove_entry(parent.as_fd(), &c_name(name)?, self.file_system, 0, Some((sent.0, sent.1)))
    }
}

/// What a copy across credentials came to, from what each side says: the
/// side that failed first tells why, not the other one that only saw it
/// stop.
pub fn settle(
    read: Result<Sent, FileError>,
    written: Result<SharePath, FileError>,
) -> Result<(SharePath, Sent), FileError> {
    let stopped = |error: &FileError| {
        matches!(error, FileError::Invalid("the copy's target stopped" | "the copy's source stopped"))
    };
    match (read, written) {
        (Ok(sent), Ok(path)) => Ok((path, sent)),
        (Err(read), Err(written)) => Err(if stopped(&read) { written } else { read }),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Copies `from` in `source` to `to` in `target` the way it goes across
/// credentials, both sides on this thread's: for tests and the simulation.
pub fn copy_through(
    source: &ShareRoot,
    from: &SharePath,
    target: &ShareRoot,
    to: &SharePath,
    taken: Taken,
) -> Result<(SharePath, Sent), FileError> {
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    std::thread::scope(|scope| {
        let reading = scope.spawn(move || source.send_entry(from, &out));
        let written = target.receive_entry(to, &pieces, taken);
        drop(pieces);
        let read = reading.join().unwrap_or(Err(FileError::Invalid("the copy's source stopped")));
        settle(read, written)
    })
}
