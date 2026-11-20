//! The host's USB and PCI devices a VM can be given (docs/architecture.md,
//! 4.13), read from sysfs as the kernel writes it, and the checks the agent
//! makes itself before one goes to a VM: an IOMMU is on, a device is not
//! one the host needs, and a PCI device's IOMMU group goes whole, none of
//! it needed either. The host needs a PCI device that is a bridge of the
//! bus, the console's graphics card, a controller with a disk in use below
//! it, a network card or controller the host's network is on, or a USB
//! controller with a USB device below it the host needs; and a USB device
//! with a disk in use below it, the host's network on it, or the UPS it
//! follows.
//!
//! A disk is in use when it or a partition of it holds another device, is
//! mounted or swap, or cannot be opened exclusively (`O_EXCL`) for any
//! reason: mounted, in a pool, claimed otherwise, or of a kind the agent's
//! sandbox may not open, which fails closed. Only a drive without a medium
//! holds nothing; the agent may open optical drives and SD/eMMC cards read
//! only for this (its unit's `DeviceAllow=block-sr r`, `block-mmc r`), so
//! an empty one is told apart from one in use. An NVMe namespace
//! with native multipath belongs to the subsystem, not to a controller: it
//! is the controllers' whose paths to it (`multipath/`) are below them.
//!
//! USB devices go by vendor and product, PCI devices by their address;
//! libvirt binds a PCI device to `vfio-pci` while the VM runs and gives it
//! back after (`managed='yes'`), so nothing is bound at boot. Which device
//! that was when the VM was given it is recorded ([`Identity`]): a PCI
//! device's vendor, device and subsystem, a USB device's serial number or,
//! without one, its port. A start is refused when the device there now is
//! another, as after the PCI bus was numbered anew.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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
/// NUT's `ups.conf` read at most.
const MAX_UPS_CONF: u64 = 256 * 1024;
/// systemd-networkd's operational states of a link the host uses: an
/// address on it, or in a bridge or bond.
const IN_USE: [&str; 4] = ["routable", "degraded", "enslaved", "degraded-carrier"];
/// `O_EXCL` on a block device: refused while it is mounted, in a pool or
/// otherwise claimed.
const O_EXCL: i32 = 0o200;
/// USB interface classes of network adapters: communications (CDC), CDC
/// data, wireless controllers (RNDIS).
const USB_NETWORK_CLASSES: [u32; 3] = [0x02, 0x0a, 0xe0];
/// Drivers of USB network adapters whose interfaces say only "vendor
/// specific" (`ff`): Realtek's and ASIX's chips, the most common ones.
const USB_NETWORK_DRIVERS: [&str; 5] = ["r8152", "ax88179_178a", "asix", "aqc111", "lan78xx"];

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
    /// Network UPS Tools' drivers, `/etc/nut/ups.conf`: the UPS on USB the
    /// host follows.
    pub ups_conf: PathBuf,
    /// udev's database, `/run/udev/data`: which USB device a network
    /// card is.
    pub udev: PathBuf,
}

impl Default for DevicePaths {
    fn default() -> Self {
        Self {
            usb: PathBuf::from("/sys/bus/usb/devices"),
            pci: PathBuf::from("/sys/bus/pci/devices"),
            iommu_groups: PathBuf::from("/sys/kernel/iommu_groups"),
            cmdline: PathBuf::from("/proc/cmdline"),
            pci_ids: PathBuf::from("/usr/share/misc/pci.ids"),
            ups_conf: PathBuf::from("/etc/nut/ups.conf"),
            udev: PathBuf::from("/run/udev/data"),
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

/// A USB device plugged in as sysfs has it, before the host's needs are
/// known.
#[derive(Debug, Clone)]
struct Plugged {
    device: VmUsbDevice,
    /// `1-2.3`.
    port: String,
    manufacturer: Option<String>,
    name: Option<String>,
    serial: Option<String>,
    /// Where it is in sysfs, canonical: the disks and network interfaces
    /// below it are its.
    real: PathBuf,
}

/// A string a USB device says of itself: printable, at most 128 characters.
fn usb_text(path: &Path, file: &str) -> Option<String> {
    attribute(&path.join(file))
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .map(|text| text.chars().take(128).collect())
}

/// The USB devices plugged in, hubs left out, by where they are plugged in.
fn plugged(paths: &DevicePaths) -> Vec<Plugged> {
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
        found.push(Plugged {
            device: VmUsbDevice {
                vendor: u16::try_from(vendor).unwrap_or(0),
                product: u16::try_from(product).unwrap_or(0),
            },
            manufacturer: usb_text(&path, "manufacturer"),
            name: usb_text(&path, "product"),
            serial: usb_text(&path, "serial"),
            real: fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
            port: name,
        });
    }
    found.sort_by(|a, b| a.port.cmp(&b.port));
    found
}

/// How many devices of `device`'s vendor and product are plugged in.
pub(crate) fn usb_count(paths: &DevicePaths, device: VmUsbDevice) -> usize {
    plugged(paths).iter().filter(|found| found.device == device).count()
}

/// What the host's own use of its devices is told by, looked up once.
struct HostUse {
    /// systemd-networkd's links the host uses; none where it does not
    /// answer.
    networks: Option<Vec<UsedLink>>,
    /// The UPSs Network UPS Tools drives over USB.
    ups: Vec<UpsOnUsb>,
}

impl HostUse {
    async fn look(paths: &DevicePaths, busctl: &Program) -> Self {
        Self { networks: network_in_use(busctl, &paths.udev).await, ups: ups_on_usb(&paths.ups_conf) }
    }
}

/// A link the host's network is on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UsedLink {
    /// udev's path of its device: `pci-0000:01:00.0`,
    /// `pci-0000:00:14.0-usb-0:2:1.0`.
    path: String,
    name: String,
    /// The vendor and product of its USB device, as udev records them: the
    /// path does not tell a controller's USB 2 bus from its USB 3 bus.
    usb: Option<(u16, u16)>,
}

/// A UPS in `ups.conf` on USB (`port = auto`), as its section names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UpsOnUsb {
    vendor: u16,
    product: Option<u16>,
    serial: Option<String>,
}

