//! A fake host for tests: one system disk, a RAID 1 on two disks with a
//! Btrfs volume, a ZFS pool on a third disk, mounted as volume 1, and two
//! unused disks (`sdd`, `sde`). Changes to pools are recorded, not made.
//! The disks sit at the ports of [`TEST_MODEL`]'s bays (`sde` at none of
//! them), and the machine is a QEMU one until a test says otherwise.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AuditLog, Clock, PortError, RequestContext};

use crate::application::ports::{
    AdminError, ArraySource, BlockDeviceSource, DiskInventoryStore, DiskLocator, EjectedDrives,
    EnclosureCatalog, EnclosureStore, MachineSource, NewPool, PlanTokens, PoolAdmin, RemovableHost, SeenDisk,
    SelfTestPlanStore, SmartSource, Steps, VolumeOccupants, ZfsSource,
};
use crate::application::{StoragePorts, StorageUseCases};
use crate::domain::{
    Array, ArrayMember, Binding, BlockDevice, Chassis, ChassisStyle, DeviceKind, DmiMatch, EnclosureChoice,
    EnclosureModel, Filesystem, FrontPanel, InventoryDetails, InventoryRecord, Layout, Machine, MemberState,
    Numbering, Occupant, Orientation, PanelSide, Pool, PoolDevice, PoolLayout, PoolSpace, Scan, ScanKind,
    ScanState, SelfTestKind, Slot, SlotForm, SlotGroup, Slots, SmartData, SmartProtocol, SmartReport,
    SpaceUsage, VdevState, VolumeNumber,
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
    /// A USB stick (`sdf`, FAT, id [`STICK_ID`]) while plugged in, and
    /// whether it is mounted.
    pub stick: Mutex<Option<bool>>,
    /// Ejecting the stick fails as for open files while set.
    pub stick_busy: Mutex<bool>,
    /// How often the stick was plugged in: each time udev sets its disk up
    /// anew.
    pub stick_plugged: Mutex<u64>,
    /// The drives ejected, as the database keeps them.
    pub ejected: Mutex<std::collections::BTreeMap<String, Option<String>>>,
    /// Reading or writing which drives are ejected fails while set.
    pub ejected_fails: Mutex<bool>,
    /// What the firmware calls the machine.
    pub machine: Mutex<Machine>,
    /// Reading the machine's names fails while set.
    pub machine_fails: Mutex<bool>,
    /// The disks read from to make them blink, one entry per burst.
    pub bursts: Mutex<Vec<String>>,
    /// Reading from a disk to make it blink fails while set.
    pub burst_fails: Mutex<bool>,
    /// The enclosure chosen.
    pub chosen: Mutex<Option<EnclosureChoice>>,
    /// The disks seen.
    pub inventory: Mutex<Vec<InventoryRecord>>,
    /// Serial numbers seen on two disks at once.
    pub shared_serials: Mutex<std::collections::BTreeSet<String>>,
    /// Reading the block devices fails while set.
    pub devices_fail: Mutex<bool>,
    /// A USB dock with two disks (`sdg`, `sdh`) whose bridge reports its
    /// own serial number, [`DOCK_SERIAL`], for both, while set.
    pub dock: Mutex<bool>,
    /// The dock's `sdg` is pulled out while set: `sdh` is there alone.
    pub dock_without_sdg: Mutex<bool>,
    /// While set, each SMART read waits for a permit of it (and gives it
    /// back), as a slow disk would.
    pub smart_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    tokens: AtomicU64,
}

/// The serial number both disks of [`FakeStorage::dock`] report.
pub const DOCK_SERIAL: &str = "BRIDGE1";

/// The model of [`test_catalog`]: four bays and two M.2 slots, the
/// machine `TEST` `NAS 4`.
pub const TEST_MODEL: &str = "test-nas4";

/// The port each fake disk is plugged into.
fn port_of(name: &str) -> Option<String> {
    let port = match name {
        "sda" => "pci-0000:05:00.0-ata-1.0",
        "sdb" => "pci-0000:05:00.0-ata-2.0",
        "sdc" => "pci-0000:05:00.0-ata-3.0",
        "sdd" => "pci-0000:05:00.0-ata-4.0",
        "sde" => "pci-0000:00:17.0-ata-1.0",
        "sdf" => "pci-0000:00:14.0-usb-0:1:1.0-scsi-0:0:0:0",
        "sdg" => "pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0:0",
        "sdh" => "pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0:1",
        "nvme0n1" => "pci-0000:01:00.0-nvme-1",
        _ => return None,
    };
    Some(port.to_owned())
}

