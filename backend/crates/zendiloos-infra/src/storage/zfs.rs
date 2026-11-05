//! `zpool status -j --json-int` and `zfs list -j --json-int` (OpenZFS 2.3)
//! into pools.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Number, Value};
use zendiloos_kernel::Timestamp;
use zendiloos_storage::domain::{
    Pool, PoolDevice, PoolLayout, PoolSpace, Replacing, Scan, ScanKind, ScanState, VdevState, VolumeNumber,
};

use super::ParseError;

/// The ZFS user property holding the volume number of pools zendiloos
/// created.
pub(crate) const VOLUME_PROPERTY: &str = "zendiloos:volume";

#[derive(Deserialize)]
struct StatusOutput {
    #[serde(default)]
    pools: HashMap<String, PoolStatus>,
}

#[derive(Deserialize)]
struct PoolStatus {
    name: String,
    state: String,
    pool_guid: Number,
    #[serde(default)]
    error_count: Option<Value>,
    scan_stats: Option<ScanStats>,
    vdevs: HashMap<String, Vdev>,
}

#[derive(Deserialize)]
struct Vdev {
    name: String,
    vdev_type: String,
    guid: Number,
    #[serde(default)]
    path: Option<String>,
    state: String,
    #[serde(default)]
    alloc_space: u64,
    #[serde(default)]
    total_space: u64,
    #[serde(default)]
    read_errors: u64,
    #[serde(default)]
    write_errors: u64,
    #[serde(default)]
    checksum_errors: u64,
    /// Children in the pool's order (serde_json keeps the document order).
    #[serde(default)]
    vdevs: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct ScanStats {
    function: String,
    state: String,
    #[serde(default)]
    start_time: i64,
    #[serde(default)]
    end_time: i64,
    #[serde(default)]
    to_examine: u64,
    #[serde(default)]
    issued: u64,
    #[serde(default)]
    errors: u64,
}

#[derive(Deserialize)]
struct ListOutput {
    #[serde(default)]
    datasets: HashMap<String, Dataset>,
}

#[derive(Deserialize)]
struct Dataset {
    name: String,
    properties: HashMap<String, Property>,
}

#[derive(Deserialize)]
struct Property {
    value: Value,
}

fn invalid(reason: impl Into<String>) -> ParseError {
    ParseError::new("zpool", reason.into())
}

/// Pools from `zpool status`, with space and volume number from
/// `zfs list` (columns `ZFS_DATASET_COLUMNS` of the agent protocol).
pub fn parse(zpool_status: &str, zfs_list: &str) -> Result<Vec<Pool>, ParseError> {
    let status: StatusOutput = serde_json::from_str(zpool_status).map_err(|e| ParseError::new("zpool", e))?;
    let list: ListOutput = serde_json::from_str(zfs_list).map_err(|e| ParseError::new("zfs", e))?;
    let mut pools = status.pools.into_values().map(pool).collect::<Result<Vec<_>, _>>()?;
    for pool in &mut pools {
        if let Some(root) = list.datasets.values().find(|dataset| dataset.name == pool.name) {
            pool.space = space(root);
            pool.volume = root
                .properties
                .get(VOLUME_PROPERTY)
                .and_then(|p| integer(&p.value))
                .and_then(|n| u16::try_from(n).ok().and_then(VolumeNumber::new));
        }
    }
    pools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(pools)
}

fn pool(status: PoolStatus) -> Result<Pool, ParseError> {
    let root =
        status.vdevs.get(&status.name).ok_or_else(|| invalid(format!("{}: no root vdev", status.name)))?;
    let children = children(root)?;
    let layout = layout(&children);
    let mut devices = Vec::new();
    for child in &children {
        leaves(child, None, &mut devices)?;
    }
    Ok(Pool {
        name: status.name,
        guid: status.pool_guid.to_string(),
        state: state(&status.state),
        layout,
        devices,
        size_bytes: root.total_space,
        allocated_bytes: root.alloc_space,
        scan: status.scan_stats.and_then(scan),
        data_errors: status.error_count.as_ref().and_then(integer).unwrap_or(0),
        volume: None,
        space: None,
    })
}

fn children(vdev: &Vdev) -> Result<Vec<Vdev>, ParseError> {
    vdev.vdevs
        .values()
        .map(|child| serde_json::from_value(child.clone()).map_err(|e| ParseError::new("zpool", e)))
        .collect()
}

fn layout(data_vdevs: &[Vdev]) -> PoolLayout {
    match data_vdevs {
        [only] => match only.vdev_type.as_str() {
            "disk" | "file" | "replacing" => PoolLayout::Single,
            "mirror" => PoolLayout::Mirror,
            // Named `raidz1-0`, `raidz2-0`, …
            "raidz" => only
                .name
                .strip_prefix("raidz")
                .and_then(|rest| rest.split('-').next())
                .and_then(|parity| parity.parse().ok())
                .map_or_else(|| PoolLayout::Other(only.name.clone()), |parity| PoolLayout::Raidz { parity }),
            other => PoolLayout::Other(other.to_owned()),
        },
        many if many.iter().all(|v| v.vdev_type == "disk") => PoolLayout::Other("stripe".to_owned()),
        many => PoolLayout::Other(format!("{} vdevs", many.len())),
    }
}

/// Leaf devices below `vdev`; a `replacing` vdev's first child is the one
/// being replaced.
fn leaves(vdev: &Vdev, replacing: Option<Replacing>, out: &mut Vec<PoolDevice>) -> Result<(), ParseError> {
    let children = children(vdev)?;
    if children.is_empty() {
        out.push(PoolDevice {
            name: vdev.name.clone(),
            guid: vdev.guid.to_string(),
            path: vdev.path.clone(),
            disk: None,
            state: state(&vdev.state),
            read_errors: vdev.read_errors,
            write_errors: vdev.write_errors,
            checksum_errors: vdev.checksum_errors,
            replacing,
        });
        return Ok(());
    }
    for (index, child) in children.iter().enumerate() {
        let role = match vdev.vdev_type.as_str() {
            "replacing" if index == 0 => Some(Replacing::Old),
            "replacing" => Some(Replacing::New),
            _ => replacing,
        };
        leaves(child, role, out)?;
    }
    Ok(())
}

fn state(value: &str) -> VdevState {
    match value {
        "ONLINE" => VdevState::Online,
        "DEGRADED" => VdevState::Degraded,
        "FAULTED" => VdevState::Faulted,
        "OFFLINE" => VdevState::Offline,
        "REMOVED" => VdevState::Removed,
        "UNAVAIL" | "CANT_OPEN" => VdevState::Unavailable,
        "SUSPENDED" => VdevState::Suspended,
        _ => VdevState::Unknown,
    }
}

fn scan(stats: ScanStats) -> Option<Scan> {
    let kind = match stats.function.as_str() {
        "SCRUB" => ScanKind::Scrub,
        "RESILVER" => ScanKind::Resilver,
        _ => return None,
    };
    let state = match stats.state.as_str() {
        "SCANNING" => ScanState::Running,
        "FINISHED" => ScanState::Finished,
        "CANCELED" => ScanState::Canceled,
        _ => return None,
    };
    Some(Scan {
        kind,
        state,
        done_bytes: stats.issued,
        total_bytes: stats.to_examine,
        started_at: Timestamp::from_unix_seconds(stats.start_time),
        finished_at: (stats.end_time > 0).then(|| Timestamp::from_unix_seconds(stats.end_time)),
        errors: stats.errors,
    })
}

fn space(root: &Dataset) -> Option<PoolSpace> {
    let get = |name: &str| root.properties.get(name).and_then(|p| integer(&p.value));
    Some(PoolSpace {
        used_bytes: get("used")?,
        available_bytes: get("available")?,
        mounted: root.properties.get("mounted").is_some_and(|p| p.value.as_str() == Some("yes")),
    })
}

/// `--json-int` prints numbers, but user properties stay text unless they
/// look like one.
fn integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("fixtures/zfs-list.json");