/// The UPSs on USB that `ups.conf` drives: each section with `port = auto`
/// and a `vendorid`. One without a vendor is not told apart from other
/// devices.
fn ups_on_usb(file: &Path) -> Vec<UpsOnUsb> {
    let mut text = String::new();
    if fs::File::open(file).and_then(|file| file.take(MAX_UPS_CONF).read_to_string(&mut text)).is_err() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut section: Option<BTreeMap<String, String>> = None;
    let mut close = |section: Option<BTreeMap<String, String>>| {
        let Some(values) = section else { return };
        let id = |key: &str| values.get(key).and_then(|value| u16::from_str_radix(value, 16).ok());
        if values.get("port").is_some_and(|port| port == "auto")
            && let Some(vendor) = id("vendorid")
        {
            found.push(UpsOnUsb { vendor, product: id("productid"), serial: values.get("serial").cloned() });
        }
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            close(section.take());
            section = Some(BTreeMap::new());
        } else if let (Some(values), Some((key, value))) = (section.as_mut(), line.split_once('=')) {
            let value = value.trim().trim_matches('"').to_owned();
            values.insert(key.trim().to_ascii_lowercase(), value);
        }
    }
    close(section);
    found
}

/// Whether the USB device is one of `ups`.
fn is_ups(device: &Plugged, ups: &[UpsOnUsb]) -> bool {
    ups.iter().any(|ups| {
        ups.vendor == device.device.vendor
            && ups.product.is_none_or(|product| product == device.device.product)
            && ups.serial.as_ref().is_none_or(|serial| device.serial.as_ref() == Some(serial))
    })
}

/// udev's path of a USB device's interfaces, up to the interface:
/// `pci-0000:00:14.0-usb-0:2:` for `1-2` on the controller 0000:00:14.0.
/// Without a PCI controller above it (a platform's), only `-usb-0:2:`.
fn usb_path_prefix(device: &Plugged) -> String {
    let ports = device.port.split_once('-').map_or("", |(_, ports)| ports);
    let controller = device
        .real
        .ancestors()
        .filter_map(Path::file_name)
        .find_map(|name| PciAddress::parse(&name.to_string_lossy()).ok());
    match controller {
        Some(address) => format!("pci-{}-usb-0:{ports}:", address.as_str()),
        None => format!("-usb-0:{ports}:"),
    }
}

/// Whether the USB device looks like a network adapter: an interface of a
/// network class, or one bound to a driver of such adapters.
fn looks_like_network(device: &Plugged) -> bool {
    let Ok(entries) = fs::read_dir(&device.real) else { return false };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&format!("{}:", device.port)) {
            return false;
        }
        let path = entry.path();
        let class = attribute(&path.join("bInterfaceClass")).and_then(|text| hex(&text));
        class.is_some_and(|class| USB_NETWORK_CLASSES.contains(&class))
            || link_name(&path.join("driver"))
                .is_some_and(|driver| USB_NETWORK_DRIVERS.contains(&driver.as_str()))
    })
}

/// Why the host needs a USB device, if it does: a disk below it in use,
/// the host's network on it (every network adapter where systemd-networkd
/// does not answer), or the UPS it follows.
fn usb_needs(device: &Plugged, host: &HostPaths, uses: &HostUse) -> Option<HostNeeds> {
    if let Some(disk) = disks_in_use(host, &device.real) {
        return Some(HostNeeds::Disk { disk });
    }
    match &uses.networks {
        Some(used) => {
            let prefix = usb_path_prefix(device);
            let on = |path: &str| {
                if prefix.starts_with('-') { path.contains(&prefix) } else { path.starts_with(&prefix) }
            };
            let this_one = |link: &UsedLink| {
                link.usb.is_none_or(|ids| ids == (device.device.vendor, device.device.product))
            };
            if let Some(link) = used.iter().find(|link| on(&link.path) && this_one(link)) {
                return Some(HostNeeds::Network { interface: Some(link.name.clone()) });
            }
        }
        None if looks_like_network(device) => return Some(HostNeeds::Network { interface: None }),
        None => {}
    }
    is_ups(device, &uses.ups).then_some(HostNeeds::Ups)
}

fn host_usb(device: &Plugged, needed: Option<HostNeeds>) -> HostUsbDevice {
    HostUsbDevice {
        vendor: device.device.vendor,
        product: device.device.product,
        port: device.port.clone(),
        manufacturer: device.manufacturer.clone(),
        name: device.name.clone(),
        needed,
    }
}

/// The USB devices plugged in, hubs left out, by where they are plugged
/// in, with why the host needs those it needs.
pub(crate) async fn usb_devices(
    paths: &DevicePaths,
    host: &HostPaths,
    busctl: &Program,
) -> Vec<HostUsbDevice> {
    let uses = HostUse::look(paths, busctl).await;
    plugged(paths).iter().map(|device| host_usb(device, usb_needs(device, host, &uses))).collect()
}

