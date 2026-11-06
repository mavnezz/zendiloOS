//! [`HostPackages`] through the system agent: apt runs in a unit of its
//! own; the agent starts it and reads back what it recorded.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, PackageReport, PackageRun, PackageRunState, PackageTask, ReleasePackage,
    Request, Response,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_updates::application::ports::{HostError, HostPackages};
use zendiloos_updates::domain::{HostReport, PackageUpdate, Release, RunState, UpdateRun};

use super::{WHAT, agent_error, call, call_raw, unexpected};

/// How often a check is looked at, and for how long at most.
const POLL: Duration = Duration::from_secs(2);
const CHECK_TIME: Duration = Duration::from_secs(15 * 60);

pub struct AgentUpdates {
    transport: Arc<dyn AgentTransport>,
}

impl AgentUpdates {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    async fn start_task(
        &self,
        task: PackageTask,
        release: Option<ReleasePackage>,
        context: &RequestContext,
    ) -> Result<String, HostError> {
        match call_raw(self.transport.as_ref(), Request::StartPackages { task, release }, context).await? {
            Response::PackagesStarted { id } => Ok(id),
            Response::Error { code: ErrorCode::Forbidden, message } => Err(HostError::Refused(message)),
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn run(&self, context: &RequestContext) -> Result<Option<PackageRun>, HostError> {
        match call(self.transport.as_ref(), Request::PackagesState {}, context).await? {
            Response::Packages { run } => Ok(run),
            other => Err(unexpected(other).into()),
        }
    }
}

fn report(report: PackageReport) -> HostReport {
    HostReport {
        packages: report
            .upgradable
            .into_iter()
            .map(|p| PackageUpdate { name: p.name, from: p.from, to: p.to, security: p.security })
            .collect(),
        running_kernel: report.running_kernel,
        newest_kernel: report.newest_kernel,
        zfs_ready: report.zfs_ready,
    }
}

#[async_trait]
impl HostPackages for AgentUpdates {
    async fn check(&self, context: &RequestContext) -> Result<HostReport, HostError> {
        let id = self.start_task(PackageTask::Check, None, context).await?;
        let deadline = tokio::time::Instant::now() + CHECK_TIME;
        loop {
            tokio::time::sleep(POLL).await;
            match self.run(context).await? {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => {
                    return run
                        .report
                        .map(report)
                        .ok_or_else(|| HostError::Failed("apt reported nothing".into()));
                }
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    return Err(HostError::Failed(run.error.unwrap_or_else(|| "apt failed".into())));
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(HostError::Failed("apt did not finish within 15 minutes".into()));
                }
                _ => {}
            }
        }
    }

    async fn start(&self, release: Option<&Release>, context: &RequestContext) -> Result<String, HostError> {
        let release = release
            .map(|release| ReleasePackage::parse(&release.package.name))
            .transpose()
            .map_err(|_| PortError::new(WHAT, "not a valid release package for the agent"))?;
        self.start_task(PackageTask::Update, release, context).await
    }

    async fn latest(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
        Ok(self.run(context).await?.map(|run| UpdateRun {
            id: run.id,
            installs: run.task == PackageTask::Update,
            state: match run.state {
                PackageRunState::Running => RunState::Running,
                PackageRunState::Succeeded => RunState::Succeeded,
                PackageRunState::Failed => RunState::Failed,
            },
            step: run.step,
            started_at: Timestamp::from_unix_seconds(run.started_at),
            finished_at: run.finished_at.map(Timestamp::from_unix_seconds),
            error: run.error,
            zendiloos: run.release,
            report: run.report.map(report),
        }))
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[tokio::test]
    async fn a_check_and_an_update_run_through_the_agent() {
        let updates = AgentUpdates::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert_eq!(updates.latest(&context).await.unwrap(), None);
        let found = updates.check(&context).await.unwrap();
        assert_eq!(found.packages.len(), 3);
        assert_eq!(found.security_updates(), 2);
        assert!(!found.restart_needed());

        let id = updates.start(None, &context).await.unwrap();
        let busy = updates.start(None, &context).await;
        assert!(matches!(busy, Err(HostError::Refused(_))), "{busy:?}");
        let run = updates.latest(&context).await.unwrap().unwrap();
        assert_eq!((run.id.as_str(), run.installs, run.state), (id.as_str(), true, RunState::Running));
    }
}
