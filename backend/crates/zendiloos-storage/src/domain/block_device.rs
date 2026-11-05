/// One node of the kernel's block device tree as the host reports it: a
/// disk, a partition, a RAID array, … with what it holds below it.
///
/// This is the raw input of the inventory; what a device *is for* (system
/// disk, array member, volume) is decided in [`super::disks`] and
/// [`super::volumes`]. A device used by several parents, such as an array
/// on two disks, appears below each of them.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDevice {
    /// Kernel name, e.g. `sda`, `nvme0n1p2`, `md0`.
    pub name: String,
    pub kind: DeviceKind,
    pub size_bytes: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    /// Transport as the kernel names it: `sata`, `nvme`, `usb`, `virtio`, …
    pub transport: Option<String>,
    pub rotational: bool,
    pub removable: bool,
    pub read_only: bool,
    /// The filesystem or other signature found on the device.
    pub filesystem: Option<Filesystem>,
    /// Where it is mounted; `[SWAP]` for active swap.
    pub mount_points: Vec<String>,
    pub children: Vec<BlockDevice>,
}

impl BlockDevice {
    /// This device and everything below it, depth first.
    pub fn descendants(&self) -> Vec<&BlockDevice> {
        let mut all = vec![self];
        for child in &self.children {
            all.extend(child.descendants());
        }
        all
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    Disk,
    Partition,
    /// A software RAID array, e.g. level `raid1`.
    Raid {
        level: String,
    },
    /// Anything else (`lvm`, `crypt`, `loop`, `rom`, …), kept by name.
    Other(String),
}

/// A signature on a device: a filesystem such as `btrfs`, but also `swap`
/// or `linux_raid_member`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filesystem {
    pub kind: String,
    pub label: Option<String>,
    pub uuid: Option<String>,
    /// Only known while mounted.
    pub space: Option<SpaceUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceUsage {
    pub size_bytes: u64,
    pub used_bytes: u64,
    /// For unprivileged users; less than size − used on most filesystems.
    pub available_bytes: u64,
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn device(name: &str, kind: DeviceKind) -> BlockDevice {
        BlockDevice {
            name: name.to_owned(),
            kind,
            size_bytes: 8 << 30,
            model: None,
            serial: None,
            wwn: None,
            transport: None,
            rotational: false,
            removable: false,
            read_only: false,
            filesystem: None,
            mount_points: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn disk(name: &str, children: Vec<BlockDevice>) -> BlockDevice {
        BlockDevice { transport: Some("sata".into()), children, ..device(name, DeviceKind::Disk) }
    }

    pub fn partition(name: &str, filesystem: Option<&str>, mount_points: &[&str]) -> BlockDevice {
        with_filesystem(device(name, DeviceKind::Partition), filesystem, mount_points)
    }

    pub fn raid(name: &str, filesystem: Option<&str>, mount_points: &[&str]) -> BlockDevice {
        with_filesystem(device(name, DeviceKind::Raid { level: "raid1".into() }), filesystem, mount_points)
    }

    pub fn with_filesystem(device: BlockDevice, kind: Option<&str>, mount_points: &[&str]) -> BlockDevice {
        BlockDevice {
            filesystem: kind.map(|kind| Filesystem {
                kind: kind.to_owned(),
                label: None,
                uuid: Some(format!("uuid-{}", device.name)),
                space: None,
            }),
            mount_points: mount_points.iter().map(|&m| m.to_owned()).collect(),
            ..device
        }
    }
}
