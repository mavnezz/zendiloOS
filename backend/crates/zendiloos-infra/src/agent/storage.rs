use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, BurstMillis, DiskId, DiskName, ErrorCode, RemovableId, Request, RequestId,
    Response,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::{
    AdminError, ArraySource, BlockDeviceSource, DiskLocator, MachineSource, Mounted, NewPool, PlanTokens,
    PoolAdmin, PoolImport, RemovableHost, SmartSource, Steps, ZfsSource,
};
use zendiloos_storage::domain::{
    Array, BlockDevice, DeviceKind, ImportablePool, Machine, Pool, PoolLayout, PoolName, RemovableDrive,
    SelfTestKind, SmartReport, VolumeNumber,
};

use super::{WHAT, call, refusal, unexpected};
use crate::storage::{emmc, lsblk, mdstat, smartctl, udev, zfs, zpool_import};

/// The Storage context's sources, through the agent: it runs the tools,
/// this side parses their output.
pub struct AgentStorage {
    transport: Arc<dyn AgentTransport>,
}

impl AgentStorage {
    pub(super) fn transport(&self) -> &dyn AgentTransport {
        self.transport.as_ref()
    }

    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl BlockDeviceSource for AgentStorage {
    async fn block_devices(&self, context: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
        match call(self.transport.as_ref(), Request::ListBlockDevices {}, context).await? {
            Response::BlockDevices { lsblk_json, udev_db } => {
                let mut devices =
                    lsblk::parse(&lsblk_json).map_err(|e| PortError::new("block device list", e))?;
                let ports = udev::port_paths(&udev_db);
                let initialized = udev::initialized(&udev_db);
                let boot = boot_id();
                for disk in devices.iter_mut().filter(|device| device.kind == DeviceKind::Disk) {
                    disk.id_path = ports.get(&disk.name).cloned();
                    disk.plugged_in =
                        initialized.get(&disk.name).map(|usec| plugged_in(boot.as_deref(), *usec));
                }
                Ok(devices)
            }
            other => Err(unexpected(other)),
        }
    }
}

/// The kernel's id of this boot: udev's times count anew from it.
fn boot_id() -> Option<String> {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(id.trim().to_owned()).filter(|id| !id.is_empty())
}

/// Which plugging in of a disk udev's time of setting it up, `usec`,
/// names, in the boot `boot`; without it, as far as the time tells.
fn plugged_in(boot: Option<&str>, usec: u64) -> String {
    match boot {
        Some(boot) => format!("{boot}/{usec}"),
        None => usec.to_string(),
    }
}

#[async_trait]
impl MachineSource for AgentStorage {
    async fn machine(&self, context: &RequestContext) -> Result<Machine, PortError> {
        match call(self.transport.as_ref(), Request::Machine {}, context).await? {
            Response::Machine { sys_vendor, product_name } => {
                Ok(Machine { vendor: sys_vendor.trim().to_owned(), product: product_name.trim().to_owned() })
            }
            other => Err(unexpected(other)),
        }
    }
}

#[async_trait]
impl DiskLocator for AgentStorage {
    async fn read_burst(
        &self,
        disk: &str,
        duration: std::time::Duration,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let device = DiskName::parse(disk).map_err(|e| PortError::new("read burst", e))?;
        let millis = u16::try_from(duration.as_millis()).unwrap_or(u16::MAX).clamp(1, BurstMillis::MAX);
        let millis = BurstMillis::new(millis).map_err(|e| PortError::new("read burst", e))?;
        match call(self.transport.as_ref(), Request::ReadBurst { device, millis }, context).await? {
            Response::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

#[async_trait]
impl SmartSource for AgentStorage {
    /// smartctl's report; for an eMMC device, which has no SMART, what its
    /// card reports of its wear.
    async fn smart_report(&self, disk: &str, context: &RequestContext) -> Result<SmartReport, PortError> {
        let device = DiskName::parse(disk).map_err(|e| PortError::new("SMART data", e))?;
        if disk.starts_with("mmcblk") {
            return match call(self.transport.as_ref(), Request::EmmcHealth { device }, context).await? {
                Response::Emmc { kind, life_time, pre_eol_info, name, serial, fwrev } => {
                    Ok(emmc::parse(&emmc::Card {
                        kind: &kind,
                        life_time: &life_time,
                        pre_eol_info: &pre_eol_info,
                        name: &name,
                        serial: &serial,
                        fwrev: &fwrev,
                    }))
                }
                other => Err(unexpected(other)),
            };
        }
        match call(self.transport.as_ref(), Request::SmartData { device }, context).await? {
            Response::Smart { exit_status, smartctl_json } => {
                smartctl::parse(exit_status, &smartctl_json).map_err(|e| PortError::new("SMART data", e))
            }
            other => Err(unexpected(other)),
        }
    }

    async fn start_self_test(
        &self,
        disk: &str,
        kind: SelfTestKind,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let device = DiskName::parse(disk).map_err(|e| PortError::new("self-test", e))?;
        let kind = match kind {
            SelfTestKind::Short => proto::SelfTestKind::Short,
            SelfTestKind::Long => proto::SelfTestKind::Long,
        };
        match call(self.transport.as_ref(), Request::StartSelfTest { device, kind }, context).await? {
            Response::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn abort_self_test(&self, disk: &str, context: &RequestContext) -> Result<(), PortError> {
        let device = DiskName::parse(disk).map_err(|e| PortError::new("self-test", e))?;
        match call(self.transport.as_ref(), Request::AbortSelfTest { device }, context).await? {
            Response::Done => Ok(()),
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
        self.reporting(request, &NoSteps, context).await
    }

    /// As [`Self::change`], for a request with an answer of its own.
    async fn answer(&self, request: Request, context: &RequestContext) -> Result<Response, AdminError> {
        let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
        let answer = self
            .transport
            .call_reporting(request, request_id, &|_: &str, _: Option<u8>| {})
            .await
            .map_err(|error| PortError::new(WHAT, error))?;
        match answer {
            Response::Error { code: ErrorCode::Forbidden, message } => {
                Err(AdminError::Refused(refusal(&message)))
            }
            Response::Error { code: ErrorCode::Conflict, .. } => Err(AdminError::Conflict),
            Response::Error { code, message } => Err(super::agent_error(code, &message).into()),
            other => Ok(other),
        }
    }

    /// As [`Self::change`], passing on the steps the agent reports.
    async fn reporting(
        &self,
        request: Request,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
        let report = |step: &str, percent: Option<u8>| steps.step(step, percent);
        let answer = self
            .transport
            .call_reporting(request, request_id, &report)
            .await
            .map_err(|error| PortError::new(WHAT, error))?;
        match answer {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden, message } => {
                Err(AdminError::Refused(refusal(&message)))
            }
            Response::Error { code: ErrorCode::Conflict, .. } => Err(AdminError::Conflict),
            Response::Error { code, message } => Err(super::agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }
}

fn removable_id(id: &str) -> Result<RemovableId, AdminError> {
    RemovableId::parse(id).map_err(|_| invalid("removable drive"))
}

#[async_trait]
impl RemovableHost for AgentStorage {
    async fn mount(&self, drive: &RemovableDrive, context: &RequestContext) -> Result<bool, AdminError> {
        let request = Request::MountRemovable { id: removable_id(&drive.id)? };
        match self.answer(request, context).await? {
            Response::RemovableMounted { read_only } => Ok(read_only),
            other => Err(unexpected(other).into()),
        }
    }

    async fn eject(
        &self,
        drive: &RemovableDrive,
        power_off: bool,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        self.change(Request::EjectRemovable { id: removable_id(&drive.id)?, power_off }, context).await
    }

    async fn clean(&self, context: &RequestContext) -> Result<Vec<String>, AdminError> {
        match self.answer(Request::CleanRemovable {}, context).await? {
            Response::RemovableCleaned { ids } => Ok(ids.into_iter().map(String::from).collect()),
            other => Err(unexpected(other).into()),
        }
    }
}

/// Where steps go that nobody follows.
struct NoSteps;

impl Steps for NoSteps {
    fn step(&self, _: &str, _: Option<u8>) {}
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
    async fn create_pool(
        &self,
        pool: &NewPool,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
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
        self.reporting(request, steps, context).await
    }

    async fn destroy_pool(
        &self,
        pool: &PoolName,
        number: VolumeNumber,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        let request = Request::DestroyPool { name: pool_name(pool)?, volume: volume(number)? };
        self.reporting(request, steps, context).await
    }

    async fn replace_disk(
        &self,
        pool: &PoolName,
        device: &str,
        disk: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        let device = device.parse().map_err(|_| invalid("device GUID"))?;
        let request = Request::ReplaceDisk { pool: pool_name(pool)?, device, disk: disk_id(disk)? };
        self.reporting(request, steps, context).await
    }

    async fn scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError> {
        self.change(Request::ScrubPool { pool: pool_name(pool)? }, context).await
    }

    async fn stop_scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError> {
        self.change(Request::StopScrub { pool: pool_name(pool)? }, context).await
    }

    async fn importable(&self, context: &RequestContext) -> Result<Vec<ImportablePool>, PortError> {
        match call(self.transport.as_ref(), Request::ImportablePools {}, context).await? {
            Response::ImportablePools { zpool_import } => {
                zpool_import::parse(&zpool_import).map_err(|e| PortError::new("pools to import", e))
            }
            other => Err(unexpected(other)),
        }
    }

    async fn import_pool(
        &self,
        pool: &PoolImport,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), AdminError> {
        let request = Request::ImportPool {
            name: pool_name(&pool.name)?,
            id: pool.id,
            volume: volume(pool.volume)?,
            adopt: pool.adopt,
            force: pool.force,
        };
        self.reporting(request, steps, context).await
    }

    async fn mount_volume(
        &self,
        pool: &PoolName,
        number: VolumeNumber,
        shares: &[String],
        context: &RequestContext,
    ) -> Result<Mounted, AdminError> {
        // A name that is no share's never reaches the agent; it is missing.
        let (names, odd): (Vec<_>, Vec<_>) =
            shares.iter().map(|share| (share, proto::ShareName::parse(share))).partition(|(_, n)| n.is_ok());
        let request = Request::MountVolume {
            pool: pool_name(pool)?,
            volume: volume(number)?,
            shares: names.into_iter().filter_map(|(_, name)| name.ok()).collect(),
        };
        match self.answer(request, context).await? {
            Response::VolumeMounted { missing, changed } => {
                let mut missing: Vec<String> = missing.into_iter().map(String::from).collect();
                missing.extend(odd.into_iter().map(|(share, _)| share.clone()));
                Ok(Mounted { missing, changed })
            }
            other => Err(unexpected(other).into()),
        }
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
    use zendiloos_storage::domain::{
        DiskUsage, Health, VdevState, disks, removable_drives, resolve_pool_disks, volumes,
    };

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
                ("sdf".into(), DiskUsage::Foreign),
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
    async fn the_simulated_stick_is_mounted_and_ejected_through_the_agent() {
        let storage = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let drives = || async { removable_drives(&storage.block_devices(&context).await.unwrap()) };
        let stick = drives().await.remove(0);
        assert_eq!(
            (stick.id.as_str(), stick.label.as_deref(), stick.is_mounted()),
            ("1A2B-3C4D", Some("STICK"), false)
        );

        assert!(!storage.mount(&stick, &context).await.unwrap(), "writable");
        assert_eq!(drives().await[0].mount_point.as_deref(), Some("/run/zendiloos-removable/1A2B-3C4D"));
        assert_eq!(storage.clean(&context).await.unwrap(), Vec::<String>::new());
        storage.eject(&stick, true, &context).await.unwrap();
        assert_eq!(drives().await, []);
        let gone = storage.mount(&stick, &context).await;
        assert!(matches!(gone, Err(AdminError::Refused(ref why)) if why.contains("no drive")), "{gone:?}");
    }

    #[tokio::test]
    async fn a_drive_in_use_is_a_conflict_and_odd_ids_never_reach_the_agent() {
        let context = RequestContext::default();
        let busy = AgentStorage::new(Arc::new(Scripted::refusing(ErrorCode::Conflict, "in use: files open")));
        let drive = |id: &str| RemovableDrive {
            id: id.into(),
            device: "sdf1".into(),
            disk: "sdf".into(),
            label: None,
            filesystem: zendiloos_storage::domain::RemovableFilesystem::Fat,
            size_bytes: 1,
            model: None,
            read_only: false,
            mount_point: None,
            space: None,
            plugged_in: None,
        };
        assert!(matches!(busy.eject(&drive("1A2B-3C4D"), true, &context).await, Err(AdminError::Conflict)));
        let refused = AgentStorage::new(Arc::new(Scripted::refusing(ErrorCode::Forbidden, "on a SATA disk")));
        let why = refused.mount(&drive("1A2B-3C4D"), &context).await;
        assert!(matches!(why, Err(AdminError::Refused(ref why)) if why == "on a SATA disk"), "{why:?}");
        let agent = Arc::new(Scripted::default());
        let storage = AgentStorage::new(agent.clone());
        assert!(matches!(storage.mount(&drive("../etc"), &context).await, Err(AdminError::Port(_))));
        assert!(matches!(storage.eject(&drive("-o"), false, &context).await, Err(AdminError::Port(_))));
        assert_eq!(agent.asked(), []);
        let odd = AgentStorage::new(Arc::new(Scripted::answering([Response::Done, Response::Done])));
        assert!(matches!(odd.mount(&drive("A"), &context).await, Err(AdminError::Port(_))));
        assert!(matches!(odd.clean(&context).await, Err(AdminError::Port(_))));
    }

    use crate::agent::scripted::Scripted;

    fn tank() -> PoolName {
        PoolName::parse("tank").unwrap()
    }

    #[tokio::test]
    async fn the_steps_the_agent_reports_reach_the_job() {
        struct Reporting;
        #[async_trait]
        impl AgentTransport for Reporting {
            async fn call(
                &self,
                _: Request,
                _: Option<RequestId>,
            ) -> Result<Response, zendiloos_agent_proto::TransportError> {
                Ok(Response::Done)
            }

            async fn call_reporting(
                &self,
                _: Request,
                _: Option<RequestId>,
                report: &dyn zendiloos_agent_proto::Report,
            ) -> Result<Response, zendiloos_agent_proto::TransportError> {
                report.step("preparing_disks", Some(0));
                report.step("creating_pool", None);
                Ok(Response::Done)
            }
        }
        #[derive(Default)]
        struct Heard(std::sync::Mutex<Vec<(String, Option<u8>)>>);
        impl Steps for Heard {
            fn step(&self, step: &str, percent: Option<u8>) {
                self.0.lock().unwrap().push((step.to_owned(), percent));
            }
        }
        let storage = AgentStorage::new(Arc::new(Reporting));
        let heard = Heard::default();
        let pool = NewPool {
            name: tank(),
            layout: PoolLayout::Single,
            disks: vec!["ata-DISK_1".into()],
            volume: VolumeNumber::new(1).unwrap(),
        };
        storage.create_pool(&pool, &heard, &RequestContext::default()).await.unwrap();
        let steps = heard.0.lock().unwrap().clone();
        assert_eq!(steps, [("preparing_disks".to_owned(), Some(0)), ("creating_pool".to_owned(), None)]);
    }

    #[tokio::test]
    async fn pools_are_made_changed_and_destroyed_through_the_agent() {
        let agent = Arc::new(Scripted::default());
        let storage = AgentStorage::new(agent.clone());
        let context = RequestContext::default();
        let one = VolumeNumber::new(1).unwrap();
        for layout in [
            PoolLayout::Single,
            PoolLayout::Mirror,
            PoolLayout::Raidz { parity: 1 },
            PoolLayout::Raidz { parity: 2 },
            PoolLayout::Raidz { parity: 3 },
        ] {
            let pool = NewPool { name: tank(), layout, disks: vec!["ata-DISK_1".into()], volume: one };
            storage.create_pool(&pool, &NoSteps, &context).await.unwrap();
        }
        storage.destroy_pool(&tank(), one, &NoSteps, &context).await.unwrap();
        storage.replace_disk(&tank(), "1234567890", "ata-DISK_2", &NoSteps, &context).await.unwrap();
        storage.scrub(&tank(), &context).await.unwrap();
        storage.stop_scrub(&tank(), &context).await.unwrap();
        let asked = agent.asked();
        assert_eq!(asked.len(), 9);
        assert!(matches!(&asked[4], Request::CreatePool { layout: proto::Layout::Raidz3, .. }));
        assert!(matches!(&asked[5], Request::DestroyPool { .. }));
        assert!(matches!(&asked[6], Request::ReplaceDisk { device: 1_234_567_890, .. }));
        assert!(matches!(&asked[7], Request::ScrubPool { .. }));
        assert!(matches!(&asked[8], Request::StopScrub { .. }));

        // What zendiloos does not make, or cannot name, is never sent.
        let odd =
            |layout, disk: &str| NewPool { name: tank(), layout, disks: vec![disk.into()], volume: one };
        assert!(
            storage
                .create_pool(&odd(PoolLayout::Raidz { parity: 4 }, "ata-1"), &NoSteps, &context)
                .await
                .is_err()
        );
        assert!(
            storage
                .create_pool(&odd(PoolLayout::Other("draid".into()), "ata-1"), &NoSteps, &context)
                .await
                .is_err()
        );
        assert!(storage.create_pool(&odd(PoolLayout::Single, "../sda"), &NoSteps, &context).await.is_err());
        assert!(storage.replace_disk(&tank(), "not a number", "ata-1", &NoSteps, &context).await.is_err());
        assert_eq!(agent.asked().len(), 9);
    }

    #[tokio::test]
    async fn refusals_and_conflicts_of_the_agent_are_told_apart() {
        let context = RequestContext::default();
        let refused = AgentStorage::new(Arc::new(Scripted::refusing(ErrorCode::Forbidden, "in use")));
        assert!(
            matches!(refused.scrub(&tank(), &context).await, Err(AdminError::Refused(ref why)) if why == "in use")
        );
        let taken = AgentStorage::new(Arc::new(Scripted::refusing(ErrorCode::Conflict, "taken")));
        assert!(matches!(taken.scrub(&tank(), &context).await, Err(AdminError::Conflict)));
        let failed = AgentStorage::new(Arc::new(Scripted::refusing(ErrorCode::Failed, "zpool")));
        assert!(matches!(failed.scrub(&tank(), &context).await, Err(AdminError::Port(_))));
        let odd = AgentStorage::new(Arc::new(Scripted::answering([Response::ShareUsage {
            zfs_list: String::new(),
        }])));
        assert!(matches!(odd.scrub(&tank(), &context).await, Err(AdminError::Port(_))));
    }

    #[tokio::test]
    async fn self_tests_and_odd_answers() {
        let context = RequestContext::default();
        let agent = Arc::new(Scripted::default());
        let storage = AgentStorage::new(agent.clone());
        storage.start_self_test("sda", SelfTestKind::Short, &context).await.unwrap();
        storage.start_self_test("sda", SelfTestKind::Long, &context).await.unwrap();
        storage.abort_self_test("sda", &context).await.unwrap();
        assert!(matches!(&agent.asked()[1], Request::StartSelfTest { kind: proto::SelfTestKind::Long, .. }));
        assert!(storage.abort_self_test("sda1", &context).await.is_err(), "a partition");
        assert!(storage.start_self_test("../x", SelfTestKind::Short, &context).await.is_err());

        // An answer of another kind is an error for every reading.
        let odd = || Response::ShareUsage { zfs_list: String::new() };
        let agent = Arc::new(Scripted::answering((0..6).map(|_| odd())));
        let storage = AgentStorage::new(agent.clone());
        assert!(storage.block_devices(&context).await.is_err());
        assert!(storage.smart_report("sda", &context).await.is_err());
        assert!(storage.arrays(&context).await.is_err());
        assert!(storage.pools(&context).await.is_err());
        assert!(storage.start_self_test("sda", SelfTestKind::Short, &context).await.is_err());
        assert!(storage.abort_self_test("sda", &context).await.is_err());
        agent.answer(Response::BlockDevices { lsblk_json: "not json".into(), udev_db: String::new() });
        assert!(storage.block_devices(&context).await.is_err());
    }

    #[tokio::test]
    async fn an_emmc_device_is_asked_for_its_wear_rather_than_smart() {
        let context = RequestContext::default();
        let card = Response::Emmc {
            kind: "MMC\n".into(),
            life_time: "0x02 0x09\n".into(),
            pre_eol_info: "0x01\n".into(),
            name: "DV4032\n".into(),
            serial: String::new(),
            fwrev: String::new(),
        };
        let agent = Arc::new(Scripted::answering([card, Response::ShareUsage { zfs_list: String::new() }]));
        let storage = AgentStorage::new(agent.clone());
        let SmartReport::Available(data) = storage.smart_report("mmcblk0", &context).await.unwrap() else {
            panic!("no wear");
        };
        assert_eq!(data.assess().health, Health::Warning, "80 % used");
        assert!(matches!(&agent.asked()[0], Request::EmmcHealth { device } if device.as_str() == "mmcblk0"));
        assert!(storage.smart_report("mmcblk0", &context).await.is_err(), "an answer of another kind");

        // The simulated NAS has none: its card says nothing.
        let simulated = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let none = simulated.smart_report("mmcblk0", &context).await.unwrap();
        assert!(matches!(none, SmartReport::Unsupported { .. }), "{none:?}");
    }

    #[test]
    fn plan_tokens_are_random_and_url_safe() {
        let (one, two) = (RandomPlanTokens.token(), RandomPlanTokens.token());
        assert_ne!(one, two);
        assert_eq!(one.len(), 22);
        assert!(one.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    #[tokio::test]
    async fn never_sends_a_name_that_is_not_a_disk() {
        let agent = Arc::new(SimulatedAgent::new());
        let storage = AgentStorage::new(agent.clone());
        assert!(storage.smart_report("sda1", &RequestContext::default()).await.is_err());
        assert!(
            storage.read_burst("sda1", Duration::from_millis(700), &RequestContext::default()).await.is_err()
        );
        assert_eq!(agent.received(), []);
    }

    use std::time::Duration;

    /// Which plugging in of a disk it is: the boot, and when udev set the
    /// disk up, which it does anew whenever the disk is added; disks only.
    #[tokio::test]
    async fn disks_are_told_by_the_boot_and_when_udev_set_them_up() {
        let lsblk_json = include_str!("../storage/fixtures/lsblk-vm.json").to_owned();
        let udev_db = include_str!("../storage/fixtures/udevadm-vm.txt").to_owned();
        let agent = Scripted::answering([Response::BlockDevices { lsblk_json, udev_db }]);
        let devices =
            AgentStorage::new(Arc::new(agent)).block_devices(&RequestContext::default()).await.unwrap();

        let boot = boot_id().expect("Linux has a boot id");
        let sdd = devices.iter().find(|device| device.name == "sdd").unwrap();
        assert_eq!(sdd.plugged_in, Some(format!("{boot}/1839511")));
        assert!(
            devices.iter().flat_map(|disk| &disk.children).all(|partition| partition.plugged_in.is_none())
        );
        assert_eq!(plugged_in(None, 6), "6", "as far as the time tells without the boot");
    }

    #[tokio::test]
    async fn the_simulated_nas_is_a_dxp4800_plus_with_its_disks_in_their_bays() {
        use zendiloos_storage::application::ports::EnclosureCatalog;
        use zendiloos_storage::domain::{Layout, SlotState, detect, place};

        let storage = AgentStorage::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let machine = storage.machine(&context).await.unwrap();
        assert_eq!(machine, Machine { vendor: "UGREEN".into(), product: "DXP4800 Plus".into() }, "trimmed");
        let models = crate::enclosure::BuiltInCatalog::load().unwrap().models();
        let model = detect(&models, &machine).unwrap();

        let devices = storage.block_devices(&context).await.unwrap();
        let ports: Vec<_> = devices.iter().map(|d| (d.name.as_str(), d.id_path.as_deref())).collect();
        assert_eq!(ports[0], ("sda", Some("pci-0000:05:00.0-ata-1.0")));
        assert_eq!(ports[7], ("sdf", Some("pci-0000:00:0d.0-usb-0:1:1.0-scsi-0:0:0:0")));
        assert!(devices[0].children.iter().all(|partition| partition.id_path.is_none()), "disks only");
        let layout: &Layout = &model.layout;
        let placement = place(layout, &disks(&devices, &[]));
        let placed: Vec<_> = placement.slots.iter().map(|p| (p.slot.as_str(), p.disk.as_deref())).collect();
        assert_eq!(
            placed,
            [
                ("bay1", Some("sda")),
                ("bay2", Some("sdb")),
                ("bay3", Some("sdc")),
                ("bay4", Some("sdd")),
                ("m2-1", Some("nvme1n1")),
                ("m2-2", None),
                ("system", Some("nvme0n1")),
            ]
        );
        assert_eq!(placement.slots[5].state, SlotState::Empty);
        assert_eq!(placement.unplaced, ["sde", "sdf"], "the spare port and the stick have no slot");

        storage.read_burst("sdc", Duration::from_millis(700), &context).await.unwrap();
        assert!(storage.read_burst("sdz", Duration::from_millis(700), &context).await.is_err());
    }

    #[tokio::test]
    async fn bursts_ask_for_two_seconds_at_most_and_odd_answers_fail() {
        let agent = Arc::new(Scripted::default());
        let storage = AgentStorage::new(agent.clone());
        let context = RequestContext::default();
        storage.read_burst("sda", Duration::from_secs(9), &context).await.unwrap();
        assert_eq!(
            agent.only(),
            Request::ReadBurst {
                device: DiskName::parse("sda").unwrap(),
                millis: BurstMillis::new(BurstMillis::MAX).unwrap()
            }
        );
        let odd = Arc::new(Scripted::answering([
            Response::ShareUsage { zfs_list: String::new() },
            Response::ShareUsage { zfs_list: String::new() },
        ]));
        let storage = AgentStorage::new(odd);
        assert!(storage.read_burst("sda", Duration::from_millis(700), &context).await.is_err());
        assert!(storage.machine(&context).await.is_err());
    }
}
