//! What a copy for a move copied, entry by entry, and removing exactly that
//! from the source once the copy is complete and written out.
//!
//! The copy records each entry it copied as it goes: its name, device and
//! inode, kind and change time, for a file also its modification time and
//! size, for a link its target, all from the descriptor it copied through,
//! taken before it read. Removing walks that record, never the folders
//! themselves: what is missing by then is passed by, what changed since
//! (another entry under the name, a file written to, a new entry in a
//! folder) stays and is counted, and only what still is what was copied
//! goes. FAT keeps times in steps of two seconds, which is why the size is
//! compared too.
//!
//! Device and inode alone do not tell an entry: ext4 gives a new entry the
//! number of one just removed, and ZFS reuses object numbers too. A file or
//! link put in a copied one's place gets a change time of its own (and a
//! link its own target), so it is told apart by those. A folder's change
//! time moves whenever something in it comes, goes or is renamed, also when
//! the move removes what it copied; so it is compared before that: a folder
//! that changed since it was copied, another empty one in its place and
//! number among them, keeps its place once what in it is still as copied
//! went.
//!
//! The record has a budget, like an archive's plan: a move of more entries
//! than that keeps its copy and its source whole and says so.

use std::collections::HashSet;
use std::ffi::CStr;
use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::{AtFlags, FileType, OFlags, Stat, fstat, statat, unlinkat};
use rustix::io::Errno;
use zendiloos_agent_proto::SharePath;

use super::{FileError, ShareRoot, c_name, identity, kind, open_beneath_flags, open_link};

/// The most entries one move records; a move of more keeps its source. With
/// its names some 50 MB at most: two moves at once and the archives being
/// written stay well within the files agent's memory (`MemoryHigh=384M`).
pub const MAX_MOVED_ENTRIES: usize = 500_000;
/// The most bytes of names and link targets one move records: whoever may
/// write the folder could fill it with long names or links to long targets,
/// and the record must not take the files agent's memory.
pub const MAX_MOVED_NAME_BYTES: usize = 16 * 1024 * 1024;

/// A time as `fstat` tells it, to the nanosecond.
type Time = (i64, u32);

/// Where bytes begin in the record's names, and how many.
type Span = (u32, u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    /// A folder; what was copied of it follows it, up to entry `end`.
    Folder {
        end: u32,
        changed: Time,
    },
    File {
        size: u64,
        changed: Time,
        modified: Time,
    },
    /// A link, and where its target is in the record's names.
    Link {
        changed: Time,
        target: Span,
    },
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Its name in the record's names, with the NUL that ends it.
    name: Span,
    device: u64,
    inode: u64,
    what: What,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing is recorded: a plain copy.
    Off,
    On,
    /// Over the budget: nothing is recorded any more, and the move keeps its
    /// source whole.
    Full,
}

/// What a copy copied, entry by entry, in the order it copied them: each
/// folder before what is in it.
#[derive(Debug)]
pub struct Copied {
    entries: Vec<Entry>,
    /// Names, and the targets of links.
    names: Vec<u8>,
    /// Entries copied, recorded or not.
    count: u64,
    state: State,
    /// The most entries and bytes of names it records.
    budget: (usize, usize),
}

/// What a move left in its source, and one of it as an example.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kept {
    /// Entries that stayed: changed or new since they were copied, or not
    /// to be removed; folders that still hold something among them.
    pub count: u64,
    /// The first of them, a path in the source's root.
    pub example: Option<SharePath>,
    /// More entries than a move records: the copy is complete, and the
    /// source stayed whole.
    pub too_many: bool,
}

impl std::fmt::Display for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let example = self.example.as_ref().map_or("", SharePath::as_str);
        if self.too_many {
            write!(
                f,
                "copied, but {} entries are too many to remove one by one: {example} stayed",
                self.count
            )
        } else {
            write!(f, "{} entries stayed where they were, such as {example}", self.count)
        }
    }
}

/// When the entry last changed: its contents, its name, its owner or mode.
#[allow(clippy::useless_conversion)]
fn changed(stat: &Stat) -> Time {
    (i64::from(stat.st_ctime), u32::try_from(stat.st_ctime_nsec).unwrap_or(0))
}

/// Size, change and modification time of a file.
#[allow(clippy::useless_conversion)]
fn file_facts(stat: &Stat) -> (u64, Time, Time) {
    (
        u64::try_from(stat.st_size).unwrap_or(0),
        changed(stat),
        (i64::from(stat.st_mtime), u32::try_from(stat.st_mtime_nsec).unwrap_or(0)),
    )
}

impl Copied {
    /// A record for a move.
    pub(crate) fn recording() -> Self {
        Self::within(MAX_MOVED_ENTRIES, MAX_MOVED_NAME_BYTES)
    }

