//! Use cases of the Containers context and the ports they need. Every one
//! is for administrators (`containers.manage`); the caller checks that.

mod backups;
mod container;
mod data;
mod engine;
pub mod ports;
mod projects;
mod records;
mod updates;
mod volumes;
mod work;

#[cfg(test)]
mod records_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod volume_tests;
#[cfg(test)]
mod work_tests;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_jobs::{JobError, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_notify::Notifier;

pub use backups::{PlanInput, PlanView, RestoreTarget};
pub use container::{ContainerDetailsView, TERMINALS_PER_ACCOUNT, Terminal, UpdateWay};
pub use data::DataLocation;
pub use engine::{ContainerView, ImageView, Overview};
use ports::{
    BackupRepository, BackupShares, DockerData, DockerEngine, EngineError, ProjectRepository, ProjectStorage,
    Registries, StorageError, UpdateSettingsRepository, VolumeArchiver,
};
pub use projects::{ComposeRisks, ComposeSource, NewProject};
pub use records::{BackupGroup, Imported};
pub use volumes::VolumeView;

use crate::domain::{InvalidBackupPlan, InvalidProjectName, Moment, Risk, UpdateReport, UsageHistory};

/// Samples every 20 seconds; twelve hours of them.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(20);
const HISTORY: usize = 12 * 60 * 3;

#[derive(Clone)]
pub struct ContainersPorts {
    pub engine: Arc<dyn DockerEngine>,
    pub projects: Arc<dyn ProjectRepository>,
    pub storage: Arc<dyn ProjectStorage>,
    pub registries: Arc<dyn Registries>,
    pub updates: Arc<dyn UpdateSettingsRepository>,
    pub data: Arc<dyn DockerData>,
    /// Volumes backed up and restored; slow.
    pub archiver: Arc<dyn VolumeArchiver>,
    pub backups: Arc<dyn BackupRepository>,
    pub backup_shares: Arc<dyn BackupShares>,
    pub jobs: Jobs,
    /// A backup that fails is told.
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum ContainersError {
    #[error("there is no such container, image, network or project")]
    NotFound,
    #[error(transparent)]
    InvalidName(#[from] InvalidProjectName),
    #[error("{0}")]
    Invalid(String),
    /// Docker refused, in its words.
    #[error("{0}")]
    Refused(String),
    /// Docker did not do it, in its words.
    #[error("{0}")]
    Failed(String),
    #[error("a project of that name exists")]
    NameTaken,
    /// An app of a catalog: the App Center changes it.
    #[error("the App Center manages this project")]
    AppProject,
    /// Only for projects made here.
    #[error("this project's Compose file is not kept here")]
    NotOwn,
    /// The Compose file gives containers power over the machine; deploy it
    /// again with the risks accepted.
    #[error("the Compose file needs confirmation")]
    Risky(Vec<Risk>),
    /// What was confirmed is not what Compose makes of the file now: the
    /// file, or one it takes in from the share, changed since its check.
    #[error("the Compose file or what it takes in changed since it was checked; check it again")]
    Changed,
    #[error("the share does not exist or cannot be written")]
    Denied,
    /// Made before zendiloOS kept what was confirmed: deployed from the
    /// share then, which others may write. Saving its Compose file once
    /// confirms it.
    #[error("the project's Compose file needs to be saved once to be confirmed")]
    Unconfirmed,
    /// Running containers use the volume; restoring it stops them only
    /// when asked to.
    #[error("running containers use the volume: {}", .0.join(", "))]
    VolumeInUse(Vec<String>),
    /// A backup or restore of the volume is under way.
    #[error("a backup or restore of the volume {0} is under way")]
    VolumeBusy(String),
    /// The volume's plan is paused: its volume was gone when it was due.
    #[error("the backup plan of {0} is paused: resume it first")]
    PlanPaused(String),
    /// A backup or restore keeps these containers stopped, or works on
    /// their volume, until it ends.
    #[error("a backup or restore keeps these containers stopped until it ends: {}", .0.join(", "))]
    Held(Vec<String>),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<EngineError> for ContainersError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Refused(reason) => Self::Refused(reason),
            EngineError::Failed(reason) => Self::Failed(reason),
            EngineError::Port(error) => Self::Port(error),
        }
    }
}

impl From<InvalidBackupPlan> for ContainersError {
    fn from(error: InvalidBackupPlan) -> Self {
        Self::Invalid(error.to_string())
    }
}

impl From<StorageError> for ContainersError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::Denied => Self::Denied,
            StorageError::NotFound => Self::NotFound,
            StorageError::Invalid(reason) => Self::Invalid(reason),
            StorageError::Port(error) => Self::Port(error),
        }
    }
}

fn job_error(error: &ContainersError) -> JobError {
    let code = match error {
        ContainersError::Refused(_)
        | ContainersError::VolumeInUse(_)
        | ContainersError::VolumeBusy(_)
        | ContainersError::PlanPaused(_)
        | ContainersError::Held(_) => "refused",
        ContainersError::Failed(_) => "agent_failed",
        _ => "internal",
    };
    JobError::new(code, Some(error.to_string()))
}

/// The use cases of this context. Cheap to clone; one sampler's history.
#[derive(Clone)]
pub struct Containers {
    ports: ContainersPorts,
    history: Arc<Mutex<UsageHistory>>,
    /// The newest check for newer images.
    report: Arc<Mutex<Option<UpdateReport>>>,
    /// The terminals each account has open.
    terminals: Arc<container::OpenTerminals>,
    /// Backups and restores of volumes: their turns, and what they hold.
    work: Arc<work::Work>,
}

impl Containers {
    pub fn new(ports: ContainersPorts) -> Self {
        Self {
            ports,
            history: Arc::new(Mutex::new(UsageHistory::new(HISTORY))),
            report: Arc::new(Mutex::new(None)),
            terminals: Arc::default(),
            work: Arc::default(),
        }
    }

    fn now(&self) -> Timestamp {
        self.ports.clock.now()
    }

    fn history(&self) -> std::sync::MutexGuard<'_, UsageHistory> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes one sample of every container's use; run every
    /// [`SAMPLE_EVERY`].
    pub async fn sample(&self) -> Result<(), ContainersError> {
        let sample = self.ports.engine.usage(&RequestContext::default()).await?;
        self.history().record(sample);
        Ok(())
    }

    /// The samples of the last `seconds`, of `containers` only if any are
    /// named, oldest first.
    pub fn usage_history(&self, seconds: u32, containers: &[String]) -> Vec<Moment> {
        let since = self.now().saturating_sub(Duration::from_secs(u64::from(seconds)));
        self.history().since(since, containers)
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.now(), action, outcome, context).by(actor).target(target.to_owned());
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "containers: cannot write the audit log");
        }
    }

    /// Records `done` in the audit log under `action` and hands it on.
    async fn audited<T>(
        &self,
        action: &'static str,
        target: &str,
        actor: &Actor,
        context: &RequestContext,
        done: Result<T, ContainersError>,
    ) -> Result<T, ContainersError> {
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit(action, outcome, target, actor, context).await;
        done
    }
}
