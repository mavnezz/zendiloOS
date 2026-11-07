use super::block_device::{BlockDevice, DeviceKind};
use super::pool::Pool;
use super::volume::DATA_FILESYSTEMS;

/// A physical (or virtual) disk with what it is used for.
#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub name: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    /// Name of its `/dev/disk/by-id` link; disks without one cannot be
    /// used for pools, since they could not be recognized again.
    pub id_link: Option<String>,
    pub size_bytes: u64,
    pub transport: Transport,
    pub media: Media,
    pub removable: bool,
    pub read_only: bool,
    pub usage: DiskUsage,
    pub partitions: Vec<Partition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Sata,
    Sas,
    Nvme,
    Usb,
    Virtio,
    Other,
}

impl Transport {
    fn from_kernel(name: Option<&str>) -> Self {
        match name {
            Some("sata" | "ata") => Transport::Sata,
            Some("sas") => Transport::Sas,
            Some("nvme") => Transport::Nvme,
            Some("usb") => Transport::Usb,
            Some("virtio") => Transport::Virtio,
            _ => Transport::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Rotational,
    SolidState,
}

/// What a disk is used for. When several apply, the first one in this
/// order wins: a disk holding the operating system is a system disk even
/// if it is also an array member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskUsage {
    /// Holds the operating system; never offered for storage.
    System,
    /// The disk, or a partition on it, belongs to a software RAID array.
    ArrayMember { array: String },
    /// The disk holds a device of a ZFS pool.
    PoolMember { pool: String },
    /// Carries a data volume directly (no array).
    Volume,
    /// Mounted as a removable drive: ejected first, then it may be used.
    Removable,
    /// Has partitions or a signature zendiloos does not manage.
    Foreign,
    /// Nothing on it: free for a new array (M3).
    Unused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub name: String,
    pub size_bytes: u64,
    pub filesystem: Option<String>,
    pub label: Option<String>,
    pub mount_points: Vec<String>,
}

/// Mount points that belong to the operating system.
pub fn is_system_mount(mount_point: &str) -> bool {
    const EXACT: [&str; 7] = ["/", "/boot", "/efi", "/usr", "/var", "/home", "[SWAP]"];
    const BELOW: [&str; 3] = ["/boot/", "/usr/", "/var/"];
    EXACT.contains(&mount_point) || BELOW.iter().any(|prefix| mount_point.starts_with(prefix))
}

/// The smallest device ZFS takes; anything smaller (a floppy drive) is no
/// disk for the NAS.
const MIN_DISK_BYTES: u64 = 64 * 1024 * 1024;

/// The disks among the host's block devices, each classified by usage;
/// `pools` must have their disks resolved ([`super::resolve_pool_disks`]).
/// Loop devices, optical drives, empty card readers and floppy drives are
/// not disks.
pub fn disks(devices: &[BlockDevice], pools: &[Pool]) -> Vec<Disk> {
    devices
        .iter()
        .filter(|device| device.kind == DeviceKind::Disk && device.size_bytes >= MIN_DISK_BYTES)
        .map(|device| Disk {
            name: device.name.clone(),
            model: device.model.clone(),
            serial: device.serial.clone(),
            wwn: device.wwn.clone(),
            id_link: device.id_link.clone(),
            size_bytes: device.size_bytes,
            transport: Transport::from_kernel(device.transport.as_deref()),
            media: if device.rotational { Media::Rotational } else { Media::SolidState },
            removable: device.removable,
            read_only: device.read_only,
            usage: usage(device, pools),
            partitions: device
                .children
                .iter()
                .filter(|child| child.kind == DeviceKind::Partition)
                .map(|child| Partition {
                    name: child.name.clone(),
                    size_bytes: child.size_bytes,
                    filesystem: child.filesystem.as_ref().map(|f| f.kind.clone()),
                    label: child.filesystem.as_ref().and_then(|f| f.label.clone()),
                    mount_points: child.mount_points.clone(),
                })
                .collect(),
        })
        .collect()
}

fn usage(disk: &BlockDevice, pools: &[Pool]) -> DiskUsage {
    let below = disk.descendants();
    if below.iter().any(|device| device.mount_points.iter().any(|m| is_system_mount(m))) {
        return DiskUsage::System;
    }
    let removable_place = format!("{}/", super::REMOVABLE_PLACE);
    if below.iter().any(|device| device.mount_points.iter().any(|m| m.starts_with(&removable_place))) {
        return DiskUsage::Removable;
    }
    if let Some(array) = below.iter().find(|device| matches!(device.kind, DeviceKind::Raid { .. })) {
        return DiskUsage::ArrayMember { array: array.name.clone() };
    }
    // By the pool's own device list: a partition may still carry the label
    // of a pool it was removed from.
    if let Some(pool) = pools.iter().find(|pool| pool.uses_disk(&disk.name)) {
        return DiskUsage::PoolMember { pool: pool.name.clone() };
    }
    let data = |device: &&BlockDevice| {
        device.filesystem.as_ref().is_some_and(|f| DATA_FILESYSTEMS.contains(&f.kind.as_str()))
    };
    if below.iter().any(data) {
        DiskUsage::Volume
    } else if below.len() > 1 || disk.filesystem.is_some() {
        DiskUsage::Foreign
    } else {
        DiskUsage::Unused
    }
}

#[cfg(test)]
mod tests {
    use super::super::block_device::testing::*;
    use super::*;

