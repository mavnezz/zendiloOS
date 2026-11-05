//! Snapshots of shares: what a share looked like at a moment. ZFS takes one
//! in an instant and it only grows as the data moves on, so a few dozen per
//! share cost little; they are the cheapest way back from a mistake.
//!
//! Every snapshot is named after the moment it was taken, in UTC and in the
//! form Samba's `shadow_copy2` reads, so Windows offers all of them as
//! "Previous Versions". Who took it (a schedule or someone by hand) and a
//! note travel as ZFS properties, not in the name.

use std::time::Duration;

use zendiloos_kernel::{Timestamp, UtcTime};

use super::ShareName;

/// Snapshot names: `zdl-YYYY-MM-DD_HH.MM.SS`, UTC.
pub const LABEL_FORMAT: &str = "zdl-%Y-%m-%d_%H.%M.%S";

/// The name of a snapshot taken at `at`.
pub fn label_at(at: Timestamp) -> String {
    let t = at.utc();
    format!("zdl-{:04}-{:02}-{:02}_{:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// The moment in a name [`label_at`] made; `None` for any other name.
pub fn label_time(label: &str) -> Option<Timestamp> {
    if !is_label(label) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| label[range].parse::<u8>().ok();
    UtcTime {
        year: label[4..8].parse().ok()?,
        month: number(9..11)?,
        day: number(12..14)?,
        hour: number(15..17)?,
        minute: number(18..20)?,
        second: number(21..23)?,
    }
    .timestamp()
}

/// Whether `label` has exactly the shape [`label_at`] gives.
pub fn is_label(label: &str) -> bool {
    let shape = "zdl-0000-00-00_00.00.00";
    label.len() == shape.len()
        && label.bytes().zip(shape.bytes()).all(|(c, s)| if s == b'0' { c.is_ascii_digit() } else { c == s })
}

/// How often a schedule takes a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Frequency {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl Frequency {
    pub const ALL: [Frequency; 4] = [Self::Hourly, Self::Daily, Self::Weekly, Self::Monthly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|frequency| frequency.as_str() == value)
    }

    /// Time between two snapshots. A month is not a fixed length; 30 days
    /// is close enough to decide whether one is due.
    pub fn period(self) -> Duration {
        let hours = match self {
            Self::Hourly => 1,
            Self::Daily => 24,
            Self::Weekly => 7 * 24,
            Self::Monthly => 30 * 24,
        };
        Duration::from_secs(hours * 3600)
    }
}

/// The most snapshots one schedule may keep: they are cheap, but hundreds
/// make listings slow and "Previous Versions" unreadable.
pub const MAX_KEEP: u32 = 100;

/// How often to snapshot a share, and how many of those to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotSchedule {
    pub frequency: Frequency,
    pub keep: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSchedule {
    #[error("keeping none would delete every snapshot as soon as it is taken")]
    KeepNothing,
    #[error("a schedule keeps at most {MAX_KEEP} snapshots")]
    KeepTooMany,
    #[error("there is one schedule per frequency")]
    Duplicate,
}

/// A share's schedules, checked and ordered by frequency.
pub fn schedules(mut list: Vec<SnapshotSchedule>) -> Result<Vec<SnapshotSchedule>, InvalidSchedule> {
    list.sort_by_key(|schedule| schedule.frequency);
    for (index, schedule) in list.iter().enumerate() {
        if schedule.keep == 0 {
            return Err(InvalidSchedule::KeepNothing);
        }
        if schedule.keep > MAX_KEEP {
            return Err(InvalidSchedule::KeepTooMany);
        }
        if index > 0 && list[index - 1].frequency == schedule.frequency {
            return Err(InvalidSchedule::Duplicate);
        }
    }
    Ok(list)
}

/// The longest note a snapshot may carry.
pub const MAX_NOTE_CHARS: usize = 200;

