//! The log: each run of a task, with what it transferred, and the
//! settings that say how long it is kept.

use zendiloos_kernel::Timestamp;

/// The most entries of the transfer list kept per run; the count goes on.
pub const TRANSFERS_KEPT: usize = 5000;
/// Days the log is kept unless told otherwise.
pub const DEFAULT_LOG_DAYS: u16 = 90;
pub const MAX_LOG_DAYS: u16 = 3650;

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// "Back up now".
    Manual,
    Schedule,
    /// Its target drive was plugged in.
    Plug,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Plug => "plug",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Manual, Self::Schedule, Self::Plug].into_iter().find(|t| t.as_str() == value)
    }
}

/// How a run ended, or that it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Succeeded,
    /// It ran to its end, but some files could not be read or vanished
    /// meanwhile (rsync's 23 and 24): the version is complete but for them.
    Partial,
    Failed,
    /// Stopped by an administrator.
    Stopped,
    /// zendiloosd stopped while it ran.
    Interrupted,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Running, Self::Succeeded, Self::Partial, Self::Failed, Self::Stopped, Self::Interrupted]
            .into_iter()
            .find(|o| o.as_str() == value)
    }

    /// Whether the backup it made counts: complete, or complete but for
    /// files that could not be read.
    pub fn complete(self) -> bool {
        matches!(self, Self::Succeeded | Self::Partial)
    }
}

/// What a restore does with a file that is there already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    Skip,
    Replace,
}

/// A run as logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: i64,
    pub task_id: i64,
    pub trigger: Trigger,
    /// Who started it by hand.
    pub by: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub outcome: Outcome,
    /// Files transferred and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// The version it made, if versioned.
    pub version: Option<String>,
    /// Why it failed, or what it could not read.
    pub error: Option<String>,
    /// Entries of the transfer list, also those not kept.
    pub transfers: u64,
    pub job_id: Option<i64>,
}

/// One entry of a run's transfer list: rsync's itemized change
/// (`>f+++++++++` a new file, `*deleting` one deleted) and the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub change: String,
    pub path: String,
}

/// The plugin's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Days runs and their transfer lists are kept.
    pub log_days: u16,
    /// KiB/s for tasks that follow the default; 0 is no limit.
    pub bwlimit_kib: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { log_days: DEFAULT_LOG_DAYS, bwlimit_kib: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSettings {
    #[error("the log is kept 1 to {MAX_LOG_DAYS} days")]
    LogDays,
    #[error("the bandwidth limit is at most {} KiB/s", super::task::MAX_BWLIMIT_KIB)]
    Bandwidth,
}

impl Settings {
    pub fn check(self) -> Result<Self, InvalidSettings> {
        if !(1..=MAX_LOG_DAYS).contains(&self.log_days) {
            return Err(InvalidSettings::LogDays);
        }
        if self.bwlimit_kib > super::task::MAX_BWLIMIT_KIB {
            return Err(InvalidSettings::Bandwidth);
        }
        Ok(self)
    }

    /// When a run must have started to be kept at `now`.
    pub fn keep_since(&self, now: Timestamp) -> Timestamp {
        Timestamp::from_unix_seconds(now.unix_seconds() - i64::from(self.log_days) * 24 * 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for outcome in [
            Outcome::Running,
            Outcome::Succeeded,
            Outcome::Partial,
            Outcome::Failed,
            Outcome::Stopped,
            Outcome::Interrupted,
        ] {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        for trigger in [Trigger::Manual, Trigger::Schedule, Trigger::Plug] {
            assert_eq!(Trigger::parse(trigger.as_str()), Some(trigger));
        }
        assert_eq!(Outcome::parse("done"), None);
        assert!(Outcome::Partial.complete() && !Outcome::Stopped.complete());
    }

    #[test]
    fn settings_have_bounds() {
        assert!(Settings::default().check().is_ok());
        assert!(Settings { log_days: 0, bwlimit_kib: 0 }.check().is_err());
        assert!(Settings { log_days: 30, bwlimit_kib: u32::MAX }.check().is_err());
        let now = Timestamp::from_unix_seconds(10 * 24 * 3600);
        assert_eq!(
            Settings { log_days: 7, bwlimit_kib: 0 }.keep_since(now),
            Timestamp::from_unix_seconds(3 * 24 * 3600)
        );
    }
}