    /// A record of at most `entries` entries and `name_bytes` of names.
    pub(crate) fn within(entries: usize, name_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            names: Vec::new(),
            count: 0,
            state: State::On,
            budget: (entries, name_bytes),
        }
    }

    /// No record: a plain copy, after which nothing is removed.
    pub(crate) fn off() -> Self {
        Self { state: State::Off, ..Self::recording() }
    }

    /// Entries copied.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Records an entry, its name, and for a link its `target`, which
    /// `what` then says where to find.
    fn add(&mut self, name: &CStr, stat: &Stat, what: What, target: &[u8]) -> Option<usize> {
        self.count += 1;
        if self.state != State::On {
            return None;
        }
        let bytes = name.to_bytes_with_nul();
        let within = self.entries.len() < self.budget.0
            && self.names.len() + bytes.len() + target.len() <= self.budget.1;
        let start = u32::try_from(self.names.len()).ok();
        let length = u16::try_from(bytes.len()).ok();
        let target_at = start.zip(length).and_then(|(start, length)| start.checked_add(u32::from(length)));
        let (Some(start), Some(length), Some(target_at), Ok(target_length), true) =
            (start, length, target_at, u16::try_from(target.len()), within)
        else {
            self.state = State::Full;
            self.entries = Vec::new();
            self.names = Vec::new();
            return None;
        };
        self.names.extend_from_slice(bytes);
        self.names.extend_from_slice(target);
        let what = match what {
            What::Link { changed, .. } => What::Link { changed, target: (target_at, target_length) },
            other => other,
        };
        let (device, inode) = identity(stat);
        self.entries.push(Entry { name: (start, length), device, inode, what });
        Some(self.entries.len() - 1)
    }

    /// A folder copied, `stat` from the descriptor it was read through,
    /// taken before it was read; what is copied next is in it, up to
    /// [`Copied::leave`].
    pub(crate) fn folder(&mut self, name: &CStr, stat: &Stat) -> Option<usize> {
        self.add(name, stat, What::Folder { end: 0, changed: changed(stat) }, &[])
    }

    /// The folder `index` names (from [`Copied::folder`]) ends.
    pub(crate) fn leave(&mut self, index: Option<usize>) {
        let end = u32::try_from(self.entries.len());
        if let (Some(index), Ok(end)) = (index, end)
            && let Some(Entry { what: What::Folder { end: at, .. }, .. }) = self.entries.get_mut(index)
        {
            *at = end;
        }
    }

    /// A regular file copied, `stat` from the descriptor it was read from,
    /// taken before it was read.
    pub(crate) fn file(&mut self, name: &CStr, stat: &Stat) {
        let (size, changed, modified) = file_facts(stat);
        self.add(name, stat, What::File { size, changed, modified }, &[]);
    }

    /// A symbolic link copied, `stat` from a descriptor of the link itself
    /// and `target` read through it.
    pub(crate) fn link(&mut self, name: &CStr, stat: &Stat, target: &CStr) {
        self.add(name, stat, What::Link { changed: changed(stat), target: (0, 0) }, target.to_bytes());
    }

    fn bytes(&self, (start, length): Span) -> &[u8] {
        let start = start as usize;
        &self.names[start..start + usize::from(length)]
    }

    fn name(&self, index: usize) -> &CStr {
        CStr::from_bytes_with_nul(self.bytes(self.entries[index].name)).unwrap_or(c"")
    }

    /// The entry after `index` and everything in it.
    fn after(&self, index: usize) -> usize {
        match self.entries[index].what {
            What::Folder { end, .. } => (end as usize).max(index + 1),
            _ => index + 1,
        }
    }

    /// As if the file system had given the entries named `name` the device
    /// and inode of `stat`, as ext4 and ZFS give the number of an entry
    /// removed to a new one.
    #[cfg(test)]
    pub(crate) fn renumber(&mut self, name: &CStr, stat: &Stat) {
        for index in 0..self.entries.len() {
            if self.name(index) == name {
                (self.entries[index].device, self.entries[index].inode) = identity(stat);
            }
        }
    }
}

/// A walk of the record, removing what still is what was copied.
struct Removal<'a> {
    copied: &'a Copied,
    /// The top's path in the source's root.
    from: &'a SharePath,
    /// The entries of the record on the way to the one looked at, the top
    /// first.
    way: Vec<usize>,
    kept: Kept,
    /// Files with more than one name that one name of went: the others
    /// changed by that alone (`st_ctime`).
    unlinked: HashSet<(u64, u64)>,
}

