//! Backups of volumes on a schedule: one plan per volume, the runs
//! recorded, and how many archives are kept.
//!
//! A plan runs daily, weekly on a weekday or monthly on a day, from a local
//! hour of the host (its time zone, daylight saving time included). A run
//! is due once the latest of those times lies after the plan was made,
//! changed or last ran: a NAS that was off at that hour backs up as soon
//! as it is on again, once, and then keeps to its hour. A day of the month
//! a month has not (the 31st in April) is its last day.

use std::collections::BTreeMap;

use zendiloos_kernel::{Timestamp, UtcTime};

/// Archives a plan keeps unless told otherwise.
pub const DEFAULT_KEEP: u32 = 7;
/// The most archives a plan keeps.
pub const MAX_KEEP: u32 = 365;
/// Failed runs remembered per volume.
pub const FAILURES_KEPT: usize = 10;

const DAY: i64 = 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupFrequency {
    Daily,
    /// `weekday` 0 is Monday.
    Weekly {
        weekday: u8,
    },
    /// `day` 1 to 31.
    Monthly {
        day: u8,
    },
}

/// When a plan backs up: how often, and from which local hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupSchedule {
    pub frequency: BackupFrequency,
    /// 0 to 23, the host's local time.
    pub hour: u8,
}

/// `at` in the host's local time, as calendar fields.
fn local(at: Timestamp, offset: i32) -> UtcTime {
    Timestamp::from_unix_seconds(at.unix_seconds() + i64::from(offset)).utc()
}

/// 0 is Monday: 1 January 1970 was a Thursday.
fn weekday(time: UtcTime) -> i64 {
    let midnight = UtcTime { hour: 0, minute: 0, second: 0, ..time };
    let days = midnight.timestamp().map_or(0, Timestamp::unix_seconds).div_euclid(DAY);
    (days + 3).rem_euclid(7)
}

fn days_in(year: i64, month: u8) -> u8 {
    [31, 30, 29, 28]
        .into_iter()
        .find(|&day| UtcTime { year, month, day, hour: 0, minute: 0, second: 0 }.timestamp().is_some())
        .unwrap_or(28)
}

/// The month before or after.
fn month_beside(year: i64, month: u8, later: bool) -> (i64, u8) {
    match (month, later) {
        (1, false) => (year - 1, 12),
        (12, true) => (year + 1, 1),
        (month, false) => (year, month - 1),
        (month, true) => (year, month + 1),
    }
}

impl BackupSchedule {
    /// `hour` local time on a date, as a moment.
    fn on(&self, year: i64, month: u8, day: u8, offset: i32) -> Timestamp {
        let day = day.min(days_in(year, month));
        let local = UtcTime { year, month, day, hour: self.hour.min(23), minute: 0, second: 0 };
        let seconds = local.timestamp().map_or(0, Timestamp::unix_seconds);
        Timestamp::from_unix_seconds(seconds - i64::from(offset))
    }

    /// The latest time of the schedule at or before `now`.
    pub fn latest(&self, now: Timestamp, offset: i32) -> Timestamp {
        let today = local(now, offset);
        let back = |at: Timestamp, days: i64| Timestamp::from_unix_seconds(at.unix_seconds() - days * DAY);
        let earlier_or = |at: Timestamp, days: i64| if at > now { back(at, days) } else { at };
        match self.frequency {
            BackupFrequency::Daily => earlier_or(self.on(today.year, today.month, today.day, offset), 1),
            BackupFrequency::Weekly { weekday: wanted } => {
                let since = (weekday(today) - i64::from(wanted)).rem_euclid(7);
                earlier_or(back(self.on(today.year, today.month, today.day, offset), since), 7)
            }
            BackupFrequency::Monthly { day } => {
                let this = self.on(today.year, today.month, day, offset);
                if this <= now {
                    return this;
                }
                let (year, month) = month_beside(today.year, today.month, false);
                self.on(year, month, day, offset)
            }
        }
    }

    /// The first time of the schedule after `now`.
    pub fn next(&self, now: Timestamp, offset: i32) -> Timestamp {
        let today = local(now, offset);
        let ahead = |at: Timestamp, days: i64| Timestamp::from_unix_seconds(at.unix_seconds() + days * DAY);
        let later_or = |at: Timestamp, days: i64| if at <= now { ahead(at, days) } else { at };
        match self.frequency {
            BackupFrequency::Daily => later_or(self.on(today.year, today.month, today.day, offset), 1),
            BackupFrequency::Weekly { weekday: wanted } => {
                let until = (i64::from(wanted) - weekday(today)).rem_euclid(7);
                later_or(ahead(self.on(today.year, today.month, today.day, offset), until), 7)
            }
            BackupFrequency::Monthly { day } => {
                let this = self.on(today.year, today.month, day, offset);
                if this > now {
                    return this;
                }
                let (year, month) = month_beside(today.year, today.month, true);
                self.on(year, month, day, offset)
            }
        }
    }
}

