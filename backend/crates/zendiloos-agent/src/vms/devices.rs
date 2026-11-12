//! The host's USB and PCI devices a VM can be given (docs/architecture.md,
//! 4.13), read from sysfs as the kernel writes it, and the checks the agent
//! makes itself before one goes to a VM: an IOMMU is on, a PCI device is
//! not one the host needs (a controller with a disk in use, the console's
//! graphics card, a network card the host's network is on, a bridge of the
//! bus), nor is any of its IOMMU group, and the group goes whole.
//!
//! USB devices go by vendor and product, PCI devices by their address;
//! libvirt binds a PCI device to `vfio-pci` while the VM runs and gives it
//! back after (`managed='yes'`), so nothing is bound at boot.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use zendiloos_agent_proto::{HostIommu, HostNeeds, HostPciDevice, HostUsbDevice, PciAddress, VmUsbDevice};
use zendiloos_exec::Program;

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// What of the PCI ID database is read at most; Debian's is some 1.5 MB.
const MAX_PCI_IDS: u64 = 8 * 1024 * 1024;
/// A sysfs attribute read at most.
const MAX_ATTRIBUTE: u64 = 4096;
/// The kernel's command line read at most.
const MAX_CMDLINE: u64 = 64 * 1024;
/// systemd-networkd's operational states of a link the host uses: an
/// address on it, or in a bridge or bond.
const IN_USE: [&str; 4] = ["routable", "degraded", "enslaved", "degraded-carrier"];
/// `O_EXCL` on a block device: refused while it is mounted, in a pool or
/// otherwise claimed.
const O_EXCL: i32 = 0o200;

/// Where the devices are looked at; other places only in tests.
#[derive(Debug, Clone)]
pub struct DevicePaths {
    /// `/sys/bus/usb/devices`.
    pub usb: PathBuf,
    /// `/sys/bus/pci/devices`.
    pub pci: PathBuf,
    /// `/sys/kernel/iommu_groups`.
    pub iommu_groups: PathBuf,
    /// `/proc/cmdline`.
    pub cmdline: PathBuf,
    /// The PCI ID database, `/usr/share/misc/pci.ids`.
    pub pci_ids: PathBuf,
}

impl Default for DevicePaths {
    fn default() -> Self {
        Self {
            usb: PathBuf::from("/sys/bus/usb/devices"),
            pci: PathBuf::from("/sys/bus/pci/devices"),
            iommu_groups: PathBuf::from("/sys/kernel/iommu_groups"),
            cmdline: PathBuf::from("/proc/cmdline"),
            pci_ids: PathBuf::from("/usr/share/misc/pci.ids"),
        }
    }
}

/// A sysfs attribute, trimmed; none if it is not there.
fn attribute(path: &Path) -> Option<String> {
    let mut text = String::new();
    fs::File::open(path).ok()?.take(MAX_ATTRIBUTE).read_to_string(&mut text).ok()?;
    Some(text.trim().to_owned())
}

/// A number sysfs writes in hex, with or without `0x`.
fn hex(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// The last part of what a link names: a driver's or a group's.
fn link_name(path: &Path) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    target.file_name().map(|name| name.to_string_lossy().into_owned())
}

