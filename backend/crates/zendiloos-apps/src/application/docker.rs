//! Turning apps on: Docker installed from Debian, its data on a pool.

use std::sync::Arc;

use zendiloos_jobs::{Job, Jobs, Progress};
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
    /// The engine's version, while it runs.
    pub version: Option<String>,
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
    jobs: Jobs,
}

impl Docker {
    pub fn new(
        host: Arc<dyn DockerHost>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        jobs: Jobs,
    ) -> Self {
        Self { host, audit, clock, jobs }
    }

    pub async fn setup(&self, request: &RequestContext) -> Result<DockerSetup, AppsError> {
        let state = self.host.state(request).await?;
        let volumes = self.host.volumes(request).await?;
        let number = state.data_root.as_deref().and_then(docker_volume);
        let volume = number.and_then(|n| volumes.iter().find(|v| v.number == n).cloned());
        Ok(DockerSetup {
            installed: state.installed,
            running: state.running,
            version: state.version,
            volume,
            volumes,
        })
    }

    /// Installs Docker and puts its data on `pool`, as a job: apps can run
    /// afterwards.
    pub async fn install(
        &self,
        pool: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, AppsError> {
        if self.host.state(request).await?.installed {
            return Err(AppsError::DockerInstalled);
        }
        let volume = self
            .host
            .volumes(request)
            .await?
            .into_iter()
            .find(|v| v.pool == pool)
            .ok_or(AppsError::PoolNotFound)?;
        let this = self.clone();
        let (owned_actor, context) = (actor.clone(), request.clone());
        let job = self
            .jobs
            .start("apps.docker_install", Some(pool.to_owned()), Some(actor), move |progress| async move {
                let outcome = this.put_in_place(&volume, &progress, &context).await;
                let result = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
                let entry = AuditEntry::new(this.clock.now(), "apps.docker_install", result, &context)
                    .by(&owned_actor)
                    .target(&volume.pool);
                if let Err(error) = this.audit.record(entry).await {
                    tracing::error!(%error, "cannot write the audit log");
                }
                outcome.map_err(|error| {
                    let code = match &error {
                        DockerError::Refused(_) => "refused",
                        DockerError::Failed(_) | DockerError::Port(_) => "agent_failed",
                    };
                    zendiloos_jobs::JobError::new(code, Some(error.to_string()))
                })
            })
            .await?;
        Ok(job)
    }

    async fn put_in_place(
        &self,
        volume: &AppVolume,
        progress: &Progress,
        request: &RequestContext,
    ) -> Result<(), DockerError> {
        progress.step("packages", Some(0.05)).await;
        self.host.install(request).await?;
        progress.step("pool", Some(0.85)).await;
        self.host.enable(volume, request).await
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
            Err(DockerError::Failed(reason)) => Err(AppsError::HostFailed(reason)),
            Err(DockerError::Port(error)) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use zendiloos_kernel::{PortError, Timestamp};

    use zendiloos_jobs::JobState;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};

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

    fn docker(host: FakeDocker) -> Docker {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        Docker::new(Arc::new(host), Arc::new(NoAudit), Arc::new(Fixed), jobs)
    }

    #[tokio::test]
    async fn docker_is_installed_and_put_on_the_pool_picked() {
        let docker = docker(FakeDocker::missing());
        let request = RequestContext::default();
        let alice = Actor { account_id: 1, name: "alice".into() };

        assert!(!docker.setup(&request).await.unwrap().installed);
        let nowhere = docker.install("nowhere", &alice, &request).await;
        assert!(matches!(nowhere, Err(AppsError::PoolNotFound)), "{nowhere:?}");
        let job = docker.install("backup", &alice, &request).await.unwrap();
        assert_eq!(job.kind, "apps.docker_install");
        let mut finished = job;
        for _ in 0..200 {
            finished = docker.jobs.find(finished.id).await.unwrap().unwrap();
            if finished.finished_at.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(finished.state, JobState::Succeeded);
        let after = docker.setup(&request).await.unwrap();
        assert!(after.installed && after.running);
        assert_eq!(after.volume, Some(AppVolume { pool: "backup".into(), number: 2 }));
        let again = docker.install("tank", &alice, &request).await;
        assert!(matches!(again, Err(AppsError::DockerInstalled)), "{again:?}");
    }

    #[tokio::test]
    async fn docker_goes_onto_a_pool_zendiloos_made() {
        let docker = docker(FakeDocker::default());
        let request = RequestContext::default();
        let alice = Actor { account_id: 1, name: "alice".into() };

        let before = docker.setup(&request).await.unwrap();
        assert!(before.installed && !before.running && before.volume.is_none());
        assert_eq!(before.volumes.len(), 2);
        let missing = docker.enable("nowhere", &alice, &request).await;
        assert!(matches!(missing, Err(AppsError::PoolNotFound)), "{missing:?}");

        let after = docker.enable("tank", &alice, &request).await.unwrap();
        assert!(after.running);
        assert_eq!(after.volume, Some(AppVolume { pool: "tank".into(), number: 1 }));
    }
}
