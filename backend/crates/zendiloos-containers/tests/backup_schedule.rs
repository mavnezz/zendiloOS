//! The backup schedule and retention against a real time zone's changes
//! (Berlin's end of summer time in 2026) and a clock put right.

use std::collections::BTreeMap;

use zendiloos_containers::domain::{
    Backup, BackupFile, BackupFrequency, BackupOutcome, BackupPlan, BackupSchedule, BackupTarget,
    WrittenBackup, beyond,
};
use zendiloos_kernel::{Timestamp, UtcTime};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;
/// Berlin: CEST (UTC+2) until 2026-10-25 01:00 UTC, CET (UTC+1) after.
const CEST: i32 = 7200;
const CET: i32 = 3600;

fn utc(year: i64, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    UtcTime { year, month, day, hour, minute, second: 0 }.timestamp().unwrap()
}

/// The offset of Europe/Berlin at `at`, around the change of 2026.
fn berlin(at: Timestamp) -> i32 {
    if at < utc(2026, 10, 25, 1, 0) { CEST } else { CET }
}

fn plan(frequency: BackupFrequency, since: Timestamp) -> BackupPlan {
    BackupPlan {
        volume: "db".into(),
        target: BackupTarget { share_id: 1, folder: String::new() },
        schedule: BackupSchedule { frequency, hour: 3 },
        keep: 7,
        stop_containers: true,
        since,
        paused: false,
    }
}

/// zendiloosd's minute loop: the times a plan starts from `from` to `to`,
/// each start noted as `ran` does, a plan noted ahead of the clock noted
/// anew.
fn starts(mut plan: BackupPlan, from: Timestamp, to: Timestamp) -> Vec<Timestamp> {
    let mut started = Vec::new();
    let mut now = from;
    while now <= to {
        if plan.noted_ahead(now) {
            plan.since = now;
        } else if plan.is_due(now, &berlin) {
            started.push(now);
            plan.since = now;
        }
        now = Timestamp::from_unix_seconds(now.unix_seconds() + 60);
    }
    started
}

#[test]
fn a_daily_plan_backs_up_once_a_day_across_the_end_of_summer_time() {
    // Ran on the 24th at three, summer time (01:00 UTC).
    let ran = utc(2026, 10, 24, 1, 0);
    let started = starts(plan(BackupFrequency::Daily, ran), ran, utc(2026, 10, 25, 12, 0));
    // Wanted: once, on the 25th at three in winter time, 02:00 UTC.
    assert_eq!(started, [utc(2026, 10, 25, 2, 0)], "starts: {started:?}");
}

#[test]
fn a_plan_at_two_runs_once_in_the_night_two_oclock_comes_twice() {
    // At two: 00:00 UTC in summer time, then 01:00 UTC once it ended.
    let ran = utc(2026, 10, 24, 0, 0);
    let mut at_two = plan(BackupFrequency::Daily, ran);
    at_two.schedule.hour = 2;
    let started = starts(at_two, ran, utc(2026, 10, 25, 12, 0));
    let names: Vec<String> =
        started.iter().map(|at| zendiloos_containers::domain::backup_stem("db", *at, berlin(*at))).collect();
    // Not twice, under one name: the second backup would be refused
    // ("exists already") and told as a failure, its containers stopped for
    // nothing.
    assert_eq!(started, [utc(2026, 10, 25, 0, 0)], "{names:?}");
}

#[test]
fn a_plan_in_the_hour_summer_time_skips_runs_the_hour_after() {
    // At two on 29 March 2026: the clocks go from two to three (01:00 UTC).
    let offsets = |at: Timestamp| if at < utc(2026, 3, 29, 1, 0) { CET } else { CEST };
    let mut at_two = plan(BackupFrequency::Daily, utc(2026, 3, 28, 1, 0));
    at_two.schedule.hour = 2;
    assert_eq!(at_two.schedule.next(utc(2026, 3, 28, 12, 0), &offsets), utc(2026, 3, 29, 1, 0));
    assert_eq!(at_two.schedule.next(utc(2026, 3, 29, 12, 0), &offsets), utc(2026, 3, 30, 0, 0));
}

#[test]
fn a_weekly_and_a_monthly_plan_do_not_run_again_when_summer_time_ends() {
    // Weekly on Saturday (5) at three: ran on Saturday the 24th.
    let ran = utc(2026, 10, 24, 1, 0);
    let weekly = starts(plan(BackupFrequency::Weekly { weekday: 5 }, ran), ran, utc(2026, 10, 30, 0, 0));
    assert!(weekly.is_empty(), "weekly: an extra run at {weekly:?}");
    // Monthly on the 1st at three: ran on 1 October (01:00 UTC).
    let ran = utc(2026, 10, 1, 1, 0);
    let monthly = starts(plan(BackupFrequency::Monthly { day: 1 }, ran), ran, utc(2026, 10, 31, 0, 0));
    assert!(monthly.is_empty(), "monthly: an extra run at {monthly:?}");
}

#[test]
fn a_plan_noted_ahead_by_a_wrong_clock_runs_when_it_says_once_the_clock_is_right() {
    // The clock was a year ahead for a while (set by hand, a bad RTC) and a
    // backup ran then: `since` is a year ahead once the clock is right.
    let now = utc(2026, 11, 5, 12, 0);
    let wrong = Timestamp::from_unix_seconds(now.unix_seconds() + 365 * DAY);
    let nightly = plan(BackupFrequency::Daily, wrong);
    let next = nightly.schedule.next(now, &|_| CET);
    assert_eq!(next, utc(2026, 11, 6, 2, 0), "the interface shows tomorrow at three");
    assert!(nightly.noted_ahead(now));
    let started = starts(nightly, now, utc(2026, 11, 7, 12, 0));
    assert_eq!(started, [utc(2026, 11, 6, 2, 0), utc(2026, 11, 7, 2, 0)], "then, and daily after");
}

fn written(id: i64, started: Timestamp) -> Backup {
    let file = BackupFile {
        path: format!("db/{id}.tar.zst"),
        bytes: 1,
        device: 1,
        inode: id as u64,
        modified: None,
    };
    Backup {
        id,
        volume: "db".into(),
        share_id: 1,
        started_at: started,
        finished_at: None,
        outcome: BackupOutcome::Succeeded(WrittenBackup {
            archive: file.clone(),
            manifest: file,
            sha256: "ab".repeat(32),
            data_bytes: 1,
            entries: Some(1),
            manifest_json: "{}".into(),
        }),
        driver: "local".into(),
        labels: BTreeMap::new(),
        by: None,
        imported: false,
    }
}

#[test]
fn retention_never_removes_the_backup_just_written() {
    let now = utc(2026, 11, 5, 2, 0);
    // One run while the clock was a year ahead, then the clock was put right.
    let ahead = written(1, Timestamp::from_unix_seconds(now.unix_seconds() + 365 * DAY));
    let just_now = written(2, now);
    let backups = [ahead, just_now];
    let removed: Vec<i64> = beyond(&backups, 1).iter().map(|b| b.id).collect();
    assert!(!removed.contains(&2), "the archive just written is removed by its own retention: {removed:?}");
}
