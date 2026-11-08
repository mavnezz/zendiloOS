//! One answer to "is my storage all right?": every concern about the pools
//! and disks, the worst first.

use std::collections::HashMap;
use std::time::Duration;

use zendiloos_kernel::Timestamp;

use super::disk::Disk;
use super::events::{FILL_THRESHOLDS, percent_used};
use super::pool::{Pool, PoolLayout, ScanState, VdevState};
use super::smart::Health;

/// A scrub runs every month (zfsutils-linux, second Sunday); a pool not
/// checked for longer missed at least one.
pub const SCRUB_OVERDUE_AFTER: Duration = Duration::from_secs(40 * 24 * 3600);

/// How bad a concern is; the worst one is the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing, nothing is at risk right now.
    Notice,
    /// Act soon: redundancy or room is running out.
    Warning,
    /// Data is unavailable, damaged, or about to be.
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Concern {
    /// The pool's data cannot be reached.
    PoolUnavailable {
        pool: String,
        state: VdevState,
    },
    /// The pool works with less redundancy than intended.
    PoolDegraded {
        pool: String,
        state: VdevState,
    },
    /// Files with errors that redundancy could not repair.
    PoolDataErrors {
        pool: String,
        files: u64,
    },
    /// Read, write or checksum errors on one of the pool's devices.
    DeviceErrors {
        pool: String,
        device: String,
        errors: u64,
    },
    VolumeFilling {
        pool: String,
        percent: u8,
    },
    DiskFailing {
        disk: String,
        model: Option<String>,
    },
    DiskWorn {
        disk: String,
        model: Option<String>,
    },
    /// One failing disk loses everything.
    NoRedundancy {
        pool: String,
    },
    /// The last check of the data is older than [`SCRUB_OVERDUE_AFTER`].
    ScrubOverdue {
        pool: String,
        days: u64,
    },
}

impl Concern {
    pub fn severity(&self) -> Severity {
        match self {
            Concern::PoolUnavailable { .. }
            | Concern::PoolDataErrors { .. }
            | Concern::DiskFailing { .. } => Severity::Critical,
            Concern::VolumeFilling { percent, .. } if *percent >= FILL_THRESHOLDS[1] => Severity::Critical,
            Concern::PoolDegraded { .. }
            | Concern::DeviceErrors { .. }
            | Concern::VolumeFilling { .. }
            | Concern::DiskWorn { .. } => Severity::Warning,
            Concern::NoRedundancy { .. } | Concern::ScrubOverdue { .. } => Severity::Notice,
        }
    }

    /// The pool or disk it is about, for a stable order.
    fn subject(&self) -> &str {
        match self {
            Concern::PoolUnavailable { pool, .. }
            | Concern::PoolDegraded { pool, .. }
            | Concern::PoolDataErrors { pool, .. }
            | Concern::DeviceErrors { pool, .. }
            | Concern::VolumeFilling { pool, .. }
            | Concern::NoRedundancy { pool }
            | Concern::ScrubOverdue { pool, .. } => pool,
            Concern::DiskFailing { disk, .. } | Concern::DiskWorn { disk, .. } => disk,
        }
    }
}

/// The concerns, worst first; none means all is well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub concerns: Vec<Concern>,
}

impl Overview {
    /// The worst severity, or `None` if there is nothing to worry about.
    pub fn verdict(&self) -> Option<Severity> {
        self.concerns.first().map(Concern::severity)
    }
}

/// What is wrong with `pools` and `disks`. `health` holds the SMART
/// verdicts last read, by serial number (or name for disks without one);
/// disks not in it are not judged.
pub fn overview(
    pools: &[Pool],
    disks: &[Disk],
    health: &HashMap<String, Health>,
    now: Timestamp,
) -> Overview {
    let mut concerns = Vec::new();
    for pool in pools {
        concerns.extend(pool_concerns(pool, now));
    }
    for disk in disks {
        let key = disk.serial.as_deref().unwrap_or(&disk.name);
        let (disk_name, model) = (disk.name.clone(), disk.model.clone());
        match health.get(key) {
            Some(Health::Failing) => concerns.push(Concern::DiskFailing { disk: disk_name, model }),
            Some(Health::Warning) => concerns.push(Concern::DiskWorn { disk: disk_name, model }),
            Some(Health::Good) | None => {}
        }
    }
    concerns.sort_by(|a, b| b.severity().cmp(&a.severity()).then_with(|| a.subject().cmp(b.subject())));
    Overview { concerns }
}

