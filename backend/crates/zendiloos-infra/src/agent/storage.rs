use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, DiskId, DiskName, ErrorCode, Request, Response};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::{
    AdminError, ArraySource, BlockDeviceSource, NewPool, PlanTokens, PoolAdmin, SmartSource, ZfsSource,
};
use zendiloos_storage::domain::{Array, BlockDevice, Pool, PoolLayout, PoolName, SmartReport, VolumeNumber};

use super::{WHAT, call, call_raw, unexpected};
use crate::storage::{lsblk, mdstat, smartctl, zfs};

/// The Storage context's sources, through the agent: it runs the tools,
/// this side parses their output.
pub struct AgentStorage {
    transport: Arc<dyn AgentTransport>,
}

impl AgentStorage {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl BlockDeviceSource for AgentStorage {
    async fn block_devices(&self, context: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
        match call(self.transport.as_ref(), Request::ListBlockDevices {}, context).await? {
            Response::BlockDevices { lsblk_json } => {
                lsblk::parse(&lsblk_json).map_err(|e| PortError::new("block device list", e))
            }
            other => Err(unexpected(other)),
        }
    }
}

#[async_trait]
impl SmartSource for AgentStorage {
    async fn smart_report(&self, disk: &str, context: &RequestContext) -> Result<SmartReport, PortError> {
        let device = DiskName::parse(disk).map_err(|e| PortError::new("SMART data", e))?;
        match call(self.transport.as_ref(), Request::SmartData { device }, context).await? {
            Response::Smart { exit_status, smartctl_json } => {
                smartctl::parse(exit_status, &smartctl_json).map_err(|e| PortError::new("SMART data", e))
            }
            other => Err(unexpected(other)),
        }
    }
}

#[async_trait]
impl ArraySource for AgentStorage {
    async fn arrays(&self, context: &RequestContext) -> Result<Vec<Array>, PortError> {
        match call(self.transport.as_ref(), Request::ArrayStatus {}, context).await? {
            Response::ArrayStatus { mdstat } => {
                mdstat::parse(&mdstat).map_err(|e| PortError::new("RAID status", e))
            }
            other => Err(unexpected(other)),
        }
    }
}

#[async_trait]
impl ZfsSource for AgentStorage {
    async fn pools(&self, context: &RequestContext) -> Result<Vec<Pool>, PortError> {
        match call(self.transport.as_ref(), Request::ZfsStatus {}, context).await? {
            Response::Zfs { zpool_status, zfs_list } => {
                zfs::parse(&zpool_status, &zfs_list).map_err(|e| PortError::new("ZFS pools", e))
            }
            other => Err(unexpected(other)),
        }
    }
}

impl AgentStorage {
    /// Changes: a refusal by the agent's own checks is not a failure of
    /// the agent, and the user should see its reason.
    async fn change(&self, request: Request, context: &RequestContext) -> Result<(), AdminError> {
        match call_raw(self.transport.as_ref(), request, context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden, message } => Err(AdminError::Refused(message)),
            Response::Error { code: ErrorCode::Conflict, .. } => Err(AdminError::Conflict),
            Response::Error { code, message } => Err(super::agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }
}

fn invalid(what: &str) -> AdminError {
    AdminError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn pool_name(name: &PoolName) -> Result<proto::PoolName, AdminError> {
    proto::PoolName::parse(name.as_str()).map_err(|_| invalid("pool name"))
}

fn volume(number: VolumeNumber) -> Result<proto::VolumeNumber, AdminError> {
    proto::VolumeNumber::new(number.get()).map_err(|_| invalid("volume number"))
}

fn disk_id(link: &str) -> Result<DiskId, AdminError> {
    DiskId::parse(link).map_err(|_| invalid("disk"))
}

#[async_trait]
impl PoolAdmin for AgentStorage {
    async fn create_pool(&self, pool: &NewPool, context: &RequestContext) -> Result<(), AdminError> {
        let layout = match pool.layout {
            PoolLayout::Single => proto::Layout::Single,
            PoolLayout::Mirror => proto::Layout::Mirror,
            PoolLayout::Raidz { parity: 1 } => proto::Layout::Raidz1,
            PoolLayout::Raidz { parity: 2 } => proto::Layout::Raidz2,
            PoolLayout::Raidz { parity: 3 } => proto::Layout::Raidz3,
            _ => return Err(invalid("layout")),
        };
        let disks = pool.disks.iter().map(|link| disk_id(link)).collect::<Result<_, _>>()?;
        let request =
            Request::CreatePool { name: pool_name(&pool.name)?, layout, disks, volume: volume(pool.volume)? };
        self.change(request, context).await
    }

    async fn destroy_pool(
        &self,
        pool: &PoolName,
        number: VolumeNumber,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        self.change(Request::DestroyPool { name: pool_name(pool)?, volume: volume(number)? }, context).await
    }

    async fn replace_disk(
        &self,
        pool: &PoolName,
        device: &str,
        disk: &str,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        let device = device.parse().map_err(|_| invalid("device GUID"))?;
        let request = Request::ReplaceDisk { pool: pool_name(pool)?, device, disk: disk_id(disk)? };
        self.change(request, context).await
    }

    async fn scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError> {
        self.change(Request::ScrubPool { pool: pool_name(pool)? }, context).await
    }

    async fn stop_scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError> {
        self.change(Request::StopScrub { pool: pool_name(pool)? }, context).await
    }
}

/// Random plan tokens: 128 bits, URL-safe.
pub struct RandomPlanTokens;

impl PlanTokens for RandomPlanTokens {
    fn token(&self) -> String {
        use base64::Engine as _;
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("the system's random source is available");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;
    use zendiloos_storage::domain::{DiskUsage, Health, VdevState, disks, resolve_pool_disks, volumes};

    use super::*;

    #[tokio::test]
    async fn reads_the_simulated_nas() {
        let storage = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();

        let devices = storage.block_devices(&context).await.unwrap();
        let mut pools = storage.pools(&context).await.unwrap();
        resolve_pool_disks(&mut pools, &devices);
        let usage: Vec<_> = disks(&devices, &pools).into_iter().map(|d| (d.name, d.usage)).collect();
        let member = |pool: &str| DiskUsage::PoolMember { pool: pool.into() };
        assert_eq!(
            usage,
            [
                ("sda".into(), member("tank")),
                ("sdb".into(), member("tank")),
                ("sdc".into(), member("backup")),
                ("sdd".into(), member("backup")),
                ("sde".into(), DiskUsage::Unused),
                ("nvme0n1".into(), DiskUsage::System),
                ("nvme1n1".into(), DiskUsage::Unused),
            ]
        );
        let states: Vec<_> =
            pools.iter().map(|p| (p.name.as_str(), p.state, p.volume.map(|v| v.get()))).collect();
        assert_eq!(states, [("backup", VdevState::Degraded, Some(2)), ("tank", VdevState::Online, Some(1))]);
        assert_eq!(pools[0].devices[1].state, VdevState::Faulted);
        let mounts: Vec<_> = volumes(&devices, &pools).into_iter().flat_map(|v| v.mount_points).collect();
        assert_eq!(mounts, ["/volume2", "/volume1"]);

        let health = |report: SmartReport| match report {
            SmartReport::Available(data) => Some(data.assess().health),
            _ => None,
        };
        let smart = |disk: &'static str| storage.smart_report(disk, &context);
        assert_eq!(health(smart("sda").await.unwrap()), Some(Health::Good));
        assert_eq!(health(smart("sdd").await.unwrap()), Some(Health::Failing));
        assert_eq!(smart("sde").await.unwrap(), SmartReport::Standby);
        assert_eq!(health(smart("nvme1n1").await.unwrap()), Some(Health::Good));
        assert_eq!(storage.arrays(&context).await.unwrap(), []);
    }

    #[tokio::test]
    async fn never_sends_a_name_that_is_not_a_disk() {
        let agent = Arc::new(SimulatedAgent::new());
        let storage = AgentStorage::new(agent.clone());
        assert!(storage.smart_report("sda1", &RequestContext::default()).await.is_err());
        assert_eq!(agent.received(), []);
    }
}
