//! Use cases of the Network context and the ports they need.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use crate::domain::{InterfaceSettings, InvalidSettings, Network};

/// How the machine is connected, read from the host.
#[async_trait]
pub trait NetworkSource: Send + Sync {
    async fn network(&self) -> Result<Network, PortError>;
}

/// Whether the network can be changed here, and a change that waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSetup {
    /// systemd-networkd manages the network.
    pub managed: bool,
    pub pending: Option<PendingChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    pub interface: String,
    /// When the old settings return unless the change is confirmed.
    pub revert_at: Timestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigureError {
    #[error("refused by the host: {0}")]
    Refused(String),
    /// Another change waits for confirmation.
    #[error("another change waits for confirmation")]
    Pending,
    #[error(transparent)]
    Invalid(#[from] InvalidSettings),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Changes the network on the host; unconfirmed changes go back by
/// themselves.
#[async_trait]
pub trait NetworkConfigurator: Send + Sync {
    async fn setup(&self, context: &RequestContext) -> Result<NetworkSetup, PortError>;

    async fn apply(
        &self,
        settings: &InterfaceSettings,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError>;

    async fn confirm(&self, context: &RequestContext) -> Result<(), ConfigureError>;
}

/// Query: interfaces, addresses, default routes and name servers.
#[derive(Clone)]
pub struct GetNetwork {
    source: Arc<dyn NetworkSource>,
}

impl GetNetwork {
    pub fn new(source: Arc<dyn NetworkSource>) -> Self {
        Self { source }
    }

    pub async fn execute(&self) -> Result<Network, PortError> {
        self.source.network().await
    }
}

/// How long a change waits for confirmation from the new address.
pub const CONFIRM_WITHIN_SECONDS: u16 = 120;

/// Changing an interface's settings: applied at once, kept only if
/// confirmed in time, which needs the interface reachable afterwards.
#[derive(Clone)]
pub struct ConfigureNetwork {
    configurator: Arc<dyn NetworkConfigurator>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl ConfigureNetwork {
    pub fn new(
        configurator: Arc<dyn NetworkConfigurator>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { configurator, audit, clock }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    pub async fn setup(&self, request: &RequestContext) -> Result<NetworkSetup, PortError> {
        self.configurator.setup(request).await
    }

    pub async fn apply(
        &self,
        settings: InterfaceSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<NetworkSetup, ConfigureError> {
        let settings = settings.checked()?;
        let applied = self.configurator.apply(&settings, CONFIRM_WITHIN_SECONDS, request).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("network.apply", outcome, &settings.interface, actor, request).await;
        applied?;
        Ok(self.configurator.setup(request).await?)
    }

    pub async fn confirm(&self, actor: &Actor, request: &RequestContext) -> Result<(), ConfigureError> {
        let pending = self.configurator.setup(request).await?.pending;
        let confirmed = self.configurator.confirm(request).await;
        let outcome = if confirmed.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = pending.map(|p| p.interface).unwrap_or_default();
        self.audit("network.confirm", outcome, &target, actor, request).await;
        confirmed
    }
}

/// All use cases of this context.
#[derive(Clone)]
pub struct NetworkUseCases {
    pub get_network: GetNetwork,
    pub configure: ConfigureNetwork,
}

impl NetworkUseCases {
    pub fn new(
        source: Arc<dyn NetworkSource>,
        configurator: Arc<dyn NetworkConfigurator>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            get_network: GetNetwork::new(source),
            configure: ConfigureNetwork::new(configurator, audit, clock),
        }
    }
}
