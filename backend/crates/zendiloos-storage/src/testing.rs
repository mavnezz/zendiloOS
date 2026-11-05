//! A fake host for tests: one system disk and a RAID 1 on two disks with a
//! Btrfs volume.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::application::StoragePorts;
use crate::application::ports::{ArraySource, BlockDeviceSource, SmartSource};
use crate::domain::{
    Array, ArrayMember, BlockDevice, DeviceKind, Filesystem, MemberState, Slots, SmartData, SmartProtocol,
    SmartReport, SpaceUsage,
};

pub struct FakeStorage;

impl FakeStorage {
    pub fn ports() -> StoragePorts {
        let fake = std::sync::Arc::new(FakeStorage);
        StoragePorts { devices: fake.clone(), smart: fake.clone(), arrays: fake }
    }
}

fn device(name: &str, kind: DeviceKind) -> BlockDevice {
    BlockDevice {
        name: name.to_owned(),
        kind,
        size_bytes: 4_000_787_030_016,
        model: None,
        serial: None,
        wwn: None,
        transport: None,
        rotational: true,
        removable: false,
        read_only: false,
        filesystem: None,
        mount_points: Vec::new(),
        children: Vec::new(),
    }
}

fn filesystem(kind: &str, label: Option<&str>, space: Option<SpaceUsage>) -> Option<Filesystem> {
    Some(Filesystem {
        kind: kind.to_owned(),
        label: label.map(str::to_owned),
        uuid: Some(format!("uuid-{kind}")),
        space,
    })
}

fn member(name: &str, serial: &str) -> BlockDevice {
    let array = BlockDevice {
        filesystem: filesystem(
            "btrfs",
            Some("data"),
            Some(SpaceUsage { size_bytes: 4 << 40, used_bytes: 1 << 40, available_bytes: 3 << 40 }),
        ),
        mount_points: vec!["/mnt/data".to_owned()],
        ..device("md0", DeviceKind::Raid { level: "raid1".to_owned() })
    };
    BlockDevice {
        model: Some("TEST HDD".to_owned()),
        serial: Some(serial.to_owned()),
        transport: Some("sata".to_owned()),
        filesystem: filesystem("linux_raid_member", None, None),
        children: vec![array],
        ..device(name, DeviceKind::Disk)
    }
}

#[async_trait]
impl BlockDeviceSource for FakeStorage {
    async fn block_devices(&self, _: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
        let root = BlockDevice {
            filesystem: filesystem("ext4", None, None),
            mount_points: vec!["/".to_owned()],
            ..device("nvme0n1p1", DeviceKind::Partition)
        };
        let system = BlockDevice {
            transport: Some("nvme".to_owned()),
            rotational: false,
            children: vec![root],
            ..device("nvme0n1", DeviceKind::Disk)
        };
        Ok(vec![member("sda", "T1"), member("sdb", "T2"), system])
    }
}

#[async_trait]
impl SmartSource for FakeStorage {
    async fn smart_report(&self, disk: &str, _: &RequestContext) -> Result<SmartReport, PortError> {
        if disk == "sdb" {
            return Ok(SmartReport::Standby);
        }
        Ok(SmartReport::Available(Box::new(SmartData {
            protocol: if disk.starts_with("nvme") { SmartProtocol::Nvme } else { SmartProtocol::Ata },
            model: Some("TEST HDD".to_owned()),
            serial: None,
            firmware: None,
            passed: Some(true),
            temperature_celsius: Some(33),
            power_on_hours: Some(1200),
            power_cycles: Some(12),
            attributes: Vec::new(),
            nvme: None,
            error_log_count: Some(0),
            last_self_test: None,
        })))
    }
}

#[async_trait]
impl ArraySource for FakeStorage {
    async fn arrays(&self, _: &RequestContext) -> Result<Vec<Array>, PortError> {
        let member = |device: &str, index| ArrayMember {
            device: device.to_owned(),
            index,
            state: MemberState::Active,
        };
        Ok(vec![Array {
            name: "md0".to_owned(),
            level: Some("raid1".to_owned()),
            active: true,
            read_only: false,
            size_bytes: Some(4 << 40),
            slots: Some(Slots { total: 2, working: 2 }),
            members: vec![member("sda", 0), member("sdb", 1)],
            sync: None,
        }])
    }
}