/// Refused unless no USB device asked for that is plugged in is one the
/// host needs. One that is not plugged in is checked when it is.
pub(crate) fn check_usb(asked: &[VmUsbDevice], devices: &[HostUsbDevice]) -> Result<(), OperationError> {
    for found in devices {
        let device = VmUsbDevice { vendor: found.vendor, product: found.product };
        if let (true, Some(needs)) = (asked.contains(&device), &found.needed) {
            return Err(OperationError::Refused(format!(
                "the USB device {} on {} is one the host needs ({}): it goes to no virtual machine",
                device.id(),
                found.port,
                needs_text(needs)
            )));
        }
    }
    Ok(())
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

/// The places in sysfs a block device is below, canonical: its own, and
/// for an NVMe namespace with native multipath (below the subsystem) those
/// of its paths through each controller (`multipath/`), also for a
/// partition of one.
fn block_places(link: &Path, real: &Path) -> Vec<PathBuf> {
    let mut places = vec![real.to_owned()];
    let disk = if link.join("partition").exists() { real.parent().unwrap_or(real) } else { real };
    if let Ok(paths) = fs::read_dir(disk.join("multipath")) {
        places.extend(paths.flatten().filter_map(|path| fs::canonicalize(path.path()).ok()));
    }
    places
}

/// Whether the block device cannot be opened exclusively: claimed (in a
/// pool, mounted, held), or refused for any other reason, which counts as
/// claimed too: the agent's sandbox may not open every kind of disk. A
/// drive without a medium holds nothing.
fn claimed(device: &Path) -> bool {
    match fs::OpenOptions::new().read(true).custom_flags(O_EXCL).open(device) {
        Ok(_) => false,
        Err(error) => error.raw_os_error() != Some(rustix::io::Errno::NOMEDIUM.raw_os_error()),
    }
}

/// The disks below a device (by its place in sysfs) that are in use: in a
/// pool, mounted, swap, holding another device, claimed otherwise, or not
/// to be told; the whole disk is named for a partition.
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
        // A path of a namespace through one controller: no device of its
        // own, the namespace stands for it (below).
        if attribute(&link.join("hidden")).as_deref() == Some("1") {
            continue;
        }
        if !block_places(&link, &real).iter().any(|place| place.starts_with(below)) {
            continue;
        }
        let holders = fs::read_dir(link.join("holders")).is_ok_and(|mut entries| entries.next().is_some());
        let number = attribute(&link.join("dev")).unwrap_or_default();
        let busy = holders
            || (!number.is_empty() && mounted.contains(number.as_str()))
            || swapped.contains(&name)
            || claimed(&host.dev.join(&name));
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

/// The host's links systemd-networkd says it uses, by udev's path of their
/// device (its `Path`), with their names and, for one on USB, its device's
/// vendor and product from udev's database (`n<index>`); none known at all
/// where it does not answer.
async fn network_in_use(busctl: &Program, udev: &Path) -> Option<Vec<UsedLink>> {
    let links = crate::network::networkd_links(busctl).await?;
    let mut used = Vec::new();
    for (index, name) in links {
        let Some(description) = crate::network::networkd_description(busctl, index).await else { continue };
        let field = |key: &str| description.get(key).and_then(serde_json::Value::as_str).map(str::to_owned);
        let Some(path) = field("Path") else { continue };
        if field("OperationalState").is_some_and(|state| IN_USE.contains(&state.as_str())) {
            let usb = if path.contains("-usb-") { usb_ids(&udev.join(format!("n{index}"))) } else { None };
            used.push(UsedLink { path, name, usb });
        }
    }
    Some(used)
}

/// `ID_VENDOR_ID` and `ID_MODEL_ID` of udev's record of a device.
fn usb_ids(record: &Path) -> Option<(u16, u16)> {
    let mut text = String::new();
    fs::File::open(record).ok()?.take(MAX_ATTRIBUTE * 16).read_to_string(&mut text).ok()?;
    let id = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("E:{key}=")))
            .and_then(|value| u16::from_str_radix(value.trim(), 16).ok())
    };
    Some((id("ID_VENDOR_ID")?, id("ID_MODEL_ID")?))
}

/// The link of the host's network on the PCI function, or below it (a
/// USB adapter on a controller: `pci-0000:00:14.0-usb-0:2:1.0`).
fn network_on(address: &PciAddress, used: &[UsedLink]) -> Option<String> {
    let own = format!("pci-{}", address.as_str());
    used.iter()
        .find(|link| link.path == own || link.path.starts_with(&format!("{own}-")))
        .map(|link| link.name.clone())
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
    let uses = HostUse::look(paths, busctl).await;
    // The USB devices the host needs keep their controller with it.
    let needed_usb: Vec<(PathBuf, HostNeeds)> = plugged(paths)
        .iter()
        .filter_map(|device| usb_needs(device, host, &uses).map(|needs| (device.real.clone(), needs)))
        .collect();
    functions
        .into_iter()
        .map(|function| {
            let network = uses.networks.as_ref().map(|used| network_on(&function.address, used));
            let needed = if function.class >> 16 == 0x06 {
                Some(HostNeeds::Bridge)
            } else if function.boot_vga {
                Some(HostNeeds::Console)
            } else if let Some(disk) = disks_in_use(host, &function.real) {
                Some(HostNeeds::Disk { disk })
            } else if let Some(Some(interface)) = &network {
                Some(HostNeeds::Network { interface: Some(interface.clone()) })
            } else if function.class >> 16 == 0x02 && network.is_none() {
                Some(HostNeeds::Network { interface: None })
            } else {
                needed_usb
                    .iter()
                    .find(|(real, _)| real.starts_with(&function.real))
                    .map(|(_, needs)| needs.clone())
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
        HostNeeds::Ups => "the UPS the NAS follows".into(),
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

/// What tells the host's devices a VM is given from others: recorded when
/// the VM is given them, compared before each start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    /// PCI devices by address.
    #[serde(default)]
    pub pci: BTreeMap<String, PciIdentity>,
    /// USB devices by vendor and product (`046d:c31c`).
    #[serde(default)]
    pub usb: BTreeMap<String, UsbIdentity>,
}

/// What a PCI function at an address is: another one there (the bus
/// numbered anew, a card moved) has other IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PciIdentity {
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: Option<u16>,
    pub subsystem_device: Option<u16>,
    pub class: u32,
}

impl PciIdentity {
    fn text(&self) -> String {
        let subsystem = match (self.subsystem_vendor, self.subsystem_device) {
            (Some(vendor), Some(device)) => format!(" {vendor:04x}:{device:04x}"),
            _ => String::new(),
        };
        format!("{:04x}:{:04x}{subsystem}, class {:06x}", self.vendor, self.device, self.class)
    }
}

/// What a USB device of a vendor and product is: its serial number, and
/// where it was plugged in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsbIdentity {
    pub serial: Option<String>,
    pub port: String,
    /// A disk or network adapter: without a serial number, its port tells
    /// it from another of the same vendor and product.
    #[serde(default)]
    pub by_port: bool,
}

/// The PCI function at `address`, as it is now.
fn pci_identity(paths: &DevicePaths, address: &PciAddress) -> Option<PciIdentity> {
    let path = paths.pci.join(address.as_str());
    let number = |file: &str| attribute(&path.join(file)).and_then(|text| hex(&text));
    let short = |file: &str| number(file).and_then(|value| u16::try_from(value).ok());
    Some(PciIdentity {
        vendor: short("vendor")?,
        device: short("device")?,
        subsystem_vendor: short("subsystem_vendor"),
        subsystem_device: short("subsystem_device"),
        class: number("class")?,
    })
}

/// Whether an interface of the USB device has the class `class`.
fn has_interface_class(device: &Plugged, class: u32) -> bool {
    let Ok(entries) = fs::read_dir(&device.real) else { return false };
    entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().starts_with(&format!("{}:", device.port))
            && attribute(&entry.path().join("bInterfaceClass")).and_then(|text| hex(&text)) == Some(class)
    })
}

