//! What this context needs from the outside world.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{
    AuditQuery, AuditRecord, JournalPage, JournalQuery, MetricsSample, PowerAction, SystemInfo,
};

/// Supplies facts about the host. Implemented in `zendiloos-infra`.
#[async_trait]
pub trait SystemInfoProvider: Send + Sync {
    async fn system_info(&self) -> Result<SystemInfo, PortError>;
}

/// Reboots or powers off the host; needs the privileged agent.
#[async_trait]
pub trait PowerControl: Send + Sync {
    /// Returns once the action is under way.
    async fn request(&self, action: PowerAction, context: &RequestContext) -> Result<(), PortError>;
}

/// The newest metrics sample, and a way to follow the ones after it. A
/// single sampler serves all subscribers, so the load does not grow with
/// the number of open browsers.
pub trait LiveMetrics: Send + Sync {
    fn latest(&self) -> Option<MetricsSample>;
    fn subscribe(&self) -> Box<dyn MetricsSubscription>;
}

#[async_trait]
pub trait MetricsSubscription: Send {
    /// Waits for the next sample; `None` once the sampler has stopped.
    async fn next(&mut self) -> Option<MetricsSample>;
}

/// The systemd journal.
#[async_trait]
pub trait JournalReader: Send + Sync {
    async fn read(&self, query: &JournalQuery) -> Result<JournalPage, PortError>;
}

/// The audit log, newest first.
#[async_trait]
pub trait AuditReader: Send + Sync {
    async fn read(&self, query: &AuditQuery) -> Result<Vec<AuditRecord>, PortError>;
}
