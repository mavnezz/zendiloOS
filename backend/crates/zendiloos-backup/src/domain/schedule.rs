//! When a task backs up on its own: hourly at a minute, daily or weekly at
//! a time of the NAS's time zone (daylight saving time included, as the
//! kernel's calendar reckons it). A run is due once the latest time of its
//! schedule lies after the task was made, changed or last started: a NAS
//! that was off at that time backs up once as soon as it is on again, and
//! then keeps to its times.

use zendiloos_kernel::Timestamp;
use zendiloos_kernel::calendar::{UtcOffsets, date, day_number, local, moment, weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    /// Every hour at `minute`.
    Hourly,
    Daily,
    /// `weekday` 0 is Monday.
    Weekly {
        weekday: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub frequency: Frequency,
    /// 0 to 23, the NAS's local time; not used hourly.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
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

const HOUR: i64 = 3600;

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

    /// The schedule's time on the local date `days` after 1 January 1970.
    fn on_day(&self, days: i64, hour: u8, offsets: UtcOffsets<'_>) -> Timestamp {
        let date = date(days);
        moment(date.year, date.month, date.day, hour, self.minute, offsets)
    }

    /// The times of the schedule around `now`: the latest at or before it,
    /// and the first after it.
    fn around(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> (Timestamp, Timestamp) {
        let today = local(now, offsets(now));
        let days = day_number(today);
        match self.frequency {
            Frequency::Hourly => {
                // This local hour's minute; each hour's moment is its own,
                // so an hour the clock shows twice runs twice and one it
                // skips not at all.
                let this = moment(today.year, today.month, today.day, today.hour, self.minute, offsets);
                let step = |at: Timestamp, by: i64| Timestamp::from_unix_seconds(at.unix_seconds() + by);
                if this <= now { (this, step(this, HOUR)) } else { (step(this, -HOUR), this) }
            }
            Frequency::Daily => {
                let this = self.on_day(days, self.hour, offsets);
                if this <= now {
                    (this, self.on_day(days + 1, self.hour, offsets))
                } else {
                    (self.on_day(days - 1, self.hour, offsets), this)
                }
            }
            Frequency::Weekly { weekday: wanted } => {
                let since = (weekday(today) - i64::from(wanted)).rem_euclid(7);
                let this = self.on_day(days - since, self.hour, offsets);
                if this <= now {
                    (this, self.on_day(days - since + 7, self.hour, offsets))
                } else {
                    (self.on_day(days - since - 7, self.hour, offsets), this)
                }
            }
        }
    }

    /// The latest time of the schedule at or before `now`.
    pub fn latest(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> Timestamp {
        self.around(now, offsets).0
    }

    /// The first time of the schedule after `now`.
    pub fn next(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> Timestamp {
        self.around(now, offsets).1
    }

    /// Whether a run is due at `now` for a task noted at `since`.
    pub fn is_due(&self, since: Timestamp, now: Timestamp, offsets: UtcOffsets<'_>) -> bool {
        self.latest(now, offsets) > since
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::UtcTime;

    use super::*;

    fn at(year: i64, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
        UtcTime { year, month, day, hour, minute, second: 0 }.timestamp().unwrap()
    }

    /// Berlin: +1 in winter, +2 from the last Sunday of March 01:00 UTC
    /// to the last Sunday of October 01:00 UTC (2026: 29 March, 25 October).
    fn berlin(at_: Timestamp) -> i32 {
        let summer = at_ >= at(2026, 3, 29, 1, 0) && at_ < at(2026, 10, 25, 1, 0);
        if summer { 7200 } else { 3600 }
    }

    fn utc(_: Timestamp) -> i32 {
        0
    }

    #[test]
    fn daily_runs_at_its_local_time() {
        let daily = Schedule { frequency: Frequency::Daily, hour: 3, minute: 30 };
        let now = at(2026, 11, 11, 12, 0);
        assert_eq!(daily.latest(now, &berlin), at(2026, 11, 11, 2, 30));
        assert_eq!(daily.next(now, &berlin), at(2026, 11, 12, 2, 30));
        // Before today's time: yesterday's.
        assert_eq!(daily.latest(at(2026, 11, 11, 1, 0), &berlin), at(2026, 11, 10, 2, 30));
        // In summer the offset is two hours.
        assert_eq!(daily.latest(at(2026, 7, 1, 12, 0), &berlin), at(2026, 7, 1, 1, 30));
    }

    #[test]
    fn a_missed_time_is_due_once() {
        let daily = Schedule { frequency: Frequency::Daily, hour: 3, minute: 0 };
        let made = at(2026, 11, 9, 12, 0);
        assert!(!daily.is_due(made, at(2026, 11, 10, 1, 59), &utc));
        assert!(daily.is_due(made, at(2026, 11, 10, 3, 0), &utc));
        // Off for three days, then on: due, once; started, not again.
        assert!(daily.is_due(made, at(2026, 11, 13, 9, 0), &utc));
        assert!(!daily.is_due(at(2026, 11, 13, 9, 0), at(2026, 11, 13, 9, 1), &utc));
    }

    #[test]
    fn hourly_and_weekly() {
        let hourly = Schedule { frequency: Frequency::Hourly, hour: 0, minute: 15 };
        let now = at(2026, 11, 11, 10, 20);
        assert_eq!(hourly.latest(now, &utc), at(2026, 11, 11, 10, 15));
        assert_eq!(hourly.next(now, &utc), at(2026, 11, 11, 11, 15));
        assert_eq!(hourly.latest(at(2026, 11, 11, 10, 10), &utc), at(2026, 11, 11, 9, 15));
        // 2026-11-11 is a Wednesday (2).
        let weekly = Schedule { frequency: Frequency::Weekly { weekday: 0 }, hour: 4, minute: 0 };
        assert_eq!(weekly.latest(now, &utc), at(2026, 11, 9, 4, 0));
        assert_eq!(weekly.next(now, &utc), at(2026, 11, 16, 4, 0));
        let wednesday = Schedule { frequency: Frequency::Weekly { weekday: 2 }, hour: 12, minute: 0 };
        assert_eq!(wednesday.latest(now, &utc), at(2026, 11, 4, 12, 0));
        assert_eq!(wednesday.next(now, &utc), at(2026, 11, 11, 12, 0));
    }

    #[test]
    fn values_are_checked() {
        let bad = |frequency, hour, minute| Schedule { frequency, hour, minute }.check().is_err();
        assert!(bad(Frequency::Daily, 24, 0));
        assert!(bad(Frequency::Daily, 0, 60));
        assert!(bad(Frequency::Weekly { weekday: 7 }, 0, 0));
        assert!(!bad(Frequency::Hourly, 0, 59));
    }
}