/// The one USB device of `device`'s vendor and product plugged in, as it
/// is now; none unless exactly one is.
fn usb_identity(found: &[Plugged], device: VmUsbDevice) -> Option<UsbIdentity> {
    let mut same = found.iter().filter(|found| found.device == device);
    let (Some(one), None) = (same.next(), same.next()) else { return None };
    Some(UsbIdentity {
        serial: one.serial.clone(),
        port: one.port.clone(),
        by_port: has_interface_class(one, 0x08) || looks_like_network(one),
    })
}

/// What the devices a VM is given are: what `before` recorded of those it
/// had, and what those new to it are now. A device stays what it was when
/// the VM was given it, whatever sits at its place since (a change of the
/// VM's memory never takes on a device the bus numbered anew); taken away
/// and given again, it is the one there then.
pub(crate) fn identify(
    paths: &DevicePaths,
    usb: &[VmUsbDevice],
    pci: &[PciAddress],
    before: &Identity,
) -> Identity {
    let found = plugged(paths);
    let mut identity = Identity::default();
    for address in pci {
        let key = address.as_str().to_owned();
        if let Some(was) = before.pci.get(&key).cloned().or_else(|| pci_identity(paths, address)) {
            identity.pci.insert(key, was);
        }
    }
    for device in usb {
        let key = device.id();
        if let Some(was) = before.usb.get(&key).cloned().or_else(|| usb_identity(&found, *device)) {
            identity.usb.insert(key, was);
        }
    }
    identity
}

/// Whether a block device (by its place in sysfs) is below one of the
/// devices a VM is given: a USB device of their vendor and product, told
/// apart by the serial number or port `recorded` holds of it, or a PCI
/// device at their address.
pub(crate) fn below_given(
    paths: &DevicePaths,
    usb: &[VmUsbDevice],
    pci: &[PciAddress],
    recorded: &Identity,
    below: &Path,
) -> bool {
    let found = plugged(paths);
    let on_usb = usb.iter().any(|device| {
        let record = recorded.usb.get(&device.id());
        found.iter().filter(|one| one.device == *device).any(|one| {
            let the_one = match record {
                Some(UsbIdentity { serial: Some(serial), .. }) => one.serial.as_ref() == Some(serial),
                Some(UsbIdentity { by_port: true, port, .. }) => one.port == *port,
                _ => true,
            };
            the_one && below.starts_with(&one.real)
        })
    });
    on_usb
        || pci.iter().any(|address| {
            fs::canonicalize(paths.pci.join(address.as_str())).is_ok_and(|real| below.starts_with(real))
        })
}

