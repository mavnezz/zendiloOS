//! Watches pools and disks and tells administrators what changed.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{Clock, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::application::inventory::known_shared;
use crate::application::ports::{BlockDeviceSource, DiskInventoryStore, SmartSource, ZfsSource};
use crate::domain::{
    self, DiskUsage, DiskVerdicts, Health, Pool, ScanKind, SmartReport, StorageEvent, VdevState,
};

/// How often SMART is read from disks that hold data. Reading wakes no
/// sleeping disk, but it is not free either.
pub const SMART_INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
struct Seen {
    pools: Option<Vec<Pool>>,
    smart_at: Option<Timestamp>,
}

pub struct StorageMonitor {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
    smart: Arc<dyn SmartSource>,
    /// Which serial numbers are shared, so that a disk of a USB dock keeps
    /// its key whichever disks are present.
    inventory: Arc<dyn DiskInventoryStore>,
    notifier: Notifier,
    clock: Arc<dyn Clock>,
    /// Held for a whole look, one at a time.
    seen: tokio::sync::Mutex<Seen>,
    /// The verdicts by [`domain::health_key`]; apart from `seen`, so that
    /// whoever asks for them does not wait for a look reading SMART from
    /// every disk, but gets each verdict as soon as it is read.
    health: Mutex<DiskVerdicts>,
}

impl StorageMonitor {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        zfs: Arc<dyn ZfsSource>,
        smart: Arc<dyn SmartSource>,
        inventory: Arc<dyn DiskInventoryStore>,
        notifier: Notifier,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let seen = tokio::sync::Mutex::new(Seen::default());
        Self { devices, zfs, smart, inventory, notifier, clock, seen, health: Mutex::default() }
    }

    /// The SMART verdicts of the disks holding data, as last read. Empty
    /// until the first are read; never waits for a look under way.
    pub async fn disk_health(&self) -> DiskVerdicts {
        self.health.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// One look; call regularly. Failures to read are logged and the look
    /// is skipped.
    pub async fn check(&self) {
        let request = RequestContext::default();
        let mut seen = self.seen.lock().await;
        let devices = match self.devices.block_devices(&request).await {
            Ok(devices) => devices,
            Err(error) => return tracing::warn!(%error, "storage monitor: cannot read the disks"),
        };
        let mut pools = match self.zfs.pools(&request).await {
            Ok(pools) => pools,
            Err(error) => return tracing::warn!(%error, "storage monitor: cannot read the pools"),
        };
        domain::resolve_pool_disks(&mut pools, &devices);
        let mut events = domain::pool_events(seen.pools.as_deref(), &pools);

        let now = self.clock.now();
        let due = seen.smart_at.is_none_or(|at| now.duration_since(at) >= SMART_INTERVAL);
        if due {
            seen.smart_at = Some(now);
            let disks = domain::disks(&devices, &pools);
            let shared = match known_shared(self.inventory.as_ref(), &disks).await {
                Ok(shared) => shared,
                // Those known before and those shared now, at least.
                Err(error) => {
                    tracing::warn!(%error, "storage monitor: cannot read which serial numbers are shared");
                    let known = self.health.lock().unwrap_or_else(PoisonError::into_inner).shared.clone();
                    known.into_iter().chain(domain::shared_serials(&disks)).collect()
                }
            };
            self.health.lock().unwrap_or_else(PoisonError::into_inner).shared.clone_from(&shared);
            for disk in &disks {
                let holds_data =
                    !matches!(disk.usage, DiskUsage::Unused | DiskUsage::Foreign | DiskUsage::Removable);
                if !holds_data {
                    continue;
                }
                let Ok(SmartReport::Available(data)) = self.smart.smart_report(&disk.name, &request).await
                else {
                    continue;
                };
                let health = data.assess().health;
                let key = domain::health_key(disk, &shared);
                let before =
                    self.health.lock().unwrap_or_else(PoisonError::into_inner).by_key.insert(key, health);
                events.extend(domain::disk_event(&disk.name, disk.model.as_deref(), before, health));
            }
        }
        seen.pools = Some(pools);
        drop(seen);

        for event in events {
            self.notifier.notify(notification(event)).await;
        }
    }
}

