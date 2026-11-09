//! Docker on the host, through the system agent.

use async_trait::async_trait;
use std::time::Duration;

use zendiloos_agent_proto::{
    ErrorCode, PackageRunState, PackageTask, PoolName, Request, Response, VolumeNumber,
};
use zendiloos_apps::application::ports::{DockerError, DockerHost};
use zendiloos_apps::domain::{AppVolume, DockerHostState};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::ZfsSource;

use super::{AgentStorage, WHAT, agent_error, call, call_raw, unexpected};

/// How often a run of apt is looked at, and for how long at most.
const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);

#[async_trait]
impl DockerHost for AgentStorage {
    async fn state(&self, context: &RequestContext) -> Result<DockerHostState, PortError> {
        match call(self.transport(), Request::DockerState {}, context).await? {
            Response::DockerState { installed, running, data_root, version } => {
                Ok(DockerHostState { installed, running, data_root, version })
            }
            other => Err(unexpected(other)),
        }
    }

    async fn install(&self, context: &RequestContext) -> Result<(), DockerError> {
        let request = Request::StartPackages { task: PackageTask::InstallDocker, release: None };
        let id = match call_raw(self.transport(), request, context).await? {
            Response::PackagesStarted { id } => id,
            Response::Error { code: ErrorCode::Forbidden, message } => {
                return Err(DockerError::Refused(message));
            }
            Response::Error { code, message } => return Err(agent_error(code, &message).into()),
            other => return Err(unexpected(other).into()),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport(), Request::PackagesState {}, context).await? {
                Response::Packages { run, .. } => run,
                other => return Err(unexpected(other).into()),
            };
            match run {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => return Ok(()),
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    return Err(DockerError::Failed(run.error.unwrap_or_else(|| "apt failed".into())));
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(DockerError::Failed("apt did not finish within an hour".into()));
                }
                _ => {}
            }
        }
    }

    async fn enable(&self, volume: &AppVolume, context: &RequestContext) -> Result<(), DockerError> {
        let invalid =
            |what: &str| DockerError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")));
        let request = Request::EnableDocker {
            pool: PoolName::parse(&volume.pool).map_err(|_| invalid("pool name"))?,
            volume: VolumeNumber::new(volume.number).map_err(|_| invalid("volume number"))?,
        };
        match call_raw(self.transport(), request, context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden, message } => Err(DockerError::Refused(message)),
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn volumes(&self, context: &RequestContext) -> Result<Vec<AppVolume>, PortError> {
        Ok(self
            .pools(context)
            .await?
            .into_iter()
            .filter_map(|pool| Some(AppVolume { number: pool.volume?.get(), pool: pool.name }))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[tokio::test]
    async fn docker_goes_onto_a_pool_through_the_agent() {
        let docker = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert_eq!(docker.state(&context).await.unwrap().data_root, None);
        let volume = AppVolume { pool: "tank".into(), number: 1 };

        docker.enable(&volume, &context).await.unwrap();

        let state = docker.state(&context).await.unwrap();
        assert!(state.running);
        assert_eq!(state.data_root.as_deref(), Some("/volume1/@docker"));
    }
}
