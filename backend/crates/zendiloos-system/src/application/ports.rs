//! What this context needs from the outside world.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{
    AuditQuery, AuditRecord, JournalPage, JournalQuery, MetricsSample, PowerAction, SshSettings, SshStatus,
    SystemInfo, TimeSettings, TimeStatus, TlsMinimum,
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

/// The SSH server; changing it needs the privileged agent.
#[async_trait]
pub trait SshControl: Send + Sync {
    async fn status(&self, context: &RequestContext) -> Result<SshStatus, PortError>;

    /// Applies checked settings.
    async fn apply(&self, settings: &SshSettings, context: &RequestContext) -> Result<(), PortError>;
}

/// The web interface's TLS; changing it needs the privileged agent.
#[async_trait]
pub trait WebTlsControl: Send + Sync {
    async fn minimum(&self, context: &RequestContext) -> Result<TlsMinimum, PortError>;

    async fn apply(&self, minimum: TlsMinimum, context: &RequestContext) -> Result<(), PortError>;
}

/// Which TCP ports something on the machine listens on.
#[async_trait]
pub trait ListeningPorts: Send + Sync {
    async fn is_listening(&self, port: u16) -> Result<bool, PortError>;
}

/// The machine's clock; changing it needs the privileged agent.
#[async_trait]
pub trait TimeControl: Send + Sync {
    async fn status(&self, context: &RequestContext) -> Result<TimeStatus, PortError>;

    /// Applies checked settings.
    async fn apply(&self, settings: &TimeSettings, context: &RequestContext) -> Result<(), PortError>;
}

/// The newest metrics sample, and a way to follow the ones after it. A
/// single sampler serves all subscribers, so the load does not grow with
/// the number of open browsers.
pub trait LiveMetrics: Send + Sync {
    fn latest(&self) -> Option<MetricsSample>;
    fn subscribe(&self) -> Box<dyn MetricsSubscription>;

    /// The samples kept, oldest first; at most [`HISTORY_SAMPLES`].
    ///
    /// [`HISTORY_SAMPLES`]: crate::domain::HISTORY_SAMPLES
    fn recent(&self) -> Vec<MetricsSample> {
        self.latest().into_iter().collect()
    }
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
