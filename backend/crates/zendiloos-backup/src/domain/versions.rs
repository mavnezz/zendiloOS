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
//! <folder>/2026-11-11_030000Z.partial        versioned: its run could not copy every file
//! <folder>/2026-11-12_030000Z.incomplete/    versioned: a run under way, or cut off
//! <folder>/2026-11-12_030000Z.finishing      versioned: a server is still given the version
//! ```
//!
//! A version is named after the time its run started, in UTC (the names
//! sort as the times do, and none comes twice when the clock goes back an
//! hour in autumn). It is written under its name with [`INCOMPLETE`] and
//! takes its name only once the run is complete, so a run cut off never
//! leaves a version that looks complete: its folder keeps the suffix, and
//! the next run goes on from it. On the NAS and through rclone the name is
//! given by a rename; a server reached with rsync cannot rename, so the
//! version is copied once more under its name, and a file [`FINISHING`]
//! beside it says so until that copy is complete. A run that ended with
//! files it could not copy leaves a file [`PARTIAL`] beside its version
//! (written before the version takes its name). A target folder that holds
//! anything but has no marker is not written: it is not a backup, and a
//! mirror would delete what is there.

use zendiloos_kernel::{Timestamp, UtcTime};

/// The file that marks a task's folder as a backup of zendiloOS's.
pub const MARKER: &str = ".zendiloos-backup";
/// A mirror's copy.
pub const CURRENT: &str = "current";
/// Where a mirror keeps what a run deleted or replaced, by run.
pub const REMOVED: &str = "removed";
/// What a version's name carries until its run is complete.
pub const INCOMPLETE: &str = ".incomplete";
/// A file `<version>.partial`: the run that made the version could not copy
/// every file (a version from the agent's own words, rsync's exit 23).
pub const PARTIAL: &str = ".partial";
/// A file `<version>.finishing`: a server is still given the version under
/// its name; until the file goes, the version is not complete.
pub const FINISHING: &str = ".finishing";
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
    /// Of those, the versions whose run could not copy every file.
    pub partial: Vec<Timestamp>,
    /// What versions not complete left, by name, sorted: their folders
    /// (cut off, or not yet given their name on a server) and the files
    /// marking them; retention removes all of it.
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
        let (mut partial, mut finishing) = (Vec::new(), Vec::new());
        let marking = |name: &str, suffix: &str| name.strip_suffix(suffix).and_then(parse_version);
        for entry in entries {
            let folder = entry.kind == EntryKind::Folder;
            let file = entry.kind == EntryKind::File;
            match entry.name.as_str() {
                "." => {}
                MARKER if file => state.marked = true,
                CURRENT if folder => state.current = true,
                REMOVED if folder => {}
                name if folder && parse_version(name).is_some() => {
                    state.versions.extend(parse_version(name));
                }
                name if folder && marking(name, INCOMPLETE).is_some() => {
                    state.incomplete.push(name.to_owned());
                }
                name if file && marking(name, PARTIAL).is_some() => partial.extend(marking(name, PARTIAL)),
                name if file && marking(name, FINISHING).is_some() => {
                    finishing.extend(marking(name, FINISHING));
                }
                _ => state.other = true,
            }
        }
        // A version a server is still given is not complete; it goes with
        // its marks.
        for version in &finishing {
            if state.versions.contains(version) {
                state.incomplete.push(version_name(*version));
            }
            state.incomplete.push(format!("{}{FINISHING}", version_name(*version)));
        }
        state.versions.retain(|version| !finishing.contains(version));
        // A mark whose version is not there (any more) goes too.
        for version in partial {
            if state.versions.contains(&version) {
                state.partial.push(version);
            } else {
                state.incomplete.push(format!("{}{PARTIAL}", version_name(version)));
            }
        }
        state.versions.sort();
        state.partial.sort();
        state.incomplete.sort();
        state
    }

    /// The names retention removes for `version`: its folder, and the file
    /// marking it partial.
    pub fn names_of(&self, version: Timestamp) -> Vec<String> {
        let name = version_name(version);
        let mut names = vec![name.clone()];
        if self.partial.contains(&version) {
            names.push(format!("{name}{PARTIAL}"));
        }
        names
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
/// either keeps stays; neither set keeps all).
///
/// Whatever the settings, it never removes:
/// - the newest version, nor the newest complete one (not `partial`);
/// - a version dated after `now`, made while the clock was ahead; nor does
///   one count towards `keep_last`.
///
/// A version whose run could not copy every file (`partial`) never pushes
/// complete ones out: `keep_last` counts complete versions alone for them;
/// a partial version stays while it is one of the newest `keep_last` of
/// all.
pub fn beyond_retention(
    versions: &[Timestamp],
    partial: &[Timestamp],
    keep_last: Option<u16>,
    keep_days: Option<u16>,
    now: Timestamp,
) -> Vec<Timestamp> {
    if keep_last.is_none() && keep_days.is_none() {
        return Vec::new();
    }
    let mut sorted: Vec<Timestamp> = versions.iter().copied().filter(|version| *version <= now).collect();
    sorted.sort();
    sorted.dedup();
    let is_partial = |version: &Timestamp| partial.contains(version);
    let newest = sorted.last().copied();
    let newest_complete = sorted.iter().rev().find(|version| !is_partial(version)).copied();
    let kept = usize::from(keep_last.unwrap_or(0));
    // How many versions, and how many complete ones, are at least as new.
    let mut at_least_as_new = vec![(0usize, 0usize); sorted.len()];
    let (mut all, mut complete) = (0, 0);
    for (index, version) in sorted.iter().enumerate().rev() {
        all += 1;
        if !is_partial(version) {
            complete += 1;
        }
        at_least_as_new[index] = (all, complete);
    }
    sorted
        .iter()
        .zip(at_least_as_new)
        .filter(|(version, (all, complete))| {
            let by_number = if is_partial(version) { *all <= kept } else { *complete <= kept };
            let by_age = keep_days
                .is_some_and(|days| now.unix_seconds() - version.unix_seconds() < i64::from(days) * DAY);
            let always = Some(**version) == newest || Some(**version) == newest_complete;
            !(always || by_number || by_age)
        })
        .map(|(version, _)| *version)
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
        assert_eq!(beyond_retention(&versions, &[], Some(3), None, now), versions[..7]);
        // Younger than three days: days 7, 8 and 9.
        assert_eq!(beyond_retention(&versions, &[], None, Some(3), now), versions[..7]);
        // Either keeps it: the last two, or younger than five days.
        assert_eq!(beyond_retention(&versions, &[], Some(2), Some(5), now), versions[..5]);
        assert!(beyond_retention(&versions, &[], None, None, now).is_empty(), "keeps all");
        // The newest stays even when every version is older than kept.
        assert_eq!(beyond_retention(&versions, &[], None, Some(1), t(100 * day)), versions[..9]);
        assert!(beyond_retention(&[], &[], Some(1), None, now).is_empty());
    }

    fn file(name: &str) -> Entry {
        Entry { name: name.into(), kind: EntryKind::File, bytes: 0, modified: None }
    }

    #[test]
    fn a_version_a_server_was_still_naming_is_not_complete() {
        // The second pass into the final name was cut off: its mark stays.
        let state = TargetState::of(&[
            file(MARKER),
            folder("2026-11-10_030000Z"),
            folder("2026-11-11_030000Z"),
            file("2026-11-11_030000Z.finishing"),
            folder("2026-11-11_030000Z.incomplete"),
            // Cut off before the folder was made.
            file("2026-11-12_030000Z.finishing"),
        ]);
        assert!(!state.other);
        assert_eq!(state.versions, [parse_version("2026-11-10_030000Z").unwrap()]);
        assert_eq!(
            state.incomplete,
            [
                "2026-11-11_030000Z",
                "2026-11-11_030000Z.finishing",
                "2026-11-11_030000Z.incomplete",
                "2026-11-12_030000Z.finishing"
            ]
        );
    }

    #[test]
    fn a_version_whose_run_missed_files_is_marked_partial() {
        let state = TargetState::of(&[
            file(MARKER),
            folder("2026-11-10_030000Z"),
            folder("2026-11-11_030000Z"),
            file("2026-11-11_030000Z.partial"),
            // A mark whose version never got its name.
            file("2026-11-12_030000Z.partial"),
            folder("2026-11-13_030000Z.incomplete"),
        ]);
        assert!(!state.other);
        let v = |name| parse_version(name).unwrap();
        assert_eq!(state.versions, [v("2026-11-10_030000Z"), v("2026-11-11_030000Z")]);
        assert_eq!(state.partial, [v("2026-11-11_030000Z")]);
        assert_eq!(state.incomplete, ["2026-11-12_030000Z.partial", "2026-11-13_030000Z.incomplete"]);
        assert_eq!(
            state.names_of(v("2026-11-11_030000Z")),
            ["2026-11-11_030000Z", "2026-11-11_030000Z.partial"]
        );
        assert_eq!(state.names_of(v("2026-11-10_030000Z")), ["2026-11-10_030000Z"]);
    }

    #[test]
    fn partial_versions_never_push_complete_ones_out() {
        let day = 24 * 3600;
        // Seven complete versions, then ten runs that missed files.
        let complete: Vec<Timestamp> = (0..7).map(|n| t(n * day)).collect();
        let partial: Vec<Timestamp> = (7..17).map(|n| t(n * day)).collect();
        let all: Vec<Timestamp> = complete.iter().chain(&partial).copied().collect();
        let now = t(16 * day + 3600);
        let removed = beyond_retention(&all, &partial, Some(7), None, now);
        // The complete ones stay; of the partial ones, those beyond the
        // newest seven of all go.
        assert_eq!(removed, partial[..3]);
        // Once runs are complete again, they push the partial ones out.
        let later: Vec<Timestamp> = (17..24).map(|n| t(n * day)).collect();
        let all: Vec<Timestamp> = all.iter().chain(&later).copied().collect();
        let removed = beyond_retention(&all, &partial, Some(7), None, t(23 * day + 3600));
        assert_eq!(removed, all[..17]);
    }

    #[test]
    fn retention_keeps_the_newest_complete_version_and_what_is_dated_ahead() {
        let day = 24 * 3600;
        let versions: Vec<Timestamp> = (0..5).map(|n| t(n * day)).collect();
        // The newest complete version stays, though older than kept and
        // the newer ones partial; so does the newest of all.
        let partial = &versions[3..];
        assert_eq!(
            beyond_retention(&versions, partial, None, Some(1), t(100 * day)),
            [versions[0], versions[1], versions[3]]
        );
        // A version dated after now (made while the clock was ahead) is
        // neither removed nor counted.
        let ahead = t(400 * day);
        let mut with_ahead = versions.clone();
        with_ahead.push(ahead);
        assert_eq!(beyond_retention(&with_ahead, &[], Some(2), None, t(5 * day)), versions[..3]);
    }

    #[test]
    fn kept_changes_go_after_their_days() {
        let day = 24 * 3600;
        let removed = [t(0), t(5 * day), t(9 * day)];
        assert_eq!(removed_beyond(&removed, 7, t(12 * day)), [t(0), t(5 * day)]);
        assert!(removed_beyond(&removed, 30, t(12 * day)).is_empty());
    }
}
