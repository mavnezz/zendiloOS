//! Copies between roots that are worked on with different credentials: a
//! removable drive, read and written with root's access to files, and a
//! folder on a volume, read and written as the person. One thread reads
//! the source with its credentials and sends what it finds, piece by piece
//! and in order, files already opened; another creates the copy with its
//! own: so what lands in a folder on a volume belongs to the person and
//! takes the folder's inherited ACL, exactly as an upload does, and only
//! what the person may read there reaches a drive. The source walks as a
//! copy does (by descriptor, no link followed, no other file system
//! entered, uploads under way left out), and the target is filled as a
//! copy is: only a folder made empty is filled, and what was made goes
//! again when anything fails. The source's side records what it sent
//! ([`Copied`]), so a move removes only that.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::mpsc::{Receiver, SyncSender};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, Timestamps, fstat, futimens, mkdirat, openat, statat, symlinkat,
    unlinkat,
};
use zendiloos_agent_proto::{SharePath, is_file_name};

use super::{
    Copied, FileError, MAX_DEPTH, MAX_NUMBERED, ShareRoot, c_name, clash, device, identity, kind, nested,
    numbered, open_beneath_flags, open_link, remove_entry, some_names, timestamps, upload_under_way,
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

/// Sends `name` in `source`, and what is in it, recording in `copied` each
/// entry as the descriptor sent or read through says it was.
fn send(
    source: BorrowedFd<'_>,
    name: &CStr,
    file_system: u64,
    depth: u32,
    out: &SyncSender<Piece>,
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
            let folder = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            let opened = fstat(&folder)?;
            if device(&opened) != file_system {
                return Ok(()); // mounted on meanwhile
            }
            put(out, Piece::Folder(name.to_owned()))?;
            let index = copied.folder(name, &opened);
            for entry in Dir::read_from(&folder)? {
                let entry = entry?;
                let child = entry.file_name();
                if child != c"." && child != c".." {
                    send(folder.as_fd(), child, file_system, depth + 1, out, copied)?;
                }
            }
            put(out, Piece::Leave)?;
            copied.leave(index);
        }
        FileType::RegularFile => {
            let file = open_beneath_flags(source, name, OFlags::RDONLY | OFlags::NONBLOCK)?;
            // Before the target reads it: whatever changes it from now on,
            // the move sees.
            let opened = fstat(&file)?;
            if kind(&opened) != FileType::RegularFile {
                return Ok(()); // something else in its place meanwhile
            }
            copied.file(name, &opened);
            put(out, Piece::File { name: name.to_owned(), file, times: timestamps(&opened) })?;
        }
        FileType::Symlink => {
            let Some((opened, target)) = open_link(source, name)? else { return Ok(()) };
            copied.link(name, &opened);
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

/// Why making `name` in `directory`, a folder of the copy, failed.
fn made_not(directory: BorrowedFd<'_>, name: &CStr, error: FileError) -> FileError {
    match error {
        FileError::Exists => clash(directory, name),
        other => nested(other),
    }
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
                let folder = new_folder(directory, &name).map_err(|e| made_not(directory, &name, e))?;
                fill(folder.as_fd(), pieces, depth + 1)?;
            }
            Piece::File { name, file, times } => {
                let to = new_file(directory, &name).map_err(|e| made_not(directory, &name, e.into()))?;
                write_file(file, to, &times)?;
            }
            Piece::Link { name, target } => {
                symlinkat(target.as_c_str(), directory, &name)
                    .map_err(|e| made_not(directory, &name, e.into()))?;
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
    /// it, ending with [`Piece::Done`]. Says what it sent: for a move
    /// (`record`) entry by entry, for [`ShareRoot::remove_copied`].
    pub fn send_entry(
        &self,
        from: &SharePath,
        out: &SyncSender<Piece>,
        record: bool,
    ) -> Result<Copied, FileError> {
        let (parent, name) = self.parent(from)?;
        let name = c_name(name)?;
        let mut copied = if record { Copied::recording() } else { Copied::off() };
        send(parent.as_fd(), &name, self.file_system, 0, out, &mut copied)?;
        put(out, Piece::Done)?;
        Ok(copied)
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
            // What was made, from its descriptor: whoever may write the
            // folder could swap another entry in under the name meanwhile,
            // which the user's rights would then remove.
            let opened = match &made {
                Made::Folder(made) | Made::File(made) => Some(fstat(made)?),
                Made::Link => None,
            };
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
                // Only what was made here, whatever has the name by now; a
                // link only while a link has the name.
                match opened {
                    Some(made) => {
                        let _ = remove_entry(parent.as_fd(), &c, device(&made), 0, Some(identity(&made)));
                    }
                    None => {
                        let link = statat(&parent, &c, AtFlags::SYMLINK_NOFOLLOW);
                        if link.is_ok_and(|link| kind(&link) == FileType::Symlink) {
                            let _ = unlinkat(&parent, &c, AtFlags::empty());
                        }
                    }
                }
                return Err(nested(error));
            }
            return directory.join(&name).map_err(|_| FileError::Invalid("the path is too long"));
        }
        Err(FileError::Exists)
    }
}

/// What a copy across credentials came to, from what each side says: the
/// side that failed first tells why, not the other one that only saw it
/// stop.
pub fn settle(
    read: Result<Copied, FileError>,
    written: Result<SharePath, FileError>,
) -> Result<(SharePath, Copied), FileError> {
    let stopped = |error: &FileError| {
        matches!(error, FileError::Invalid("the copy's target stopped" | "the copy's source stopped"))
    };
    match (read, written) {
        (Ok(copied), Ok(path)) => Ok((path, copied)),
        (Err(read), Err(written)) => Err(if stopped(&read) { written } else { read }),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Copies `from` in `source` to `to` in `target` the way it goes across
/// credentials, both sides on this thread's: for tests and the simulation.
/// With `record`, says what it copied entry by entry, as for a move.
pub fn copy_through(
    source: &ShareRoot,
    from: &SharePath,
    target: &ShareRoot,
    to: &SharePath,
    taken: Taken,
    record: bool,
) -> Result<(SharePath, Copied), FileError> {
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    std::thread::scope(|scope| {
        let reading = scope.spawn(move || source.send_entry(from, &out, record));
        let written = target.receive_entry(to, &pieces, taken);
        drop(pieces);
        let read = reading.join().unwrap_or(Err(FileError::Invalid("the copy's source stopped")));
        settle(read, written)
    })
}
