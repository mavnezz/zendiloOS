//! What the updates need from outside: GitHub, apt on the NAS, storage.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{Channel, CheckResult, HostReport, Release, UpdateRun, UpdateSettings};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The agent refused, e.g. while apt is busy, with its reason.
    #[error("refused: {0}")]
    Refused(String),
    /// apt or another program failed, in its words.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// zendiloOS's releases.
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// The releases with a package for this machine, newest first.
    async fn releases(&self) -> Result<Vec<Release>, String>;

    /// Downloads the release's package, checksums and signature to where
    /// the agent takes them from; the agent checks them itself.
    async fn download(&self, release: &Release) -> Result<(), String>;
}

/// Debian's packages on the NAS, through the agent.
#[async_trait]
pub trait HostPackages: Send + Sync {
    /// Fetches apt's lists and says what could be upgraded; minutes at most.
    async fn check(&self, context: &RequestContext) -> Result<HostReport, HostError>;

    /// Starts the update, in a unit of its own: Debian's packages, then
    /// zendiloOS from `release` when given (downloaded before), which the
    /// NAS's `channel` must take. Returns the run's id at once.
    async fn start(
        &self,
        release: Option<&Release>,
        channel: Channel,
        context: &RequestContext,
    ) -> Result<String, HostError>;

    /// Starts finishing an update that was cut off, in a unit of its own:
    /// dpkg configures what it unpacked, apt completes what is missing.
    /// Returns the run's id at once.
    async fn finish(&self, context: &RequestContext) -> Result<String, HostError>;

    /// The latest run of apt, if any.
    async fn latest(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError>;

    /// The last run that updated, as the NAS keeps it apart: the runs
    /// after it (a check, a plugin installed) do not replace it.
    async fn last_update(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError>;
}

/// Settings, the last check and what was said already.
#[async_trait]
pub trait UpdateStore: Send + Sync {
    async fn settings(&self) -> Result<UpdateSettings, PortError>;
    async fn save_settings(&self, settings: &UpdateSettings) -> Result<(), PortError>;
    async fn last_check(&self) -> Result<Option<CheckResult>, PortError>;
    async fn save_check(&self, check: &CheckResult) -> Result<(), PortError>;
    /// The key of what was last said about `topic`.
    async fn said(&self, topic: &str) -> Result<Option<String>, PortError>;
    async fn save_said(&self, topic: &str, key: &str) -> Result<(), PortError>;
    /// The last update run that installed, kept once it finished: the NAS
    /// keeps its own record of it too, but one from before that record was
    /// kept apart is here only.
    async fn last_run(&self) -> Result<Option<UpdateRun>, PortError>;
    async fn save_last_run(&self, run: &UpdateRun) -> Result<(), PortError>;
}