fn pool_concerns(pool: &Pool, now: Timestamp) -> Vec<Concern> {
    let name = || pool.name.clone();
    let mut concerns = Vec::new();
    match pool.state {
        VdevState::Online => {}
        VdevState::Degraded | VdevState::Offline | VdevState::Unknown => {
            concerns.push(Concern::PoolDegraded { pool: name(), state: pool.state });
        }
        VdevState::Faulted | VdevState::Unavailable | VdevState::Suspended | VdevState::Removed => {
            concerns.push(Concern::PoolUnavailable { pool: name(), state: pool.state });
        }
    }
    if pool.data_errors > 0 {
        concerns.push(Concern::PoolDataErrors { pool: name(), files: pool.data_errors });
    }
    for device in &pool.devices {
        let errors = device.read_errors + device.write_errors + device.checksum_errors;
        if errors > 0 {
            concerns.push(Concern::DeviceErrors { pool: name(), device: device.name.clone(), errors });
        }
    }
    if let Some(percent) = percent_used(pool).filter(|&p| p >= FILL_THRESHOLDS[0]) {
        concerns.push(Concern::VolumeFilling { pool: name(), percent });
    }
    let single = match pool.layout {
        PoolLayout::Single => true,
        PoolLayout::Mirror => pool.devices.len() < 2,
        PoolLayout::Raidz { .. } | PoolLayout::Other(_) => false,
    };
    if single {
        concerns.push(Concern::NoRedundancy { pool: name() });
    }
    // Only when a check finished once: the pool's age is not known, and a
    // new pool has nothing to check yet.
    let last_check =
        pool.scan.as_ref().filter(|scan| scan.state == ScanState::Finished).and_then(|s| s.finished_at);
    if let Some(at) = last_check
        && now.duration_since(at) > SCRUB_OVERDUE_AFTER
    {
        let days = now.duration_since(at).as_secs() / (24 * 3600);
        concerns.push(Concern::ScrubOverdue { pool: name(), days });
    }
    concerns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::disk::{DiskUsage, Media, Transport};
    use crate::domain::pool::{PoolDevice, PoolSpace, Scan, ScanKind};

    const DAY: i64 = 24 * 3600;
    const NOW: i64 = 1_700_000_000;

    fn device(name: &str) -> PoolDevice {
        PoolDevice {
            name: name.into(),
            guid: "1".into(),
            path: None,
            disk: None,
            state: VdevState::Online,
            read_errors: 0,
            write_errors: 0,
            checksum_errors: 0,
            replacing: None,
        }
    }

    fn pool(name: &str, layout: PoolLayout, devices: usize) -> Pool {
        Pool {
            name: name.into(),
            guid: "1".into(),
            state: VdevState::Online,
            layout,
            devices: (0..devices).map(|i| device(&format!("disk-{i}"))).collect(),
            size_bytes: 100,
            allocated_bytes: 10,
            scan: None,
            data_errors: 0,
            volume: None,
            space: Some(PoolSpace { used_bytes: 10, available_bytes: 90, mounted: true }),
        }
    }

    fn disk(name: &str, serial: Option<&str>) -> Disk {
        Disk {
            name: name.into(),
            model: Some("Model".into()),
            serial: serial.map(str::to_owned),
            wwn: None,
            id_link: None,
            id_path: None,
            size_bytes: 1,
            transport: Transport::Sata,
            media: Media::Rotational,
            removable: false,
            read_only: false,
            usage: DiskUsage::Unused,
            partitions: vec![],
        }
    }

    fn checked(days_ago: i64) -> Option<Scan> {
        Some(Scan {
            kind: ScanKind::Scrub,
            state: ScanState::Finished,
            done_bytes: 1,
            total_bytes: 1,
            started_at: Timestamp::from_unix_seconds(NOW - days_ago * DAY - 3600),
            finished_at: Some(Timestamp::from_unix_seconds(NOW - days_ago * DAY)),
            errors: 0,
            pass: None,
        })
    }

    fn now() -> Timestamp {
        Timestamp::from_unix_seconds(NOW)
    }

    #[test]
    fn healthy_storage_has_no_concerns() {
        let mut tank = pool("tank", PoolLayout::Mirror, 2);
        tank.scan = checked(3);
        let health = HashMap::from([("S1".to_owned(), Health::Good)]);

        let overview = overview(&[tank], &[disk("sda", Some("S1"))], &health, now());

        assert_eq!(overview, Overview { concerns: vec![] });
        assert_eq!(overview.verdict(), None);
    }

    #[test]
    fn the_worst_comes_first() {
        let single = pool("backup", PoolLayout::Single, 1);
        let mut tank = pool("tank", PoolLayout::Raidz { parity: 1 }, 3);
        tank.state = VdevState::Degraded;
        tank.devices[1].checksum_errors = 3;
        tank.space = Some(PoolSpace { used_bytes: 85, available_bytes: 15, mounted: true });
        let health = HashMap::from([("S1".to_owned(), Health::Failing), ("sdb".to_owned(), Health::Warning)]);
        let disks = [disk("sda", Some("S1")), disk("sdb", None), disk("sdc", Some("unread"))];

        let overview = overview(&[single, tank], &disks, &health, now());

        let model = Some("Model".to_owned());
        assert_eq!(
            overview.concerns,
            [
                Concern::DiskFailing { disk: "sda".into(), model: model.clone() },
                Concern::DiskWorn { disk: "sdb".into(), model },
                Concern::PoolDegraded { pool: "tank".into(), state: VdevState::Degraded },
                Concern::DeviceErrors { pool: "tank".into(), device: "disk-1".into(), errors: 3 },
                Concern::VolumeFilling { pool: "tank".into(), percent: 85 },
                Concern::NoRedundancy { pool: "backup".into() },
            ]
        );
        assert_eq!(overview.verdict(), Some(Severity::Critical));
    }

    #[test]
    fn lost_data_and_a_full_volume_are_critical() {
        let mut tank = pool("tank", PoolLayout::Mirror, 2);
        tank.state = VdevState::Suspended;
        tank.data_errors = 2;
        tank.space = Some(PoolSpace { used_bytes: 95, available_bytes: 5, mounted: true });

        let concerns = overview(&[tank], &[], &HashMap::new(), now()).concerns;

        assert!(concerns.iter().all(|c| c.severity() == Severity::Critical), "{concerns:?}");
        assert_eq!(concerns.len(), 3);
    }

    #[test]
    fn a_mirror_that_lost_its_partner_is_not_redundant() {
        let concerns = overview(&[pool("tank", PoolLayout::Mirror, 1)], &[], &HashMap::new(), now()).concerns;
        assert_eq!(concerns, [Concern::NoRedundancy { pool: "tank".into() }]);
    }

    #[test]
    fn a_missed_monthly_check_is_noticed_but_a_new_pool_is_not() {
        let mut old = pool("old", PoolLayout::Mirror, 2);
        old.scan = checked(45);
        let mut recent = pool("recent", PoolLayout::Mirror, 2);
        recent.scan = checked(30);
        let new = pool("new", PoolLayout::Mirror, 2);

        let concerns = overview(&[old, recent, new], &[], &HashMap::new(), now()).concerns;

        assert_eq!(concerns, [Concern::ScrubOverdue { pool: "old".into(), days: 45 }]);
        assert_eq!(concerns[0].severity(), Severity::Notice);
    }
}
