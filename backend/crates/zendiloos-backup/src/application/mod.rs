//! Use cases: the plugin installed, tasks made, changed and removed, runs
//! started by hand, on schedule and when a drive is plugged in, stopped,
//! logged and told, retention, and restores. All for administrators
//! (`backup.manage`); the caller checks that.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use zendiloos_jobs::{JobError, Jobs};
use zendiloos_kernel::{
    Actor, AuditEntry, AuditLog, Clock, Outcome as Audited, PortError, RequestContext, Timestamp,
};
use zendiloos_notify::Notifier;

use crate::domain::{Invalid, NasPlace, Place, Task};

mod cloud;
mod ports;
mod restore;
mod runs;
mod tasks;

pub use cloud::SignIn;
pub use ports::{
    BackupHost, CloudAccountRepository, CloudCredentials, CloudSignIn, Crypt, Drive, Drives, HostError,
    HostFacts, HostKey, Listing, Location, NewAccount, NewRun, NewTask, PasswordChange, Readers, RestorePlan,
    RestoreReport, RunEnd, RunMode, RunPlan, RunReport, RunRepository, Secrets, SettingsRepository,
    ShareSpot, Shares, Steps, TaskRepository,
};
pub use restore::{RestoreRequest, Versions};
pub use tasks::{Plugin, TaskView};

pub struct BackupPorts {
    pub tasks: Arc<dyn TaskRepository>,
    pub runs: Arc<dyn RunRepository>,
    pub settings: Arc<dyn SettingsRepository>,
    pub host: Arc<dyn BackupHost>,
    pub shares: Arc<dyn Shares>,
    pub drives: Arc<dyn Drives>,
    pub accounts: Arc<dyn CloudAccountRepository>,
    pub sign_in: Arc<dyn CloudSignIn>,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("rsync is not installed")]
    NotInstalled,
    #[error("rsync is installed already")]
    Installed,
    #[error("another task has that name")]
    NameTaken,
    /// Another task writes into the same folder.
    #[error("another task backs up into that folder: {0}")]
    FolderTaken(String),
    #[error("the task is running")]
    Running,
    #[error("the task is not running")]
    NotRunning,
    #[error("a restore of the task is under way")]
    Restoring,
    #[error("the share is gone")]
    ShareMissing,
    /// Its drive is not plugged in, or not mounted.
    #[error("the drive is not plugged in")]
    DriveMissing,
    #[error("the drive is read-only")]
    DriveReadOnly,
    #[error("the server's host key changed: {0}")]
    HostKeyChanged(String),
    /// Tasks back up into the cloud account.
    #[error("the cloud account is in use by {}", .0.join(", "))]
    AccountInUse(Vec<String>),
    /// No sign-in of that state is under way (it ended, or took too long).
    #[error("the sign-in is over: begin again")]
    SignInExpired,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    HostFailed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for BackupError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => Self::Refused(reason),
            HostError::Failed(reason) => Self::HostFailed(reason),
            HostError::HostKeyChanged(detail) => Self::HostKeyChanged(detail),
            HostError::Port(error) => Self::Port(error),
        }
    }
}

/// A use case's error as its job tells it.
fn job_error(error: &BackupError) -> JobError {
    let code = match error {
        BackupError::HostKeyChanged(_) => "host_key_changed",
        BackupError::DriveMissing => "drive_missing",
        BackupError::ShareMissing => "share_missing",
        BackupError::Refused(_) | BackupError::Invalid(_) | BackupError::DriveReadOnly => "refused",
        _ => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

fn audited<T, E>(done: &Result<T, E>) -> Audited {
    if done.is_ok() { Audited::Success } else { Audited::Failure }
}

/// The run of a task under way.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// An administrator asked it to stop.
    stopping: bool,
}

/// What runs now, by task: one run, or one restore, per task at a time.
#[derive(Debug, Default)]
struct Busy {
    runs: BTreeMap<i64, Slot>,
    restores: std::collections::BTreeSet<i64>,
}

/// A task's run claimed, given back when dropped.
struct Claim {
    busy: Arc<Mutex<Busy>>,
    task: i64,
    restore: bool,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        if self.restore {
            busy.restores.remove(&self.task);
        } else {
            busy.runs.remove(&self.task);
        }
    }
}

#[derive(Clone)]
pub struct Backups {
    ports: Arc<BackupPorts>,
    busy: Arc<Mutex<Busy>>,
    /// Sign-ins to clouds under way, by their state.
    flows: Arc<Mutex<BTreeMap<String, cloud::Flow>>>,
}

impl Backups {
    pub fn new(ports: BackupPorts) -> Self {
        Self { ports: Arc::new(ports), busy: Arc::default(), flows: Arc::default() }
    }