impl Removal<'_> {
    /// The entry looked at stays.
    fn keep(&mut self) {
        self.kept.count += 1;
        if self.kept.example.is_none() {
            let mut path = self.from.clone();
            for &index in self.way.iter().skip(1) {
                match path.join(&self.copied.name(index).to_string_lossy()) {
                    Ok(longer) => path = longer,
                    Err(_) => break,
                }
            }
            self.kept.example = Some(path);
        }
    }

    /// Removes entry `index` of the record from `directory` if it still is
    /// what was copied; what is in a folder first, then the folder if that
    /// left it empty.
    fn remove(&mut self, directory: BorrowedFd<'_>, index: usize) {
        self.way.push(index);
        self.remove_one(directory, index);
        self.way.pop();
    }

    fn remove_one(&mut self, directory: BorrowedFd<'_>, index: usize) {
        let entry = self.copied.entries[index];
        let recorded = (entry.device, entry.inode);
        let name = self.copied.name(index);
        let stat = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => return, // gone meanwhile
            Err(_) => return self.keep(),
            Ok(stat) => stat,
        };
        let same = identity(&stat) == recorded;
        let removed = match entry.what {
            What::Folder { changed: then, .. } => {
                if !same || kind(&stat) != FileType::Directory {
                    return self.keep();
                }
                // Only the folder looked at: whoever may write here could
                // swap it for another meanwhile.
                let inner = match open_beneath_flags(directory, name, OFlags::PATH | OFlags::DIRECTORY) {
                    Err(FileError::NotFound) => return,
                    Err(_) => return self.keep(),
                    Ok(inner) => inner,
                };
                let Ok(opened) = fstat(&inner) else { return self.keep() };
                if identity(&opened) != recorded {
                    return self.keep();
                }
                // Before what is in it goes, which changes it too.
                let unchanged = changed(&opened) == then;
                let mut child = index + 1;
                while child < self.copied.after(index) {
                    self.remove(inner.as_fd(), child);
                    child = self.copied.after(child);
                }
                if !unchanged {
                    return self.keep();
                }
                unlinkat(directory, name, AtFlags::REMOVEDIR)
            }
            What::File { size, changed, modified } => {
                let (now_size, now_changed, now_modified) = file_facts(&stat);
                // Another name of it went: that alone changed it.
                let by_us = self.unlinked.contains(&recorded);
                let unchanged = same
                    && kind(&stat) == FileType::RegularFile
                    && now_size == size
                    && now_modified == modified
                    && (now_changed == changed || by_us);
                if !unchanged {
                    return self.keep();
                }
                if stat.st_nlink > 1 {
                    self.unlinked.insert(recorded);
                }
                unlinkat(directory, name, AtFlags::empty())
            }
            What::Link { changed: then, target } => {
                // The link itself and what it points to, of the one link
                // that has the name now.
                let (opened, now_target) = match open_link(directory, name) {
                    Err(FileError::NotFound) => return,
                    Ok(Some(link)) => link,
                    Ok(None) | Err(_) => return self.keep(),
                };
                let unchanged = identity(&opened) == recorded
                    && changed(&opened) == then
                    && now_target.as_bytes() == self.copied.bytes(target);
                if !unchanged {
                    return self.keep();
                }
                unlinkat(directory, name, AtFlags::empty())
            }
        };
        match removed {
            Ok(()) | Err(Errno::NOENT) => {}
            // Something not copied is in the folder, or the entry may not be
            // removed: it stays, with what is in it.
            Err(_) => self.keep(),
        }
    }
}

impl ShareRoot {
    /// Removes from `from` exactly what `copied` says was copied of it, once
    /// its copy is complete and written out; anything else stays (see the
    /// module's description). A move that left something reports what as
    /// [`FileError::PartlyMoved`]: it is never a plain success.
    pub fn remove_copied(&self, from: &SharePath, copied: &Copied) -> Result<(), FileError> {
        let (parent, name) = self.parent(from)?;
        let partly =
            |count, too_many| FileError::PartlyMoved(Kept { count, example: Some(from.clone()), too_many });
        match copied.state {
            State::On => {}
            State::Off => return Err(FileError::Invalid("nothing was recorded to remove")),
            State::Full => return Err(partly(copied.count, true)),
        }
        // Nothing of it could be copied (a pipe, say): it stays.
        if copied.entries.is_empty() {
            return Err(partly(1, false));
        }
        if copied.name(0) != c_name(name)?.as_c_str() {
            return Err(FileError::Invalid("the record is of another entry"));
        }
        let mut removal =
            Removal { copied, from, way: Vec::new(), kept: Kept::default(), unlinked: HashSet::new() };
        removal.remove(parent.as_fd(), 0);
        if removal.kept.count > 0 {
            return Err(FileError::PartlyMoved(removal.kept));
        }
        Ok(())
    }
}