/// One model, as the catalog would have it.
pub fn test_catalog() -> Vec<EnclosureModel> {
    let slot = |id: &str, label: &str, port: &str| Slot {
        id: id.to_owned(),
        label: label.to_owned(),
        binding: Some(Binding::Port(port.to_owned())),
    };
    let bays = SlotGroup {
        id: "bays".to_owned(),
        name: "Front bays".to_owned(),
        rows: 1,
        columns: 4,
        orientation: Orientation::Vertical,
        form: SlotForm::Inch35,
        numbering: Numbering::LeftToRight,
        slots: (1..=4)
            .map(|n| slot(&format!("bay{n}"), &format!("Bay {n}"), &format!("pci-0000:05:00.0-ata-{n}")))
            .collect(),
    };
    let m2 = SlotGroup {
        id: "m2".to_owned(),
        name: "M.2 slots".to_owned(),
        rows: 2,
        columns: 1,
        orientation: Orientation::Horizontal,
        form: SlotForm::M2,
        numbering: Numbering::TopToBottom,
        slots: vec![
            slot("m2-1", "M.2 1", "pci-0000:01:00.0-nvme-1"),
            slot("m2-2", "M.2 2", "pci-0000:02:00.0-nvme-1"),
        ],
    };
    vec![EnclosureModel {
        id: TEST_MODEL.to_owned(),
        vendor: "TEST".to_owned(),
        name: "NAS 4".to_owned(),
        dmi: DmiMatch { vendor: "TEST".to_owned(), product: "NAS 4".to_owned() },
        chassis: Chassis {
            style: ChassisStyle::Tower,
            panel: Some(FrontPanel {
                side: PanelSide::Left,
                power_button: true,
                leds: vec!["power".to_owned(), "network".to_owned()],
            }),
        },
        layout: Layout::new(vec![bays, m2]).expect("a valid layout"),
        verified: false,
        notes: "Made up for tests.".to_owned(),
    }]
}

/// [`test_catalog`] as the catalog port.
pub struct TestCatalog;

impl EnclosureCatalog for TestCatalog {
    fn models(&self) -> Vec<EnclosureModel> {
        test_catalog()
    }
}

/// Audit entries kept in memory.
#[derive(Default)]
pub struct RecordingAudit(Mutex<Vec<zendiloos_kernel::AuditEntry>>);

impl RecordingAudit {
    pub fn entries(&self) -> Vec<zendiloos_kernel::AuditEntry> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuditLog for RecordingAudit {
    async fn record(&self, entry: zendiloos_kernel::AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry);
        Ok(())
    }
}

/// The use cases on the fake host, at a fixed time.
pub fn use_cases() -> (StorageUseCases, Arc<FakeStorage>, Arc<RecordingAudit>) {
    let clock = Arc::new(zendiloos_jobs::testing::FixedClock);
    let jobs = Jobs::new(Arc::new(zendiloos_jobs::testing::MemoryJobs::default()), clock.clone());
    let audit = Arc::new(RecordingAudit::default());
    let (ports, fake) = FakeStorage::ports(jobs, audit.clone(), clock);
    (StorageUseCases::new(ports), fake, audit)
}

/// The id of the stick [`FakeStorage::plug_stick`] plugs in.
pub const STICK_ID: &str = "1A2B-3C4D";

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
        let fake = Arc::new(FakeStorage {
            sdd_serial: Mutex::new("T4".to_owned()),
            machine: Mutex::new(Machine {
                vendor: "QEMU".to_owned(),
                product: "Standard PC (Q35 + ICH9, 2009)".to_owned(),
            }),
            ..FakeStorage::default()
        });
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
            removable: fake.clone(),
            machine: fake.clone(),
            catalog: Arc::new(TestCatalog),
            enclosure: fake.clone(),
            locator: fake.clone(),
            inventory: fake.clone(),
            ejected: fake.clone(),
        };
        (ports, fake)
    }

    /// Plugs the USB stick in, not mounted.
    pub fn plug_stick(&self) {
        *self.stick_plugged.lock().unwrap() += 1;
        *self.stick.lock().unwrap() = Some(false);
    }

    /// The machine is now `vendor` `product`.
    pub fn set_machine(&self, vendor: &str, product: &str) {
        *self.machine.lock().unwrap() = Machine { vendor: vendor.to_owned(), product: product.to_owned() };
    }

    /// Stores a choice without the use case's checks, as an older release
    /// might have.
    pub fn choose_directly(&self, choice: EnclosureChoice) {
        *self.chosen.lock().unwrap() = Some(choice);
    }

    pub fn bursts(&self) -> Vec<String> {
        self.bursts.lock().unwrap().clone()
    }

    /// A disk seen once, last at `last_seen`.
    pub fn remember(&self, serial: &str, last_seen: i64) {
        self.inventory.lock().unwrap().push(InventoryRecord {
            serial: serial.to_owned(),
            model: Some("OLD HDD".to_owned()),
            size_bytes: 1 << 40,
            details: InventoryDetails::default(),
            first_seen: zendiloos_kernel::Timestamp::from_unix_seconds(last_seen - 86_400),
            last_seen: zendiloos_kernel::Timestamp::from_unix_seconds(last_seen),
        });
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
        id_path: port_of(name),
        ..device(name, DeviceKind::Disk)
    }
}

