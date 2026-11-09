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
//! compared too. A folder that stays without being looked into is counted
//! with everything of it the record holds; so is the moved entry itself
//! when it is gone from its place by then: renamed or removed meanwhile,
//! nothing of it was moved.
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
//! FAT and exFAT have no inode numbers: the kernel numbers an entry anew
//! (`iunique()`) whenever it forgot it and reads it again, which a big copy
//! makes it do, and shows its modification time as its change time once
//! it read it again. On those ([`Copied::on_fat`]) an entry is told by its
//! name, kind, size and modification time alone.
//!
//! A move tried again after it left part of its source finds its earlier
//! copy in the target: it goes on into a folder of the name there, adding
//! what is missing. A file the target has under the same name already
//! counts as moved if it is the same, byte for byte; otherwise the source's
//! stays ([`Copied::stays`]), and the target's is never touched.
//!
//! The record has a budget, like an archive's plan: a move of more entries
//! than that keeps its copy and its source whole and says so.

use std::collections::HashSet;
use std::ffi::CStr;
use std::fs::File;
use std::io::Read;
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

/// `statfs` types (`f_type`) of file systems without inode numbers of their
/// own: FAT (`MSDOS_SUPER_MAGIC`, also vfat) and exFAT.
const FAT_TYPES: [i64; 2] = [0x4d44, 0x2011_bab0];

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

/// What of an entry stays in the source whatever it is by then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stays {
    /// Nothing: it goes if it is still what was copied.
    No,
    /// It and everything of it: the target has another under its name.
    All,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Its name in the record's names, with the NUL that ends it.
    name: Span,
    device: u64,
    inode: u64,
    what: What,
    stays: Stays,
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
    /// The source is FAT or exFAT: entries are told without inode numbers
    /// and change times.
    fat: bool,
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

/// When the entry's contents last changed, as far as its times say.
#[allow(clippy::useless_conversion)]
fn modified(stat: &Stat) -> Time {
    (i64::from(stat.st_mtime), u32::try_from(stat.st_mtime_nsec).unwrap_or(0))
}

/// Whether a file system of `statfs` type `f_type` numbers its entries anew
/// whenever the kernel reads them again: FAT and exFAT.
pub(crate) fn is_fat(f_type: i64) -> bool {
    FAT_TYPES.contains(&f_type)
}

/// Whether `directory` is on FAT or exFAT.
#[allow(clippy::useless_conversion)]
pub(crate) fn is_fat_at(directory: BorrowedFd<'_>) -> bool {
    rustix::fs::fstatfs(directory).is_ok_and(|fs| is_fat(i64::from(fs.f_type)))
}

/// Whether `a` and `b` hold the same bytes, read from where each is now.
fn same_bytes(a: &File, b: &File) -> bool {
    let (mut a, mut b) = (a, b);
    let mut left = vec![0u8; 64 * 1024];
    let mut right = vec![0u8; 64 * 1024];
    loop {
        let read = match a.read(&mut left) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        };
        if read == 0 {
            // Both at their end.
            return matches!(b.read(&mut right[..1]), Ok(0));
        }
        if b.read_exact(&mut right[..read]).is_err() || left[..read] != right[..read] {
            return false;
        }
    }
}

/// Whether the regular file `name` in `directory` holds what `file` holds:
/// for a move gone on into its earlier copy, which has it already then.
/// Anything that cannot be read is not the same.
pub(crate) fn same_file(file: &File, directory: BorrowedFd<'_>, name: &CStr) -> bool {
    let Ok(other) = open_beneath_flags(directory, name, OFlags::RDONLY | OFlags::NONBLOCK) else {
        return false;
    };
    let (Ok(mine), Ok(theirs)) = (fstat(file), fstat(&other)) else { return false };
    kind(&theirs) == FileType::RegularFile
        && mine.st_size == theirs.st_size
        && same_bytes(file, &File::from(other))
}

