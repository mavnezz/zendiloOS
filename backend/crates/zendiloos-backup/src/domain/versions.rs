//! How a task's backup looks on its target, and what is kept of it.
//!
//! A task writes into one folder of its target, `<target>/<folder>`, named
//! after the task when it was made, marked by a file [`MARKER`]:
//!
//! ```text
//! <folder>/.zendiloos-backup                 the marker: this is a backup of zendiloOS's
//! <folder>/current/                          mirror: the copy
//! <folder>/removed/2026-11-11_030000Z/       mirror, if kept: what that run deleted or replaced
//! <folder>/2026-11-11_030000Z/               versioned: a complete version
//! <folder>/2026-11-12_030000Z.incomplete/    versioned: a run under way, or cut off
//! ```
//!
//! A version is named after the time its run started, in UTC (the names
//! sort as the times do, and none comes twice when the clock goes back an
//! hour in autumn). It is written under its name with [`INCOMPLETE`] and
//! takes its name only once the run is complete, so a run cut off never
//! leaves a version that looks complete: its folder keeps the suffix, and
//! the next run goes on from it. A target folder that holds anything but
//! has no marker is not written: it is not a backup, and a mirror would
//! delete what is there.

use zendiloos_kernel::{Timestamp, UtcTime};

/// The file that marks a task's folder as a backup of zendiloOS's.
pub const MARKER: &str = ".zendiloos-backup";
/// A mirror's copy.
pub const CURRENT: &str = "current";
/// Where a mirror keeps what a run deleted or replaced, by run.
pub const REMOVED: &str = "removed";
/// What a version's name carries until its run is complete.
pub const INCOMPLETE: &str = ".incomplete";
const DAY: i64 = 24 * 3600;

/// `2026-11-11_030000Z`: when a run started, in UTC.
pub fn version_name(at: Timestamp) -> String {
    let t = at.utc();
    format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}Z", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// The time a version's name stands for; `None` for any other name.
pub fn parse_version(name: &str) -> Option<Timestamp> {
    let b = name.as_bytes();
    if b.len() != 18 || b[4] != b'-' || b[7] != b'-' || b[10] != b'_' || b[17] != b'Z' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = name.get(range)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok()).flatten()
    };
    let narrow = |value: i64| u8::try_from(value).ok();
    UtcTime {
        year: digits(0..4)?,
        month: narrow(digits(5..7)?)?,
        day: narrow(digits(8..10)?)?,
        hour: narrow(digits(11..13)?)?,
        minute: narrow(digits(13..15)?)?,
        second: narrow(digits(15..17)?)?,
    }
    .timestamp()
}

/// What an entry of a listing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    File,
    Link,
    Other,
}

/// One entry of a folder, as rsync lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub modified: Option<Timestamp>,
}

/// A task's folder on its target, as listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetState {
    /// The marker is there.
    pub marked: bool,
    /// Complete versions, oldest first.
    pub versions: Vec<Timestamp>,
    /// Folders of versions not complete, by name.
    pub incomplete: Vec<String>,
    /// A mirror's copy is there.
    pub current: bool,
    /// A mirror's kept changes, oldest first.
    pub removed: Vec<Timestamp>,
    /// Anything else is there.
    pub other: bool,
}

impl TargetState {
    /// The folder as `entries` (its listing) show it.
    pub fn of(entries: &[Entry]) -> Self {
        let mut state = Self::default();
        for entry in entries {
            let folder = entry.kind == EntryKind::Folder;
            match entry.name.as_str() {
                "." => {}
                MARKER if entry.kind == EntryKind::File => state.marked = true,
                CURRENT if folder => state.current = true,
                REMOVED if folder => {}
                name if folder && parse_version(name).is_some() => {
                    state.versions.extend(parse_version(name));
                }
                name if folder && name.strip_suffix(INCOMPLETE).and_then(parse_version).is_some() => {
                    state.incomplete.push(name.to_owned());
                }
                _ => state.other = true,
            }
        }
        state.versions.sort();
        state.incomplete.sort();
        state
    }

    /// Whether the folder may be written as a task's backup: it is marked
    /// as one, or nothing is in it.
    pub fn writable(&self) -> bool {
        self.marked || *self == Self::default()
    }

    /// The newest complete version.
    pub fn latest(&self) -> Option<Timestamp> {
        self.versions.last().copied()
    }
}

