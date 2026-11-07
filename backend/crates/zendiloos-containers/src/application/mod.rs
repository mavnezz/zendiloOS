//! Use cases of the Containers context and the ports they need. Every one
//! is for administrators (`containers.manage`); the caller checks that.

mod container;
mod data;
mod engine;
pub mod ports;
mod projects;
mod updates;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_jobs::{JobError, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

pub use container::{ContainerDetailsView, TERMINALS_PER_ACCOUNT, Terminal, UpdateWay};
pub use data::DataLocation;
pub use engine::{ContainerView, ImageView, Overview};
use ports::{
    DockerData, DockerEngine, EngineError, ProjectRepository, ProjectStorage, Registries, StorageError,
    UpdateSettingsRepository,
};
pub use projects::{ComposeSource, NewProject};

use crate::domain::{InvalidProjectName, Moment, Risk, UpdateReport, UsageHistory};

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
    pub jobs: Jobs,
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
    #[error("the share does not exist or cannot be written")]
    Denied,
    /// Made before zendiloOS kept what was confirmed: deployed from the
    /// share then, which others may write. Saving its Compose file once
    /// confirms it.
    #[error("the project's Compose file needs to be saved once to be confirmed")]
    Unconfirmed,
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
        ContainersError::Refused(_) => "refused",
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
}

impl Containers {
    pub fn new(ports: ContainersPorts) -> Self {
        Self {
            ports,
            history: Arc::new(Mutex::new(UsageHistory::new(HISTORY))),
            report: Arc::new(Mutex::new(None)),
            terminals: Arc::default(),
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
