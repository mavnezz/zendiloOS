//! Snapshots of a VM's disks: all of them at one moment, named after it as
//! the shares' snapshots are (`zdl-YYYY-MM-DD_HH.MM.SS`, UTC), with a name
//! an administrator gives, the schedules they were taken for and how
//! consistent they are. Schedules take them hourly, daily, weekly or
//! monthly and keep the newest few, as the shares' do; one taken by hand is
//! never let go by a schedule.

use std::time::Duration;

use zendiloos_kernel::{Timestamp, UtcTime};

use super::{Invalid, invalid};

/// The name of a snapshot taken at `at`.
pub fn label_at(at: Timestamp) -> String {
    let t = at.utc();
    format!("zdl-{:04}-{:02}-{:02}_{:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// Whether `label` has exactly the shape [`label_at`] gives.
pub fn is_label(label: &str) -> bool {
    let shape = "zdl-0000-00-00_00.00.00";
    label.len() == shape.len()
        && label.bytes().zip(shape.bytes()).all(|(c, s)| if s == b'0' { c.is_ascii_digit() } else { c == s })
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

/// The longest name a snapshot is given.
pub const SNAPSHOT_NAME_MAX: usize = 64;

/// A snapshot's name: trimmed, at most [`SNAPSHOT_NAME_MAX`] characters, no
/// control characters; none for an empty one.
pub fn snapshot_name(text: &str) -> Result<Option<String>, Invalid> {
    let text = text.trim();
    if text.chars().count() > SNAPSHOT_NAME_MAX || text.chars().any(char::is_control) {
        return Err(invalid(format!(
            "a snapshot's name has at most {SNAPSHOT_NAME_MAX} characters, without line breaks"
        )));
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

/// How consistent a snapshot is with itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
    /// Taken while the VM was off.
    Off,
    /// Taken while it ran, its file systems frozen by the guest agent.
    Frozen,
    /// Taken while it ran without the guest agent: as after a power cut.
    Crash,
}

impl Consistency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Frozen => "frozen",
            Self::Crash => "crash",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Off, Self::Frozen, Self::Crash].into_iter().find(|kind| kind.as_str() == text)
    }
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

    /// Time between two snapshots; a month is 30 days here.
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

/// The most snapshots one schedule keeps.
pub const MAX_KEEP: u32 = 100;

/// How often to snapshot a VM, and how many of those to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotSchedule {
    pub frequency: Frequency,
    pub keep: u32,
}

/// A VM's schedules, checked and ordered by frequency.
pub fn schedules(mut list: Vec<SnapshotSchedule>) -> Result<Vec<SnapshotSchedule>, Invalid> {
    list.sort_by_key(|schedule| schedule.frequency);
    for (index, schedule) in list.iter().enumerate() {
        if !(1..=MAX_KEEP).contains(&schedule.keep) {
            return Err(invalid(format!("a schedule keeps 1 to {MAX_KEEP} snapshots")));
        }
        if index > 0 && list[index - 1].frequency == schedule.frequency {
            return Err(invalid("there is one schedule per frequency"));
        }
    }
    Ok(list)
}

/// A snapshot of a VM's disks, as the host has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSnapshot {
    pub vm: String,
    /// What follows the `@`, as [`label_at`] makes it.
    pub label: String,
    pub taken_at: Timestamp,
    pub name: Option<String>,
    /// The schedules it was taken for; empty if taken by hand.
    pub schedules: Vec<Frequency>,
    pub consistency: Consistency,
    /// The slots of the disks it has.
    pub disks: Vec<u8>,
    /// What it costs, of all its disks: data changed since.
    pub used_bytes: u64,
    /// What the disks held then.
    pub referenced_bytes: u64,
}

/// Running a little early is fine: the scheduler looks every few minutes.
const EARLY: Duration = Duration::from_secs(120);

