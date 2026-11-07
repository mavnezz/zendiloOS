//! A fake host for tests: one system disk, a RAID 1 on two disks with a
//! Btrfs volume, a ZFS pool on a third disk, mounted as volume 1, and two
//! unused disks (`sdd`, `sde`). Changes to pools are recorded, not made.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AuditLog, Clock, PortError, RequestContext};

use crate::application::StoragePorts;
use crate::application::ports::{
    AdminError, ArraySource, BlockDeviceSource, NewPool, PlanTokens, PoolAdmin, SelfTestPlanStore,
    SmartSource, VolumeOccupants, ZfsSource,
};
use crate::domain::{
    Array, ArrayMember, BlockDevice, DeviceKind, Filesystem, MemberState, Occupant, Pool, PoolDevice,
    PoolLayout, PoolSpace, Scan, ScanKind, ScanState, SelfTestKind, Slots, SmartData, SmartProtocol,
    SmartReport, SpaceUsage, VdevState, VolumeNumber,
};

#[derive(Default)]
pub struct FakeStorage {
    /// Pool changes asked for, e.g. `create tank mirror ata-A,ata-B 2`.
    pub calls: Mutex<Vec<String>>,
    /// Serial number reported for `sdd`; change it to swap the disk.
    pub sdd_serial: Mutex<String>,
    /// Pool `tank`'s scan: a scrub runs once one was started, until stopped.
    pub scan: Mutex<Option<Scan>>,
    /// Disks testing themselves: from a start until stopped.
    pub testing: Mutex<Vec<String>>,
    /// What other contexts keep on each volume.
    pub occupants: Mutex<Vec<(u16, Occupant)>>,
    tokens: AtomicU64,
}

