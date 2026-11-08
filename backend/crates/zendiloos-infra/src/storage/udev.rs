//! `udevadm info --export-db` of the block devices that are disks: where
//! each is plugged in, udev's `ID_PATH` (the name of its link in
//! `/dev/disk/by-path`). zendiloosd cannot look into `/dev` itself
//! (`PrivateDevices`), so the agent passes udev's database on.

use std::collections::HashMap;

/// Kernel name to port path, for each disk udev gave one.
pub fn port_paths(export: &str) -> HashMap<String, String> {
    let mut paths = HashMap::new();
    // One device per paragraph: `P:` its sysfs path, `E:` its properties.
    for device in export.split("\n\n") {
        let (mut name, mut path, mut disk) = (None, None, true);
        for line in device.lines() {
            let Some(property) = line.strip_prefix("E: ") else { continue };
            match property.split_once('=') {
                Some(("DEVNAME", value)) => name = value.strip_prefix("/dev/"),
                Some(("ID_PATH", value)) => path = Some(value),
                // Older udev may not filter: partitions have the disk's path.
                Some(("DEVTYPE", value)) => disk = value == "disk",
                _ => {}
            }
        }
        if let (Some(name), Some(path), true) = (name, path, disk)
            && !name.contains('/')
            && !path.is_empty()
        {
            paths.insert(name.to_owned(), path.to_owned());
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    const VM: &str = include_str!("fixtures/udevadm-vm.txt");

    #[test]
    fn reads_the_ports_of_the_test_vm_s_disks() {
        let paths = port_paths(VM);
        let mut listed: Vec<_> = paths.iter().map(|(name, path)| (name.as_str(), path.as_str())).collect();
        listed.sort_unstable();
        assert_eq!(
            listed,
            [
                ("nvme0n1", "pci-0000:00:04.0-nvme-1"),
                ("sda", "pci-0000:00:1f.2-ata-1.0"),
                ("sdb", "pci-0000:00:1f.2-ata-2.0"),
                ("sdc", "pci-0000:00:1f.2-ata-3.0"),
                ("sdd", "pci-0000:00:1f.2-ata-4.0"),
                ("vda", "pci-0000:00:02.0"),
                ("vdb", "pci-0000:00:03.0"),
            ]
        );
    }

    #[test]
    fn skips_partitions_devices_without_a_port_and_odd_names() {
        let export = "P: /devices/a/block/sda/sda1\nE: DEVNAME=/dev/sda1\nE: DEVTYPE=partition\nE: ID_PATH=pci-x\n\n\
                      P: /devices/virtual/block/loop0\nE: DEVNAME=/dev/loop0\nE: DEVTYPE=disk\n\n\
                      E: DEVNAME=/dev/mapper/x\nE: ID_PATH=pci-y\n\n\
                      E: DEVNAME=/dev/sdq\nE: ID_PATH=\n\n\
                      E: ID_PATH=pci-z\n";
        assert!(port_paths(export).is_empty());
        assert!(port_paths("").is_empty());
    }
}