#[async_trait]
impl PoolAdmin for FakeStorage {
    async fn create_pool(
        &self,
        pool: &NewPool,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<(), AdminError> {
        steps.step("creating_pool", None);
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
        _: &dyn Steps,
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
        _: &dyn Steps,
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

fn stick(mounted: bool) -> BlockDevice {
    let space =
        SpaceUsage { size_bytes: 32_000_000_000, used_bytes: 7_400_000_000, available_bytes: 24_600_000_000 };
    let partition = BlockDevice {
        filesystem: Some(Filesystem {
            kind: "vfat".to_owned(),
            label: Some("STICK".to_owned()),
            uuid: Some(STICK_ID.to_owned()),
            space: mounted.then_some(space),
        }),
        mount_points: if mounted { vec![crate::domain::removable_place(STICK_ID)] } else { Vec::new() },
        size_bytes: 32_015_998_976,
        rotational: false,
        ..device("sdf1", DeviceKind::Partition)
    };
    BlockDevice {
        model: Some("TEST Stick".to_owned()),
        serial: Some("U1".to_owned()),
        transport: Some("usb".to_owned()),
        id_path: port_of("sdf"),
        removable: true,
        rotational: false,
        size_bytes: 32_017_047_552,
        children: vec![partition],
        ..device("sdf", DeviceKind::Disk)
    }
}

#[async_trait]
impl RemovableHost for FakeStorage {
    async fn mount(
        &self,
        drive: &crate::domain::RemovableDrive,
        _: &RequestContext,
    ) -> Result<bool, AdminError> {
        self.record(format!("mount {}", drive.id));
        let mut stick = self.stick.lock().unwrap();
        match stick.as_mut() {
            Some(mounted) if drive.id == STICK_ID => {
                *mounted = true;
                Ok(false)
            }
            _ => Err(AdminError::Refused(format!("there is no drive {}", drive.id))),
        }
    }

    async fn eject(
        &self,
        drive: &crate::domain::RemovableDrive,
        power_off: bool,
        _: &RequestContext,
    ) -> Result<(), AdminError> {
        self.record(format!("eject {} {power_off}", drive.id));
        if *self.stick_busy.lock().unwrap() {
            return Err(AdminError::Conflict);
        }
        let mut stick = self.stick.lock().unwrap();
        *stick = if power_off { None } else { Some(false) };
        Ok(())
    }

    async fn clean(&self, _: &RequestContext) -> Result<Vec<String>, AdminError> {
        Ok(Vec::new())
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
        id_path: None,
        plugged_in: None,
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
        id_path: port_of(name),
        filesystem: filesystem("linux_raid_member", None, None),
        children: vec![array],
        ..device(name, DeviceKind::Disk)
    }
}

#[async_trait]
impl BlockDeviceSource for FakeStorage {
    async fn block_devices(&self, _: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
        if *self.devices_fail.lock().unwrap() {
            return Err(PortError::new("block devices", std::io::Error::other("the agent does not answer")));
        }
        let root = BlockDevice {
            filesystem: filesystem("ext4", None, None),
            mount_points: vec!["/".to_owned()],
            ..device("nvme0n1p1", DeviceKind::Partition)
        };
        let system = BlockDevice {
            transport: Some("nvme".to_owned()),
            id_path: port_of("nvme0n1"),
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
            id_path: port_of("sdc"),
            children: vec![zfs_partition],
            ..device("sdc", DeviceKind::Disk)
        };
        let sdd_serial = self.sdd_serial.lock().unwrap().clone();
        let mut devices = vec![
            member("sda", "T1"),
            member("sdb", "T2"),
            zfs_disk,
            unused("sdd", &sdd_serial),
            unused("sde", "T5"),
            system,
        ];
        if let Some(mounted) = *self.stick.lock().unwrap() {
            let plugged_in = format!("boot/{}", self.stick_plugged.lock().unwrap());
            devices.push(BlockDevice { plugged_in: Some(plugged_in), ..stick(mounted) });
        }
        if *self.dock.lock().unwrap() {
            let without_sdg = *self.dock_without_sdg.lock().unwrap();
            for name in ["sdg", "sdh"].into_iter().filter(|name| !(without_sdg && *name == "sdg")) {
                devices.push(BlockDevice { transport: Some("usb".to_owned()), ..unused(name, DOCK_SERIAL) });
            }
        }
        Ok(devices)
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
        let gate = self.smart_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            let _permit = gate.acquire().await.expect("the gate stays open");
        }
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
            emmc: None,
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

#[async_trait]
impl MachineSource for FakeStorage {
    async fn machine(&self, _: &RequestContext) -> Result<Machine, PortError> {
        if *self.machine_fails.lock().unwrap() {
            return Err(PortError::new("machine", std::io::Error::other("no DMI")));
        }
        Ok(self.machine.lock().unwrap().clone())
    }
}

/// Each burst takes as long as asked, as reading would.
#[async_trait]
impl DiskLocator for FakeStorage {
    async fn read_burst(
        &self,
        disk: &str,
        duration: std::time::Duration,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        if *self.burst_fails.lock().unwrap() {
            return Err(PortError::new("read burst", std::io::Error::other("I/O error")));
        }
        self.bursts.lock().unwrap().push(disk.to_owned());
        tokio::time::sleep(duration).await;
        Ok(())
    }
}

#[async_trait]
impl EnclosureStore for FakeStorage {
    async fn chosen(&self) -> Result<Option<EnclosureChoice>, PortError> {
        Ok(self.chosen.lock().unwrap().clone())
    }

    async fn save(&self, choice: Option<&EnclosureChoice>) -> Result<(), PortError> {
        *self.chosen.lock().unwrap() = choice.cloned();
        Ok(())
    }
}

#[async_trait]
impl EjectedDrives for FakeStorage {
    async fn ejected(&self) -> Result<std::collections::BTreeMap<String, Option<String>>, PortError> {
        if *self.ejected_fails.lock().unwrap() {
            return Err(PortError::new("ejected drives", std::io::Error::other("the database is gone")));
        }
        Ok(self.ejected.lock().unwrap().clone())
    }

    async fn eject(&self, id: &str, plugged_in: Option<&str>) -> Result<(), PortError> {
        if *self.ejected_fails.lock().unwrap() {
            return Err(PortError::new("ejected drives", std::io::Error::other("the database is gone")));
        }
        self.ejected.lock().unwrap().insert(id.to_owned(), plugged_in.map(str::to_owned));
        Ok(())
    }

    async fn forget(&self, id: &str) -> Result<(), PortError> {
        self.ejected.lock().unwrap().remove(id);
        Ok(())
    }
}

#[async_trait]
impl DiskInventoryStore for FakeStorage {
    async fn records(&self) -> Result<Vec<InventoryRecord>, PortError> {
        Ok(self.inventory.lock().unwrap().clone())
    }

    async fn seen(&self, disks: &[SeenDisk], at: zendiloos_kernel::Timestamp) -> Result<(), PortError> {
        let mut records = self.inventory.lock().unwrap();
        for disk in disks {
            match records.iter_mut().find(|record| record.serial == disk.serial) {
                Some(record) => {
                    record.model.clone_from(&disk.model);
                    record.size_bytes = disk.size_bytes;
                    record.last_seen = record.last_seen.max(at);
                }
                None => records.push(InventoryRecord {
                    serial: disk.serial.clone(),
                    model: disk.model.clone(),
                    size_bytes: disk.size_bytes,
                    details: InventoryDetails::default(),
                    first_seen: at,
                    last_seen: at,
                }),
            }
        }
        Ok(())
    }

    async fn set_details(&self, serial: &str, details: &InventoryDetails) -> Result<bool, PortError> {
        let mut records = self.inventory.lock().unwrap();
        let record = records.iter_mut().find(|record| record.serial == serial);
        Ok(record.map(|record| record.details = details.clone()).is_some())
    }

    async fn remove(&self, serial: &str) -> Result<bool, PortError> {
        let mut records = self.inventory.lock().unwrap();
        let before = records.len();
        records.retain(|record| record.serial != serial);
        Ok(records.len() < before)
    }

    async fn shared_serials(&self) -> Result<std::collections::BTreeSet<String>, PortError> {
        Ok(self.shared_serials.lock().unwrap().clone())
    }

    async fn note_shared(&self, serials: &std::collections::BTreeSet<String>) -> Result<(), PortError> {
        self.shared_serials.lock().unwrap().extend(serials.iter().cloned());
        Ok(())
    }
}
