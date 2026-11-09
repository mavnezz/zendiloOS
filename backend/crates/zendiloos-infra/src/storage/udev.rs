//! `udevadm info --export-db` of the block devices that are disks: where
//! each is plugged in, udev's `ID_PATH` (the name of its link in
//! `/dev/disk/by-path`), and the kernel's sequence number of each
//! (`DISKSEQ`), which a disk gets anew whenever it appears. zendiloosd
//! cannot look into `/dev` itself (`PrivateDevices`), so the agent passes
//! udev's database on.

use std::collections::HashMap;

/// Kernel name to the value of `property`, for each disk udev gave one.
fn of_disks(export: &str, property: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    // One device per paragraph: `P:` its sysfs path, `E:` its properties.
    for device in export.split("\n\n") {
        let (mut name, mut found, mut disk) = (None, None, true);
        for line in device.lines() {
            let Some(line) = line.strip_prefix("E: ") else { continue };
            match line.split_once('=') {
                Some(("DEVNAME", value)) => name = value.strip_prefix("/dev/"),
                Some((key, value)) if key == property => found = Some(value),
                // Older udev may not filter: partitions have the disk's path.
                Some(("DEVTYPE", value)) => disk = value == "disk",
                _ => {}
            }
        }
        if let (Some(name), Some(value), true) = (name, found, disk)
            && !name.contains('/')
            && !value.is_empty()
        {
            values.insert(name.to_owned(), value.to_owned());
        }
    }
    values
}

/// Kernel name to port path, for each disk udev gave one.
pub fn port_paths(export: &str) -> HashMap<String, String> {
    of_disks(export, "ID_PATH")
}

/// Kernel name to the kernel's sequence number of the disk (`DISKSEQ`).
pub fn disk_sequences(export: &str) -> HashMap<String, u64> {
    of_disks(export, "DISKSEQ")
        .into_iter()
        .filter_map(|(name, sequence)| Some((name, sequence.parse().ok()?)))
        .collect()
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
    fn reads_the_sequence_numbers_of_the_test_vm_s_disks() {
        let sequences = disk_sequences(VM);
        assert_eq!(sequences.get("sdd"), Some(&6));
        assert_eq!(sequences.len(), 7, "{sequences:?}");
        let odd = "E: DEVNAME=/dev/sdq\nE: DEVTYPE=disk\nE: DISKSEQ=many\n\n\
                   E: DEVNAME=/dev/sdq1\nE: DEVTYPE=partition\nE: DISKSEQ=9\n";
        assert!(disk_sequences(odd).is_empty());
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
