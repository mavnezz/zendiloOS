use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ErrorCode, GroupName, NfsClient, NfsExport, Request, Response,
    ShareAccess, SmbShare, UserName,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_sharing::application::ports::{HostError, ShareHost, Volume, Volumes};
use zendiloos_sharing::domain::{Access, Principal, ShareName, SharingConfig};
use zendiloos_storage::application::ports::ZfsSource;

use super::{WHAT, agent_error, call_raw, unexpected};
use crate::agent::AgentStorage;

/// [`ShareHost`] through the agents: datasets and server configuration
/// through the system agent, directory permissions through the files agent.
pub struct AgentSharing {
    system: Arc<dyn AgentTransport>,
    files: Arc<dyn AgentTransport>,
}

impl AgentSharing {
    pub fn new(system: Arc<dyn AgentTransport>, files: Arc<dyn AgentTransport>) -> Self {
        Self { system, files }
    }
}

async fn change(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<(), HostError> {
    match call_raw(transport, request, context).await? {
        Response::Done => Ok(()),
        Response::Error { code: ErrorCode::Forbidden, message } => Err(HostError::Refused(message)),
        Response::Error { code: ErrorCode::Conflict, .. } => Err(HostError::Conflict),
        Response::Error { code, message } => Err(agent_error(code, &message).into()),
        other => Err(unexpected(other).into()),
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn share(name: &ShareName) -> Result<proto::ShareName, HostError> {
    proto::ShareName::parse(name.as_str()).map_err(|_| invalid("share name"))
}

fn volume(number: u16) -> Result<proto::VolumeNumber, HostError> {
    proto::VolumeNumber::new(number).map_err(|_| invalid("volume number"))
}

fn pool(name: &str) -> Result<proto::PoolName, HostError> {
    proto::PoolName::parse(name).map_err(|_| invalid("pool name"))
}

fn principal(principal: &Principal) -> Result<proto::Principal, HostError> {
    Ok(match principal {
        Principal::User(name) => {
            proto::Principal::User(UserName::parse(name).map_err(|_| invalid("user name"))?)
        }
        Principal::Group(name) => {
            proto::Principal::Group(GroupName::parse(name).map_err(|_| invalid("group name"))?)
        }
    })
}

fn access(rules: &[(Principal, Access)]) -> Result<Vec<ShareAccess>, HostError> {
    rules
        .iter()
        .map(|(p, a)| Ok(ShareAccess { principal: principal(p)?, write: *a == Access::ReadWrite }))
        .collect()
}

#[async_trait]
impl ShareHost for AgentSharing {
    async fn create_dataset(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request =
            Request::CreateShare { pool: pool(pool_name)?, volume: volume(number)?, name: share(name)? };
        change(self.system.as_ref(), request, context).await
    }

    async fn destroy_dataset(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request =
            Request::DestroyShare { pool: pool(pool_name)?, volume: volume(number)?, name: share(name)? };
        change(self.system.as_ref(), request, context).await
    }

    async fn apply(&self, config: &SharingConfig, context: &RequestContext) -> Result<(), HostError> {
        let smb = config
            .smb
            .iter()
            .map(|spec| {
                Ok(SmbShare {
                    name: share(&spec.name)?,
                    volume: volume(spec.volume)?,
                    access: access(&spec.access)?,
                    recycle_bin: spec.recycle_bin,
                })
            })
            .collect::<Result<_, HostError>>()?;
        let nfs = config
            .nfs
            .iter()
            .map(|spec| {
                let clients = spec
                    .clients
                    .iter()
                    .map(|c| {
                        let network = proto::Network::parse(&c.network).map_err(|_| invalid("network"))?;
                        Ok(NfsClient { network, write: c.write })
                    })
                    .collect::<Result<_, HostError>>()?;
                Ok(NfsExport { name: share(&spec.name)?, volume: volume(spec.volume)?, clients })
            })
            .collect::<Result<_, HostError>>()?;
        change(self.system.as_ref(), Request::ApplySharing { smb, nfs }, context).await
    }

    async fn set_access(
        &self,
        number: u16,
        name: &ShareName,
        rules: &[(Principal, Access)],
        removed: &[Principal],
        recursive: bool,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::SetShareAccess {
            volume: volume(number)?,
            name: share(name)?,
            access: access(rules)?,
            removed: removed.iter().map(principal).collect::<Result<_, _>>()?,
            recursive,
        };
        change(self.files.as_ref(), request, context).await
    }
}

/// Volumes for shares: the pools zendiloos created.
#[async_trait]
impl Volumes for AgentStorage {
    async fn volumes(&self, context: &RequestContext) -> Result<Vec<Volume>, PortError> {
        Ok(self
            .pools(context)
            .await?
            .into_iter()
            .filter_map(|pool| {
                let number = pool.volume?.get();
                Some(Volume { pool: pool.name, number, mounted: pool.space.is_some_and(|s| s.mounted) })
            })
            .collect())
    }
}
