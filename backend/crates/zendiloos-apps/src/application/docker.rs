//! Turning apps on: Docker's data on a pool.

use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext};

use super::AppsError;
use super::ports::{DockerError, DockerHost};
use crate::domain::{AppVolume, docker_volume};

/// Whether apps can run, and where they could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerSetup {
    /// `docker.io` is installed.
    pub installed: bool,
    pub running: bool,
    /// The volume Docker's data is on, if zendiloOS put it there.
    pub volume: Option<AppVolume>,
    /// Where it could go.
    pub volumes: Vec<AppVolume>,
}

#[derive(Clone)]
pub struct Docker {
    host: Arc<dyn DockerHost>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl Docker {
    pub fn new(host: Arc<dyn DockerHost>, audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        Self { host, audit, clock }
    }

    pub async fn setup(&self, request: &RequestContext) -> Result<DockerSetup, AppsError> {
        let state = self.host.state(request).await?;
        let volumes = self.host.volumes(request).await?;
        let number = state.data_root.as_deref().and_then(docker_volume);
        let volume = number.and_then(|n| volumes.iter().find(|v| v.number == n).cloned());
        Ok(DockerSetup { installed: state.installed, running: state.running, volume, volumes })
    }

    /// Puts Docker's data on `pool`, one zendiloos created.
    pub async fn enable(
        &self,
        pool: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<DockerSetup, AppsError> {
        let volume = self
            .host
            .volumes(request)
            .await?
            .into_iter()
            .find(|v| v.pool == pool)
            .ok_or(AppsError::PoolNotFound)?;
        let enabled = self.host.enable(&volume, request).await;
        let outcome = if enabled.is_ok() { Outcome::Success } else { Outcome::Failure };
        let entry =
            AuditEntry::new(self.clock.now(), "apps.docker_enable", outcome, request).by(actor).target(pool);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
        match enabled {
            Ok(()) => self.setup(request).await,
            Err(DockerError::Refused(reason)) => Err(AppsError::Refused(reason)),
            Err(DockerError::Port(error)) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use zendiloos_kernel::{PortError, Timestamp};

    use super::*;
    use crate::testing::FakeDocker;

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(0)
        }
    }

    struct NoAudit;
    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn docker_goes_onto_a_pool_zendiloos_made() {
        let docker = Docker::new(Arc::new(FakeDocker::default()), Arc::new(NoAudit), Arc::new(Fixed));
        let request = RequestContext::default();
        let alice = Actor { account_id: 1, name: "alice".into() };

        let before = docker.setup(&request).await.unwrap();
        assert!(before.installed && !before.running && before.volume.is_none());
        assert_eq!(before.volumes.len(), 1);
        let missing = docker.enable("nowhere", &alice, &request).await;
        assert!(matches!(missing, Err(AppsError::PoolNotFound)), "{missing:?}");

        let after = docker.enable("tank", &alice, &request).await.unwrap();
        assert!(after.running);
        assert_eq!(after.volume, Some(AppVolume { pool: "tank".into(), number: 1 }));
    }
}
