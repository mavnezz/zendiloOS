//! [`HostPackages`] through the system agent: apt runs in a unit of its
//! own; the agent starts it and reads back what it recorded.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, BootKernel, ErrorCode, PackageReport, PackageRun, PackageRunState, PackageTask,
    ReleaseChannel, ReleaseInstall, ReleasePackage, Request, Response,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_updates::application::ports::{HostError, HostPackages};
use zendiloos_updates::domain::{
    BootChoice, BootReason, Channel, HostReport, PackageUpdate, Release, RunState, UpdateRun,
};

use super::{WHAT, agent_error, call, call_raw, refusal, unexpected};

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
        release: Option<ReleaseInstall>,
        context: &RequestContext,
    ) -> Result<String, HostError> {
        match call_raw(self.transport.as_ref(), Request::StartPackages { task, release }, context).await? {
            Response::PackagesStarted { id } => Ok(id),
            Response::Error { code: ErrorCode::Forbidden, message } => {
                Err(HostError::Refused(refusal(&message)))
            }
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }

    /// The latest run, the last update's, and the kernel that runs.
    async fn state(&self, context: &RequestContext) -> Result<State, HostError> {
        match call(self.transport.as_ref(), Request::PackagesState {}, context).await? {
            Response::Packages { run, update, running_kernel } => Ok(State { run, update, running_kernel }),
            other => Err(unexpected(other).into()),
        }
    }
}

/// What the agent says of apt's runs.
struct State {
    run: Option<PackageRun>,
    update: Option<Box<PackageRun>>,
    running_kernel: Option<String>,
}

fn update_run(run: PackageRun) -> UpdateRun {
    UpdateRun {
        id: run.id,
        installs: run.task.updates(),
        finishes: run.task == PackageTask::Finish,
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
        interrupted: report.interrupted,
        boot: report.boot.map(boot),
    }
}

fn boot(boot: BootKernel) -> BootChoice {
    use zendiloos_agent_proto::BootReason as Told;
    BootChoice {
        kernel: boot.kernel,
        reason: match boot.reason {
            Told::Newest => BootReason::Newest,
            Told::NewestLacksZfs => BootReason::NewestLacksZfs,
            Told::NoZfs => BootReason::NoZfs,
            Told::NotSaved => BootReason::NotSaved,
            Told::NotGrub => BootReason::NotGrub,
            Told::Failed => BootReason::Failed,
        },
        detail: boot.detail,
    }
}

#[async_trait]
impl HostPackages for AgentUpdates {
    async fn check(&self, context: &RequestContext) -> Result<HostReport, HostError> {
        let id = self.start_task(PackageTask::Check, None, context).await?;
        let deadline = tokio::time::Instant::now() + CHECK_TIME;
        loop {
            tokio::time::sleep(POLL).await;
            match self.state(context).await?.run {
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

    async fn start(
        &self,
        release: Option<&Release>,
        channel: Channel,
        context: &RequestContext,
    ) -> Result<String, HostError> {
        let channel = match channel {
            Channel::Stable => ReleaseChannel::Stable,
            Channel::Test => ReleaseChannel::Test,
            Channel::Development => ReleaseChannel::Development,
        };
        let release = release
            .map(|release| ReleasePackage::parse(&release.package.name))
            .transpose()
            .map_err(|_| PortError::new(WHAT, "not a valid release package for the agent"))?
            .map(|package| ReleaseInstall { package, channel });
        self.start_task(PackageTask::Update, release, context).await
    }

    async fn finish(&self, context: &RequestContext) -> Result<String, HostError> {
        self.start_task(PackageTask::Finish, None, context).await
    }

    async fn latest(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
        Ok(self.state(context).await?.run.map(update_run))
    }

    async fn last_update(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
        Ok(self.state(context).await?.update.map(|run| update_run(*run)))
    }

    async fn running_kernel(&self, context: &RequestContext) -> Result<Option<String>, HostError> {
        Ok(self.state(context).await?.running_kernel)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;
    use crate::agent::scripted::Scripted;

    #[tokio::test]
    async fn a_check_and_an_update_run_through_the_agent() {
        let updates = AgentUpdates::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert_eq!(updates.latest(&context).await.unwrap(), None);
        let found = updates.check(&context).await.unwrap();
        assert_eq!(found.packages.len(), 3);
        assert_eq!(found.security_updates(), 2);
        assert!(!found.restart_needed());

        let id = updates.start(None, Channel::Stable, &context).await.unwrap();
        let busy = updates.start(None, Channel::Stable, &context).await;
        assert!(matches!(busy, Err(HostError::Refused(_))), "{busy:?}");
        let run = updates.latest(&context).await.unwrap().unwrap();
        assert_eq!((run.id.as_str(), run.installs, run.state), (id.as_str(), true, RunState::Running));
        assert_eq!(updates.last_update(&context).await.unwrap().map(|run| run.id), Some(id.clone()));

        // A check after it is the latest run; the update stays the last one.
        tokio::time::sleep(Duration::from_millis(4100)).await;
        updates.check(&context).await.unwrap();
        let latest = updates.latest(&context).await.unwrap().unwrap();
        assert!(!latest.installs && latest.id != id);
        let update = updates.last_update(&context).await.unwrap().unwrap();
        assert_eq!((update.id, update.state), (id, RunState::Succeeded));
    }

    #[tokio::test]
    async fn a_release_goes_to_the_agent_with_the_channel_it_must_take() {
        use zendiloos_updates::testing::release;

        let updates = AgentUpdates::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let build = release("dev", "0.3.0+dev.20261109120000.gabc1234", true);
        let refused = updates.start(Some(&build), Channel::Stable, &context).await;
        assert!(
            matches!(&refused, Err(HostError::Refused(why)) if why.contains("stable channel")),
            "{refused:?}"
        );
        // Said as the administrator reads it: "refused" once, not the
        // agent's word and the error's own.
        assert_eq!(refused.unwrap_err().to_string(), "refused: the stable channel does not take it");
        assert_eq!(updates.latest(&context).await.unwrap(), None, "nothing ran");

        updates.start(Some(&build), Channel::Development, &context).await.unwrap();
        let run = updates.latest(&context).await.unwrap().unwrap();
        assert_eq!(run.zendiloos.as_deref(), Some("0.3.0+dev.20261109120000.gabc1234"));
    }

    #[tokio::test]
    async fn the_kernel_that_runs_is_the_agents_to_say() {
        let context = RequestContext::default();
        let updates = AgentUpdates::new(Arc::new(SimulatedAgent::new()));
        assert_eq!(updates.running_kernel(&context).await.unwrap().as_deref(), Some("6.12.48+deb13-amd64"));

        // An agent from before it said: the reports' kernel stands.
        let older: Response = serde_json::from_str(r#"{"status":"packages","run":null}"#).unwrap();
        let updates = AgentUpdates::new(Arc::new(Scripted::answering([older])));
        assert_eq!(updates.running_kernel(&context).await.unwrap(), None);
    }
}
