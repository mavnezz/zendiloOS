//! [`DockerData`] through the system agent: where Docker's data is, and
//! moving it, which can take hours.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{AgentTransport, ErrorCode, PoolName, Request, Response, VolumeNumber};
use zendiloos_apps::application::ports::DockerHost;
use zendiloos_containers::application::ports::{DataPlace, DockerData, EngineError};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::ZfsSource;

use super::{AgentStorage, WHAT, call_raw, unexpected};

pub struct AgentDockerData {
    storage: AgentStorage,
    /// One that waits as long as copying takes.
    long: Arc<dyn AgentTransport>,
}

impl AgentDockerData {
    pub fn new(storage: AgentStorage, long: Arc<dyn AgentTransport>) -> Self {
        Self { storage, long }
    }
}

/// The volume of `/volume<N>/@docker`.
fn volume_of(data_root: &str) -> Option<u16> {
    data_root.strip_prefix("/volume")?.strip_suffix("/@docker")?.parse().ok()
}

#[async_trait]
impl DockerData for AgentDockerData {
    async fn places(
        &self,
        context: &RequestContext,
    ) -> Result<(Option<DataPlace>, Vec<DataPlace>), PortError> {
        let state = self.storage.state(context).await?;
        let pools = self.storage.pools(context).await?;
        let places: Vec<DataPlace> = pools
            .into_iter()
            .filter_map(|pool| {
                Some(DataPlace {
                    volume: pool.volume?.get(),
                    available_bytes: pool.space.map(|space| space.available_bytes),
                    pool: pool.name,
                })
            })
            .collect();
        let current = state
            .data_root
            .as_deref()
            .and_then(volume_of)
            .and_then(|volume| places.iter().find(|place| place.volume == volume).cloned());
        Ok((current, places))
    }

    async fn move_to(
        &self,
        from: &DataPlace,
        to: &DataPlace,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let invalid =
            |what: &str| EngineError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")));
        let request = Request::MoveDocker {
            from_pool: PoolName::parse(&from.pool).map_err(|_| invalid("pool"))?,
            from_volume: VolumeNumber::new(from.volume).map_err(|_| invalid("volume"))?,
            to_pool: PoolName::parse(&to.pool).map_err(|_| invalid("pool"))?,
            to_volume: VolumeNumber::new(to.volume).map_err(|_| invalid("volume"))?,
        };
        match call_raw(self.long.as_ref(), request, context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
                Err(EngineError::Refused(message))
            }
            Response::Error { message, .. } => Err(EngineError::Failed(message)),
            other => Err(unexpected(other).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[test]
    fn the_volume_comes_from_dockers_data_root() {
        assert_eq!(volume_of("/volume2/@docker"), Some(2));
        assert_eq!(volume_of("/var/lib/docker"), None);
    }

    #[tokio::test]
    async fn dockers_data_moves_through_the_agent() {
        let agent: Arc<dyn AgentTransport> = Arc::new(SimulatedAgent::new());
        let context = RequestContext::default();
        let storage = AgentStorage::new(agent.clone());
        let tank = zendiloos_apps::domain::AppVolume { pool: "tank".into(), number: 1 };
        storage.enable(&tank, &context).await.unwrap();
        let data = AgentDockerData::new(AgentStorage::new(agent.clone()), agent);

        let (current, places) = data.places(&context).await.unwrap();
        let current = current.expect("on tank");
        let other = places.into_iter().find(|p| p.pool != current.pool).expect("another pool");
        data.move_to(&current, &other, &context).await.unwrap();

        let (now, _) = data.places(&context).await.unwrap();
        assert_eq!(now.map(|p| p.pool), Some(other.pool));
    }
}
