//! Docker on the host, through the system agent.

use async_trait::async_trait;
use zendiloos_agent_proto::{ErrorCode, PoolName, Request, Response, VolumeNumber};
use zendiloos_apps::application::ports::{DockerError, DockerHost};
use zendiloos_apps::domain::{AppVolume, DockerHostState};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::ZfsSource;

use super::{AgentStorage, WHAT, agent_error, call, call_raw, unexpected};

#[async_trait]
impl DockerHost for AgentStorage {
    async fn state(&self, context: &RequestContext) -> Result<DockerHostState, PortError> {
        match call(self.transport(), Request::DockerState {}, context).await? {
            Response::DockerState { installed, running, data_root } => {
                Ok(DockerHostState { installed, running, data_root })
            }
            other => Err(unexpected(other)),
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