/// Whether the link `name` in `directory` points to `target`.
pub(crate) fn same_link(directory: BorrowedFd<'_>, name: &CStr, target: &CStr) -> bool {
    matches!(open_link(directory, name), Ok(Some((_, now))) if now.as_c_str() == target)
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
            fat: false,
        }
    }

    /// No record: a plain copy, after which nothing is removed.
    pub(crate) fn off() -> Self {
        Self { state: State::Off, ..Self::recording() }
    }

    /// The record of a source on FAT or exFAT (`fat`): entries are told
    /// without inode numbers and change times.
    pub(crate) fn on_fat(self, fat: bool) -> Self {
        Self { fat, ..self }
    }

    /// Entries copied.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// For a move: a name taken in a folder that was there before is gone
    /// on with, not refused.
    pub(crate) fn moving(&self) -> bool {
        self.state != State::Off
    }

    /// Whether `stat` is of the entry recorded as `recorded` (device and
    /// inode), as far as its number tells: on FAT and exFAT it does not.
    fn is(&self, recorded: (u64, u64), stat: &Stat) -> bool {
        let (device, inode) = identity(stat);
        device == recorded.0 && (self.fat || inode == recorded.1)
    }

    /// When the entry last changed, as far as its file system keeps it.
    fn changed(&self, stat: &Stat) -> Time {
        if self.fat { modified(stat) } else { changed(stat) }
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
        self.entries.push(Entry { name: (start, length), device, inode, what, stays: Stays::No });
        Some(self.entries.len() - 1)
    }

    /// A folder copied, `stat` from the descriptor it was read through,
    /// taken before it was read; what is copied next is in it, up to
    /// [`Copied::leave`].
    pub(crate) fn folder(&mut self, name: &CStr, stat: &Stat) -> Option<usize> {
        let changed = self.changed(stat);
        self.add(name, stat, What::Folder { end: 0, changed }, &[])
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
    pub(crate) fn file(&mut self, name: &CStr, stat: &Stat) -> Option<usize> {
        let (size, changed, modified) =
            (u64::try_from(stat.st_size).unwrap_or(0), self.changed(stat), modified(stat));
        self.add(name, stat, What::File { size, changed, modified }, &[])
    }

    /// A symbolic link copied, `stat` from a descriptor of the link itself
    /// and `target` read through it.
    pub(crate) fn link(&mut self, name: &CStr, stat: &Stat, target: &CStr) -> Option<usize> {
        let changed = self.changed(stat);
        self.add(name, stat, What::Link { changed, target: (0, 0) }, target.to_bytes())
    }

    /// The entry `index` names (from [`Copied::folder`], [`Copied::file`]
    /// or [`Copied::link`]) stays in the source as `stays` says.
    pub(crate) fn stays(&mut self, index: Option<usize>, stays: Stays) {
        if let Some(entry) = index.and_then(|index| self.entries.get_mut(index)) {
            entry.stays = stays;
        }
    }

    /// The entries the target had others under the name of stay, with
    /// everything of them: `pieces` counts what was sent, folders, files
    /// and links, from 0, as the record does.
    pub(crate) fn stay_where_taken(&mut self, pieces: &[usize]) {
        for &piece in pieces {
            self.stays(Some(piece), Stays::All);
        }
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

    /// As if the kernel had forgotten every entry and numbered it anew when
    /// it read it again, as FAT and exFAT do (`iunique()`).
    #[cfg(test)]
    pub(crate) fn number_anew(&mut self) {
        for entry in &mut self.entries {
            entry.inode = entry.inode.wrapping_add(1_000_003);
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
    /// The entry looked at stays, by itself.
    fn keep(&mut self) {
        self.keep_counting(1);
    }

    /// The entry looked at stays, with everything of it the record holds,
    /// not looked into.
    fn keep_all(&mut self, index: usize) {
        let count = self.copied.after(index) - index;
        self.keep_counting(u64::try_from(count).unwrap_or(u64::MAX));
    }

    fn keep_counting(&mut self, count: u64) {
        self.kept.count += count;
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
        if entry.stays == Stays::All {
            return self.keep_all(index);
        }
        let recorded = (entry.device, entry.inode);
        let name = self.copied.name(index);
        let stat = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            // The moved entry itself, renamed or removed meanwhile: it was
            // not moved, wherever it is.
            Err(Errno::NOENT) if index == 0 => return self.keep_all(index),
            Err(Errno::NOENT) => return, // gone meanwhile
            Err(_) => return self.keep_all(index),
            Ok(stat) => stat,
        };
        let same = self.copied.is(recorded, &stat);
        let removed = match entry.what {
            What::Folder { changed: then, .. } => {
                if !same || kind(&stat) != FileType::Directory {
                    return self.keep_all(index);
                }
                // Only the folder looked at: whoever may write here could
                // swap it for another meanwhile.
                let inner = match open_beneath_flags(directory, name, OFlags::PATH | OFlags::DIRECTORY) {
                    Err(FileError::NotFound) if index > 0 => return,
                    Err(_) => return self.keep_all(index),
                    Ok(inner) => inner,
                };
                let Ok(opened) = fstat(&inner) else { return self.keep_all(index) };
                if !self.copied.is(recorded, &opened) {
                    return self.keep_all(index);
                }
                // Before what is in it goes, which changes it too.
                let unchanged = self.copied.changed(&opened) == then;
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
            What::File { size, changed, modified: then } => {
                // Another name of it went: that alone changed it.
                let by_us = self.unlinked.contains(&recorded);
                let unchanged = same
                    && kind(&stat) == FileType::RegularFile
                    && u64::try_from(stat.st_size).unwrap_or(0) == size
                    && modified(&stat) == then
                    && (self.copied.changed(&stat) == changed || by_us);
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
                    Err(FileError::NotFound) if index > 0 => return,
                    Ok(Some(link)) => link,
                    Ok(None) | Err(_) => return self.keep(),
                };
                let unchanged = self.copied.is(recorded, &opened)
                    && self.copied.changed(&opened) == then
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat_and_exfat_are_told_by_their_statfs_type() {
        // MSDOS_SUPER_MAGIC (vfat and msdos), EXFAT_SUPER_MAGIC.
        assert!(is_fat(0x4d44) && is_fat(0x2011_bab0));
        // ext4, tmpfs, ZFS, ntfs3: inode numbers of their own.
        for other in [0xef53, 0x0102_1994, 0x2fc1_2fc1, 0x7366_746e] {
            assert!(!is_fat(other), "{other:#x}");
        }
    }

    #[test]
    fn on_fat_neither_inode_numbers_nor_change_times_tell_an_entry() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.jpg"), "a").unwrap();
        let stat = rustix::fs::stat(directory.path().join("a.jpg")).unwrap();
        // As the kernel shows it after it read it again: another number,
        // the modification time as the change time.
        let mut again = stat;
        again.st_ino = stat.st_ino.wrapping_add(7);
        again.st_ctime = stat.st_mtime + 3;
        let (fat, other) = (Copied::recording().on_fat(true), Copied::recording());
        let recorded = identity(&stat);
        assert!(fat.is(recorded, &again) && fat.changed(&again) == fat.changed(&stat));
        assert!(!other.is(recorded, &again) && other.changed(&again) != other.changed(&stat));
        // Another device is another entry anywhere.
        again.st_dev = stat.st_dev.wrapping_add(1);
        assert!(!fat.is(recorded, &again));
    }
}
