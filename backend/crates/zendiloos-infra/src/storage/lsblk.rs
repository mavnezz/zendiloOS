//! `lsblk --json --bytes --tree --output LSBLK_COLUMNS` into the block
//! device tree of the Storage context.

use serde::Deserialize;
use zendiloos_storage::domain::{BlockDevice, DeviceKind, Filesystem, SpaceUsage};

use super::ParseError;

#[derive(Deserialize)]
struct Output {
    blockdevices: Vec<Node>,
}

/// One device as lsblk prints it. Older util-linux versions print flags as
/// `"0"`/`"1"` and sizes as strings, hence the lenient types.
#[derive(Deserialize)]
struct Node {
    kname: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<Number>,
    model: Option<String>,
    serial: Option<String>,
    wwn: Option<String>,
    #[serde(rename = "id-link")]
    id_link: Option<String>,
    tran: Option<String>,
    rota: Option<Flag>,
    rm: Option<Flag>,
    ro: Option<Flag>,
    fstype: Option<String>,
    label: Option<String>,
    uuid: Option<String>,
    fssize: Option<Number>,
    fsused: Option<Number>,
    fsavail: Option<Number>,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
    #[serde(default)]
    children: Vec<Node>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Flag {
    Bool(bool),
    Text(String),
}

impl Flag {
    fn get(flag: Option<&Flag>) -> bool {
        match flag {
            Some(Flag::Bool(value)) => *value,
            Some(Flag::Text(text)) => text == "1",
            None => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Number {
    Integer(u64),
    Text(String),
}

impl Number {
    fn get(number: Option<&Number>) -> Option<u64> {
        match number? {
            Number::Integer(value) => Some(*value),
            Number::Text(text) => text.parse().ok(),
        }
    }
}

pub fn parse(json: &str) -> Result<Vec<BlockDevice>, ParseError> {
    let output: Output = serde_json::from_str(json).map_err(|e| ParseError::new("lsblk", e))?;
    Ok(output.blockdevices.into_iter().map(convert).collect())
}

fn convert(node: Node) -> BlockDevice {
    let kind = match node.kind.as_str() {
        "disk" => DeviceKind::Disk,
        "part" => DeviceKind::Partition,
        level if node.kname.starts_with("md") && (level.starts_with("raid") || level == "linear") => {
            DeviceKind::Raid { level: level.to_owned() }
        }
        other => DeviceKind::Other(other.to_owned()),
    };
    let space = match (
        Number::get(node.fssize.as_ref()),
        Number::get(node.fsused.as_ref()),
        Number::get(node.fsavail.as_ref()),
    ) {
        (Some(size_bytes), Some(used_bytes), Some(available_bytes)) => {
            Some(SpaceUsage { size_bytes, used_bytes, available_bytes })
        }
        _ => None,
    };
    let filesystem = node.fstype.map(|kind| Filesystem {
        kind,
        label: non_empty(node.label),
        uuid: non_empty(node.uuid),
        space,
    });
    BlockDevice {
        name: node.kname,
        kind,
        size_bytes: Number::get(node.size.as_ref()).unwrap_or(0),
        // lsblk pads some values with spaces.
        model: non_empty(node.model),
        serial: non_empty(node.serial),
        wwn: non_empty(node.wwn),
        id_link: non_empty(node.id_link),
        transport: non_empty(node.tran),
        rotational: Flag::get(node.rota.as_ref()),
        removable: Flag::get(node.rm.as_ref()),
        read_only: Flag::get(node.ro.as_ref()),
        filesystem,
        mount_points: node.mountpoints.into_iter().flatten().collect(),
        children: node.children.into_iter().map(convert).collect(),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::domain::{DiskUsage, Transport, disks, volumes};

    use super::*;

    const VM: &str = include_str!("fixtures/lsblk-vm.json");

    #[test]
    fn reads_the_test_vm() {
        let devices = parse(VM).unwrap();
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["sda", "sdb", "sdc", "sdd", "vda", "vdb", "nvme0n1"]);

        let sdb = &devices[1];
        assert_eq!(sdb.serial.as_deref(), Some("ZDL-SATA-3"));
        assert_eq!(sdb.id_link.as_deref(), Some("ata-QEMU_HARDDISK_ZDL-SATA-3"));
        assert_eq!(sdb.model.as_deref(), Some("QEMU HARDDISK"));
        assert_eq!(sdb.size_bytes, 8_589_934_592);
        assert!(sdb.rotational);
        assert_eq!(sdb.filesystem.as_ref().unwrap().kind, "linux_raid_member");
        let md0 = &sdb.children[0];
        assert_eq!(md0.kind, DeviceKind::Raid { level: "raid1".into() });
        assert_eq!(md0.mount_points, ["/mnt/testvolume"]);
        let space = md0.filesystem.as_ref().unwrap().space.unwrap();
        assert_eq!(space.size_bytes, 8_580_497_408);
        assert!(space.used_bytes > 0 && space.available_bytes < space.size_bytes);
    }

    #[test]
    fn the_test_vm_has_the_expected_disks_and_one_volume() {
        let devices = parse(VM).unwrap();
        let usage: Vec<_> =
            disks(&devices, &[]).into_iter().map(|d| (d.name, d.transport, d.usage)).collect();
        assert_eq!(
            usage,
            [
                ("sda".into(), Transport::Sata, DiskUsage::Unused),
                ("sdb".into(), Transport::Sata, DiskUsage::ArrayMember { array: "md0".into() }),
                ("sdc".into(), Transport::Sata, DiskUsage::ArrayMember { array: "md0".into() }),
                ("sdd".into(), Transport::Sata, DiskUsage::Unused),
                ("vda".into(), Transport::Virtio, DiskUsage::System),
                // vdb, cloud-init's 370 KiB seed, is too small to be a disk.
                ("nvme0n1".into(), Transport::Nvme, DiskUsage::Unused),
            ]
        );

        let volumes = volumes(&devices, &[]);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].label.as_deref(), Some("testvolume"));
        assert_eq!(volumes[0].filesystem, "btrfs");
        assert_eq!(volumes[0].array.as_deref(), Some("md0"));
        assert_eq!(volumes[0].disks, ["sdb", "sdc"]);
    }

    #[test]
    fn accepts_the_older_text_format() {
        let json = r#"{"blockdevices": [{"kname": "sda", "type": "disk", "size": "1000",
            "rota": "1", "rm": "0", "ro": "0", "model": "  ", "mountpoints": [null]}]}"#;
        let devices = parse(json).unwrap();
        assert_eq!(devices[0].size_bytes, 1000);
        assert!(devices[0].rotational);
        assert_eq!(devices[0].model, None);
        assert!(devices[0].mount_points.is_empty());
    }

    #[test]
    fn refuses_other_output() {
        assert!(parse("lsblk: unknown column").is_err());
        assert!(parse(r#"{"devices": []}"#).is_err());
    }
}
