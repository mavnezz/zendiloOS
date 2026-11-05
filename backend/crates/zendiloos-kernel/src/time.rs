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
