//! Self-tests on a schedule. A drive tests itself while it keeps working,
//! only a little slower; the long test reads every sector, which is the only
//! way to find the ones that went bad quietly. Run it at night, every week
//! or month, and a failing disk shows up before the pool needs it.
//!
//! There is one plan per kind of test. It starts in a window of a few hours
//! from a local hour, so a machine that was off at that hour still gets
//! its tests the same night, and never in the middle of the day.

use std::time::Duration;

use zendiloos_kernel::Timestamp;

use super::SelfTestKind;

/// How often a plan has the disks test themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SelfTestFrequency {
    Daily,
    Weekly,
    Monthly,
}

impl SelfTestFrequency {
    pub const ALL: [SelfTestFrequency; 3] = [Self::Daily, Self::Weekly, Self::Monthly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|frequency| frequency.as_str() == value)
    }

    /// Time between two runs; a month counts as 30 days.
    pub fn period(self) -> Duration {
        let days = match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
        };
        Duration::from_secs(days * 24 * 3600)
    }
}

/// How long after its hour a plan may still start: a machine switched on
/// a little late still tests the same night.
pub const SELF_TEST_WINDOW_HOURS: u8 = 4;

/// The most disks one plan names; past that, name none to mean all.
pub const MAX_PLAN_DISKS: usize = 64;

/// The longest serial number a plan accepts.
const MAX_SERIAL_CHARS: usize = 64;

/// A standing arrangement for one kind of self-test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTestPlan {
    pub kind: SelfTestKind,
    pub frequency: SelfTestFrequency,
    /// The local hour from which it starts, 0 to 23.
    pub hour: u8,
    /// The disks by serial number, so a disk keeps its plan when it moves
    /// to another port; empty means every disk that reports SMART.
    pub disks: Vec<String>,
}

/// A plan and when it last started its tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSelfTest {
    pub plan: SelfTestPlan,
    pub last_run: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSelfTestPlan {
    #[error("a long self-test takes hours; run it weekly or monthly")]
    TooOften,
    #[error("the hour is from 0 to 23")]
    Hour,
    #[error("there is one plan per kind of self-test")]
    Duplicate,
    #[error("a plan names at most {MAX_PLAN_DISKS} disks; name none to mean all of them")]
    TooManyDisks,
    #[error("a disk is named twice in the same plan")]
    DuplicateDisk,
    #[error("a serial number is one line of at most {MAX_SERIAL_CHARS} characters")]
    Serial,
}

/// Every plan, checked and ordered by kind.
pub fn self_test_plans(mut list: Vec<SelfTestPlan>) -> Result<Vec<SelfTestPlan>, InvalidSelfTestPlan> {
    list.sort_by_key(|plan| plan.kind);
    for (index, plan) in list.iter().enumerate() {
        if index > 0 && list[index - 1].kind == plan.kind {
            return Err(InvalidSelfTestPlan::Duplicate);
        }
        if plan.kind == SelfTestKind::Long && plan.frequency == SelfTestFrequency::Daily {
            return Err(InvalidSelfTestPlan::TooOften);
        }
        if plan.hour > 23 {
            return Err(InvalidSelfTestPlan::Hour);
        }
        if plan.disks.len() > MAX_PLAN_DISKS {
            return Err(InvalidSelfTestPlan::TooManyDisks);
        }
        for (position, serial) in plan.disks.iter().enumerate() {
            let length = serial.chars().count();
            if length == 0 || length > MAX_SERIAL_CHARS || serial.chars().any(char::is_control) {
                return Err(InvalidSelfTestPlan::Serial);
            }
            if plan.disks[..position].contains(serial) {
                return Err(InvalidSelfTestPlan::DuplicateDisk);
            }
        }
    }
    Ok(list)
}

/// A run may come this much earlier than a full period after the last, so
/// that it falls into the same window each time instead of a little later.
const EARLY: Duration = Duration::from_secs(12 * 3600);

impl SelfTestPlan {
    /// Whether the plan covers a disk with this serial number. A disk
    /// without one is covered only by a plan that names none.
    pub fn covers(&self, serial: Option<&str>) -> bool {
        self.disks.is_empty() || serial.is_some_and(|serial| self.disks.iter().any(|d| d == serial))
    }

    /// Whether the plan should start its tests at `now`, given when it last
    /// did and the host's offset from UTC. A plan that never ran starts in
    /// its next window.
    pub fn is_due(&self, last_run: Option<Timestamp>, now: Timestamp, utc_offset: i32) -> bool {
        let local = Timestamp::from_unix_seconds(now.unix_seconds() + i64::from(utc_offset)).utc();
        let since_start = (24 + local.hour - self.hour) % 24;
        since_start < SELF_TEST_WINDOW_HOURS
            && last_run.is_none_or(|last| now.duration_since(last) + EARLY >= self.frequency.period())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    const MIDNIGHT: i64 = 1_793_836_800; // 2026-11-05 00:00:00 UTC

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(MIDNIGHT + seconds)
    }