/// Whether `name` is a USB device's entry in sysfs, not an interface
/// (`1-2:1.0`) or a root hub (`usb1`): `<bus>-<port>[.<port>…]`.
fn is_usb_device(name: &str) -> bool {
    let Some((bus, ports)) = name.split_once('-') else { return false };
    !bus.is_empty()
        && bus.bytes().all(|b| b.is_ascii_digit())
        && !ports.is_empty()
        && ports.split('.').all(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

/// The USB devices plugged in, hubs left out, by where they are plugged in.
pub(crate) fn usb_devices(paths: &DevicePaths) -> Vec<HostUsbDevice> {
    let Ok(entries) = fs::read_dir(&paths.usb) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_usb_device(&name) {
            continue;
        }
        let path = paths.usb.join(&name);
        let number = |file: &str| attribute(&path.join(file)).and_then(|text| hex(&text));
        let (Some(vendor), Some(product)) = (number("idVendor"), number("idProduct")) else { continue };
        // Class 09: a hub, which goes to no VM.
        if number("bDeviceClass") == Some(9) {
            continue;
        }
        let text = |file: &str| {
            attribute(&path.join(file))
                .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
                .map(|text| text.chars().take(128).collect())
        };
        found.push(HostUsbDevice {
            vendor: u16::try_from(vendor).unwrap_or(0),
            product: u16::try_from(product).unwrap_or(0),
            port: name,
            manufacturer: text("manufacturer"),
            name: text("product"),
        });
    }
    found.sort_by(|a, b| a.port.cmp(&b.port));
    found
}

/// How many devices of `device`'s vendor and product are plugged in.
pub(crate) fn usb_count(paths: &DevicePaths, device: VmUsbDevice) -> usize {
    usb_devices(paths)
        .iter()
        .filter(|found| found.vendor == device.vendor && found.product == device.product)
        .count()
}

/// A PCI function as sysfs has it, before the host's needs are known.
#[derive(Debug, Clone)]
struct Function {
    address: PciAddress,
    vendor: u16,
    device: u16,
    class: u32,
    group: Option<u32>,
    driver: Option<String>,
    boot_vga: bool,
    /// Where it is in sysfs, canonical: the block devices below it are its.
    real: PathBuf,
}

fn functions(paths: &DevicePaths) -> Vec<Function> {
    let Ok(entries) = fs::read_dir(&paths.pci) else { return Vec::new() };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(address) = PciAddress::parse(&name) else { continue };
        let path = paths.pci.join(&name);
        let number = |file: &str| attribute(&path.join(file)).and_then(|text| hex(&text));
        let (Some(vendor), Some(device), Some(class)) = (number("vendor"), number("device"), number("class"))
        else {
            continue;
        };
        found.push(Function {
            address,
            vendor: u16::try_from(vendor).unwrap_or(0),
            device: u16::try_from(device).unwrap_or(0),
            class,
            group: link_name(&path.join("iommu_group")).and_then(|group| group.parse().ok()),
            driver: link_name(&path.join("driver")),
            boot_vga: attribute(&path.join("boot_vga")).as_deref() == Some("1"),
            real: fs::canonicalize(&path).unwrap_or(path),
        });
    }
    found.sort_by(|a, b| a.address.cmp(&b.address));
    found
}

/// Names from the PCI ID database for the vendors and devices asked:
/// `vvvv  Vendor` and below it `\tdddd  Device`.
fn pci_names(paths: &DevicePaths, wanted: &BTreeSet<(u16, u16)>) -> BTreeMap<(u16, Option<u16>), String> {
    let mut text = String::new();
    let read = fs::File::open(&paths.pci_ids)
        .and_then(|file| file.take(MAX_PCI_IDS).read_to_string(&mut text).map(|_| ()));
    let mut names = BTreeMap::new();
    if read.is_err() {
        return names;
    }
    let vendors: BTreeSet<u16> = wanted.iter().map(|(vendor, _)| *vendor).collect();
    let mut current: Option<u16> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        // The classes come last, `C 01  Mass storage controller`.
        if line.starts_with('C') {
            break;
        }
        let split = |rest: &str| -> Option<(u16, String)> {
            let (id, name) = rest.split_once("  ")?;
            Some((u16::from_str_radix(id, 16).ok()?, name.trim().to_owned()))
        };
        if line.starts_with("\t\t") {
            // A subsystem's name.
        } else if let Some(rest) = line.strip_prefix('\t') {
            if let (Some(vendor), Some((device, name))) = (current, split(rest))
                && wanted.contains(&(vendor, device))
            {
                names.insert((vendor, Some(device)), name);
            }
        } else if let Some((vendor, name)) = split(line) {
            current = vendors.contains(&vendor).then_some(vendor);
            if current.is_some() {
                names.insert((vendor, None), name);
            }
        }
    }
    names
}