impl FakeStorage {
    pub fn ports(
        jobs: Jobs,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> (StoragePorts, Arc<FakeStorage>) {
        let notifier = zendiloos_notify::Notifier::new(
            Arc::new(zendiloos_notify::testing::MemoryNotifications::default()),
            clock.clone(),
        );
        let fake =
            Arc::new(FakeStorage { sdd_serial: Mutex::new("T4".to_owned()), ..FakeStorage::default() });
        let ports = StoragePorts {
            devices: fake.clone(),
            smart: fake.clone(),
            arrays: fake.clone(),
            zfs: fake.clone(),
            admin: fake.clone(),
            jobs,
            audit,
            clock,
            tokens: fake.clone(),
            notifier,
            self_test_plans: Arc::new(MemorySelfTestPlans::default()),
            occupants: fake.clone(),
        };
        (ports, fake)
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

fn unused(name: &str, serial: &str) -> BlockDevice {
    BlockDevice {
        model: Some("TEST HDD".to_owned()),
        serial: Some(serial.to_owned()),
        transport: Some("sata".to_owned()),
        id_link: Some(format!("ata-TEST_HDD_{serial}")),
        ..device(name, DeviceKind::Disk)
    }
}

#[async_trait]
impl PoolAdmin for FakeStorage {
    async fn create_pool(&self, pool: &NewPool, _: &RequestContext) -> Result<(), AdminError> {
        self.record(format!(
            "create {} {:?} {} {}",
            pool.name,
            pool.layout,
            pool.disks.join(","),
            pool.volume.get()
        ));
        Ok(())
    }

    async fn destroy_pool(
        &self,
        pool: &crate::domain::PoolName,
        volume: VolumeNumber,
        _: &RequestContext,
    ) -> Result<(), AdminError> {
        self.record(format!("destroy {pool} {}", volume.get()));
        Ok(())
    }

    async fn replace_disk(
        &self,
        pool: &crate::domain::PoolName,
        device: &str,
        disk: &str,
        _: &RequestContext,
    ) -> Result<(), AdminError> {
        self.record(format!("replace {pool} {device} {disk}"));
        Ok(())
    }

    async fn scrub(&self, pool: &crate::domain::PoolName, _: &RequestContext) -> Result<(), AdminError> {
        self.record(format!("scrub {pool}"));
        *self.scan.lock().unwrap() = Some(Scan {
            kind: ScanKind::Scrub,
            state: ScanState::Running,
            done_bytes: 0,
            total_bytes: 1 << 30,
            started_at: zendiloos_kernel::Timestamp::from_unix_seconds(0),
            finished_at: None,
            errors: 0,
            pass: None,
        });
        Ok(())
    }

    async fn stop_scrub(&self, pool: &crate::domain::PoolName, _: &RequestContext) -> Result<(), AdminError> {
        self.record(format!("stop scrub {pool}"));
        if let Some(scan) = self.scan.lock().unwrap().as_mut() {
            scan.state = ScanState::Canceled;
        }
        Ok(())
    }
}

#[async_trait]
impl VolumeOccupants for FakeStorage {
    async fn on_volume(&self, volume: VolumeNumber, _: &RequestContext) -> Result<Vec<Occupant>, PortError> {
        let occupants = self.occupants.lock().unwrap();
        Ok(occupants.iter().filter(|(v, _)| *v == volume.get()).map(|(_, o)| o.clone()).collect())
    }
}

impl PlanTokens for FakeStorage {
    fn token(&self) -> String {
        format!("plan-{}", self.tokens.fetch_add(1, Ordering::Relaxed) + 1)
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
        id_link: None,
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
        let zfs_partition = BlockDevice {
            filesystem: filesystem("zfs_member", Some("tank"), None),
            id_link: Some("ata-TEST_HDD_T3-part1".to_owned()),
            ..device("sdc1", DeviceKind::Partition)
        };
        let zfs_disk = BlockDevice {
            model: Some("TEST HDD".to_owned()),
            serial: Some("T3".to_owned()),
            transport: Some("sata".to_owned()),
            id_link: Some("ata-TEST_HDD_T3".to_owned()),
            children: vec![zfs_partition],
            ..device("sdc", DeviceKind::Disk)
        };
        let sdd_serial = self.sdd_serial.lock().unwrap().clone();
        Ok(vec![
            member("sda", "T1"),
            member("sdb", "T2"),
            zfs_disk,
            unused("sdd", &sdd_serial),
            unused("sde", "T5"),
            system,
        ])
    }
}

#[async_trait]
impl SmartSource for FakeStorage {
    async fn start_self_test(
        &self,
        disk: &str,
        kind: SelfTestKind,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        self.record(format!("self-test {disk} {kind:?}"));
        self.testing.lock().unwrap().push(disk.to_owned());
        Ok(())
    }

    async fn abort_self_test(&self, disk: &str, _: &RequestContext) -> Result<(), PortError> {
        self.record(format!("abort self-test {disk}"));
        self.testing.lock().unwrap().retain(|d| d != disk);
        Ok(())
    }

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
            running_self_test: self
                .testing
                .lock()
                .unwrap()
                .iter()
                .any(|d| d == disk)
                .then_some(crate::domain::RunningSelfTest { remaining_percent: Some(90) }),
            self_test_minutes: None,
            self_tests_supported: !disk.starts_with("nvme"),
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

#[async_trait]
impl ZfsSource for FakeStorage {
    async fn pools(&self, _: &RequestContext) -> Result<Vec<Pool>, PortError> {
        let device = PoolDevice {
            name: "ata-TEST_HDD_T3-part1".to_owned(),
            guid: "4933705814018298948".to_owned(),
            path: Some("/dev/disk/by-id/ata-TEST_HDD_T3-part1".to_owned()),
            disk: None,
            state: VdevState::Online,
            read_errors: 0,
            write_errors: 0,
            checksum_errors: 0,
            replacing: None,
        };
        Ok(vec![Pool {
            name: "tank".to_owned(),
            guid: "9846106214133009000".to_owned(),
            state: VdevState::Online,
            layout: PoolLayout::Single,
            devices: vec![device],
            size_bytes: 4 << 40,
            allocated_bytes: 1 << 40,
            scan: self.scan.lock().unwrap().clone(),
            data_errors: 0,
            volume: VolumeNumber::new(1),
            space: Some(PoolSpace { used_bytes: 1 << 40, available_bytes: 3 << 40, mounted: true }),
        }])
    }
}

/// [`SelfTestPlanStore`] in memory.
#[derive(Default)]
pub struct MemorySelfTestPlans {
    plans: Mutex<Vec<crate::domain::ScheduledSelfTest>>,
}

#[async_trait]
impl SelfTestPlanStore for MemorySelfTestPlans {
    async fn plans(&self) -> Result<Vec<crate::domain::ScheduledSelfTest>, PortError> {
        Ok(self.plans.lock().unwrap().clone())
    }

    async fn save(&self, plans: &[crate::domain::SelfTestPlan]) -> Result<(), PortError> {
        let mut stored = self.plans.lock().unwrap();
        let next = plans
            .iter()
            .map(|plan| crate::domain::ScheduledSelfTest {
                plan: plan.clone(),
                last_run: stored.iter().find(|s| s.plan.kind == plan.kind).and_then(|s| s.last_run),
            })
            .collect();
        *stored = next;
        Ok(())
    }

    async fn ran(&self, kind: SelfTestKind, at: zendiloos_kernel::Timestamp) -> Result<(), PortError> {
        for scheduled in self.plans.lock().unwrap().iter_mut().filter(|s| s.plan.kind == kind) {
            scheduled.last_run = Some(at);
        }
        Ok(())
    }
}
