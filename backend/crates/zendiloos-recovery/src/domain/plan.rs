//! The backups made on their own: when, where to, and how many are kept.
//! The passphrase is kept with the plan, as the mail server's password is
//! (4.3): never sent back, and a change without one keeps it.

use zendiloos_kernel::{Secret, Timestamp};

use super::{InvalidSchedule, Schedule};

/// The most backups a target folder keeps; the default keeps a week.
pub const KEEP_MAX: u8 = 60;
pub const KEEP_DEFAULT: u8 = 7;

/// Where the backups go: a folder of a share, or of a removable drive
/// (when it is plugged in; otherwise that run fails and is told).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Share { share_id: i64, folder: String },
    Drive { drive: String, folder: String },
}

impl Target {
    pub fn folder(&self) -> &str {
        match self {
            Self::Share { folder, .. } | Self::Drive { folder, .. } => folder,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub enabled: bool,
    pub schedule: Schedule,
    pub target: Option<Target>,
    pub keep: u8,
    /// The administrator who set it up; the backups are written with that
    /// account's access to the folder.
    pub account_id: Option<i64>,
    pub passphrase: Option<Secret>,
    /// When it was set, or last started: a time before it is not due.
    pub since: Timestamp,
    pub last: Option<LastRun>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: Schedule::default(),
            target: None,
            keep: KEEP_DEFAULT,
            account_id: None,
            passphrase: None,
            since: Timestamp::from_unix_seconds(0),
            last: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastRun {
    pub at: Timestamp,
    pub succeeded: bool,
    /// The file written.
    pub file: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPlan {
    #[error(transparent)]
    Schedule(#[from] InvalidSchedule),
    #[error("between 1 and {KEEP_MAX} backups are kept")]
    Keep,
    #[error("a plan that is on needs a folder to write to")]
    NoTarget,
    #[error("a plan that is on needs a passphrase")]
    NoPassphrase,
    #[error("not a folder: {0}")]
    Folder(String),
}

/// A folder path within a share or drive: names apart by `/`, none empty,
/// `.` or `..`; no leading or trailing `/`. Empty: its root.
pub fn check_folder(folder: &str) -> Result<(), InvalidPlan> {
    let valid = folder.len() <= 1024
        && (folder.is_empty()
            || folder.split('/').all(|name| {
                !name.is_empty() && name != "." && name != ".." && !name.chars().any(char::is_control)
            }));
    if valid { Ok(()) } else { Err(InvalidPlan::Folder(folder.to_owned())) }
}

impl Plan {
    pub fn check(&self) -> Result<(), InvalidPlan> {
        self.schedule.check()?;
        if !(1..=KEEP_MAX).contains(&self.keep) {
            return Err(InvalidPlan::Keep);
        }
        if let Some(target) = &self.target {
            check_folder(target.folder())?;
        }
        if self.enabled && self.target.is_none() {
            return Err(InvalidPlan::NoTarget);
        }
        if self.enabled && self.passphrase.is_none() {
            return Err(InvalidPlan::NoPassphrase);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_that_is_on_has_a_target_and_a_passphrase() {
        let target = Target::Share { share_id: 1, folder: "zendiloOS/config".into() };
        let on = Plan {
            enabled: true,
            target: Some(target.clone()),
            passphrase: Some(Secret::new("correct horse battery")),
            ..Plan::default()
        };
        assert!(on.check().is_ok());
        assert_eq!(Plan { target: None, ..on.clone() }.check(), Err(InvalidPlan::NoTarget));
        assert_eq!(Plan { passphrase: None, ..on.clone() }.check(), Err(InvalidPlan::NoPassphrase));
        assert_eq!(Plan { keep: 0, ..on.clone() }.check(), Err(InvalidPlan::Keep));
        assert_eq!(Plan { keep: 61, ..on.clone() }.check(), Err(InvalidPlan::Keep));
        assert!(Plan::default().check().is_ok(), "off, nothing needed");
        for bad in ["/abs", "a//b", "a/../b", "a/", "x\ny"] {
            let target = Target::Drive { drive: "1A2B-3C4D".into(), folder: bad.into() };
            assert!(Plan { target: Some(target), ..on.clone() }.check().is_err(), "{bad}");
        }
        assert!(check_folder("").is_ok());
    }
}