/// The disks below a PCI device (by its place in sysfs) that are in use:
/// in a pool, mounted, swap, holding another device, or claimed otherwise.
fn disks_in_use(host: &HostPaths, below: &Path) -> Option<String> {
    let entries = fs::read_dir(&host.sys_block).ok()?;
    let mountinfo = fs::read_to_string(&host.mountinfo).unwrap_or_default();
    let mounted: HashSet<&str> =
        mountinfo.lines().filter_map(|line| line.split_whitespace().nth(2)).collect();
    let swaps = fs::read_to_string(&host.swaps).unwrap_or_default();
    let swapped: HashSet<String> = swaps
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter_map(|device| fs::canonicalize(device).ok())
        .filter_map(|device| device.file_name().map(|name| name.to_string_lossy().into_owned()))
        .collect();
    let mut names: Vec<String> =
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let link = host.sys_block.join(&name);
        let Ok(real) = fs::canonicalize(&link) else { continue };
        if !real.starts_with(below) {
            continue;
        }
        let holders = fs::read_dir(link.join("holders")).is_ok_and(|mut entries| entries.next().is_some());
        let number = attribute(&link.join("dev")).unwrap_or_default();
        let busy = holders
            || (!number.is_empty() && mounted.contains(number.as_str()))
            || swapped.contains(&name)
            || fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_EXCL)
                .open(host.dev.join(&name))
                .is_err_and(|error| error.raw_os_error() == Some(rustix::io::Errno::BUSY.raw_os_error()));
        if busy {
            // The whole disk, for a partition.
            let disk = if link.join("partition").exists() {
                real.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned())
            } else {
                None
            };
            return Some(disk.unwrap_or(name));
        }
    }
    None
}

/// The host's network cards systemd-networkd says it uses, by their PCI
/// address; none known at all where it does not answer.
async fn network_in_use(busctl: &Program) -> Option<BTreeMap<String, String>> {
    let links = crate::network::networkd_links(busctl).await?;
    let mut used = BTreeMap::new();
    for (index, name) in links {
        let Some(description) = crate::network::networkd_description(busctl, index).await else { continue };
        let field = |key: &str| description.get(key).and_then(serde_json::Value::as_str).map(str::to_owned);
        let Some(address) = field("Path").and_then(|path| path.strip_prefix("pci-").map(str::to_owned))
        else {
            continue;
        };
        if field("OperationalState").is_some_and(|state| IN_USE.contains(&state.as_str())) {
            used.insert(address, name);
        }
    }
    Some(used)
}

/// Every PCI function, with why the host needs those it needs.
pub(crate) async fn pci_devices(
    paths: &DevicePaths,
    host: &HostPaths,
    busctl: &Program,
) -> Vec<HostPciDevice> {
    let functions = functions(paths);
    let wanted: BTreeSet<(u16, u16)> = functions.iter().map(|f| (f.vendor, f.device)).collect();
    let names = pci_names(paths, &wanted);
    let networks =
        if functions.iter().any(|f| f.class >> 16 == 0x02) { network_in_use(busctl).await } else { None };
    functions
        .into_iter()
        .map(|function| {
            let needed = if function.class >> 16 == 0x06 {
                Some(HostNeeds::Bridge)
            } else if function.boot_vga {
                Some(HostNeeds::Console)
            } else if let Some(disk) = disks_in_use(host, &function.real) {
                Some(HostNeeds::Disk { disk })
            } else if function.class >> 16 == 0x02 {
                match &networks {
                    Some(used) => used
                        .get(function.address.as_str())
                        .map(|interface| HostNeeds::Network { interface: Some(interface.clone()) }),
                    None => Some(HostNeeds::Network { interface: None }),
                }
            } else {
                None
            };
            HostPciDevice {
                vendor_name: names.get(&(function.vendor, None)).cloned(),
                device_name: names.get(&(function.vendor, Some(function.device))).cloned(),
                address: function.address,
                vendor: function.vendor,
                device: function.device,
                class: function.class,
                iommu_group: function.group,
                driver: function.driver,
                needed,
            }
        })
        .collect()
}

