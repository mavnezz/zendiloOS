//! Watches pools and disks and tells administrators what changed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use zendiloos_kernel::{Clock, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::application::ports::{BlockDeviceSource, SmartSource, ZfsSource};
use crate::domain::{self, DiskUsage, Health, Pool, ScanKind, SmartReport, StorageEvent, VdevState};

/// How often SMART is read from disks that hold data. Reading wakes no
/// sleeping disk, but it is not free either.
pub const SMART_INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
struct Seen {
    pools: Option<Vec<Pool>>,
    /// By serial number, or name if there is none.
    health: HashMap<String, Health>,
    smart_at: Option<Timestamp>,
}

pub struct StorageMonitor {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
    smart: Arc<dyn SmartSource>,
    notifier: Notifier,
    clock: Arc<dyn Clock>,
    seen: tokio::sync::Mutex<Seen>,
}

impl StorageMonitor {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        zfs: Arc<dyn ZfsSource>,
        smart: Arc<dyn SmartSource>,
        notifier: Notifier,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { devices, zfs, smart, notifier, clock, seen: tokio::sync::Mutex::new(Seen::default()) }
    }

    /// The SMART verdicts of the disks holding data, as last read, by serial
    /// number (or name for disks without one). Empty until the first look.
    pub async fn disk_health(&self) -> HashMap<String, Health> {
        self.seen.lock().await.health.clone()
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
            for disk in domain::disks(&devices, &pools) {
                let holds_data = !matches!(disk.usage, DiskUsage::Unused | DiskUsage::Foreign);
                if !holds_data {
                    continue;
                }
                let Ok(SmartReport::Available(data)) = self.smart.smart_report(&disk.name, &request).await
                else {
                    continue;
                };
                let health = data.assess().health;
                let key = disk.serial.clone().unwrap_or_else(|| disk.name.clone());
                let before = seen.health.insert(key, health);
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
    use zendiloos_notify::testing::MemoryNotifications;

    use super::*;
    use crate::testing::FakeStorage;

    #[tokio::test]
    async fn the_first_look_reports_problems_and_smart_once() {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let (ports, _) = FakeStorage::ports(jobs, Arc::new(NoAudit), Arc::new(FixedClock));
        let repository = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(repository.clone(), Arc::new(FixedClock));
        let monitor =
            StorageMonitor::new(ports.devices, ports.zfs, ports.smart, notifier, Arc::new(FixedClock));

        monitor.check().await;
        monitor.check().await;

        // The fake host is healthy: nothing to report, twice.
        assert_eq!(repository.all(), []);
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
}
