use std::time::{SystemTime, UNIX_EPOCH};

use zendiloos_kernel::{Clock, Timestamp};

/// The real time of the host, in its time zone (`/etc/localtime`, or `TZ`).
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Timestamp::from_unix_seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
    }

    /// Without a time zone configured (or readable), UTC.
    fn utc_offset(&self, at: Timestamp) -> i32 {
        match jiff::Timestamp::from_second(at.unix_seconds()) {
            Ok(at) => jiff::tz::TimeZone::system().to_offset(at).seconds(),
            Err(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offset_is_one_the_earth_has() {
        let clock = SystemClock;
        let offset = clock.utc_offset(clock.now());
        assert!((-12 * 3600..=14 * 3600).contains(&offset), "{offset}");
    }
}