    /// A place as the host reaches it now: a share's volume, a drive
    /// mounted, a server as given, a cloud account's provider.
    async fn reach(
        &self,
        place: &Place,
        context: &RequestContext,
    ) -> Result<(Location, Option<Drive>), BackupError> {
        match place {
            Place::Nas(nas) => self.locate(nas, context).await,
            Place::Remote(remote) => {
                Ok((Location::Remote { server: remote.server.clone(), path: remote.path.clone() }, None))
            }
            Place::Cloud(cloud) => {
                let account = self.ports.accounts.find(cloud.account).await?.ok_or(BackupError::NotFound)?;
                Ok((
                    Location::Cloud {
                        provider: account.provider,
                        account: account.id,
                        path: cloud.path.clone(),
                    },
                    None,
                ))
            }
        }
    }

    /// What a task's runs need: a daemon's password, the crypt's, a cloud's sign-in.
    async fn secrets(&self, task: &Task) -> Result<Secrets, BackupError> {
        let mut secrets =
            Secrets { password: self.ports.tasks.password(task.id).await?, ..Secrets::default() };
        if task.settings.encrypted {
            secrets.crypt = Some(self.ports.tasks.crypt(task.id).await?.ok_or_else(|| {
                BackupError::Refused("the task is encrypted but its password is not stored".into())
            })?);
        }
        if let Place::Cloud(cloud) = &task.settings.target {
            let account = self.ports.accounts.find(cloud.account).await?.ok_or(BackupError::NotFound)?;
            let (token, client_secret) =
                self.ports.accounts.secrets(account.id).await?.ok_or(BackupError::NotFound)?;
            secrets.cloud = Some(CloudCredentials {
                provider: account.provider,
                token,
                client_id: account.client_id,
                client_secret,
                drive_id: account.drive_id,
                drive_type: account.drive_type,
            });
        }
        Ok(secrets)
    }

    /// Keeps a token rclone refreshed for the task's cloud account.
    async fn keep_token(&self, task: &Task, token: Option<&zendiloos_kernel::Secret>) {
        let (Some(token), Place::Cloud(cloud)) = (token, &task.settings.target) else { return };
        if let Err(error) = self.ports.accounts.set_token(cloud.account, token).await {
            tracing::warn!(%error, task = task.id, "backup: cannot keep the cloud's refreshed token");
        }
    }

    fn now(&self) -> Timestamp {
        self.ports.clock.now()
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Audited,
        target: &str,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) {
        let mut entry = AuditEntry::new(self.now(), action, outcome, context).target(target.to_owned());
        if let Some(actor) = actor {
            entry = entry.by(actor);
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "backup: cannot write the audit log");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Busy> {
        self.busy.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn running(&self, task: i64) -> bool {
        self.lock().runs.contains_key(&task)
    }

    /// Claims a task for a run (or a restore): one of either at a time.
    fn claim(&self, task: i64, restore: bool) -> Result<Claim, BackupError> {
        let mut busy = self.lock();
        if busy.runs.contains_key(&task) {
            return Err(BackupError::Running);
        }
        if busy.restores.contains(&task) {
            return Err(BackupError::Restoring);
        }
        if restore {
            busy.restores.insert(task);
        } else {
            busy.runs.insert(task, Slot::default());
        }
        Ok(Claim { busy: Arc::clone(&self.busy), task, restore })
    }

    async fn find(&self, id: i64) -> Result<crate::domain::Task, BackupError> {
        self.ports.tasks.find(id).await?.ok_or(BackupError::NotFound)
    }

    async fn require_installed(&self, context: &RequestContext) -> Result<(), BackupError> {
        if self.ports.host.facts(context).await?.rsync { Ok(()) } else { Err(BackupError::NotInstalled) }
    }

    /// Where a place on the NAS is now: its share's volume and name, or its
    /// drive, which must be mounted.
    async fn locate(
        &self,
        place: &NasPlace,
        context: &RequestContext,
    ) -> Result<(Location, Option<Drive>), BackupError> {
        match place {
            NasPlace::Share { share_id, path } => {
                let spot = self.ports.shares.locate(*share_id).await?.ok_or(BackupError::ShareMissing)?;
                Ok((Location::Share { volume: spot.volume, share: spot.name, path: path.clone() }, None))
            }
            NasPlace::Drive { drive, path } => {
                let found = self
                    .ports
                    .drives
                    .drives(context)
                    .await?
                    .into_iter()
                    .find(|candidate| candidate.id == *drive && candidate.mounted)
                    .ok_or(BackupError::DriveMissing)?;
                Ok((Location::Drive { drive: drive.clone(), path: path.clone() }, Some(found)))
            }
        }
    }
}

#[cfg(test)]
mod tests;