    fn only(status: &str) -> Pool {
        let mut pools = parse(status, LIST).unwrap();
        assert_eq!(pools.len(), 1);
        pools.remove(0)
    }

    #[test]
    fn a_healthy_mirror_with_its_volume() {
        let pool = only(include_str!("fixtures/zpool-status-mirror-online.json"));
        assert_eq!(pool.name, "tank");
        assert_eq!(pool.guid, "9846106214133009000");
        assert_eq!((pool.state, pool.layout.clone()), (VdevState::Online, PoolLayout::Mirror));
        assert_eq!(pool.size_bytes, 8_053_063_680);
        let names: Vec<_> = pool.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["ata-QEMU_HARDDISK_ZDL-SATA-1-part1", "ata-QEMU_HARDDISK_ZDL-SATA-2-part1"]);
        assert_eq!(pool.devices[0].guid, "4933705814018298948");
        assert_eq!(
            pool.devices[0].path.as_deref(),
            Some("/dev/disk/by-id/ata-QEMU_HARDDISK_ZDL-SATA-1-part1")
        );
        assert_eq!(pool.scan, None);
        assert_eq!(pool.volume, VolumeNumber::new(1));
        let space = pool.space.unwrap();
        assert_eq!((space.used_bytes, space.available_bytes), (1_574_395_904, 6_227_009_536));
    }

    #[test]
    fn a_mirror_that_lost_a_disk() {
        let pool = only(include_str!("fixtures/zpool-status-mirror-degraded.json"));
        assert_eq!(pool.state, VdevState::Degraded);
        let states: Vec<_> = pool.devices.iter().map(|d| d.state).collect();
        assert_eq!(states, [VdevState::Removed, VdevState::Online]);
    }

    #[test]
    fn a_mirror_resilvering_onto_a_new_disk() {
        let pool = only(include_str!("fixtures/zpool-status-mirror-resilvering.json"));
        let roles: Vec<_> = pool.devices.iter().map(|d| (d.name.as_str(), d.state, d.replacing)).collect();
        assert_eq!(
            roles,
            [
                ("ata-QEMU_HARDDISK_ZDL-SATA-1-part1", VdevState::Removed, Some(Replacing::Old)),
                ("ata-QEMU_HARDDISK_ZDL-SATA-3-part1", VdevState::Online, Some(Replacing::New)),
                ("ata-QEMU_HARDDISK_ZDL-SATA-2-part1", VdevState::Online, None),
            ]
        );
        let scan = pool.scan.clone().unwrap();
        assert_eq!((scan.kind, scan.state), (ScanKind::Resilver, ScanState::Running));
        assert!(scan.percent() > 0.0 && scan.percent() < 100.0, "{}", scan.percent());
        assert!(pool.is_rebuilding());
        assert_eq!(pool.layout, PoolLayout::Mirror);
    }

    #[test]
    fn finished_resilver_and_scrubs() {
        let resilvered = only(include_str!("fixtures/zpool-status-mirror-resilvered.json"));
        let scan = resilvered.scan.unwrap();
        assert_eq!((scan.kind, scan.state, scan.percent()), (ScanKind::Resilver, ScanState::Finished, 100.0));
        assert!(scan.finished_at.is_some());
        assert_eq!(resilvered.devices.iter().filter(|d| d.replacing.is_some()).count(), 0);

        let scrubbing = only(include_str!("fixtures/zpool-status-mirror-scrubbing.json")).scan.unwrap();
        assert_eq!((scrubbing.kind, scrubbing.state), (ScanKind::Scrub, ScanState::Running));
        assert!(scrubbing.finished_at.is_none());

        let scrubbed = only(include_str!("fixtures/zpool-status-mirror-scrubbed.json")).scan.unwrap();
        assert_eq!((scrubbed.state, scrubbed.errors), (ScanState::Finished, 0));
    }

    #[test]
    fn raidz_and_single_disk_layouts() {
        let raidz = only(include_str!("fixtures/zpool-status-raidz1.json"));
        assert_eq!((raidz.name.as_str(), raidz.layout.clone()), ("media", PoolLayout::Raidz { parity: 1 }));
        assert_eq!(raidz.devices.len(), 3);
        assert_eq!(raidz.volume, None, "not created by zendiloos");
        assert_eq!(raidz.space, None, "not in this zfs list");

        let single = only(include_str!("fixtures/zpool-status-single.json"));
        assert_eq!(single.layout, PoolLayout::Single);
        assert_eq!(
            single.devices[0].path.as_deref(),
            Some("/dev/disk/by-id/nvme-QEMU_NVMe_Ctrl_ZDL-NVME-1-part1")
        );
    }

    #[test]
    fn no_pools_and_bad_output() {
        let empty =
            r#"{"output_version":{"command":"zpool status","vers_major":0,"vers_minor":1},"pools":{}}"#;
        let no_datasets = r#"{"output_version":{"command":"zfs list","vers_major":0,"vers_minor":1}}"#;
        assert_eq!(parse(empty, no_datasets).unwrap(), []);
        assert!(parse("The ZFS modules cannot be auto-loaded.", LIST).is_err());
    }

    #[test]
    fn pools_resolve_to_the_disks_lsblk_reports() {
        use zendiloos_storage::domain::{DiskUsage, disks, resolve_pool_disks};

        let devices =
            crate::storage::lsblk::parse(include_str!("fixtures/lsblk-zfs-stale-label.json")).unwrap();
        let mut pools = parse(include_str!("fixtures/zpool-status-mirror-resilvered.json"), LIST).unwrap();
        resolve_pool_disks(&mut pools, &devices);

        let usage: Vec<_> = disks(&devices, &pools).into_iter().map(|d| (d.name, d.usage)).collect();
        let tank = || DiskUsage::PoolMember { pool: "tank".into() };
        // sdd came back after it was replaced: its label is stale.
        assert_eq!(
            usage,
            [
                ("sda".into(), tank()),
                ("sdb".into(), tank()),
                ("sdc".into(), DiskUsage::Unused),
                ("sdd".into(), DiskUsage::Foreign),
                ("vda".into(), DiskUsage::System),
                ("vdb".into(), DiskUsage::Foreign),
                ("nvme0n1".into(), DiskUsage::Unused),
            ]
        );
    }
}