/// Whether the IOMMU is on, and what the kernel's command line and the
/// CPU say of it.
pub(crate) fn iommu(paths: &DevicePaths, cpuinfo: &str) -> HostIommu {
    let on = fs::read_dir(&paths.iommu_groups).is_ok_and(|mut entries| entries.next().is_some());
    let mut cmdline = String::new();
    if let Ok(file) = fs::File::open(&paths.cmdline) {
        let _ = file.take(MAX_CMDLINE).read_to_string(&mut cmdline);
    }
    let cmdline = cmdline
        .split_whitespace()
        .filter(|word| {
            ["intel_iommu=", "amd_iommu=", "iommu=", "iommu.passthrough="].iter().any(|p| word.starts_with(p))
        })
        .map(|word| word.chars().take(64).collect())
        .collect();
    let cpu_vendor = cpuinfo
        .lines()
        .find_map(|line| line.split_once(':').filter(|(key, _)| key.trim() == "vendor_id"))
        .map(|(_, value)| value.trim().to_owned())
        .filter(|vendor| vendor == "GenuineIntel" || vendor == "AuthenticAMD");
    HostIommu { on, cmdline, cpu_vendor }
}

/// Refused unless every PCI device asked for may go to a VM: the IOMMU on,
/// each there, neither it nor any device of its group one the host needs
/// (bridges of the bus aside, which stay), and every other device of its
/// group asked for too, as VFIO hands over groups whole.
pub(crate) fn check_pci(
    asked: &[PciAddress],
    devices: &[HostPciDevice],
    iommu_on: bool,
) -> Result<(), OperationError> {
    if asked.is_empty() {
        return Ok(());
    }
    let refused = |reason: String| Err(OperationError::Refused(reason));
    if !iommu_on {
        return refused("the IOMMU is off: no PCI device can be handed to a virtual machine".into());
    }
    for address in asked {
        let Some(device) = devices.iter().find(|device| device.address == *address) else {
            return refused(format!("there is no PCI device {}", address.as_str()));
        };
        let Some(group) = device.iommu_group else {
            return refused(format!("the PCI device {} is in no IOMMU group", address.as_str()));
        };
        for other in devices.iter().filter(|other| other.iommu_group == Some(group)) {
            match &other.needed {
                Some(HostNeeds::Bridge) if other.class >> 8 == 0x0604 && other.address != *address => {
                    continue;
                }
                Some(needs) => {
                    return refused(format!(
                        "the PCI device {} is in IOMMU group {group} with {}, which the host needs ({})",
                        address.as_str(),
                        other.address.as_str(),
                        needs_text(needs)
                    ));
                }
                None if !asked.contains(&other.address) => {
                    return refused(format!(
                        "the PCI device {} goes only with every device of its IOMMU group {group}: {} too",
                        address.as_str(),
                        other.address.as_str()
                    ));
                }
                None => {}
            }
        }
    }
    Ok(())
}

/// Why the host needs a device, in words.
pub(crate) fn needs_text(needs: &HostNeeds) -> String {
    match needs {
        HostNeeds::Disk { disk } => format!("the disk {disk} on it is in use"),
        HostNeeds::Console => "the console's graphics".into(),
        HostNeeds::Network { interface: Some(interface) } => format!("its network is on {interface}"),
        HostNeeds::Network { interface: None } => {
            "a network card, and systemd-networkd does not say which the host uses".into()
        }
        HostNeeds::Bridge => "a bridge of the PCI bus".into(),
    }
}