/// The versions retention removes, oldest first: all but the newest
/// `keep_last` and those younger than `keep_days` days at `now` (a version
/// either keeps stays; neither set keeps all). The newest is never
/// removed, whatever the clock says.
pub fn beyond_retention(
    versions: &[Timestamp],
    keep_last: Option<u16>,
    keep_days: Option<u16>,
    now: Timestamp,
) -> Vec<Timestamp> {
    if keep_last.is_none() && keep_days.is_none() {
        return Vec::new();
    }
    let mut sorted = versions.to_vec();
    sorted.sort();
    sorted.dedup();
    let count = sorted.len();
    sorted
        .iter()
        .enumerate()
        .filter(|(index, version)| {
            let newest = index + 1 == count;
            let by_number = keep_last.is_some_and(|keep| count - index <= usize::from(keep));
            let by_age = keep_days
                .is_some_and(|days| now.unix_seconds() - version.unix_seconds() < i64::from(days) * DAY);
            !(newest || by_number || by_age)
        })
        .map(|(_, version)| *version)
        .collect()
}

/// A mirror's kept changes older than `days` at `now`.
pub fn removed_beyond(removed: &[Timestamp], days: u16, now: Timestamp) -> Vec<Timestamp> {
    removed
        .iter()
        .copied()
        .filter(|at| now.unix_seconds() - at.unix_seconds() >= i64::from(days) * DAY)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    #[test]
    fn version_names_are_utc_times_and_back() {
        let at = t(1_794_381_600); // 2026-11-11 07:20:00 UTC
        assert_eq!(version_name(at), "2026-11-11_072000Z");
        assert_eq!(parse_version("2026-11-11_072000Z"), Some(at));
        for other in [
            "2026-11-11_082000",
            "2026-13-11_082000Z",
            "2026-11-11_082000Z.incomplete",
            "current",
            "2026-11-1a_082000Z",
        ] {
            assert_eq!(parse_version(other), None, "{other}");
        }
        // Their order is the times'.
        assert!(version_name(t(100)) < version_name(t(3700)));
    }

    fn folder(name: &str) -> Entry {
        Entry { name: name.into(), kind: EntryKind::Folder, bytes: 0, modified: None }
    }

    #[test]
    fn a_target_folder_is_read_from_its_listing() {
        let marker = Entry { name: MARKER.into(), kind: EntryKind::File, bytes: 2, modified: None };
        let state = TargetState::of(&[
            folder("."),
            marker,
            folder("2026-11-11_030000Z"),
            folder("2026-11-10_030000Z"),
            folder("2026-11-12_030000Z.incomplete"),
            folder("current"),
            folder("removed"),
        ]);
        assert!(state.marked && state.current && !state.other);
        assert_eq!(
            state.versions,
            [parse_version("2026-11-10_030000Z").unwrap(), parse_version("2026-11-11_030000Z").unwrap()]
        );
        assert_eq!(state.incomplete, ["2026-11-12_030000Z.incomplete"]);
        assert_eq!(state.latest(), parse_version("2026-11-11_030000Z"));
        assert!(state.writable());

        assert!(TargetState::of(&[folder(".")]).writable(), "empty");
        assert!(TargetState::of(&[]).writable(), "not there yet");
        let foreign = TargetState::of(&[folder("."), folder("Holiday 2019")]);
        assert!(foreign.other && !foreign.writable(), "someone else's folder");
        // A file named like a version is not one.
        let file =
            Entry { name: "2026-11-11_030000Z".into(), kind: EntryKind::File, bytes: 1, modified: None };
        assert!(!TargetState::of(&[file]).writable());
    }

    #[test]
    fn retention_keeps_by_number_and_age_and_always_the_newest() {
        let day = 24 * 3600;
        let versions: Vec<Timestamp> = (0..10).map(|n| t(n * day)).collect();
        let now = t(9 * day + 3600);
        assert_eq!(beyond_retention(&versions, Some(3), None, now), versions[..7]);
        // Younger than three days: days 7, 8 and 9.
        assert_eq!(beyond_retention(&versions, None, Some(3), now), versions[..7]);
        // Either keeps it: the last two, or younger than five days.
        assert_eq!(beyond_retention(&versions, Some(2), Some(5), now), versions[..5]);
        assert!(beyond_retention(&versions, None, None, now).is_empty(), "keeps all");
        // The newest stays even when every version is older than kept.
        assert_eq!(beyond_retention(&versions, None, Some(1), t(100 * day)), versions[..9]);
        assert!(beyond_retention(&[], Some(1), None, now).is_empty());
    }

    #[test]
    fn kept_changes_go_after_their_days() {
        let day = 24 * 3600;
        let removed = [t(0), t(5 * day), t(9 * day)];
        assert_eq!(removed_beyond(&removed, 7, t(12 * day)), [t(0), t(5 * day)]);
        assert!(removed_beyond(&removed, 30, t(12 * day)).is_empty());
    }
}