/// Where a plan's archives go: a folder of a share, `""` its root. They
/// land in `<folder>/<volume>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupTarget {
    pub share_id: i64,
    pub folder: String,
}

/// How one volume is backed up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupPlan {
    pub volume: String,
    pub target: BackupTarget,
    pub schedule: BackupSchedule,
    /// Archives kept, the newest.
    pub keep: u32,
    /// The containers that use the volume are stopped while it is backed
    /// up, and started again afterwards: a database's files are then
    /// consistent.
    pub stop_containers: bool,
    /// When it was made or changed, or last ran: runs are due from then.
    pub since: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidBackupPlan {
    #[error("the hour is from 0 to 23")]
    Hour,
    #[error("the weekday is from 0 (Monday) to 6 (Sunday)")]
    Weekday,
    #[error("the day of the month is from 1 to 31")]
    Day,
    #[error("a plan keeps 1 to {MAX_KEEP} backups")]
    Keep,
    #[error("the folder is no folder of a share")]
    Folder,
}

/// Whether `folder` names a folder inside a share: names separated by `/`,
/// none empty, `.` or `..`, nor with control characters; empty for its root.
pub fn is_share_folder(folder: &str) -> bool {
    folder.is_empty()
        || (folder.len() <= 1024
            && folder.split('/').all(|name| {
                !name.is_empty()
                    && name.len() <= 255
                    && name != "."
                    && name != ".."
                    && !name.chars().any(char::is_control)
            }))
}

impl BackupPlan {
    pub fn checked(self) -> Result<Self, InvalidBackupPlan> {
        if self.schedule.hour > 23 {
            return Err(InvalidBackupPlan::Hour);
        }
        match self.schedule.frequency {
            BackupFrequency::Weekly { weekday } if weekday > 6 => return Err(InvalidBackupPlan::Weekday),
            BackupFrequency::Monthly { day } if !(1..=31).contains(&day) => {
                return Err(InvalidBackupPlan::Day);
            }
            _ => {}
        }
        if !(1..=MAX_KEEP).contains(&self.keep) {
            return Err(InvalidBackupPlan::Keep);
        }
        if !is_share_folder(&self.target.folder) {
            return Err(InvalidBackupPlan::Folder);
        }
        Ok(self)
    }

    /// Whether it should back up at `now`, with the host `offset` seconds
    /// ahead of UTC.
    pub fn is_due(&self, now: Timestamp, offset: i32) -> bool {
        self.schedule.latest(now, offset) > self.since
    }
}

/// A file a backup wrote into its share: its path there, and which file it
/// is, so that only that file is ever removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    pub path: String,
    pub bytes: u64,
    pub device: u64,
    pub inode: u64,
}

/// What a backup that succeeded wrote, and of what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenBackup {
    pub archive: BackupFile,
    pub manifest: BackupFile,
    pub sha256: String,
    /// What the volume's files held.
    pub data_bytes: u64,
    /// The archive's entries; unknown for backups recorded before they
    /// were counted.
    pub entries: Option<u64>,
    /// The manifest as written beside the archive.
    pub manifest_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupOutcome {
    Running,
    Succeeded(WrittenBackup),
    Failed(String),
}

/// One run of a backup, recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub id: i64,
    pub volume: String,
    pub share_id: i64,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub outcome: BackupOutcome,
    /// As the manifest says: what a volume made again from it gets.
    pub driver: String,
    pub labels: BTreeMap<String, String>,
    /// Who started it; none for the schedule.
    pub by: Option<String>,
}

/// A container a backup or restore of `volume` stopped, not started again
/// since: started once nothing holds it any more, also after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedStart {
    pub container: String,
    pub volume: String,
    pub since: Timestamp,
}

impl Backup {
    pub fn written(&self) -> Option<&WrittenBackup> {
        match &self.outcome {
            BackupOutcome::Succeeded(written) => Some(written),
            _ => None,
        }
    }
}

/// The archives retention removes: those of runs that succeeded beyond the
/// newest `keep`.
pub fn beyond(backups: &[Backup], keep: u32) -> Vec<&Backup> {
    let mut kept: Vec<&Backup> = backups.iter().filter(|b| b.written().is_some()).collect();
    kept.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    kept.into_iter().skip(keep as usize).collect()
}