/// The schedules of one VM due at `now`, given its snapshots (newest
/// first or not). One that never took one is due at once; one that missed
/// its time is due on the next look.
pub fn due(schedules: &[SnapshotSchedule], snapshots: &[VmSnapshot], now: Timestamp) -> Vec<Frequency> {
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

/// The snapshots of one VM its schedules no longer keep: each keeps its
/// newest `keep`; one taken for several goes only once all let it go; one
/// taken by hand, or for a schedule that went, never goes here.
pub fn prunable<'a>(schedules: &[SnapshotSchedule], snapshots: &'a [VmSnapshot]) -> Vec<&'a VmSnapshot> {
    let kept = |snapshot: &VmSnapshot, frequency: Frequency| -> bool {
        let Some(schedule) = schedules.iter().find(|s| s.frequency == frequency) else { return true };
        let mut own: Vec<&VmSnapshot> =
            snapshots.iter().filter(|s| s.schedules.contains(&frequency)).collect();
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
    const START: i64 = 1_793_836_800; // 2026-11-05 00:00:00 UTC

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(START + seconds)
    }

    fn snapshot(seconds: i64, schedules: &[Frequency]) -> VmSnapshot {
        VmSnapshot {
            vm: "0a1b2c3d4e5f".into(),
            label: label_at(at(seconds)),
            taken_at: at(seconds),
            name: None,
            schedules: schedules.to_vec(),
            consistency: Consistency::Crash,
            disks: vec![0],
            used_bytes: 0,
            referenced_bytes: 0,
        }
    }

    #[test]
    fn labels_are_the_utc_time_and_names_one_short_line() {
        assert_eq!(label_at(at(19 * HOUR + 40 * 60 + 1)), "zdl-2026-11-05_19.40.01");
        assert_eq!(label_time("zdl-2026-11-05_19.40.01"), Some(at(19 * HOUR + 40 * 60 + 1)));
        assert_eq!(label_time("zdl-2026-02-30_19.40.01"), None);
        assert!(!is_label("zdl-2026-11-05 19.40.01") && !is_label("-rf"));
        assert_eq!(snapshot_name("  before the update "), Ok(Some("before the update".into())));
        assert_eq!(snapshot_name(""), Ok(None));
        assert!(snapshot_name("a\nb").is_err() && snapshot_name(&"x".repeat(65)).is_err());
        for kind in [Consistency::Off, Consistency::Frozen, Consistency::Crash] {
            assert_eq!(Consistency::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Consistency::parse("x"), None);
        for frequency in Frequency::ALL {
            assert_eq!(Frequency::parse(frequency.as_str()), Some(frequency));
        }
        assert_eq!(Frequency::parse("yearly"), None);
    }

    #[test]
    fn schedules_keep_one_to_a_hundred_and_take_and_let_go_on_time() {
        use Frequency::{Daily, Hourly, Weekly};
        let plan = |frequency, keep| SnapshotSchedule { frequency, keep };
        assert_eq!(
            schedules(vec![plan(Daily, 7), plan(Hourly, 24)]).unwrap(),
            [plan(Hourly, 24), plan(Daily, 7)]
        );
        assert!(schedules(vec![plan(Daily, 0)]).is_err() && schedules(vec![plan(Daily, 101)]).is_err());
        assert!(schedules(vec![plan(Daily, 1), plan(Daily, 2)]).is_err());
        let plans = [plan(Hourly, 1), plan(Daily, 7)];
        assert_eq!(due(&plans, &[], at(0)), [Hourly, Daily]);
        let taken = [snapshot(0, &[Hourly, Daily])];
        assert_eq!(due(&plans, &taken, at(30 * 60)), []);
        assert_eq!(due(&plans, &taken, at(HOUR - 60)), [Hourly]);
        let snapshots = [
            snapshot(0, &[Hourly, Daily]),
            snapshot(HOUR, &[Hourly]),
            snapshot(2 * HOUR, &[Hourly]),
            snapshot(3 * HOUR, &[]),
            snapshot(4 * HOUR, &[Weekly]),
        ];
        let gone: Vec<&str> = prunable(&plans, &snapshots).iter().map(|s| s.label.as_str()).collect();
        assert_eq!(gone, [label_at(at(HOUR)).as_str()]);
    }
}