/// A note for a snapshot: trimmed, at most [`MAX_NOTE_CHARS`] characters,
/// no control characters; `None` for an empty one.
pub fn note(value: &str) -> Result<Option<String>, InvalidNote> {
    let value = value.trim();
    if value.chars().count() > MAX_NOTE_CHARS || value.chars().any(char::is_control) {
        return Err(InvalidNote);
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a note has at most {MAX_NOTE_CHARS} characters, without line breaks")]
pub struct InvalidNote;

/// A snapshot of a share as the host has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pool: String,
    pub share: ShareName,
    /// What follows the `@`, as [`label_at`] makes it.
    pub label: String,
    pub taken_at: Timestamp,
    /// The schedules it was taken for; empty if someone took it by hand.
    pub schedules: Vec<Frequency>,
    pub note: Option<String>,
    /// What it costs: data that has changed since.
    pub used_bytes: u64,
    /// What the share held at that moment.
    pub referenced_bytes: u64,
}

/// Running a little early is fine: the scheduler looks every few minutes,
/// and waiting for the next look would make every snapshot a little later
/// than the one before.
const EARLY: Duration = Duration::from_secs(120);

/// The schedules of one share due at `now`, given its snapshots. A schedule
/// that never took one is due at once; one that missed its time (the
/// machine was off) is due on the next look.
pub fn due(schedules: &[SnapshotSchedule], snapshots: &[Snapshot], now: Timestamp) -> Vec<Frequency> {
    schedules
        .iter()
        .filter(|schedule| {
            let last = snapshots
                .iter()
                .filter(|s| s.schedules.contains(&schedule.frequency))
                .map(|s| s.taken_at)
                .max();
            last.is_none_or(|last| now.duration_since(last) + EARLY >= schedule.frequency.period())
        })
        .map(|schedule| schedule.frequency)
        .collect()
}

/// The snapshots of one share its schedules no longer keep. Each schedule
/// keeps its newest `keep`; a snapshot taken for several schedules goes
/// only when all of them let it go. Snapshots taken by hand, and those of a
/// schedule that was removed, are never deleted here.
pub fn prunable<'a>(schedules: &[SnapshotSchedule], snapshots: &'a [Snapshot]) -> Vec<&'a Snapshot> {
    let kept = |snapshot: &Snapshot, frequency: Frequency| -> bool {
        let Some(schedule) = schedules.iter().find(|s| s.frequency == frequency) else { return true };
        let mut own: Vec<&Snapshot> = snapshots.iter().filter(|s| s.schedules.contains(&frequency)).collect();
        own.sort_by_key(|s| std::cmp::Reverse(s.taken_at));
        own.iter().take(schedule.keep as usize).any(|s| s.label == snapshot.label)
    };
    snapshots
        .iter()
        .filter(|snapshot| !snapshot.schedules.is_empty())
        .filter(|snapshot| snapshot.schedules.iter().all(|&frequency| !kept(snapshot, frequency)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    const START: i64 = 1_793_836_800; // 2026-11-05 00:00:00 UTC

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(START + seconds)
    }

    fn snapshot(seconds: i64, schedules: &[Frequency]) -> Snapshot {
        Snapshot {
            pool: "tank".into(),
            share: ShareName::parse("photos").unwrap(),
            label: label_at(at(seconds)),
            taken_at: at(seconds),
            schedules: schedules.to_vec(),
            note: None,
            used_bytes: 0,
            referenced_bytes: 0,
        }
    }

    fn schedule(frequency: Frequency, keep: u32) -> SnapshotSchedule {
        SnapshotSchedule { frequency, keep }
    }

    #[test]
    fn names_are_the_utc_time_samba_can_read() {
        assert_eq!(label_at(at(19 * HOUR + 40 * 60 + 1)), "zdl-2026-11-05_19.40.01");
        assert!(is_label("zdl-2026-11-05_19.40.01"));
        assert_eq!(label_time("zdl-2026-11-05_19.40.01"), Some(at(19 * HOUR + 40 * 60 + 1)));
        assert_eq!(label_time("zdl-2026-02-30_19.40.01"), None);
        for bad in
            ["zdl-2026-11-05_19.40.0", "zdl-2026-11-05 19.40.01", "auto-2026-11-05_19.40.01", "-rf", ""]
        {
            assert!(!is_label(bad), "{bad:?}");
        }
    }

    #[test]
    fn schedules_keep_between_one_and_a_hundred_once_per_frequency() {
        use Frequency::{Daily, Hourly};
        let sorted = schedules(vec![schedule(Daily, 7), schedule(Hourly, 24)]).unwrap();
        assert_eq!(sorted, [schedule(Hourly, 24), schedule(Daily, 7)]);
        assert_eq!(schedules(vec![schedule(Daily, 0)]), Err(InvalidSchedule::KeepNothing));
        assert_eq!(schedules(vec![schedule(Daily, MAX_KEEP + 1)]), Err(InvalidSchedule::KeepTooMany));
        assert_eq!(schedules(vec![schedule(Daily, 1), schedule(Daily, 2)]), Err(InvalidSchedule::Duplicate));
    }

    #[test]
    fn notes_are_one_short_line() {
        assert_eq!(note("  before the move "), Ok(Some("before the move".to_owned())));
        assert_eq!(note("   "), Ok(None));
        assert_eq!(note("a\nb"), Err(InvalidNote));
        assert_eq!(note(&"x".repeat(MAX_NOTE_CHARS + 1)), Err(InvalidNote));
    }

    #[test]
    fn a_schedule_is_due_at_first_and_then_once_per_period_catching_up_after_a_pause() {
        use Frequency::{Daily, Hourly};
        let plans = [schedule(Hourly, 24), schedule(Daily, 7)];
        assert_eq!(due(&plans, &[], at(0)), [Hourly, Daily], "never taken: due at once");

        let taken = [snapshot(0, &[Hourly, Daily])];
        assert_eq!(due(&plans, &taken, at(30 * 60)), []);
        // A few minutes early is fine: the scheduler looks every five.
        assert_eq!(due(&plans, &taken, at(HOUR - 60)), [Hourly]);
        // Off for two days: both are due once, not once per missed period.
        assert_eq!(due(&plans, &taken, at(2 * DAY)), [Hourly, Daily]);
    }

    #[test]
    fn each_schedule_lets_go_only_of_its_own_beyond_what_it_keeps() {
        use Frequency::{Daily, Hourly, Weekly};
        let snapshots = [
            snapshot(0, &[Hourly, Daily]),
            snapshot(HOUR, &[Hourly]),
            snapshot(2 * HOUR, &[Hourly]),
            snapshot(3 * HOUR, &[]),
            snapshot(4 * HOUR, &[Weekly]),
        ];
        let plans = [schedule(Hourly, 1), schedule(Daily, 7)];

        let gone: Vec<&str> = prunable(&plans, &snapshots).iter().map(|s| s.label.as_str()).collect();

        // The first is beyond the one hourly kept, but the daily
        // schedule still keeps it; the one taken by hand and the weekly one
        // (its schedule is gone) stay.
        assert_eq!(gone, [label_at(at(HOUR)).as_str()]);
        let none: [SnapshotSchedule; 0] = [];
        assert!(prunable(&none, &snapshots).is_empty());
    }
}