fn notification(event: StorageEvent) -> NewNotification {
    match event {
        StorageEvent::PoolTrouble { pool, state } => {
            let (level, state) = match state {
                VdevState::Degraded => (Level::Warning, "degraded"),
                VdevState::Faulted => (Level::Error, "faulted"),
                VdevState::Unavailable => (Level::Error, "unavailable"),
                VdevState::Suspended => (Level::Error, "suspended"),
                VdevState::Offline => (Level::Warning, "offline"),
                VdevState::Removed => (Level::Error, "removed"),
                VdevState::Online | VdevState::Unknown => (Level::Warning, "unknown"),
            };
            NewNotification::new(level, "pool_trouble").with("pool", pool).with("state", state)
        }
        StorageEvent::PoolRecovered { pool } => {
            NewNotification::new(Level::Success, "pool_recovered").with("pool", pool)
        }
        StorageEvent::ScanFinished { pool, kind, errors } => {
            let (code, level) = match (kind, errors) {
                (ScanKind::Resilver, 0) => ("resilver_finished", Level::Success),
                (ScanKind::Resilver, _) => ("resilver_finished", Level::Warning),
                (ScanKind::Scrub, 0) => ("scrub_finished", Level::Info),
                (ScanKind::Scrub, _) => ("scrub_finished", Level::Warning),
            };
            NewNotification::new(level, code).with("pool", pool).with("errors", errors.to_string())
        }
        StorageEvent::VolumeFilling { pool, percent } => {
            let level = if percent >= 90 { Level::Error } else { Level::Warning };
            NewNotification::new(level, "volume_filling")
                .with("pool", pool)
                .with("percent", percent.to_string())
        }
        StorageEvent::DiskHealth { disk, model, health } => {
            let (level, health) = match health {
                Health::Failing => (Level::Error, "failing"),
                _ => (Level::Warning, "warning"),
            };
            NewNotification::new(level, "disk_health")
                .with("disk", disk)
                .with("model", model.unwrap_or_default())
                .with("health", health)
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_jobs::Jobs;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_kernel::PortError;
    use zendiloos_notify::testing::MemoryNotifications;

    use super::*;
    use crate::testing::FakeStorage;

    #[tokio::test]
    async fn the_first_look_reports_problems_and_smart_once() {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let (ports, _) = FakeStorage::ports(jobs, Arc::new(NoAudit), Arc::new(FixedClock));
        let repository = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(repository.clone(), Arc::new(FixedClock));
        let monitor = StorageMonitor::new(
            ports.devices,
            ports.zfs,
            ports.smart,
            ports.inventory,
            notifier,
            Arc::new(FixedClock),
        );

        monitor.check().await;
        monitor.check().await;

        // The fake host is healthy: nothing to report, twice.
        assert_eq!(repository.all(), []);
    }

    /// Two disks of a USB dock whose bridge reports its own serial number
    /// for both, holding data; one fails.
    #[derive(Default)]
    struct Dock {
        /// The other disk is pulled out.
        alone: std::sync::Mutex<bool>,
        hours: std::sync::Mutex<i64>,
    }

    #[async_trait::async_trait]
    impl BlockDeviceSource for Dock {
        async fn block_devices(&self, _: &RequestContext) -> Result<Vec<domain::BlockDevice>, PortError> {
            use domain::{BlockDevice, DeviceKind, Filesystem};
            let disk = |name: &str, n: u8| {
                let kind = Filesystem { kind: "xfs".into(), label: None, uuid: None, space: None };
                let partition = BlockDevice {
                    name: format!("{name}1"),
                    kind: DeviceKind::Partition,
                    filesystem: Some(kind),
                    id_path: None,
                    children: vec![],
                    ..Self::device(name)
                };
                BlockDevice {
                    id_path: Some(format!("pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0:{n}")),
                    children: vec![partition],
                    ..Self::device(name)
                }
            };
            let both = !*self.alone.lock().unwrap();
            Ok([disk("sdg", 0), disk("sdh", 1)].into_iter().filter(|d| both || d.name == "sdh").collect())
        }
    }

    impl Dock {
        fn device(name: &str) -> domain::BlockDevice {
            domain::BlockDevice {
                name: name.into(),
                kind: domain::DeviceKind::Disk,
                size_bytes: 4_000_000_000_000,
                model: Some("Dock disk".into()),
                serial: Some(crate::testing::DOCK_SERIAL.into()),
                wwn: None,
                id_link: None,
                id_path: None,
                plugged_in: None,
                transport: Some("usb".into()),
                rotational: true,
                removable: false,
                read_only: false,
                filesystem: None,
                mount_points: vec![],
                children: vec![],
            }
        }
    }

    #[async_trait::async_trait]
    impl ZfsSource for Dock {
        async fn pools(&self, _: &RequestContext) -> Result<Vec<Pool>, PortError> {
            Ok(Vec::new())
        }
    }

    #[async_trait::async_trait]
    impl SmartSource for Dock {
        async fn smart_report(&self, disk: &str, _: &RequestContext) -> Result<SmartReport, PortError> {
            let failing = disk == "sdh";
            Ok(SmartReport::Available(Box::new(domain::SmartData {
                protocol: domain::SmartProtocol::Ata,
                model: None,
                serial: None,
                firmware: None,
                passed: Some(!failing),
                temperature_celsius: None,
                power_on_hours: None,
                power_cycles: None,
                attributes: Vec::new(),
                nvme: None,
                error_log_count: None,
                last_self_test: None,
                running_self_test: None,
                self_test_minutes: None,
                self_tests_supported: true,
            })))
        }

        async fn start_self_test(
            &self,
            _: &str,
            _: domain::SelfTestKind,
            _: &RequestContext,
        ) -> Result<(), PortError> {
            Ok(())
        }

        async fn abort_self_test(&self, _: &str, _: &RequestContext) -> Result<(), PortError> {
            Ok(())
        }
    }

    impl Clock for Dock {
        fn now(&self) -> zendiloos_kernel::Timestamp {
            zendiloos_kernel::Timestamp::from_unix_seconds(1_793_880_000 + *self.hours.lock().unwrap() * 3600)
        }
    }

    /// The failing disk of the dock is told of once, whether the other one
    /// is there or not: its verdict is kept by the same key.
    #[tokio::test]
    async fn a_failing_dock_disk_is_told_of_once_whichever_disks_are_present() {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let (ports, _) = FakeStorage::ports(jobs, Arc::new(NoAudit), Arc::new(FixedClock));
        let repository = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(repository.clone(), Arc::new(FixedClock));
        let dock = Arc::new(Dock::default());
        let monitor = StorageMonitor::new(
            dock.clone(),
            dock.clone(),
            dock.clone(),
            ports.inventory,
            notifier,
            dock.clone(),
        );

        for alone in [false, true, false] {
            *dock.alone.lock().unwrap() = alone;
            monitor.check().await;
            // The next look reads SMART again.
            *dock.hours.lock().unwrap() += 7;
        }

        let told: Vec<_> = repository.all().into_iter().map(|n| (n.code, n.params["disk"].clone())).collect();
        assert_eq!(told, [("disk_health".to_owned(), "sdh".to_owned())]);
        let health = monitor.disk_health().await;
        let key = format!("{}@pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0:1", crate::testing::DOCK_SERIAL);
        assert_eq!(health.by_key.get(&key), Some(&Health::Failing));
    }

    struct NoAudit;

    #[async_trait::async_trait]
    impl zendiloos_kernel::AuditLog for NoAudit {
        async fn record(&self, _: zendiloos_kernel::AuditEntry) -> Result<(), zendiloos_kernel::PortError> {
            Ok(())
        }
    }

    #[test]
    fn events_become_coded_notifications() {
        let degraded =
            notification(StorageEvent::PoolTrouble { pool: "tank".into(), state: VdevState::Degraded });
        assert_eq!((degraded.level, degraded.code.as_str()), (Level::Warning, "pool_trouble"));
        assert_eq!(degraded.params["state"], "degraded");
        let full = notification(StorageEvent::VolumeFilling { pool: "tank".into(), percent: 90 });
        assert_eq!((full.level, full.params["percent"].as_str()), (Level::Error, "90"));
        let disk = notification(StorageEvent::DiskHealth {
            disk: "sdd".into(),
            model: None,
            health: Health::Failing,
        });
        assert_eq!((disk.level, disk.params["health"].as_str()), (Level::Error, "failing"));
    }

    /// Where a notification goes by default depends on its kind's most
    /// serious level; none raised here may be more serious than that, or a
    /// channel for errors only would miss it.
    #[test]
    fn no_notification_is_more_serious_than_its_kind() {
        use zendiloos_notify::routes::{carries, find, kind_of};
        let pool = || "tank".to_owned();
        let states = [
            VdevState::Online,
            VdevState::Degraded,
            VdevState::Faulted,
            VdevState::Offline,
            VdevState::Unavailable,
            VdevState::Removed,
            VdevState::Suspended,
            VdevState::Unknown,
        ];
        let mut events: Vec<StorageEvent> =
            states.into_iter().map(|state| StorageEvent::PoolTrouble { pool: pool(), state }).collect();
        events.push(StorageEvent::PoolRecovered { pool: pool() });
        for kind in [ScanKind::Scrub, ScanKind::Resilver] {
            for errors in [0, 3] {
                events.push(StorageEvent::ScanFinished { pool: pool(), kind, errors });
            }
        }
        for percent in [80, 90] {
            events.push(StorageEvent::VolumeFilling { pool: pool(), percent });
        }
        for health in [Health::Good, Health::Warning, Health::Failing] {
            events.push(StorageEvent::DiskHealth { disk: "sdd".into(), model: None, health });
        }
        for event in events {
            let raised = notification(event);
            let kind = find(kind_of(&raised.code, raised.level)).expect("a kind the routes know");
            for baseline in [Level::Info, Level::Warning, Level::Error] {
                assert!(
                    !carries(baseline, raised.level) || carries(baseline, kind.level),
                    "{} at {:?} is more serious than its kind",
                    raised.code,
                    raised.level
                );
            }
        }
    }
}