/// Failed runs beyond the newest [`FAILURES_KEPT`], to be forgotten.
pub fn stale_failures(backups: &[Backup]) -> Vec<&Backup> {
    let mut failed: Vec<&Backup> =
        backups.iter().filter(|b| matches!(b.outcome, BackupOutcome::Failed(_))).collect();
    failed.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    failed.into_iter().skip(FAILURES_KEPT).collect()
}

/// What a backup's files are called, without their extensions: the volume
/// and the local time it started, `tesla_db-20261108-030000`.
pub fn backup_stem(volume: &str, at: Timestamp, offset: i32) -> String {
    let t = local(at, offset);
    format!("{volume}-{:04}{:02}{:02}-{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3600;
    /// 2026-11-05 00:00:00 UTC, a Thursday.
    const THURSDAY: i64 = 1_793_836_800;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(THURSDAY + seconds)
    }

    fn schedule(frequency: BackupFrequency, hour: u8) -> BackupSchedule {
        BackupSchedule { frequency, hour }
    }

    fn plan(frequency: BackupFrequency, hour: u8, since: i64) -> BackupPlan {
        BackupPlan {
            volume: "tesla_db".into(),
            target: BackupTarget { share_id: 1, folder: "docker".into() },
            schedule: schedule(frequency, hour),
            keep: DEFAULT_KEEP,
            stop_containers: true,
            since: at(since),
        }
    }

    #[test]
    fn a_daily_plan_runs_at_its_hour_and_catches_up_once() {
        let nightly = plan(BackupFrequency::Daily, 3, -HOUR);
        assert_eq!(nightly.schedule.latest(at(2 * HOUR), 0), at(3 * HOUR - DAY));
        assert!(!nightly.is_due(at(2 * HOUR + 59 * 60), 0), "made after the last one");
        assert!(nightly.is_due(at(3 * HOUR), 0));
        // Off all night, on at ten: due once, then at three again.
        assert!(nightly.is_due(at(10 * HOUR), 0));
        let ran = BackupPlan { since: at(10 * HOUR), ..nightly };
        assert!(!ran.is_due(at(23 * HOUR), 0));
        assert!(ran.is_due(at(DAY + 3 * HOUR), 0));
        assert_eq!(ran.schedule.next(at(10 * HOUR), 0), at(DAY + 3 * HOUR));
        assert_eq!(ran.schedule.next(at(2 * HOUR), 0), at(3 * HOUR));
        assert_eq!(ran.schedule.next(at(3 * HOUR), 0), at(DAY + 3 * HOUR), "after, not at");
    }

    #[test]
    fn the_hour_is_the_hosts_local_one() {
        let nightly = schedule(BackupFrequency::Daily, 3);
        // Three in Berlin in winter is two UTC.
        assert_eq!(nightly.latest(at(2 * HOUR), 3600), at(2 * HOUR));
        assert_eq!(nightly.next(at(HOUR), 3600), at(2 * HOUR));
        // Three in New York (UTC-5) is eight UTC, still the 4th there at one UTC.
        assert_eq!(nightly.latest(at(HOUR), -5 * 3600), at(8 * HOUR - DAY));
    }

    #[test]
    fn a_weekly_plan_runs_on_its_weekday() {
        // Monday at four; the 5th is a Thursday.
        let monday = schedule(BackupFrequency::Weekly { weekday: 0 }, 4);
        let last_monday = at(-3 * DAY + 4 * HOUR);
        assert_eq!(monday.latest(at(12 * HOUR), 0), last_monday);
        assert_eq!(monday.next(at(12 * HOUR), 0), at(4 * DAY + 4 * HOUR));
        // On the Monday itself: before four the one a week earlier.
        assert_eq!(monday.latest(at(-3 * DAY + 3 * HOUR), 0), at(-10 * DAY + 4 * HOUR));
        assert_eq!(monday.next(at(-3 * DAY + 3 * HOUR), 0), last_monday);
        let thursday = schedule(BackupFrequency::Weekly { weekday: 3 }, 1);
        assert_eq!(thursday.latest(at(2 * HOUR), 0), at(HOUR));
        assert_eq!(thursday.next(at(2 * HOUR), 0), at(7 * DAY + HOUR));
    }

    #[test]
    fn a_monthly_plan_runs_on_its_day_or_the_months_last() {
        let first = schedule(BackupFrequency::Monthly { day: 1 }, 2);
        // 2026-11-01 02:00 UTC is four days before the 5th.
        assert_eq!(first.latest(at(0), 0), at(-4 * DAY + 2 * HOUR));
        assert_eq!(first.next(at(0), 0), at(26 * DAY + 2 * HOUR), "1 December");
        let date = |year, month, day, hour| {
            UtcTime { year, month, day, hour, minute: 0, second: 0 }.timestamp().unwrap()
        };
        let last = schedule(BackupFrequency::Monthly { day: 31 }, 2);
        assert_eq!(last.latest(at(0), 0), date(2026, 10, 31, 2));
        assert_eq!(last.next(at(0), 0), date(2026, 11, 30, 2), "November has thirty days");
        let february = date(2027, 2, 15, 0);
        assert_eq!(last.latest(february, 0), date(2027, 1, 31, 2));
        assert_eq!(last.next(february, 0), date(2027, 2, 28, 2));
        // Over the year's end, both ways.
        let new_year = date(2027, 1, 1, 0);
        assert_eq!(first.latest(new_year, 0), date(2026, 12, 1, 2));
        assert_eq!(last.next(date(2026, 12, 31, 3), 0), date(2027, 1, 31, 2));
    }

    #[test]
    fn plans_are_checked() {
        let good = plan(BackupFrequency::Daily, 3, 0);
        assert_eq!(good.clone().checked(), Ok(good.clone()));
        let with = |frequency, hour| BackupPlan { schedule: schedule(frequency, hour), ..good.clone() };
        assert_eq!(with(BackupFrequency::Daily, 24).checked(), Err(InvalidBackupPlan::Hour));
        assert_eq!(
            with(BackupFrequency::Weekly { weekday: 7 }, 3).checked(),
            Err(InvalidBackupPlan::Weekday)
        );
        assert_eq!(with(BackupFrequency::Monthly { day: 0 }, 3).checked(), Err(InvalidBackupPlan::Day));
        assert_eq!(with(BackupFrequency::Monthly { day: 32 }, 3).checked(), Err(InvalidBackupPlan::Day));
        assert!(with(BackupFrequency::Monthly { day: 31 }, 23).checked().is_ok());
        for keep in [0, MAX_KEEP + 1] {
            assert_eq!(BackupPlan { keep, ..good.clone() }.checked(), Err(InvalidBackupPlan::Keep));
        }
        for folder in ["..", "a/../b", "a//b", "/abs", "a\nb"] {
            let target = BackupTarget { share_id: 1, folder: folder.into() };
            assert_eq!(
                BackupPlan { target, ..good.clone() }.checked(),
                Err(InvalidBackupPlan::Folder),
                "{folder}"
            );
        }
        assert!(is_share_folder("") && is_share_folder("Backups/Docker volumes"));
    }

    fn run(id: i64, started: i64, outcome: BackupOutcome) -> Backup {
        Backup {
            id,
            volume: "tesla_db".into(),
            share_id: 1,
            started_at: at(started),
            finished_at: None,
            outcome,
            driver: "local".into(),
            labels: BTreeMap::new(),
            by: None,
        }
    }

    fn succeeded(id: i64) -> BackupOutcome {
        let file = BackupFile {
            path: format!("docker/tesla_db/{id}.tar.zst"),
            bytes: 1,
            device: 1,
            inode: id as u64,
        };
        BackupOutcome::Succeeded(WrittenBackup {
            archive: file.clone(),
            manifest: file,
            sha256: "ab".repeat(32),
            data_bytes: 1,
            entries: Some(1),
            manifest_json: "{}".into(),
        })
    }

    #[test]
    fn retention_keeps_the_newest_archives_and_some_failures() {
        let mut backups: Vec<Backup> = (1..=9).map(|n| run(n, n * DAY, succeeded(n))).collect();
        backups.push(run(10, 10 * DAY, BackupOutcome::Failed("disk full".into())));
        backups.push(run(11, 11 * DAY, BackupOutcome::Running));
        let gone: Vec<i64> = beyond(&backups, 7).iter().map(|b| b.id).collect();
        assert_eq!(gone, [2, 1]);
        assert!(beyond(&backups, 9).is_empty());
        assert_eq!(beyond(&backups, 1).len(), 8);
        assert!(backups[0].written().is_some() && backups[9].written().is_none());

        let failures: Vec<Backup> =
            (1..=12).map(|n| run(n, n * HOUR, BackupOutcome::Failed("x".into()))).collect();
        let stale: Vec<i64> = stale_failures(&failures).iter().map(|b| b.id).collect();
        assert_eq!(stale, [2, 1]);
    }

    #[test]
    fn a_backup_is_named_after_its_volume_and_local_time() {
        assert_eq!(backup_stem("tesla_db", at(3 * HOUR + 7), 0), "tesla_db-20261105-030007");
        assert_eq!(backup_stem("tesla_db", at(23 * HOUR + 30 * 60), 3600), "tesla_db-20261106-003000");
    }
}
