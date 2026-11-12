//! When the configuration is backed up on its own: daily or weekly at a
//! time of the NAS's time zone (daylight saving time included, as the
//! kernel's calendar reckons it). Due once the latest time lies after the
//! plan was set or last started: a NAS that was off then backs up once as
//! soon as it is on again.

use zendiloos_kernel::Timestamp;
use zendiloos_kernel::calendar::{UtcOffsets, date, day_number, local, moment, weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    /// `weekday` 0 is Monday.
    Weekly {
        weekday: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub frequency: Frequency,
    /// 0 to 23, the NAS's local time.
    pub hour: u8,
    pub minute: u8,
}

impl Default for Schedule {
    /// Daily at three at night.
    fn default() -> Self {
        Self { frequency: Frequency::Daily, hour: 3, minute: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSchedule {
    #[error("the hour is from 0 to 23")]
    Hour,
    #[error("the minute is from 0 to 59")]
    Minute,
    #[error("the weekday is from 0 (Monday) to 6 (Sunday)")]
    Weekday,
}

impl Schedule {
    pub fn check(&self) -> Result<(), InvalidSchedule> {
        if self.hour > 23 {
            return Err(InvalidSchedule::Hour);
        }
        if self.minute > 59 {
            return Err(InvalidSchedule::Minute);
        }
        if let Frequency::Weekly { weekday } = self.frequency
            && weekday > 6
        {
            return Err(InvalidSchedule::Weekday);
        }
        Ok(())
    }

    fn on_day(&self, days: i64, offsets: UtcOffsets<'_>) -> Timestamp {
        let date = date(days);
        moment(date.year, date.month, date.day, self.hour, self.minute, offsets)
    }

    /// The latest time at or before `now` and the first after it.
    fn around(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> (Timestamp, Timestamp) {
        let today = local(now, offsets(now));
        let days = day_number(today);
        let (back, step) = match self.frequency {
            Frequency::Daily => (0, 1),
            Frequency::Weekly { weekday: wanted } => ((weekday(today) - i64::from(wanted)).rem_euclid(7), 7),
        };
        let this = self.on_day(days - back, offsets);
        if this <= now {
            (this, self.on_day(days - back + step, offsets))
        } else {
            (self.on_day(days - back - step, offsets), this)
        }
    }

    pub fn latest(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> Timestamp {
        self.around(now, offsets).0
    }

    pub fn next(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> Timestamp {
        self.around(now, offsets).1
    }

    /// Whether a backup is due at `now` for a plan noted at `since`.
    pub fn is_due(&self, since: Timestamp, now: Timestamp, offsets: UtcOffsets<'_>) -> bool {
        self.latest(now, offsets) > since
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::UtcTime;

    use super::*;

    fn at(month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
        UtcTime { year: 2026, month, day, hour, minute, second: 0 }.timestamp().unwrap()
    }

    fn berlin(at_: Timestamp) -> i32 {
        let summer = at_ >= at(3, 29, 1, 0) && at_ < at(10, 25, 1, 0);
        if summer { 7200 } else { 3600 }
    }

    fn utc(_: Timestamp) -> i32 {
        0
    }

    #[test]
    fn daily_at_its_local_time_and_a_missed_one_once() {
        let daily = Schedule::default();
        let now = at(11, 12, 12, 0);
        assert_eq!(daily.latest(now, &berlin), at(11, 12, 2, 0));
        assert_eq!(daily.next(now, &berlin), at(11, 13, 2, 0));
        assert_eq!(daily.latest(at(7, 1, 12, 0), &berlin), at(7, 1, 1, 0), "two hours in summer");
        let set = at(11, 9, 12, 0);
        assert!(!daily.is_due(set, at(11, 10, 2, 59), &utc));
        assert!(daily.is_due(set, at(11, 13, 9, 0), &utc), "off for days: due once");
        assert!(!daily.is_due(at(11, 13, 9, 0), at(11, 13, 9, 1), &utc));
    }

    #[test]
    fn weekly_on_its_day() {
        // 2026-11-11 is a Wednesday (2).
        let monday = Schedule { frequency: Frequency::Weekly { weekday: 0 }, hour: 4, minute: 30 };
        let now = at(11, 11, 10, 0);
        assert_eq!(monday.latest(now, &utc), at(11, 9, 4, 30));
        assert_eq!(monday.next(now, &utc), at(11, 16, 4, 30));
        let wednesday = Schedule { frequency: Frequency::Weekly { weekday: 2 }, hour: 12, minute: 0 };
        assert_eq!(wednesday.latest(now, &utc), at(11, 4, 12, 0));
        assert_eq!(wednesday.next(now, &utc), at(11, 11, 12, 0));
    }

    #[test]
    fn values_are_checked() {
        let bad = |frequency, hour, minute| Schedule { frequency, hour, minute }.check().is_err();
        assert!(bad(Frequency::Daily, 24, 0));
        assert!(bad(Frequency::Daily, 0, 60));
        assert!(bad(Frequency::Weekly { weekday: 7 }, 0, 0));
        assert!(!bad(Frequency::Weekly { weekday: 6 }, 23, 59));
    }
}
