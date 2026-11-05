use std::time::{SystemTime, UNIX_EPOCH};

use zendiloos_kernel::{Clock, Timestamp};

/// The real time of the host.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Timestamp::from_unix_seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}