/// Refused unless each device the VM is given that `recorded` knows is
/// the one recorded: a PCI function with the same IDs at its address, a
/// USB device with the same serial number, or, a disk or network adapter
/// without one, on the same port. Devices not recorded pass.
pub(crate) fn check_identity(
    paths: &DevicePaths,
    usb: &[VmUsbDevice],
    pci: &[PciAddress],
    recorded: &Identity,
) -> Result<(), OperationError> {
    let refused = |reason: String| Err(OperationError::Refused(reason));
    for address in pci {
        let (Some(was), Some(now)) = (recorded.pci.get(address.as_str()), pci_identity(paths, address))
        else {
            continue;
        };
        if *was != now {
            return refused(format!(
                "the PCI device at {} is not the one the virtual machine was given ({}; now {}): the PCI bus may \
                 have been numbered anew. Give the virtual machine its PCI devices again",
                address.as_str(),
                was.text(),
                now.text()
            ));
        }
    }
    let found = plugged(paths);
    for device in usb {
        let (Some(was), Some(now)) = (recorded.usb.get(&device.id()), usb_identity(&found, *device)) else {
            continue;
        };
        match &was.serial {
            Some(serial) if now.serial.as_ref() != Some(serial) => {
                return refused(format!(
                    "the USB device {} plugged in is not the one the virtual machine was given (serial number \
                     {serial}, this one {}): give the virtual machine its USB devices again to take this one",
                    device.id(),
                    now.serial.as_deref().unwrap_or("none")
                ));
            }
            None if was.by_port && now.port != was.port => {
                return refused(format!(
                    "the USB device {} is plugged in on {}, and the one the virtual machine was given was on {} \
                     with no serial number to tell them apart: plug it in there, or give the virtual machine its \
                     USB devices again",
                    device.id(),
                    now.port,
                    was.port
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A machine as sysfs shows it: an AHCI controller with the disk `sda`
    /// (mounted), an Ethernet card the network is on and one it is not on,
    /// a graphics card the firmware set up, a NVMe disk unused, the host
    /// bridge, a root port, an LPC bridge with an SMBus controller in its
    /// group; an xHCI controller with a hub, a stick whose partition is
    /// mounted, a keyboard, a network adapter the network is on and a UPS
    /// NUT drives.
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
        pci("0000:00:1c.0", None, "0x8086", "0x2940", "0x060400", 1, Some("pcieport"));
        let card =
            pci("0000:01:00.0", Some("0000:00:1c.0"), "0x8086", "0x10d3", "0x020000", 1, Some("e1000e"));
        fs::write(card.join("subsystem_vendor"), "0x8086\n").unwrap();
        fs::write(card.join("subsystem_device"), "0xa01f\n").unwrap();
        pci("0000:00:02.0", None, "0x1234", "0x1111", "0x030000", 2, Some("bochs-drm"));
        fs::write(devices.join("0000:00:02.0/boot_vga"), "1\n").unwrap();
        let ahci = pci("0000:00:1f.2", None, "0x8086", "0x2922", "0x010601", 3, Some("ahci"));
        pci("0000:00:1f.0", None, "0x8086", "0x2918", "0x060100", 4, Some("lpc_ich"));
        pci("0000:00:1f.3", None, "0x8086", "0x2930", "0x0c0500", 4, Some("i801_smbus"));
        pci("0000:00:03.0", None, "0x1af4", "0x1041", "0x020000", 5, Some("virtio-pci"));
        let nvme = pci("0000:00:04.0", None, "0x1b36", "0x0010", "0x010802", 6, Some("nvme"));
        let xhci = pci("0000:00:14.0", None, "0x8086", "0xa36d", "0x0c0330", 7, Some("xhci_hcd"));
        // Block devices: sda on the AHCI controller, nvme0n1 on the NVMe
        // one, sdb on the stick.
        let block = root.join("sys/class/block");
        fs::create_dir_all(&block).unwrap();
        let usb_place = xhci.join("usb1/1-1/1-1.2");
        for (name, under, dev) in [
            ("sda", ahci.join("ata1/host0/target0:0:0/0:0:0:0/block/sda"), "8:0"),
            ("nvme0n1", nvme.join("nvme/nvme0/nvme0n1"), "259:0"),
            ("sdb", usb_place.join("1-1.2:1.0/host2/target2:0:0/2:0:0:0/block/sdb"), "8:16"),
        ] {
            fs::create_dir_all(under.join("holders")).unwrap();
            fs::write(under.join("dev"), format!("{dev}\n")).unwrap();
            symlink(&under, block.join(name)).unwrap();
        }
        for (disk, partition, dev) in [
            (ahci.join("ata1/host0/target0:0:0/0:0:0:0/block/sda"), "sda1", "8:1"),
            (usb_place.join("1-1.2:1.0/host2/target2:0:0/2:0:0:0/block/sdb"), "sdb1", "8:17"),
        ] {
            let place = disk.join(partition);
            fs::create_dir_all(place.join("holders")).unwrap();
            fs::write(place.join("dev"), format!("{dev}\n")).unwrap();
            fs::write(place.join("partition"), "1\n").unwrap();
            symlink(&place, block.join(partition)).unwrap();
        }
        fs::create_dir_all(root.join("dev")).unwrap();
        for name in ["sda", "sda1", "nvme0n1", "sdb", "sdb1"] {
            fs::write(root.join("dev").join(name), "").unwrap();
        }
        fs::write(
            root.join("mountinfo"),
            "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
             90 22 8:17 / /run/zendiloos-removable/stick rw shared:9 - exfat /dev/sdb1 rw\n",
        )
        .unwrap();
        fs::write(root.join("swaps"), "Filename\tType\tSize\tUsed\tPriority\n").unwrap();
        // USB below the xHCI controller: the root hub, a hub, and behind it
        // a stick, a keyboard, a network adapter and a UPS.
        let usb = root.join("sys/bus/usb/devices");
        fs::create_dir_all(&usb).unwrap();
        let device = |path: &str, vendor: &str, product: &str, class: &str, strings: &[(&str, &str)]| {
            let place = xhci.join(path);
            let name = place.file_name().unwrap().to_string_lossy().into_owned();
            fs::create_dir_all(&place).unwrap();
            for (file, value) in [("idVendor", vendor), ("idProduct", product), ("bDeviceClass", class)] {
                fs::write(place.join(file), format!("{value}\n")).unwrap();
            }
            for (file, value) in strings {
                fs::write(place.join(file), format!("{value}\n")).unwrap();
            }
            symlink(&place, usb.join(&name)).unwrap();
            place
        };
        let interface = |device: &Path, class: &str, driver: Option<&str>| {
            let name = format!("{}:1.0", device.file_name().unwrap().to_string_lossy());
            let place = device.join(&name);
            fs::create_dir_all(&place).unwrap();
            fs::write(place.join("bInterfaceClass"), format!("{class}\n")).unwrap();
            if let Some(driver) = driver {
                let driver_dir = root.join("sys/bus/usb/drivers").join(driver);
                fs::create_dir_all(&driver_dir).unwrap();
                symlink(&driver_dir, place.join("driver")).unwrap();
            }
            symlink(&place, usb.join(name)).unwrap();
        };
        device("usb1", "1d6b", "0002", "09", &[("product", "xHCI Host Controller")]);
        device("usb1/1-1", "05e3", "0608", "09", &[("product", "USB2.0 Hub")]);
        let stick = device(
            "usb1/1-1/1-1.2",
            "0781",
            "5583",
            "00",
            &[("manufacturer", "SanDisk"), ("product", "Ultra Fit"), ("serial", "4C530001")],
        );
        interface(&stick, "08", Some("usb-storage"));
        let keyboard = device(
            "usb1/1-1/1-1.3",
            "046d",
            "c31c",
            "00",
            &[("manufacturer", "Logitech"), ("product", "USB Keyboard")],
        );
        interface(&keyboard, "03", Some("usbhid"));
        let adapter = device("usb1/1-1/1-1.4", "0bda", "8153", "00", &[("product", "USB 10/100/1000 LAN")]);
        interface(&adapter, "ff", Some("r8152"));
        let ups = device(
            "usb1/1-1/1-1.5",
            "0463",
            "ffff",
            "00",
            &[("manufacturer", "EATON"), ("serial", "000000000")],
        );
        interface(&ups, "03", Some("usbhid"));
        // The controller's USB 3 bus, whose ports udev's path names as the
        // USB 2 bus's: a camera on the port the network adapter has there.
        device("usb2", "1d6b", "0003", "09", &[("product", "xHCI Host Controller")]);
        device("usb2/2-1", "05e3", "0626", "09", &[("product", "USB3.1 Hub")]);
        let camera = device("usb2/2-1/2-1.4", "046d", "0825", "ef", &[("product", "Webcam C270")]);
        interface(&camera, "0e", Some("uvcvideo"));
        // udev's record of the adapter's network card (link 4).
        fs::create_dir_all(root.join("udev")).unwrap();
        fs::write(root.join("udev/n4"), "I:12345\nE:ID_BUS=usb\nE:ID_VENDOR_ID=0bda\nE:ID_MODEL_ID=8153\n")
            .unwrap();
        fs::write(root.join("cmdline"), "BOOT_IMAGE=/vmlinuz root=UUID=x ro quiet intel_iommu=on iommu=pt\n")
            .unwrap();
        fs::write(
            root.join("pci.ids"),
            "# comment\n8086  Intel Corporation\n\t10d3  82574L Gigabit Network Connection\n\t\t8086 a01f  Gigabit CT\n\
             \t2922  82801IR/IO/IH (ICH9R/DO/DH) 6 port SATA Controller [AHCI mode]\n1af4  Red Hat, Inc.\n\t1041  Virtio 1.0 network device\n\
             C 01  Mass storage controller\n\t06  SATA controller\n",
        )
        .unwrap();
        fs::write(
            root.join("ups.conf"),
            "# Written by zendiloOS\nmaxretry = 3\n\n[ups]\n    driver = usbhid-ups\n    port = auto\n    \
             vendorid = 0463\n    productid = ffff\n",
        )
        .unwrap();
        let paths = DevicePaths {
            usb,
            pci: bus,
            iommu_groups: groups,
            cmdline: root.join("cmdline"),
            pci_ids: root.join("pci.ids"),
            ups_conf: root.join("ups.conf"),
            udev: root.join("udev"),
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

    /// systemd-networkd's busctl: `enp1s0` on 0000:01:00.0 without an
    /// address, `enp0s3` on 0000:00:03.0 carrying the NAS's, and `enx` on
    /// the USB adapter 1-1.4 behind the xHCI controller, a bond's member.
    fn busctl(root: &Path) -> Program {
        let path = root.join("busctl");
        let describe = |index: u8, path: &str, state: &str| {
            format!(
                "*'DescribeLink i {index}'*) echo '{{\"type\":\"s\",\"data\":[\"{{\\\"Path\\\":\\\"{path}\\\",\\\"OperationalState\\\":\\\"{state}\\\"}}\"]}}' ;;\n"
            )
        };
        fs::write(
            &path,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n\
                 *ListLinks*) echo '{{\"type\":\"a(iso)\",\"data\":[[[1,\"lo\",\"/org/freedesktop/network1/link/_31\"],[2,\"enp1s0\",\"/org/freedesktop/network1/link/_32\"],[3,\"enp0s3\",\"/org/freedesktop/network1/link/_33\"],[4,\"enx\",\"/org/freedesktop/network1/link/_34\"]]]}}' ;;\n\
                 {}{}{}\
                 *) echo '{{\"type\":\"s\",\"data\":[\"{{}}\"]}}' ;;\nesac\n",
                describe(2, "pci-0000:01:00.0", "no-carrier"),
                describe(3, "pci-0000:00:03.0", "routable"),
                describe(4, "pci-0000:00:14.0-usb-0:1.4:1.0", "enslaved"),
            ),
        )
        .unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    /// A busctl for a host without systemd-networkd.
    fn silent(root: &Path) -> Program {
        let path = root.join("silent");
        fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    fn root_of(dir: &tempfile::TempDir) -> PathBuf {
        fs::canonicalize(dir.path()).unwrap()
    }

    fn usb(vendor: u16, product: u16) -> VmUsbDevice {
        VmUsbDevice { vendor, product }
    }

    #[tokio::test]
    async fn usb_devices_are_read_by_where_they_are_plugged_in_hubs_left_out() {
        let (dir, paths, host) = machine();
        let found = usb_devices(&paths, &host, &busctl(&root_of(&dir))).await;
        let ids: Vec<(String, String)> =
            found.iter().map(|d| (d.port.clone(), usb(d.vendor, d.product).id())).collect();
        assert_eq!(
            ids,
            [
                ("1-1.2".to_owned(), "0781:5583".to_owned()),
                ("1-1.3".to_owned(), "046d:c31c".to_owned()),
                ("1-1.4".to_owned(), "0bda:8153".to_owned()),
                ("1-1.5".to_owned(), "0463:ffff".to_owned()),
                ("2-1.4".to_owned(), "046d:0825".to_owned()),
            ]
        );
        assert_eq!(found[0].manufacturer.as_deref(), Some("SanDisk"));
        assert_eq!(found[0].name.as_deref(), Some("Ultra Fit"));
        assert_eq!(usb_count(&paths, usb(0x046d, 0xc31c)), 1);
        assert_eq!(usb_count(&paths, usb(0x046d, 0xc52b)), 0);
        assert!(
            is_usb_device("3-10.4.1")
                && !is_usb_device("usb1")
                && !is_usb_device("1-1:1.0")
                && !is_usb_device("1-")
        );
    }

    #[tokio::test]
    async fn usb_devices_the_host_needs_go_to_no_vm() {
        let (dir, paths, host) = machine();
        let root = root_of(&dir);
        let found = usb_devices(&paths, &host, &busctl(&root)).await;
        let needs = |port: &str| found.iter().find(|d| d.port == port).unwrap().needed.clone();
        assert_eq!(needs("1-1.2"), Some(HostNeeds::Disk { disk: "sdb".into() }), "its partition is mounted");
        assert_eq!(needs("1-1.3"), None, "a keyboard");
        assert_eq!(needs("1-1.4"), Some(HostNeeds::Network { interface: Some("enx".into()) }));
        assert_eq!(needs("1-1.5"), Some(HostNeeds::Ups));
        assert_eq!(needs("2-1.4"), None, "the adapter's port on the other bus, another device");
        let stick = check_usb(&[usb(0x0781, 0x5583)], &found).unwrap_err().to_string();
        assert!(stick.contains("0781:5583 on 1-1.2 is one the host needs (the disk sdb on it is in use)"));
        let ups = check_usb(&[usb(0x046d, 0xc31c), usb(0x0463, 0xffff)], &found).unwrap_err().to_string();
        assert!(ups.contains("the UPS the NAS follows"));
        assert!(check_usb(&[usb(0x046d, 0xc31c), usb(0x1234, 0x5678)], &found).is_ok());

        // Without systemd-networkd an adapter is the host's by its driver;
        // the stick, ejected, and the keyboard are free.
        fs::write(root.join("mountinfo"), "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n").unwrap();
        let found = usb_devices(&paths, &host, &silent(&root)).await;
        let needs = |port: &str| found.iter().find(|d| d.port == port).unwrap().needed.clone();
        assert_eq!(needs("1-1.2"), None);
        assert_eq!(needs("1-1.4"), Some(HostNeeds::Network { interface: None }));
        assert_eq!(needs("1-1.5"), Some(HostNeeds::Ups));
        // A UPS of another serial number, or no UPS on USB.
        fs::write(
            &paths.ups_conf,
            "[ups]\n driver = usbhid-ups\n port = auto\n vendorid = 0463\n serial = 1\n",
        )
        .unwrap();
        assert!(
            usb_devices(&paths, &host, &silent(&root)).await.iter().all(|d| d.needed != Some(HostNeeds::Ups))
        );
        fs::write(&paths.ups_conf, "[ups]\n driver = snmp-ups\n port = 10.0.0.9\n").unwrap();
        assert!(
            usb_devices(&paths, &host, &silent(&root)).await.iter().all(|d| d.needed != Some(HostNeeds::Ups))
        );
    }

    #[tokio::test]
    async fn pci_devices_say_what_the_host_needs() {
        let (dir, paths, host) = machine();
        let root = root_of(&dir);
        let devices = pci_devices(&paths, &host, &busctl(&root)).await;
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
        assert_eq!(needs("0000:00:14.0"), Some(HostNeeds::Disk { disk: "sdb".into() }), "the stick on it");
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

        // The stick ejected: the USB controller carries the network on the
        // adapter, and without systemd-networkd still the UPS.
        fs::write(root.join("mountinfo"), "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n").unwrap();
        let devices = pci_devices(&paths, &host, &busctl(&root)).await;
        let xhci = devices.iter().find(|d| d.address.as_str() == "0000:00:14.0").unwrap();
        assert_eq!(xhci.needed, Some(HostNeeds::Network { interface: Some("enx".into()) }));
        fs::remove_file(&paths.ups_conf).unwrap();
        fs::remove_file(paths.usb.join("1-1.4:1.0/driver")).unwrap();
        let devices = pci_devices(&paths, &host, &silent(&root)).await;
        let needs =
            |address: &str| devices.iter().find(|d| d.address.as_str() == address).unwrap().needed.clone();
        // Without systemd-networkd, every network card is the host's.
        assert_eq!(needs("0000:01:00.0"), Some(HostNeeds::Network { interface: None }));
        assert_eq!(needs("0000:00:14.0"), None, "nothing on it the host needs");
    }

    /// A disk below a controller that cannot be opened, for any reason,
    /// counts as in use: eMMC, which the agent's sandbox may not open.
    #[tokio::test]
    async fn a_disk_that_cannot_be_opened_counts_as_in_use() {
        let (dir, paths, host) = machine();
        let root = root_of(&dir);
        let devices_dir = root.join("sys/devices/pci0000:00/0000:00:1a.0");
        fs::create_dir_all(&devices_dir).unwrap();
        for (file, value) in [("vendor", "0x8086"), ("device", "0x9d2b"), ("class", "0x080501")] {
            fs::write(devices_dir.join(file), format!("{value}\n")).unwrap();
        }
        symlink(&devices_dir, paths.pci.join("0000:00:1a.0")).unwrap();
        let disk = devices_dir.join("mmc_host/mmc0/mmc0:0001/block/mmcblk0");
        fs::create_dir_all(disk.join("holders")).unwrap();
        fs::write(disk.join("dev"), "179:0\n").unwrap();
        symlink(&disk, host.sys_block.join("mmcblk0")).unwrap();
        // No device node the agent may open.
        let devices = pci_devices(&paths, &host, &busctl(&root)).await;
        let emmc = devices.iter().find(|d| d.address.as_str() == "0000:00:1a.0").unwrap();
        assert_eq!(emmc.needed, Some(HostNeeds::Disk { disk: "mmcblk0".into() }));
        fs::write(host.dev.join("mmcblk0"), "").unwrap();
        let devices = pci_devices(&paths, &host, &busctl(&root)).await;
        let emmc = devices.iter().find(|d| d.address.as_str() == "0000:00:1a.0").unwrap();
        assert_eq!(emmc.needed, None, "opened: nothing claims it");
    }

    /// An NVMe namespace with native multipath sits below its subsystem;
    /// it is the controllers' whose paths to it are below them, and the
    /// hidden paths themselves are not looked at.
    #[tokio::test]
    async fn an_nvme_namespace_through_multipath_belongs_to_its_controllers() {
        let (dir, paths, host) = machine();
        let root = root_of(&dir);
        let controller = root.join("sys/devices/pci0000:00/0000:00:05.0");
        fs::create_dir_all(&controller).unwrap();
        for (file, value) in [("vendor", "0x144d"), ("device", "0xa824"), ("class", "0x010802")] {
            fs::write(controller.join(file), format!("{value}\n")).unwrap();
        }
        symlink(&controller, paths.pci.join("0000:00:05.0")).unwrap();
        let path = controller.join("nvme/nvme1/nvme1c1n1");
        fs::create_dir_all(path.join("holders")).unwrap();
        fs::write(path.join("hidden"), "1\n").unwrap();
        fs::write(path.join("dev"), "259:4\n").unwrap();
        symlink(&path, host.sys_block.join("nvme1c1n1")).unwrap();
        let head = root.join("sys/devices/virtual/nvme-subsystem/nvme-subsys1/nvme1n1");
        fs::create_dir_all(head.join("holders")).unwrap();
        fs::create_dir_all(head.join("multipath")).unwrap();
        fs::write(head.join("dev"), "259:5\n").unwrap();
        symlink(&path, head.join("multipath/nvme1c1n1")).unwrap();
        symlink(&head, host.sys_block.join("nvme1n1")).unwrap();
        let partition = head.join("nvme1n1p1");
        fs::create_dir_all(partition.join("holders")).unwrap();
        fs::write(partition.join("dev"), "259:6\n").unwrap();
        fs::write(partition.join("partition"), "1\n").unwrap();
        symlink(&partition, host.sys_block.join("nvme1n1p1")).unwrap();
        for name in ["nvme1n1", "nvme1n1p1"] {
            fs::write(host.dev.join(name), "").unwrap();
        }
        let needed = |devices: &[HostPciDevice]| {
            devices.iter().find(|d| d.address.as_str() == "0000:00:05.0").unwrap().needed.clone()
        };
        assert_eq!(needed(&pci_devices(&paths, &host, &busctl(&root)).await), None, "nothing uses it");
        fs::write(
            root.join("mountinfo"),
            "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n23 22 259:6 / /data rw - xfs x rw\n",
        )
        .unwrap();
        assert_eq!(
            needed(&pci_devices(&paths, &host, &busctl(&root)).await),
            Some(HostNeeds::Disk { disk: "nvme1n1".into() }),
            "its partition is mounted"
        );
    }

    #[tokio::test]
    async fn a_pci_device_goes_with_its_whole_group_and_never_one_the_host_needs() {
        let (dir, paths, host) = machine();
        let devices = pci_devices(&paths, &host, &busctl(&root_of(&dir))).await;
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

    /// What a device was when the VM was given it is recorded; a start
    /// with another one there is refused.
    #[test]
    fn another_device_where_the_vm_had_one_is_refused() {
        let (_dir, paths, _) = machine();
        let card = PciAddress::parse("0000:01:00.0").unwrap();
        let (stick, keyboard) = (usb(0x0781, 0x5583), usb(0x046d, 0xc31c));
        let recorded = identify(
            &paths,
            &[stick, keyboard, usb(0x1234, 0x5678)],
            std::slice::from_ref(&card),
            &Identity::default(),
        );
        assert_eq!(
            recorded.pci["0000:01:00.0"],
            PciIdentity {
                vendor: 0x8086,
                device: 0x10d3,
                subsystem_vendor: Some(0x8086),
                subsystem_device: Some(0xa01f),
                class: 0x020000
            }
        );
        assert_eq!(recorded.usb["0781:5583"].serial.as_deref(), Some("4C530001"));
        assert!(recorded.usb["0781:5583"].by_port, "a disk");
        assert!(!recorded.usb["046d:c31c"].by_port && recorded.usb["046d:c31c"].port == "1-1.3");
        assert!(!recorded.usb.contains_key("1234:5678"), "not plugged in");
        assert!(check_identity(&paths, &[stick, keyboard], std::slice::from_ref(&card), &recorded).is_ok());
        // Recorded through JSON, as it is kept.
        let kept: Identity = serde_json::from_str(&serde_json::to_string(&recorded).unwrap()).unwrap();
        assert_eq!(kept, recorded);
        // Unplugged now: what was recorded stays.
        let again = identify(
            &paths,
            &[usb(0x1234, 0x5678)],
            &[],
            &Identity {
                usb: [("1234:5678".to_owned(), recorded.usb["0781:5583"].clone())].into(),
                ..Identity::default()
            },
        );
        assert_eq!(again.usb["1234:5678"].port, "1-1.2");

        // The bus numbered anew: another card at the address.
        let place = paths.pci.join("0000:01:00.0");
        fs::write(place.join("device"), "0x1533\n").unwrap();
        let moved =
            check_identity(&paths, &[], std::slice::from_ref(&card), &recorded).unwrap_err().to_string();
        assert!(
            moved.contains("0000:01:00.0 is not the one the virtual machine was given (8086:10d3 8086:a01f")
        );
        assert!(moved.contains("now 8086:1533 8086:a01f"));
        assert!(check_identity(&paths, &[], &[card], &Identity::default()).is_ok(), "nothing recorded");

        // Another stick of the same make.
        fs::write(paths.usb.join("1-1.2/serial"), "4C530002\n").unwrap();
        let other = check_identity(&paths, &[stick], &[], &recorded).unwrap_err().to_string();
        assert!(other.contains("serial number 4C530001, this one 4C530002"));
        // Without serial numbers a disk is told by its port, a keyboard is not.
        let mut portless = recorded.clone();
        for identity in portless.usb.values_mut() {
            identity.serial = None;
            identity.port = "2-1".into();
        }
        let elsewhere = check_identity(&paths, &[stick], &[], &portless).unwrap_err().to_string();
        assert!(
            elsewhere.contains("plugged in on 1-1.2, and the one the virtual machine was given was on 2-1")
        );
        assert!(check_identity(&paths, &[keyboard], &[], &portless).is_ok());
    }

    /// A device stays what it was when the VM was given it: a change of
    /// the VM records no other device the bus put at its address since.
    #[test]
    fn a_change_keeps_what_the_vm_was_given() {
        let (_dir, paths, _) = machine();
        let card = PciAddress::parse("0000:01:00.0").unwrap();
        let stick = usb(0x0781, 0x5583);
        let given = identify(&paths, &[stick], std::slice::from_ref(&card), &Identity::default());
        fs::write(paths.pci.join("0000:01:00.0/device"), "0x1533\n").unwrap();
        fs::write(paths.usb.join("1-1.2/serial"), "4C530002\n").unwrap();
        let changed = identify(&paths, &[stick], std::slice::from_ref(&card), &given);
        assert!(changed == given, "a change took on the devices there now");
        assert!(check_identity(&paths, &[stick], std::slice::from_ref(&card), &changed).is_err());
        // Taken away and given again: the one there then.
        let again = identify(&paths, &[stick], std::slice::from_ref(&card), &Identity::default());
        assert_eq!(again.pci["0000:01:00.0"].device, 0x1533);
        assert_eq!(again.usb["0781:5583"].serial.as_deref(), Some("4C530002"));
        assert!(check_identity(&paths, &[stick], std::slice::from_ref(&card), &again).is_ok());
    }

    /// A disk below a device a VM is given is the VM's, also while the VM
    /// is off: the NAS does not mount it.
    #[test]
    fn a_disk_below_a_device_a_vm_is_given_is_told() {
        let (_dir, paths, host) = machine();
        let sdb = fs::canonicalize(host.sys_block.join("sdb1")).unwrap();
        let sda = fs::canonicalize(host.sys_block.join("sda1")).unwrap();
        let stick = usb(0x0781, 0x5583);
        let recorded = identify(&paths, &[stick], &[], &Identity::default());
        assert!(below_given(&paths, &[stick], &[], &recorded, &sdb));
        assert!(below_given(&paths, &[stick], &[], &Identity::default(), &sdb), "nothing recorded yet");
        assert!(!below_given(&paths, &[stick], &[], &recorded, &sda));
        assert!(!below_given(&paths, &[usb(0x046d, 0xc31c)], &[], &Identity::default(), &sdb));
        // Another stick of the make the VM is given, on the same port.
        fs::write(paths.usb.join("1-1.2/serial"), "4C530002\n").unwrap();
        assert!(!below_given(&paths, &[stick], &[], &recorded, &sdb));
        // The whole USB controller, or the AHCI controller.
        let xhci = PciAddress::parse("0000:00:14.0").unwrap();
        assert!(below_given(&paths, &[], std::slice::from_ref(&xhci), &Identity::default(), &sdb));
        assert!(!below_given(&paths, &[], std::slice::from_ref(&xhci), &Identity::default(), &sda));
    }

    #[test]
    fn the_ups_is_read_from_nuts_drivers() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ups.conf");
        fs::write(
            &file,
            "maxretry = 3\n[one]\n  driver = usbhid-ups\n  port = auto\n  vendorid = \"0463\"\n  productid = FFFF\n\
             [two]\n  driver = nutdrv_qx\n  port = auto\n  vendorid = 0665\n  serial = AB12\n\
             [three]\n  driver = snmp-ups\n  port = 10.0.0.2\n  vendorid = 0463\n[four]\n  port = auto\n",
        )
        .unwrap();
        assert_eq!(
            ups_on_usb(&file),
            [
                UpsOnUsb { vendor: 0x0463, product: Some(0xffff), serial: None },
                UpsOnUsb { vendor: 0x0665, product: None, serial: Some("AB12".into()) },
            ]
        );
        assert!(ups_on_usb(&dir.path().join("none")).is_empty());
    }
}