    fn plan(kind: SelfTestKind, frequency: SelfTestFrequency, hour: u8) -> SelfTestPlan {
        SelfTestPlan { kind, frequency, hour, disks: Vec::new() }
    }

    #[test]
    fn one_plan_per_kind_and_no_daily_long_test() {
        use SelfTestFrequency::{Daily, Monthly, Weekly};
        use SelfTestKind::{Long, Short};
        let sorted = self_test_plans(vec![plan(Long, Monthly, 3), plan(Short, Daily, 2)]).unwrap();
        assert_eq!(sorted, [plan(Short, Daily, 2), plan(Long, Monthly, 3)]);

        let error = |list| self_test_plans(list).unwrap_err();
        assert_eq!(error(vec![plan(Long, Daily, 2)]), InvalidSelfTestPlan::TooOften);
        assert_eq!(error(vec![plan(Short, Daily, 24)]), InvalidSelfTestPlan::Hour);
        assert_eq!(
            error(vec![plan(Short, Daily, 2), plan(Short, Weekly, 3)]),
            InvalidSelfTestPlan::Duplicate
        );
        let named = |disks: &[&str]| SelfTestPlan {
            disks: disks.iter().map(|d| (*d).to_owned()).collect(),
            ..plan(Short, Weekly, 2)
        };
        assert_eq!(error(vec![named(&["A1", "A1"])]), InvalidSelfTestPlan::DuplicateDisk);
        assert_eq!(error(vec![named(&[""])]), InvalidSelfTestPlan::Serial);
        assert_eq!(error(vec![named(&["A\n1"])]), InvalidSelfTestPlan::Serial);
        let many: Vec<String> = (0..=MAX_PLAN_DISKS).map(|n| format!("S{n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(error(vec![named(&many)]), InvalidSelfTestPlan::TooManyDisks);
    }

    #[test]
    fn a_plan_covers_the_disks_it_names_or_all() {
        let all = plan(SelfTestKind::Short, SelfTestFrequency::Daily, 2);
        assert!(all.covers(Some("A1")) && all.covers(None));
        let named = SelfTestPlan { disks: vec!["A1".to_owned()], ..all };
        assert!(named.covers(Some("A1")));
        assert!(!named.covers(Some("B2")) && !named.covers(None));
    }

    #[test]
    fn a_plan_starts_in_its_local_window_once_per_period() {
        let weekly = plan(SelfTestKind::Long, SelfTestFrequency::Weekly, 2);
        // In UTC: due from two for four hours, not before, not after.
        assert!(!weekly.is_due(None, at(HOUR + 59 * 60), 0));
        assert!(weekly.is_due(None, at(2 * HOUR), 0));
        assert!(weekly.is_due(None, at(5 * HOUR + 59 * 60), 0));
        assert!(!weekly.is_due(None, at(6 * HOUR), 0));

        // Once it ran, not again that night, nor the next; a week later it
        // is due a few minutes before the hour it ran at.
        let ran = Some(at(2 * HOUR + 5 * 60));
        assert!(!weekly.is_due(ran, at(3 * HOUR), 0));
        assert!(!weekly.is_due(ran, at(DAY + 2 * HOUR), 0));
        assert!(weekly.is_due(ran, at(7 * DAY + 2 * HOUR), 0));
        // Off that night: due the next, not in the afternoon.
        assert!(!weekly.is_due(ran, at(7 * DAY + 14 * HOUR), 0));
        assert!(weekly.is_due(ran, at(8 * DAY + 3 * HOUR), 0));
    }

    #[test]
    fn the_hour_is_the_hosts_local_one() {
        let nightly = plan(SelfTestKind::Short, SelfTestFrequency::Daily, 2);
        // At one UTC it is two in Berlin in winter (UTC+1).
        assert!(nightly.is_due(None, at(HOUR), 3600));
        assert!(!nightly.is_due(None, at(HOUR), 0));
        // A window across midnight: from 23 to 3.
        let late = plan(SelfTestKind::Short, SelfTestFrequency::Daily, 23);
        assert!(late.is_due(None, at(DAY + HOUR), 0));
        assert!(!late.is_due(None, at(DAY + 3 * HOUR), 0));
        // West of UTC: two in New York (UTC-5) is seven UTC.
        assert!(nightly.is_due(None, at(7 * HOUR), -5 * 3600));
    }
}
