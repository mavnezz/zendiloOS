use std::collections::BTreeSet;

use super::block_device::{BlockDevice, DeviceKind, SpaceUsage};
use super::disk::is_system_mount;
use super::pool::Pool;

/// Filesystems zendiloos treats as data volumes.
pub const DATA_FILESYSTEMS: [&str; 3] = ["btrfs", "ext4", "xfs"];

/// A data filesystem, on an array, a disk or a partition.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// The filesystem UUID; the device name if there is none.
    pub id: String,
    pub label: Option<String>,
    pub filesystem: String,
    /// The block devices carrying it: one, or several for a multi-device
    /// Btrfs.
    pub devices: Vec<String>,
    /// The software RAID array it lives on, if any.
    pub array: Option<String>,
    /// The ZFS pool it is the root dataset of, if any.
    pub pool: Option<String>,
    /// The disks underneath.
    pub disks: Vec<String>,
    pub mount_points: Vec<String>,
    pub space: Option<SpaceUsage>,
}

/// The data volumes: the root datasets of ZFS pools, then filesystems on
/// the host's block devices. Filesystems on a system disk belong to the
/// operating system and are left out. `pools` must have their disks
/// resolved ([`super::resolve_pool_disks`]).
pub fn volumes(devices: &[BlockDevice], pools: &[Pool]) -> Vec<Volume> {
    let mut volumes: Vec<Volume> = pools.iter().map(pool_volume).collect();
    volumes.extend(block_volumes(devices));
    volumes
}

fn pool_volume(pool: &Pool) -> Volume {
    let disks: BTreeSet<String> = pool.devices.iter().filter_map(|d| d.disk.clone()).collect();
    let mounted = pool.space.is_some_and(|space| space.mounted);
    Volume {
        id: pool.guid.clone(),
        label: Some(pool.name.clone()),
        filesystem: "zfs".to_owned(),
        devices: vec![pool.name.clone()],
        array: None,
        pool: Some(pool.name.clone()),
        disks: disks.into_iter().collect(),
        mount_points: match (mounted, pool.volume) {
            (true, Some(number)) => vec![number.mount_point()],
            _ => Vec::new(),
        },
        space: pool.space.map(|space| SpaceUsage {
            size_bytes: space.used_bytes.saturating_add(space.available_bytes),
            used_bytes: space.used_bytes,
            available_bytes: space.available_bytes,
        }),
    }
}

fn block_volumes(devices: &[BlockDevice]) -> Vec<Volume> {
    let mut volumes: Vec<Volume> = Vec::new();
    for disk in devices.iter().filter(|device| device.kind == DeviceKind::Disk) {
        let below = disk.descendants();
        if below.iter().any(|device| device.mount_points.iter().any(|m| is_system_mount(m))) {
            continue;
        }
        for device in below {
            let Some(filesystem) = &device.filesystem else { continue };
            if !DATA_FILESYSTEMS.contains(&filesystem.kind.as_str()) {
                continue;
            }
            let id = filesystem.uuid.clone().unwrap_or_else(|| device.name.clone());
            let index = match volumes.iter().position(|volume| volume.id == id) {
                Some(index) => index,
                None => {
                    volumes.push(Volume {
                        id,
                        label: filesystem.label.clone(),
                        filesystem: filesystem.kind.clone(),
                        devices: Vec::new(),
                        array: None,
                        pool: None,
                        disks: Vec::new(),
                        mount_points: Vec::new(),
                        space: None,
                    });
                    volumes.len() - 1
                }
            };
            let volume = &mut volumes[index];
            add_unique(&mut volume.devices, &device.name);
            add_unique(&mut volume.disks, &disk.name);
            if matches!(device.kind, DeviceKind::Raid { .. }) {
                volume.array = Some(device.name.clone());
            }
            if volume.mount_points.is_empty() {
                volume.mount_points.clone_from(&device.mount_points);
            }
            volume.space = volume.space.or(filesystem.space);
        }
    }
    for volume in &mut volumes {
        volume.disks = volume.disks.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
    }
    volumes
}

fn add_unique(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|n| n == name) {
        names.push(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::super::block_device::testing::*;
    use super::*;

    #[test]
    fn an_array_shared_by_two_disks_is_one_volume() {
        let array = || raid("md0", Some("btrfs"), &["/mnt/data"]);
        let devices = [disk("sdc", vec![array()]), disk("sdb", vec![array()])];

        let volumes = volumes(&devices, &[]);

        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].devices, ["md0"]);
        assert_eq!(volumes[0].array.as_deref(), Some("md0"));
        assert_eq!(volumes[0].disks, ["sdb", "sdc"]);
        assert_eq!(volumes[0].mount_points, ["/mnt/data"]);
    }

    #[test]
    fn a_multi_device_btrfs_is_one_volume() {
        let member = |name: &str, mounted: &[&str]| {
            let mut disk = with_filesystem(disk(name, vec![]), Some("btrfs"), mounted);
            disk.filesystem.as_mut().unwrap().uuid = Some("shared".into());
            disk
        };
        let volumes = volumes(&[member("sdb", &["/mnt/pool"]), member("sdc", &[])], &[]);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].devices, ["sdb", "sdc"]);
        assert_eq!(volumes[0].array, None);
        assert_eq!(volumes[0].mount_points, ["/mnt/pool"]);
    }

    #[test]
    fn leaves_out_the_system_and_foreign_filesystems() {
        let devices = [
            disk(
                "nvme0n1",
                vec![partition("nvme0n1p1", Some("ext4"), &["/"]), partition("nvme0n1p2", Some("xfs"), &[])],
            ),
            disk("sdd", vec![partition("sdd1", Some("ntfs"), &[]), partition("sdd2", Some("ext4"), &[])]),
        ];
        let volumes = volumes(&devices, &[]);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].devices, ["sdd2"]);
        assert!(volumes[0].mount_points.is_empty(), "not mounted");
    }
}
