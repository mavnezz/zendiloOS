//! Reads. Nothing in this context changes the host yet.

use std::sync::Arc;

use zendiloos_kernel::{PortError, RequestContext};

use crate::application::StorageError;
use crate::application::ports::{ArraySource, BlockDeviceSource, SmartSource, ZfsSource};
use crate::domain::{self, Array, BlockDevice, Disk, Pool, SmartReport, Volume};

/// Block devices and pools with resolved disks. Without ZFS (the module
/// failed to build, say) disks and other volumes are still shown.
pub(crate) async fn inventory(
    devices: &dyn BlockDeviceSource,
    zfs: &dyn ZfsSource,
    context: &RequestContext,
) -> Result<(Vec<BlockDevice>, Vec<Pool>), PortError> {
    let devices = devices.block_devices(context).await?;
    let mut pools = zfs.pools(context).await.unwrap_or_else(|error| {
        tracing::warn!(error = %error_chain(&error), "ZFS pools unavailable; showing disks without them");
        Vec::new()
    });
    domain::resolve_pool_disks(&mut pools, &devices);
    Ok((devices, pools))
}

fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

/// Query: all disks with their usage.
#[derive(Clone)]
pub struct ListDisks {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
}

impl ListDisks {
    pub fn new(devices: Arc<dyn BlockDeviceSource>, zfs: Arc<dyn ZfsSource>) -> Self {
        Self { devices, zfs }
    }

    pub async fn execute(&self, context: &RequestContext) -> Result<Vec<Disk>, PortError> {
        let (devices, pools) = inventory(self.devices.as_ref(), self.zfs.as_ref(), context).await?;
        Ok(domain::disks(&devices, &pools))
    }
}

/// Query: the ZFS pools with their devices' disks.
#[derive(Clone)]
pub struct ListPools {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
}

impl ListPools {
    pub fn new(devices: Arc<dyn BlockDeviceSource>, zfs: Arc<dyn ZfsSource>) -> Self {
        Self { devices, zfs }
    }

    /// Unlike the other queries, fails if ZFS is unavailable: that is the
    /// answer to "what about my pools".
    pub async fn execute(&self, context: &RequestContext) -> Result<Vec<Pool>, PortError> {
        let mut pools = self.zfs.pools(context).await?;
        let devices = self.devices.block_devices(context).await?;
        domain::resolve_pool_disks(&mut pools, &devices);
        pools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(pools)
    }
}

/// Query: the SMART report of one disk.
#[derive(Clone)]
pub struct GetSmartReport {
    devices: Arc<dyn BlockDeviceSource>,
    smart: Arc<dyn SmartSource>,
}

impl GetSmartReport {
    pub fn new(devices: Arc<dyn BlockDeviceSource>, smart: Arc<dyn SmartSource>) -> Self {
        Self { devices, smart }
    }

    /// Only a disk the host currently has is asked about, so a name from a
    /// request never reaches the agent unchecked.
    pub async fn execute(&self, disk: &str, context: &RequestContext) -> Result<SmartReport, StorageError> {
        let disks = domain::disks(&self.devices.block_devices(context).await?, &[]);
        let disk = disks.iter().find(|d| d.name == disk).ok_or(StorageError::DiskNotFound)?;
        Ok(self.smart.smart_report(&disk.name, context).await?)
    }
}

/// Query: the software RAID arrays.
#[derive(Clone)]
pub struct ListArrays {
    arrays: Arc<dyn ArraySource>,
}

impl ListArrays {
    pub fn new(arrays: Arc<dyn ArraySource>) -> Self {
        Self { arrays }
    }

    pub async fn execute(&self, context: &RequestContext) -> Result<Vec<Array>, PortError> {
        let mut arrays = self.arrays.arrays(context).await?;
        arrays.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(arrays)
    }
}

/// Query: the data volumes.
#[derive(Clone)]
pub struct ListVolumes {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
}

impl ListVolumes {
    pub fn new(devices: Arc<dyn BlockDeviceSource>, zfs: Arc<dyn ZfsSource>) -> Self {
        Self { devices, zfs }
    }

    pub async fn execute(&self, context: &RequestContext) -> Result<Vec<Volume>, PortError> {
        let (devices, pools) = inventory(self.devices.as_ref(), self.zfs.as_ref(), context).await?;
        Ok(domain::volumes(&devices, &pools))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::domain::{BlockDevice, DeviceKind};

    struct Fake {
        asked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl BlockDeviceSource for Fake {
        async fn block_devices(&self, _: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
            let partition =
                BlockDevice { name: "sda1".into(), kind: DeviceKind::Partition, children: vec![], ..disk() };
            Ok(vec![BlockDevice { children: vec![partition], ..disk() }])
        }
    }

    #[async_trait]
    impl SmartSource for Fake {
        async fn smart_report(&self, disk: &str, _: &RequestContext) -> Result<SmartReport, PortError> {
            self.asked.lock().unwrap().push(disk.to_owned());
            Ok(SmartReport::Standby)
        }
    }

    fn disk() -> BlockDevice {
        BlockDevice {
            name: "sda".into(),
            kind: DeviceKind::Disk,
            size_bytes: 1 << 30,
            model: None,
            serial: None,
            wwn: None,
            id_link: None,
            transport: None,
            rotational: true,
            removable: false,
            read_only: false,
            filesystem: None,
            mount_points: vec![],
            children: vec![],
        }
    }

    #[tokio::test]
    async fn smart_is_read_only_for_existing_disks() {
        let fake = Arc::new(Fake { asked: Mutex::new(vec![]) });
        let query = GetSmartReport::new(fake.clone(), fake.clone());
        let context = RequestContext::default();

        assert_eq!(query.execute("sda", &context).await.unwrap(), SmartReport::Standby);
        for name in ["sdb", "sda1", "../sda", ""] {
            assert!(matches!(query.execute(name, &context).await, Err(StorageError::DiskNotFound)), "{name}");
        }
        assert_eq!(*fake.asked.lock().unwrap(), ["sda"]);
    }
}