/// The USB devices and PCI addresses a domain's XML (as the agent wrote
/// it) hands over.
pub(crate) fn handed_over(xml: &str) -> (Vec<VmUsbDevice>, Vec<PciAddress>) {
    let mut usb = Vec::new();
    let mut pci = Vec::new();
    let mut vendor: Option<u16> = None;
    let value = |line: &str, attribute: &str| {
        let start = line.find(&format!("{attribute}='"))? + attribute.len() + 2;
        let end = line[start..].find('\'')? + start;
        Some(line[start..end].trim_start_matches("0x").to_owned())
    };
    let mut in_hostdev = false;
    for line in xml.lines().map(str::trim) {
        if line.starts_with("<hostdev ") {
            in_hostdev = true;
        } else if line.starts_with("</hostdev>") {
            in_hostdev = false;
        } else if in_hostdev && line.starts_with("<vendor ") {
            vendor = value(line, "id").and_then(|id| u16::from_str_radix(&id, 16).ok());
        } else if in_hostdev && line.starts_with("<product ") {
            if let (Some(vendor), Some(product)) =
                (vendor.take(), value(line, "id").and_then(|id| u16::from_str_radix(&id, 16).ok()))
            {
                usb.push(VmUsbDevice { vendor, product });
            }
        } else if in_hostdev && line.starts_with("<address ") {
            let parts = ["domain", "bus", "slot", "function"].map(|name| value(line, name));
            if let [Some(domain), Some(bus), Some(slot), Some(function)] = parts
                && let Ok(address) = PciAddress::parse(&format!("{domain}:{bus}:{slot}.{function}"))
            {
                pci.push(address);
            }
        }
    }
    (usb, pci)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A machine as sysfs shows it: an AHCI controller with the disk `sda`
    /// (mounted), an Ethernet card the network is on and one it is not on,
    /// a graphics card the firmware set up, a NVMe disk unused, the host
    /// bridge, a root port, an LPC bridge with an SMBus controller in its
    /// group; two USB devices, a hub and the root hub.
    fn machine() -> (tempfile::TempDir, DevicePaths, HostPaths) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let devices = root.join("sys/devices/pci0000:00");
        let bus = root.join("sys/bus/pci/devices");
        let groups = root.join("sys/kernel/iommu_groups");
        fs::create_dir_all(&bus).unwrap();
        fs::create_dir_all(root.join("sys/bus/pci/drivers")).unwrap();
        let pci = |address: &str,
                   parent: Option<&str>,
                   vendor: &str,
                   device: &str,
                   class: &str,
                   group: u32,
                   driver: Option<&str>| {
            let place = match parent {
                Some(parent) => devices.join(parent).join(address),
                None => devices.join(address),
            };
            fs::create_dir_all(&place).unwrap();
            for (file, value) in [("vendor", vendor), ("device", device), ("class", class)] {
                fs::write(place.join(file), format!("{value}\n")).unwrap();
            }
            let group_dir = groups.join(group.to_string());
            fs::create_dir_all(group_dir.join("devices")).unwrap();
            symlink(&group_dir, place.join("iommu_group")).unwrap();
            if let Some(driver) = driver {
                let driver_dir = root.join("sys/bus/pci/drivers").join(driver);
                fs::create_dir_all(&driver_dir).unwrap();
                symlink(&driver_dir, place.join("driver")).unwrap();
            }
            symlink(&place, bus.join(address)).unwrap();
            place
        };
        pci("0000:00:00.0", None, "0x8086", "0x29c0", "0x060000", 0, None);
        let port = pci("0000:00:1c.0", None, "0x8086", "0x2940", "0x060400", 1, Some("pcieport"));
        pci("0000:01:00.0", Some("0000:00:1c.0"), "0x8086", "0x10d3", "0x020000", 1, Some("e1000e"));
        let _ = port;
        pci("0000:00:02.0", None, "0x1234", "0x1111", "0x030000", 2, Some("bochs-drm"));
        fs::write(devices.join("0000:00:02.0/boot_vga"), "1\n").unwrap();
        let ahci = pci("0000:00:1f.2", None, "0x8086", "0x2922", "0x010601", 3, Some("ahci"));
        pci("0000:00:1f.0", None, "0x8086", "0x2918", "0x060100", 4, Some("lpc_ich"));
        pci("0000:00:1f.3", None, "0x8086", "0x2930", "0x0c0500", 4, Some("i801_smbus"));
        pci("0000:00:03.0", None, "0x1af4", "0x1041", "0x020000", 5, Some("virtio-pci"));
        let nvme = pci("0000:00:04.0", None, "0x1b36", "0x0010", "0x010802", 6, Some("nvme"));
        // Block devices: sda on the AHCI controller, nvme0n1 on the NVMe one.
        let block = root.join("sys/class/block");
        fs::create_dir_all(&block).unwrap();
        for (name, under, dev) in [
            ("sda", ahci.join("ata1/host0/target0:0:0/0:0:0:0/block/sda"), "8:0"),
            ("nvme0n1", nvme.join("nvme/nvme0/nvme0n1"), "259:0"),
        ] {
            fs::create_dir_all(under.join("holders")).unwrap();
            fs::write(under.join("dev"), format!("{dev}\n")).unwrap();
            symlink(&under, block.join(name)).unwrap();
        }
        let sda1 = ahci.join("ata1/host0/target0:0:0/0:0:0:0/block/sda/sda1");
        fs::create_dir_all(sda1.join("holders")).unwrap();
        fs::write(sda1.join("dev"), "8:1\n").unwrap();
        fs::write(sda1.join("partition"), "1\n").unwrap();
        symlink(&sda1, block.join("sda1")).unwrap();
        fs::create_dir_all(root.join("dev")).unwrap();
        for name in ["sda", "sda1", "nvme0n1"] {
            fs::write(root.join("dev").join(name), "").unwrap();
        }
        fs::write(root.join("mountinfo"), "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n").unwrap();
        fs::write(root.join("swaps"), "Filename\tType\tSize\tUsed\tPriority\n").unwrap();
        // USB: a root hub, a hub, a stick and a keyboard behind the hub.
        let usb = root.join("sys/bus/usb/devices");
        fs::create_dir_all(&usb).unwrap();
        let device = |name: &str, vendor: &str, product: &str, class: &str, strings: &[(&str, &str)]| {
            let place = usb.join(name);
            fs::create_dir_all(&place).unwrap();
            for (file, value) in [("idVendor", vendor), ("idProduct", product), ("bDeviceClass", class)] {
                fs::write(place.join(file), format!("{value}\n")).unwrap();
            }
            for (file, value) in strings {
                fs::write(place.join(file), format!("{value}\n")).unwrap();
            }
        };
        device("usb1", "1d6b", "0002", "09", &[("product", "xHCI Host Controller")]);
        device("1-1", "05e3", "0608", "09", &[("product", "USB2.0 Hub")]);
        device("1-1.2", "0781", "5583", "00", &[("manufacturer", "SanDisk"), ("product", "Ultra Fit")]);
        device("1-1.3", "046d", "c31c", "00", &[("manufacturer", "Logitech"), ("product", "USB Keyboard")]);
        fs::create_dir_all(usb.join("1-1.2:1.0")).unwrap();
        fs::write(root.join("cmdline"), "BOOT_IMAGE=/vmlinuz root=UUID=x ro quiet intel_iommu=on iommu=pt\n")
            .unwrap();
        fs::write(
            root.join("pci.ids"),
            "# comment\n8086  Intel Corporation\n\t10d3  82574L Gigabit Network Connection\n\t\t8086 a01f  Gigabit CT\n\
             \t2922  82801IR/IO/IH (ICH9R/DO/DH) 6 port SATA Controller [AHCI mode]\n1af4  Red Hat, Inc.\n\t1041  Virtio 1.0 network device\n\
             C 01  Mass storage controller\n\t06  SATA controller\n",
        )
        .unwrap();
        let paths = DevicePaths {
            usb,
            pci: bus,
            iommu_groups: groups,
            cmdline: root.join("cmdline"),
            pci_ids: root.join("pci.ids"),
        };
        let host = HostPaths {
            dev: root.join("dev"),
            sys_block: block,
            mountinfo: root.join("mountinfo"),
            swaps: root.join("swaps"),
            ..HostPaths::default()
        };
        (dir, paths, host)
    }

    /// systemd-networkd's busctl: two links, `enp1s0` on 0000:01:00.0
    /// without an address, `enp0s3` on 0000:00:03.0 carrying the NAS's.
    fn busctl(root: &Path) -> Program {
        let path = root.join("busctl");
        fs::write(
            &path,
            "#!/bin/sh\ncase \"$*\" in\n\
             *ListLinks*) echo '{\"type\":\"a(iso)\",\"data\":[[[1,\"lo\",\"/org/freedesktop/network1/link/_31\"],[2,\"enp1s0\",\"/org/freedesktop/network1/link/_32\"],[3,\"enp0s3\",\"/org/freedesktop/network1/link/_33\"]]]}' ;;\n\
             *'DescribeLink i 2'*) echo '{\"type\":\"s\",\"data\":[\"{\\\"Path\\\":\\\"pci-0000:01:00.0\\\",\\\"OperationalState\\\":\\\"no-carrier\\\"}\"]}' ;;\n\
             *'DescribeLink i 3'*) echo '{\"type\":\"s\",\"data\":[\"{\\\"Path\\\":\\\"pci-0000:00:03.0\\\",\\\"OperationalState\\\":\\\"routable\\\"}\"]}' ;;\n\
             *) echo '{\"type\":\"s\",\"data\":[\"{}\"]}' ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    #[test]
    fn usb_devices_are_read_by_where_they_are_plugged_in_hubs_left_out() {
        let (_dir, paths, _) = machine();
        let found = usb_devices(&paths);
        let ids: Vec<(String, String)> = found
            .iter()
            .map(|d| (d.port.clone(), VmUsbDevice { vendor: d.vendor, product: d.product }.id()))
            .collect();
        assert_eq!(
            ids,
            [("1-1.2".to_owned(), "0781:5583".to_owned()), ("1-1.3".to_owned(), "046d:c31c".to_owned())]
        );
        assert_eq!(found[0].manufacturer.as_deref(), Some("SanDisk"));
        assert_eq!(found[0].name.as_deref(), Some("Ultra Fit"));
        assert_eq!(usb_count(&paths, VmUsbDevice { vendor: 0x046d, product: 0xc31c }), 1);
        assert_eq!(usb_count(&paths, VmUsbDevice { vendor: 0x046d, product: 0xc52b }), 0);
        assert!(
            is_usb_device("3-10.4.1")
                && !is_usb_device("usb1")
                && !is_usb_device("1-1:1.0")
                && !is_usb_device("1-")
        );
    }

    #[tokio::test]
    async fn pci_devices_say_what_the_host_needs() {
        let (dir, paths, host) = machine();
        let devices = pci_devices(&paths, &host, &busctl(&fs::canonicalize(dir.path()).unwrap())).await;
        let needs =
            |address: &str| devices.iter().find(|d| d.address.as_str() == address).unwrap().needed.clone();
        assert_eq!(needs("0000:00:00.0"), Some(HostNeeds::Bridge));
        assert_eq!(needs("0000:00:1c.0"), Some(HostNeeds::Bridge));
        assert_eq!(needs("0000:00:02.0"), Some(HostNeeds::Console));
        assert_eq!(
            needs("0000:00:1f.2"),
            Some(HostNeeds::Disk { disk: "sda".into() }),
            "its partition is mounted"
        );
        assert_eq!(needs("0000:00:03.0"), Some(HostNeeds::Network { interface: Some("enp0s3".into()) }));
        assert_eq!(needs("0000:01:00.0"), None, "a card without the network");
        assert_eq!(needs("0000:00:04.0"), None, "a disk nothing uses");
        assert_eq!(needs("0000:00:1f.3"), None);
        let card = devices.iter().find(|d| d.address.as_str() == "0000:01:00.0").unwrap();
        assert_eq!(
            (card.vendor, card.device, card.class, card.iommu_group),
            (0x8086, 0x10d3, 0x020000, Some(1))
        );
        assert_eq!(card.driver.as_deref(), Some("e1000e"));
        assert_eq!(card.vendor_name.as_deref(), Some("Intel Corporation"));
        assert_eq!(card.device_name.as_deref(), Some("82574L Gigabit Network Connection"));
        let virtio = devices.iter().find(|d| d.address.as_str() == "0000:00:03.0").unwrap();
        assert_eq!(virtio.device_name.as_deref(), Some("Virtio 1.0 network device"));

        // Without systemd-networkd, every network card is the host's.
        let silent = fs::canonicalize(dir.path()).unwrap().join("silent");
        fs::write(&silent, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&silent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let devices = pci_devices(&paths, &host, &Program::new(&silent).unwrap()).await;
        let card = devices.iter().find(|d| d.address.as_str() == "0000:01:00.0").unwrap();
        assert_eq!(card.needed, Some(HostNeeds::Network { interface: None }));
    }

    #[tokio::test]
    async fn a_pci_device_goes_with_its_whole_group_and_never_one_the_host_needs() {
        let (dir, paths, host) = machine();
        let devices = pci_devices(&paths, &host, &busctl(&fs::canonicalize(dir.path()).unwrap())).await;
        let address = |text: &str| PciAddress::parse(text).unwrap();
        // The card shares its group with the root port, a bridge that stays.
        assert!(check_pci(&[address("0000:01:00.0")], &devices, true).is_ok());
        assert!(check_pci(&[], &devices, false).is_ok());
        let off = check_pci(&[address("0000:01:00.0")], &devices, false).unwrap_err().to_string();
        assert!(off.contains("IOMMU is off"), "{off}");
        let disk = check_pci(&[address("0000:00:1f.2")], &devices, true).unwrap_err().to_string();
        assert!(disk.contains("the disk sda on it is in use"), "{disk}");
        let vga = check_pci(&[address("0000:00:02.0")], &devices, true).unwrap_err().to_string();
        assert!(vga.contains("console"), "{vga}");
        // The SMBus controller's group holds the LPC bridge, which the host needs.
        let lpc = check_pci(&[address("0000:00:1f.3")], &devices, true).unwrap_err().to_string();
        assert!(lpc.contains("0000:00:1f.0, which the host needs"), "{lpc}");
        let port = check_pci(&[address("0000:00:1c.0")], &devices, true).unwrap_err().to_string();
        assert!(port.contains("bridge"), "{port}");
        let missing = check_pci(&[address("0000:09:00.0")], &devices, true).unwrap_err().to_string();
        assert!(missing.contains("there is no PCI device"), "{missing}");
    }

    #[test]
    fn whether_the_iommu_is_on_and_what_the_command_line_says() {
        let (_dir, paths, _) = machine();
        let iommu = iommu(&paths, "processor : 0\nvendor_id\t: GenuineIntel\n");
        assert!(iommu.on);
        assert_eq!(iommu.cmdline, ["intel_iommu=on", "iommu=pt"]);
        assert_eq!(iommu.cpu_vendor.as_deref(), Some("GenuineIntel"));
        let empty = tempfile::tempdir().unwrap();
        let off = DevicePaths {
            iommu_groups: empty.path().to_owned(),
            cmdline: empty.path().join("none"),
            ..paths
        };
        let iommu = super::iommu(&off, "vendor_id : AuthenticAMD\n");
        assert!(!iommu.on && iommu.cmdline.is_empty());
        assert_eq!(iommu.cpu_vendor.as_deref(), Some("AuthenticAMD"));
    }

    #[test]
    fn the_devices_a_domain_hands_over_are_read_from_its_xml() {
        let xml = "<domain>\n  <devices>\n    <hostdev mode='subsystem' type='usb' managed='yes'>\n      <source>\n        \
                   <vendor id='0x046d'/>\n        <product id='0xc31c'/>\n      </source>\n    </hostdev>\n    \
                   <hostdev mode='subsystem' type='pci' managed='yes'>\n      <source>\n        \
                   <address domain='0x0000' bus='0x01' slot='0x00' function='0x0'/>\n      </source>\n    </hostdev>\n    \
                   <controller type='usb' model='qemu-xhci'>\n      <address type='pci' bus='0x02'/>\n    </controller>\n  </devices>\n</domain>\n";
        let (usb, pci) = handed_over(xml);
        assert_eq!(usb, [VmUsbDevice { vendor: 0x046d, product: 0xc31c }]);
        assert_eq!(pci, [PciAddress::parse("0000:01:00.0").unwrap()]);
    }
}
