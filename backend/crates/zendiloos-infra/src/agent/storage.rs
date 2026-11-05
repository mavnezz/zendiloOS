use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{AgentTransport, DiskName, Request, Response};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::{ArraySource, BlockDeviceSource, SmartSource};
use zendiloos_storage::domain::{Array, BlockDevice, SmartReport};

use super::{call, unexpected};
use crate::storage::{lsblk, mdstat, smartctl};

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

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;
    use zendiloos_storage::domain::{ArrayHealth, DiskUsage, Health, disks, volumes};

    use super::*;

    #[tokio::test]
    async fn reads_the_simulated_nas() {
        let storage = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();

        let devices = storage.block_devices(&context).await.unwrap();
        let usage: Vec<_> = disks(&devices).into_iter().map(|d| (d.name, d.usage)).collect();
        let member = |array: &str| DiskUsage::ArrayMember { array: array.into() };
        assert_eq!(
            usage,
            [
                ("sda".into(), member("md0")),
                ("sdb".into(), member("md0")),
                ("sdc".into(), member("md1")),
                ("sdd".into(), member("md1")),
                ("sde".into(), DiskUsage::Unused),
                ("nvme0n1".into(), DiskUsage::System),
                ("nvme1n1".into(), DiskUsage::Unused),
            ]
        );
        let labels: Vec<_> = volumes(&devices).into_iter().filter_map(|v| v.label).collect();
        assert_eq!(labels, ["data", "backup"]);

        let health = |report: SmartReport| match report {
            SmartReport::Available(data) => Some(data.assess().health),
            _ => None,
        };
        let smart = |disk: &'static str| storage.smart_report(disk, &context);
        assert_eq!(health(smart("sda").await.unwrap()), Some(Health::Good));
        assert_eq!(health(smart("sdd").await.unwrap()), Some(Health::Failing));
        assert_eq!(smart("sde").await.unwrap(), SmartReport::Standby);
        assert_eq!(health(smart("nvme1n1").await.unwrap()), Some(Health::Good));

        let arrays: Vec<_> = storage.arrays(&context).await.unwrap().iter().map(|a| a.health()).collect();
        assert_eq!(arrays, [ArrayHealth::Healthy, ArrayHealth::Degraded]);
    }

    #[tokio::test]
    async fn never_sends_a_name_that_is_not_a_disk() {
        let agent = Arc::new(SimulatedAgent::new());
        let storage = AgentStorage::new(agent.clone());
        assert!(storage.smart_report("sda1", &RequestContext::default()).await.is_err());
        assert_eq!(agent.received(), []);
    }
}
