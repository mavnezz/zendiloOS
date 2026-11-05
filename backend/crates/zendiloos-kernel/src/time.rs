use std::time::Duration;

/// A point in time as whole seconds since the Unix epoch (UTC).
///
/// Second precision is enough for sessions, audit logs and schedules, and
/// maps directly onto an SQLite `INTEGER` and a JSON number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    pub const fn unix_seconds(self) -> i64 {
        self.0
    }

    pub fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(seconds(duration)))
    }

    pub fn saturating_sub(self, duration: Duration) -> Self {
        Self(self.0.saturating_sub(seconds(duration)))
    }

    /// Time from `earlier` to `self`; zero if `earlier` is actually later.
    pub fn duration_since(self, earlier: Self) -> Duration {
        Duration::from_secs(u64::try_from(self.0.saturating_sub(earlier.0)).unwrap_or(0))
    }

    /// The calendar date and time in UTC (proleptic Gregorian calendar).
    pub fn utc(self) -> UtcTime {
        let days = self.0.div_euclid(86_400);
        let second_of_day = self.0.rem_euclid(86_400);
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        UtcTime {
            year,
            month: month as u8,
            day: day as u8,
            hour: (second_of_day / 3600) as u8,
            minute: (second_of_day % 3600 / 60) as u8,
            second: (second_of_day % 60) as u8,
        }
    }
}

/// A [`Timestamp`] as a UTC calendar date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcTime {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl UtcTime {
    /// Back to a [`Timestamp`]; `None` for a date or time that does not
    /// exist, such as 30 February or 24:00.
    pub fn timestamp(self) -> Option<Timestamp> {
        let days_in_month = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if self.year % 4 == 0 && (self.year % 100 != 0 || self.year % 400 == 0) => 29,
            2 => 28,
            _ => return None,
        };
        if self.day == 0 || self.day > days_in_month || self.hour > 23 || self.minute > 59 || self.second > 59
        {
            return None;
        }
        // Howard Hinnant's days_from_civil.
        let (month, day) = (i64::from(self.month), i64::from(self.day));
        let year = if month <= 2 { self.year - 1 } else { self.year };
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days = era * 146_097 + day_of_era - 719_468;
        let seconds = i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second);
        Some(Timestamp::from_unix_seconds(days * 86_400 + seconds))
    }
}

fn seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

/// Source of the current time. A port, so time-dependent rules can be
/// tested deterministically.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates_in_utc() {
        let utc = |seconds| Timestamp::from_unix_seconds(seconds).utc();
        assert_eq!(utc(0), UtcTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
        // 2026-11-05 19:40:01 UTC
        assert_eq!(
            utc(1_793_907_601),
            UtcTime { year: 2026, month: 11, day: 5, hour: 19, minute: 40, second: 1 }
        );
        // A leap day, and the second before the epoch.
        assert_eq!((utc(951_782_400).month, utc(951_782_400).day), (2, 29));
        assert_eq!(utc(-1), UtcTime { year: 1969, month: 12, day: 31, hour: 23, minute: 59, second: 59 });
    }

    #[test]
    fn calendar_dates_back_to_timestamps() {
        for seconds in [0, -1, 951_782_400, 1_793_907_601, 4_102_444_799] {
            assert_eq!(
                Timestamp::from_unix_seconds(seconds).utc().timestamp(),
                Some(Timestamp::from_unix_seconds(seconds))
            );
        }
        let date = |year, month, day| UtcTime { year, month, day, hour: 0, minute: 0, second: 0 };
        assert_eq!(date(2026, 2, 29).timestamp(), None);
        assert!(date(2024, 2, 29).timestamp().is_some());
        assert_eq!(date(2026, 13, 1).timestamp(), None);
    }

    #[test]
    fn arithmetic_saturates() {
        let t = Timestamp::from_unix_seconds(i64::MAX - 1);
        assert_eq!(t.saturating_add(Duration::from_secs(10)).unix_seconds(), i64::MAX);
        let t = Timestamp::from_unix_seconds(i64::MIN + 1);
        assert_eq!(t.saturating_sub(Duration::from_secs(10)).unix_seconds(), i64::MIN);
    }

    #[test]
    fn duration_since_never_goes_negative() {
        let early = Timestamp::from_unix_seconds(5);
        let late = Timestamp::from_unix_seconds(65);
        assert_eq!(late.duration_since(early), Duration::from_secs(60));
        assert_eq!(early.duration_since(late), Duration::ZERO);
    }
}
