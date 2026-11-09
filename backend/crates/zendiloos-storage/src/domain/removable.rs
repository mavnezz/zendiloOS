//! Removable drives: USB sticks and disks, memory cards. zendiloOS mounts
//! those it can read safely, for administrators only, and leaves anything
//! that could belong to the system or to a pool alone.

use std::collections::BTreeMap;

use super::block_device::{BlockDevice, DeviceKind, SpaceUsage};

/// Where removable drives are mounted, a folder each, named by the drive's
/// id: root's, outside the volumes, so no share reaches one.
pub const REMOVABLE_PLACE: &str = "/run/zendiloos-removable";

/// Signatures of a device that belongs to something bigger: a disk with
/// one of them anywhere holds no removable drive.
const MEMBER_SIGNATURES: [&str; 6] =
    ["zfs_member", "linux_raid_member", "LVM2_member", "crypto_LUKS", "bcache", "ddf_raid_member"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovableFilesystem {
    /// FAT12, FAT16 or FAT32.
    Fat,
    Exfat,
    Ntfs,
    Ext4,
}

impl RemovableFilesystem {
    /// From the signature lsblk reports.
    pub fn from_signature(signature: &str) -> Option<Self> {
        match signature {
            "vfat" => Some(Self::Fat),
            "exfat" => Some(Self::Exfat),
            "ntfs" => Some(Self::Ntfs),
            "ext4" => Some(Self::Ext4),
            _ => None,
        }
    }
}

/// A file system on a removable drive that zendiloOS mounts.
#[derive(Debug, Clone, PartialEq)]
pub struct RemovableDrive {
    /// The file system's UUID; also names its place.
    pub id: String,
    /// The device carrying it: a partition (`sdf1`) or the whole disk.
    pub device: String,
    /// The disk it is on: `sdf`.
    pub disk: String,
    pub label: Option<String>,
    pub filesystem: RemovableFilesystem,
    pub size_bytes: u64,
    /// The disk's model, e.g. `SanDisk Ultra`.
    pub model: Option<String>,
    /// Write-protected, or mounted so because it cannot be written.
    pub read_only: bool,
    /// Its place while mounted there: `/run/zendiloos-removable/<id>`.
    pub mount_point: Option<String>,
    /// Only known while mounted.
    pub space: Option<SpaceUsage>,
    /// Which plugging in of its disk this is ([`BlockDevice::plugged_in`]):
    /// a drive ejected stays so until it is plugged in again.
    pub plugged_in: Option<String>,
}

impl RemovableDrive {
    pub fn is_mounted(&self) -> bool {
        self.mount_point.is_some()
    }
}

/// `/run/zendiloos-removable/<id>`.
pub fn removable_place(id: &str) -> String {
    format!("{REMOVABLE_PLACE}/{id}")
}

/// An id that can name a place: letters, digits and `-`, not starting with
/// `-`, at most 64, as file system UUIDs are.
pub fn is_removable_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && !id.starts_with('-')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The removable drives among the host's block devices, by id. A drive is
/// a file system zendiloOS mounts (FAT, exFAT, NTFS, ext4) with an id,
/// directly on a disk or on a partition, nothing held on top of it, on a
/// USB or removable disk where:
///
/// - nothing is a ZFS, RAID, LVM, bcache or LUKS member, an array or a
///   mapping (zendiloOS may boot from a USB disk, a pool may live on one);
/// - nothing is mounted or used as swap, except a drive at its own place.
///
/// Two file systems with the same id (a cloned stick) are both left out:
/// neither could be told from the other. The agent checks all of this
/// again before it mounts or powers anything off.
pub fn removable_drives(devices: &[BlockDevice]) -> Vec<RemovableDrive> {
    let mut drives: Vec<RemovableDrive> = devices.iter().flat_map(on_disk).collect();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for drive in &drives {
        *seen.entry(drive.id.clone()).or_default() += 1;
    }
    drives.retain(|drive| seen[&drive.id] == 1);
    drives.sort_by(|a, b| a.disk.cmp(&b.disk).then_with(|| a.device.cmp(&b.device)));
    drives
}

fn on_disk(disk: &BlockDevice) -> Vec<RemovableDrive> {
    let removable = disk.removable || disk.transport.as_deref() == Some("usb");
    if disk.kind != DeviceKind::Disk || !removable || disk.size_bytes == 0 {
        return Vec::new();
    }
    let below = disk.descendants();
    let member = below.iter().any(|device| {
        let signature = device.filesystem.as_ref().map(|f| f.kind.as_str());
        signature.is_some_and(|s| MEMBER_SIGNATURES.contains(&s))
            || !matches!(device.kind, DeviceKind::Disk | DeviceKind::Partition)
    });
    if member {
        return Vec::new();
    }
    let candidates: Vec<(&BlockDevice, RemovableFilesystem, &str)> = below
        .iter()
        .filter(|device| device.children.is_empty())
        .filter_map(|device| {
            let filesystem = device.filesystem.as_ref()?;
            let kind = RemovableFilesystem::from_signature(&filesystem.kind)?;
            let id = filesystem.uuid.as_deref().filter(|id| is_removable_id(id))?;
            Some((*device, kind, id))
        })
        .collect();
    // Anything mounted elsewhere on the disk, the system or swap among it,
    // keeps the whole disk out.
    let elsewhere = below.iter().any(|device| {
        let own = candidates.iter().find(|(candidate, _, _)| std::ptr::eq(*candidate, *device));
        device.mount_points.iter().any(|mount| own.is_none_or(|(_, _, id)| *mount != removable_place(id)))
    });
    if elsewhere {
        return Vec::new();
    }
    candidates
        .into_iter()
        .map(|(device, filesystem, id)| {
            let mount_point = device.mount_points.first().cloned();
            let found = device.filesystem.as_ref();
            RemovableDrive {
                id: id.to_owned(),
                device: device.name.clone(),
                disk: disk.name.clone(),
                label: found.and_then(|f| f.label.clone()),
                filesystem,
                size_bytes: device.size_bytes,
                model: disk.model.clone(),
                read_only: disk.read_only || device.read_only,
                space: mount_point.as_ref().and_then(|_| found.and_then(|f| f.space)),
                mount_point,
                plugged_in: disk.plugged_in.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::block_device::testing::*;
    use super::*;
    use crate::domain::Filesystem;

    const ID: &str = "1A2B-3C4D";

    fn fat(name: &str, id: &str, mount_points: &[&str]) -> BlockDevice {
        let mut partition = partition(name, Some("vfat"), mount_points);
        if let Some(filesystem) = partition.filesystem.as_mut() {
            filesystem.uuid = Some(id.to_owned());
            filesystem.label = Some("STICK".to_owned());
        }
        partition
    }

    fn stick(name: &str, children: Vec<BlockDevice>) -> BlockDevice {
        BlockDevice {
            transport: Some("usb".into()),
            removable: true,
            model: Some("SanDisk Ultra".into()),
            ..disk(name, children)
        }
    }

    #[test]
    fn a_stick_with_fat_is_a_drive_unmounted_or_at_its_place() {
        let drives = removable_drives(&[stick("sdf", vec![fat("sdf1", ID, &[])])]);
        assert_eq!(drives.len(), 1);
        let drive = &drives[0];
        assert_eq!((drive.id.as_str(), drive.device.as_str(), drive.disk.as_str()), (ID, "sdf1", "sdf"));
        assert_eq!((drive.filesystem, drive.label.as_deref()), (RemovableFilesystem::Fat, Some("STICK")));
        assert_eq!(drive.model.as_deref(), Some("SanDisk Ultra"));
        assert!(!drive.is_mounted() && drive.space.is_none());

        let place = removable_place(ID);
        let mut mounted = fat("sdf1", ID, &[&place]);
        let space = SpaceUsage { size_bytes: 100, used_bytes: 40, available_bytes: 60 };
        mounted.filesystem.as_mut().unwrap().space = Some(space);
        let drive = removable_drives(&[stick("sdf", vec![mounted])]).remove(0);
        assert_eq!((drive.mount_point.as_deref(), drive.space), (Some(place.as_str()), Some(space)));
    }

    #[test]
    fn a_disk_on_its_own_and_every_listed_file_system_count() {
        let whole = with_filesystem(stick("sdg", vec![]), Some("exfat"), &[]);
        let mut whole = whole;
        whole.filesystem.as_mut().unwrap().uuid = Some("5E6F-7A8B".into());
        let mut reader = disk("mmcblk1", vec![]);
        reader.removable = true;
        reader.transport = None;
        let mut card = partition("mmcblk1p1", Some("ext4"), &[]);
        card.filesystem.as_mut().unwrap().uuid = Some("2f6b0c4e-8d1a".into());
        card.read_only = true;
        reader.children = vec![card];
        let ntfs = stick(
            "sdh",
            vec![{
                let mut p = partition("sdh1", Some("ntfs"), &[]);
                p.filesystem.as_mut().unwrap().uuid = Some("0123456789ABCDEF".into());
                p
            }],
        );
        let drives = removable_drives(&[whole, reader, ntfs]);
        let found: Vec<_> = drives.iter().map(|d| (d.device.as_str(), d.filesystem, d.read_only)).collect();
        assert_eq!(
            found,
            [
                ("mmcblk1p1", RemovableFilesystem::Ext4, true),
                ("sdg", RemovableFilesystem::Exfat, false),
                ("sdh1", RemovableFilesystem::Ntfs, false),
            ]
        );
    }

    #[test]
    fn nothing_that_could_be_the_system_or_a_pool_is_a_drive() {
        let none =
            |disk: BlockDevice| assert_eq!(removable_drives(std::slice::from_ref(&disk)), [], "{disk:?}");
        // A SATA disk with FAT on it.
        none(disk("sda", vec![fat("sda1", ID, &[])]));
        // The system on a USB disk: its EFI partition is FAT too.
        none(stick("sdf", vec![fat("sdf1", ID, &["/boot/efi"]), partition("sdf2", Some("ext4"), &["/"])]));
        // Mounted somewhere else, or swap beside it.
        none(stick("sdf", vec![fat("sdf1", ID, &["/media/stick"])]));
        none(stick("sdf", vec![fat("sdf1", ID, &[]), partition("sdf2", Some("swap"), &["[SWAP]"])]));
        // A pool's disk, an array's, an encrypted one, a mapping on top.
        for signature in ["zfs_member", "linux_raid_member", "crypto_LUKS", "LVM2_member"] {
            none(stick("sdf", vec![fat("sdf1", ID, &[]), partition("sdf2", Some(signature), &[])]));
        }
        none(stick("sdf", vec![partition("sdf1", None, &[]).with(raid("md0", Some("vfat"), &[]))]));
        let mut held = fat("sdf1", ID, &[]);
        held.children.push(device("dm-0", DeviceKind::Other("crypt".into())));
        none(stick("sdf", vec![held]));
        // A file system that is not for removable drives, or without an id.
        none(stick("sdf", vec![partition("sdf1", Some("btrfs"), &[])]));
        none(stick(
            "sdf",
            vec![{
                let mut p = fat("sdf1", ID, &[]);
                p.filesystem = Some(Filesystem { uuid: None, ..p.filesystem.unwrap() });
                p
            }],
        ));
        none(stick("sdf", vec![fat("sdf1", "../etc", &[])]));
        // An empty card reader.
        none(BlockDevice { size_bytes: 0, ..stick("sdi", vec![]) });
    }

    #[test]
    fn cloned_sticks_are_left_out_others_stay() {
        let drives = removable_drives(&[
            stick("sdf", vec![fat("sdf1", ID, &[])]),
            stick("sdg", vec![fat("sdg1", ID, &[])]),
            stick("sdh", vec![fat("sdh1", "9F00-0001", &[])]),
        ]);
        assert_eq!(drives.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["9F00-0001"]);
    }

    #[test]
    fn ids_name_places_and_nothing_else() {
        assert_eq!(removable_place(ID), "/run/zendiloos-removable/1A2B-3C4D");
        assert!(is_removable_id("2f6b0c4e-8d1a-4f7e-9c3b-5a6d7e8f9a0b"));
        for odd in ["", "-o", "a/b", "..", "a b", &"a".repeat(65)] {
            assert!(!is_removable_id(odd), "{odd}");
        }
        assert_eq!(RemovableFilesystem::from_signature("iso9660"), None);
    }

    impl BlockDevice {
        fn with(mut self, child: BlockDevice) -> Self {
            self.children.push(child);
            self
        }
    }
}