    fn usage_of(device: BlockDevice) -> DiskUsage {
        disks(&[device], &[]).remove(0).usage
    }

    #[test]
    fn classifies_disks_by_what_they_hold() {
        let system = disk(
            "nvme0n1",
            vec![
                partition("nvme0n1p1", Some("vfat"), &["/boot/efi"]),
                partition("nvme0n1p2", Some("ext4"), &["/"]),
            ],
        );
        assert_eq!(usage_of(system), DiskUsage::System);

        let member = disk("sdb", vec![raid("md0", Some("btrfs"), &["/mnt/data"])]);
        assert_eq!(usage_of(member), DiskUsage::ArrayMember { array: "md0".into() });

        let volume = disk("sdd", vec![partition("sdd1", Some("xfs"), &[])]);
        assert_eq!(usage_of(volume), DiskUsage::Volume);

        let foreign = disk("sde", vec![partition("sde1", Some("ntfs"), &[])]);
        assert_eq!(usage_of(foreign), DiskUsage::Foreign);
        let signature = with_filesystem(disk("vdb", vec![]), Some("iso9660"), &[]);
        assert_eq!(usage_of(signature), DiskUsage::Foreign);

        assert_eq!(usage_of(disk("sdf", vec![])), DiskUsage::Unused);
    }

    #[test]
    fn a_mounted_removable_drive_is_no_disk_to_take() {
        // A stick's file system often sits on the disk itself, without a
        // partition table.
        let stick =
            with_filesystem(disk("sdg", vec![]), Some("vfat"), &["/run/zendiloos-removable/1A2B-3C4D"]);
        assert_eq!(usage_of(stick), DiskUsage::Removable);
        let partitioned =
            disk("sdh", vec![partition("sdh1", Some("exfat"), &["/run/zendiloos-removable/5E6F-7A8B"])]);
        assert_eq!(usage_of(partitioned), DiskUsage::Removable);
        // Only below the place itself.
        let beside = with_filesystem(disk("sdi", vec![]), Some("vfat"), &["/run/zendiloos-removable-x"]);
        assert_eq!(usage_of(beside), DiskUsage::Foreign);
    }

    #[test]
    fn pool_membership_comes_from_the_pool_not_from_labels() {
        use crate::domain::{Pool, PoolDevice, PoolLayout, VdevState};
        let member = disk("sda", vec![partition("sda1", Some("zfs_member"), &[])]);
        // Removed from the pool, but its partition still carries the label.
        let stale = disk("sdd", vec![partition("sdd1", Some("zfs_member"), &[])]);
        let device = PoolDevice {
            name: "ata-A-part1".into(),
            guid: "7".into(),
            path: None,
            disk: Some("sda".into()),
            state: VdevState::Online,
            read_errors: 0,
            write_errors: 0,
            checksum_errors: 0,
            replacing: None,
        };
        let pool = Pool {
            name: "tank".into(),
            guid: "1".into(),
            state: VdevState::Online,
            layout: PoolLayout::Single,
            devices: vec![device],
            size_bytes: 0,
            allocated_bytes: 0,
            scan: None,
            data_errors: 0,
            volume: None,
            space: None,
        };
        let usage: Vec<_> = disks(&[member, stale], &[pool]).into_iter().map(|d| d.usage).collect();
        assert_eq!(usage, [DiskUsage::PoolMember { pool: "tank".into() }, DiskUsage::Foreign]);
    }

    #[test]
    fn the_operating_system_wins_over_other_uses() {
        // Root on a RAID 1 across two disks.
        let disk =
            disk("sda", vec![partition("sda2", None, &[]).with_child(raid("md1", Some("ext4"), &["/"]))]);
        assert_eq!(usage_of(disk), DiskUsage::System);
        let swap = self::disk("sdb", vec![partition("sdb1", Some("swap"), &["[SWAP]"])]);
        assert_eq!(usage_of(swap), DiskUsage::System);
    }

    #[test]
    fn skips_devices_that_are_not_disks() {
        let devices = [
            device("loop0", DeviceKind::Other("loop".into())),
            device("sr0", DeviceKind::Other("rom".into())),
            BlockDevice { size_bytes: 0, ..disk("sdg", vec![]) },
            BlockDevice { size_bytes: 1_474_560, ..disk("fd0", vec![]) },
            disk("sdh", vec![]),
        ];
        let names: Vec<_> = disks(&devices, &[]).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["sdh"]);
    }

    #[test]
    fn system_mounts() {
        for system in ["/", "/boot", "/boot/efi", "/var", "/var/lib/docker", "[SWAP]"] {
            assert!(is_system_mount(system), "{system}");
        }
        for data in ["/mnt/data", "/srv/volume1", "/variable", "/home2"] {
            assert!(!is_system_mount(data), "{data}");
        }
    }

    impl BlockDevice {
        fn with_child(mut self, child: BlockDevice) -> Self {
            self.children.push(child);
            self
        }
    }
}
