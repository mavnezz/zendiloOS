//! Virtual machines through libvirt (docs/architecture.md, 4.13): what
//! zendiloosd may ask of the agent about them. Every value is checked here,
//! while the request is read; the agent builds the domain's XML from these
//! types alone, never from text a client sent.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{ArchiveDigest, InterfaceName, MacAddress, PoolName, ShareLocation, SharePath, VolumeNumber};

/// Every domain zendiloOS defines is named so; the agent touches no other.
pub const VM_DOMAIN_PREFIX: &str = "zendiloos-";
/// The VMs' dataset below a pool: `<pool>/zendiloos.vms`. The dot keeps it
/// from ever being a share, as for Docker's.
pub const VMS_DATASET: &str = "zendiloos.vms";
/// Where it is mounted on the volume: `/volume<N>/@vms`.
pub const VMS_DIRECTORY: &str = "@vms";
/// The image library below it: `/volume<N>/@vms/isos/<image>.iso`.
pub const VM_IMAGES_DIRECTORY: &str = "isos";
/// Where QEMU puts each VM's console, a VNC server on a Unix socket:
/// `<this>/<id>.vnc`.
pub const VM_CONSOLES: &str = "/run/zendiloos-vms";
/// Disks a VM has at most: `<id>-disk0` to `<id>-disk7`.
pub const VM_DISKS_MAX: usize = 8;
/// Network cards a VM has at most.
pub const VM_NICS_MAX: usize = 4;
/// CD drives a VM has at most, each with an image of the library or empty.
pub const VM_DRIVES_MAX: usize = 3;
/// The largest ISO a browser uploads into the library: 32 GiB, more than
/// any installation medium (Windows' are 6 to 7 GB).
pub const VM_IMAGE_UPLOAD_MAX: u64 = 32 << 30;
/// USB devices of the host's a VM is given at most.
pub const VM_USB_MAX: usize = 8;
/// PCI devices of the host's a VM is given at most: a card or two with
/// the functions of their IOMMU groups.
pub const VM_PCI_MAX: usize = 8;

/// Twelve lowercase hexadecimal digits, made by zendiloosd at random.
fn is_hex12(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A VM's id: twelve lowercase hexadecimal digits, made by zendiloosd. It
/// names the domain (`zendiloos-<id>`), its disks (`<id>-disk<N>`) and its
/// console.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VmId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a VM id")]
pub struct InvalidVmId;

impl VmId {
    pub const LENGTH: usize = 12;

    pub fn parse(id: &str) -> Result<Self, InvalidVmId> {
        if is_hex12(id) { Ok(Self(id.to_owned())) } else { Err(InvalidVmId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The libvirt domain: `zendiloos-<id>`.
    pub fn domain(&self) -> String {
        format!("{VM_DOMAIN_PREFIX}{}", self.0)
    }

    /// The id of a domain zendiloOS defined, from its name.
    pub fn of_domain(name: &str) -> Option<Self> {
        Self::parse(name.strip_prefix(VM_DOMAIN_PREFIX)?).ok()
    }

    /// A disk's zvol below the VMs' dataset: `<id>-disk<N>`.
    pub fn disk(&self, slot: VmDiskSlot) -> String {
        format!("{}-disk{}", self.0, slot.get())
    }
}

impl TryFrom<String> for VmId {
    type Error = InvalidVmId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<VmId> for String {
    fn from(id: VmId) -> Self {
        id.0
    }
}

/// An image of the library: twelve lowercase hexadecimal digits, made by
/// zendiloosd. Its file is `<id>.iso` in the library's folder; what it is
/// called is zendiloOS's alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VmImageId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an image id")]
pub struct InvalidVmImageId;

impl VmImageId {
    pub fn parse(id: &str) -> Result<Self, InvalidVmImageId> {
        if is_hex12(id) { Ok(Self(id.to_owned())) } else { Err(InvalidVmImageId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `<id>.iso`.
    pub fn file_name(&self) -> String {
        format!("{}.iso", self.0)
    }

    /// The image whose file this is, if it is one.
    pub fn of_file_name(name: &str) -> Option<Self> {
        Self::parse(name.strip_suffix(".iso")?).ok()
    }
}

impl TryFrom<String> for VmImageId {
    type Error = InvalidVmImageId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<VmImageId> for String {
    fn from(id: VmImageId) -> Self {
        id.0
    }
}

/// What a VM runs, which decides the devices that are not chosen one by
/// one: the video card, the memory balloon, the random source, Hyper-V's
/// enlightenments and the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmOs {
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmFirmware {
    /// OVMF, chosen by libvirt from the firmware QEMU describes.
    Uefi,
    /// SeaBIOS.
    Bios,
}

/// Virtual CPUs, 1 to 256; zendiloosd holds them to the host's cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct VmCpus(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a VM has 1 to 256 virtual CPUs")]
pub struct InvalidVmCpus;

impl VmCpus {
    pub fn new(count: u16) -> Result<Self, InvalidVmCpus> {
        if (1..=256).contains(&count) { Ok(Self(count)) } else { Err(InvalidVmCpus) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for VmCpus {
    type Error = InvalidVmCpus;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VmCpus> for u16 {
    fn from(cpus: VmCpus) -> Self {
        cpus.0
    }
}

/// Memory in MiB, 128 MiB to 4 TiB; zendiloosd holds it to the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct VmMemory(u32);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a VM has 128 MiB to 4 TiB of memory")]
pub struct InvalidVmMemory;

impl VmMemory {
    pub const MIN_MIB: u32 = 128;
    pub const MAX_MIB: u32 = 4 * 1024 * 1024;

    pub fn new(mib: u32) -> Result<Self, InvalidVmMemory> {
        if (Self::MIN_MIB..=Self::MAX_MIB).contains(&mib) { Ok(Self(mib)) } else { Err(InvalidVmMemory) }
    }

    pub fn mib(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for VmMemory {
    type Error = InvalidVmMemory;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VmMemory> for u32 {
    fn from(memory: VmMemory) -> Self {
        memory.0
    }
}

/// A disk's size in GiB, 1 to 64 TiB; a sparse zvol takes only what the VM
/// writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct VmDiskSize(u32);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a VM's disk has 1 GiB to 64 TiB")]
pub struct InvalidVmDiskSize;

impl VmDiskSize {
    pub const MAX_GIB: u32 = 64 * 1024;

    pub fn new(gib: u32) -> Result<Self, InvalidVmDiskSize> {
        if (1..=Self::MAX_GIB).contains(&gib) { Ok(Self(gib)) } else { Err(InvalidVmDiskSize) }
    }

    pub fn gib(self) -> u32 {
        self.0
    }

    pub fn bytes(self) -> u64 {
        u64::from(self.0) << 30
    }
}

impl TryFrom<u32> for VmDiskSize {
    type Error = InvalidVmDiskSize;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VmDiskSize> for u32 {
    fn from(size: VmDiskSize) -> Self {
        size.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no such slot")]
pub struct InvalidVmSlot;

/// A place among a VM's devices of one kind: 0 to `MAX - 1`. A disk's names
/// its zvol, a drive's its target, so they are bounded where they are read.
macro_rules! slot {
    ($(#[$doc:meta])* $name:ident, $max:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "u8", into = "u8")]
        pub struct $name(u8);

        impl $name {
            pub fn new(slot: u8) -> Result<Self, InvalidVmSlot> {
                if usize::from(slot) < $max { Ok(Self(slot)) } else { Err(InvalidVmSlot) }
            }

            pub fn get(self) -> u8 {
                self.0
            }
        }

        impl TryFrom<u8> for $name {
            type Error = InvalidVmSlot;

            fn try_from(value: u8) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for u8 {
            fn from(slot: $name) -> Self {
                slot.0
            }
        }
    };
}

slot!(
    /// A disk's place, 0 to 7: its zvol is `<id>-disk<N>`.
    VmDiskSlot,
    VM_DISKS_MAX
);
slot!(
    /// A network card's place, 0 to 3: the order of the cards.
    VmNicSlot,
    VM_NICS_MAX
);
slot!(
    /// A CD drive's place, 0 to 2: its target is `sdi` to `sdk`.
    VmDriveSlot,
    VM_DRIVES_MAX
);

/// How a disk is attached: virtio for systems that bring its driver
/// (Linux), SATA for those that may not (Windows' installer, anything else).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmDiskBus {
    Virtio,
    Sata,
}

/// The network card a VM sees: virtio, or an Intel e1000e every system
/// knows without drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmNicModel {
    Virtio,
    E1000e,
}

/// How a network card reaches the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VmNetwork {
    /// User-mode networking through passt: the VM reaches the LAN and the
    /// internet from the NAS's address, nothing reaches the VM. No bridge,
    /// no NAT rules, no DHCP server of libvirt's.
    User,
    /// A Linux bridge the machine has (`br0`): the VM is on the LAN as a
    /// machine of its own. The agent checks it is a bridge.
    Bridge { bridge: InterfaceName },
    /// macvtap on a wired card or bond of the host's, in bridge mode: the
    /// VM is on the card's LAN without a bridge, and the NAS and the VM do
    /// not reach each other (the card does not hand frames back). The agent
    /// checks it is a card or a bond.
    Direct { card: InterfaceName },
    /// A bridge zendiloOS made without members and without an address of
    /// the NAS's, which the NAS keeps itself off (frames to and from it
    /// dropped): VMs on it reach each other only. The agent checks it is
    /// such a bridge and that the NAS is kept off it.
    Internal { bridge: InterfaceName },
}

/// A disk: a sparse zvol `<pool>/zendiloos.vms/<id>-disk<slot>` on the
/// location it was made on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmDisk {
    pub slot: VmDiskSlot,
    pub size: VmDiskSize,
    pub bus: VmDiskBus,
    pub location: VmStorage,
}

/// A network card, with the MAC address zendiloosd gave it, which stays
/// whatever else of the VM changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmNic {
    pub slot: VmNicSlot,
    pub model: VmNicModel,
    pub network: VmNetwork,
    pub mac: MacAddress,
}

/// A SATA CD drive, read-only, with an image of the library in it or empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmDrive {
    pub slot: VmDriveSlot,
    pub image: Option<VmImageId>,
}

/// A device a VM may boot from; a VM boots from its devices in the order of
/// [`VmSpec::boot`], and from none other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmBoot {
    Disk(VmDiskSlot),
    Drive(VmDriveSlot),
}

/// A USB device of the host's a VM is given, by its vendor and product:
/// that stays when it is plugged in elsewhere or again, where its bus
/// and device number change. Two devices of the same vendor and product
/// plugged in at once cannot be told apart that way; libvirt then refuses
/// to start the VM, and the agent says so first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmUsbDevice {
    pub vendor: u16,
    pub product: u16,
}

impl VmUsbDevice {
    /// `1d6b:0002`, as `lsusb` writes it.
    pub fn id(self) -> String {
        format!("{:04x}:{:04x}", self.vendor, self.product)
    }
}

/// A PCI function's address on the host, `0000:01:00.0`: domain, bus,
/// slot (device) and function, as sysfs names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PciAddress(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a PCI address")]
pub struct InvalidPciAddress;

impl PciAddress {
    pub fn parse(text: &str) -> Result<Self, InvalidPciAddress> {
        let hex = |part: &str, length: usize| {
            part.len() == length && part.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let shape = (|| {
            let (domain, rest) = text.split_once(':')?;
            let (bus, rest) = rest.split_once(':')?;
            let (slot, function) = rest.split_once('.')?;
            let slot_ok = hex(slot, 2) && u8::from_str_radix(slot, 16).ok()? < 32;
            let function_ok = function.len() == 1 && (b'0'..=b'7').contains(&function.as_bytes()[0]);
            Some(hex(domain, 4) && hex(bus, 2) && slot_ok && function_ok)
        })();
        if shape == Some(true) { Ok(Self(text.to_owned())) } else { Err(InvalidPciAddress) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Domain, bus, slot and function as numbers.
    pub fn parts(&self) -> (u16, u8, u8, u8) {
        let number = |range: std::ops::Range<usize>| u16::from_str_radix(&self.0[range], 16).unwrap_or(0);
        (number(0..4), number(5..7) as u8, number(8..10) as u8, number(11..12) as u8)
    }
}

impl TryFrom<String> for PciAddress {
    type Error = InvalidPciAddress;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PciAddress> for String {
    fn from(address: PciAddress) -> Self {
        address.0
    }
}

/// A USB device plugged into the host, as sysfs describes it: hubs left
/// out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostUsbDevice {
    pub vendor: u16,
    pub product: u16,
    /// Where it is plugged in: `1-2.3`, bus 1, port 2 of the root hub, port
    /// 3 of the hub there.
    pub port: String,
    /// What the device calls its maker and itself, if it says.
    pub manufacturer: Option<String>,
    pub name: Option<String>,
}

/// Why the host keeps a PCI device for itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostNeeds {
    /// A disk on it is in use: in a pool, mounted, swap, or the system's.
    Disk { disk: String },
    /// The graphics card the firmware set up for the console.
    Console,
    /// A network card the host's network uses (an address, a bridge or bond
    /// it is in); every card where systemd-networkd cannot tell.
    Network { interface: Option<String> },
    /// A bridge of the PCI bus (a host bridge, a root port): never handed
    /// over, and it does not keep the other devices of its group from a
    /// VM, as VFIO lets bridges be.
    Bridge,
}

/// A PCI function of the host's, as sysfs describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPciDevice {
    pub address: PciAddress,
    pub vendor: u16,
    pub device: u16,
    /// Class, subclass and interface: `0x020000` is an Ethernet card.
    pub class: u32,
    /// From the PCI ID database, if it names them.
    pub vendor_name: Option<String>,
    pub device_name: Option<String>,
    /// Its IOMMU group: the devices that go to a VM together. None without
    /// an IOMMU.
    pub iommu_group: Option<u32>,
    /// The driver it is bound to now (`vfio-pci` while a VM has it).
    pub driver: Option<String>,
    /// Why the host keeps it, if it does; such a device is never handed
    /// over, nor is any of its group.
    pub needed: Option<HostNeeds>,
}

/// Whether the host has an IOMMU on, and what says how to switch it on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostIommu {
    /// `/sys/kernel/iommu_groups` lists groups: devices can be handed to
    /// VMs.
    pub on: bool,
    /// What the kernel's command line says of it: `intel_iommu=on`,
    /// `amd_iommu=off`, `iommu=pt`, …
    pub cmdline: Vec<String>,
    /// `GenuineIntel` or `AuthenticAMD`, from `/proc/cpuinfo`.
    pub cpu_vendor: Option<String>,
}

/// A VM as the agent defines it. Read only whole: 1 to 8 disks, up to 4
/// cards and 3 drives, each slot and MAC address once, and a boot order of
/// devices it has, each once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpecFields", into = "SpecFields")]
pub struct VmSpec {
    pub id: VmId,
    pub os: VmOs,
    pub cpus: VmCpus,
    pub memory: VmMemory,
    pub firmware: VmFirmware,
    disks: Vec<VmDisk>,
    nics: Vec<VmNic>,
    drives: Vec<VmDrive>,
    boot: Vec<VmBoot>,
    usb: Vec<VmUsbDevice>,
    pci: Vec<PciAddress>,
    tpm: bool,
    secure_boot: bool,
    /// libvirt starts it with the machine.
    pub autostart: bool,
    /// The volume of the location that holds the image library, where the
    /// images in its drives are.
    pub library: VolumeNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidVmSpec(String);

/// The devices of a [`VmSpec`], checked together by [`VmSpec::new`]:
/// its own, the host's it is given, a software TPM, and whether its UEFI
/// firmware boots only what is signed (Secure Boot).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VmDevices {
    pub disks: Vec<VmDisk>,
    pub nics: Vec<VmNic>,
    pub drives: Vec<VmDrive>,
    pub boot: Vec<VmBoot>,
    /// USB devices of the host's, by vendor and product.
    pub usb: Vec<VmUsbDevice>,
    /// PCI devices of the host's, handed over through VFIO.
    pub pci: Vec<PciAddress>,
    /// A TPM 2.0, emulated by swtpm.
    pub tpm: bool,
    /// Secure Boot with Microsoft's keys enrolled; UEFI only.
    pub secure_boot: bool,
}

/// Whether each of `items` has a key of its own.
fn unique<T, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> bool {
    let mut seen = BTreeSet::new();
    items.iter().all(|item| seen.insert(key(item)))
}

impl VmSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: VmId,
        os: VmOs,
        cpus: VmCpus,
        memory: VmMemory,
        firmware: VmFirmware,
        devices: VmDevices,
        autostart: bool,
        library: VolumeNumber,
    ) -> Result<Self, InvalidVmSpec> {
        let VmDevices { disks, nics, drives, boot, usb, pci, tpm, secure_boot } = devices;
        let invalid = |text: &str| Err(InvalidVmSpec(text.to_owned()));
        if disks.is_empty() || disks.len() > VM_DISKS_MAX {
            return invalid("a VM has 1 to 8 disks");
        }
        if nics.len() > VM_NICS_MAX {
            return invalid("a VM has at most 4 network cards");
        }
        if drives.len() > VM_DRIVES_MAX {
            return invalid("a VM has at most 3 CD drives");
        }
        if !unique(&disks, |disk| disk.slot) || !unique(&nics, |nic| nic.slot) || !unique(&drives, |d| d.slot)
        {
            return invalid("each device's slot is given once");
        }
        if !unique(&nics, |nic| nic.mac.as_str().to_owned()) {
            return invalid("each network card has a MAC address of its own");
        }
        if boot.is_empty() || !unique(&boot, |device| *device) {
            return invalid("a VM boots from at least one device, each named once");
        }
        let exists = |device: &VmBoot| match device {
            VmBoot::Disk(slot) => disks.iter().any(|disk| disk.slot == *slot),
            VmBoot::Drive(slot) => drives.iter().any(|drive| drive.slot == *slot),
        };
        if !boot.iter().all(exists) {
            return invalid("a VM boots only from devices it has");
        }
        if usb.len() > VM_USB_MAX || !unique(&usb, |device| *device) || usb.iter().any(|d| d.vendor == 0) {
            return invalid("a VM is given up to 8 USB devices, each once");
        }
        if pci.len() > VM_PCI_MAX || !unique(&pci, |address| address.clone()) {
            return invalid("a VM is given up to 8 PCI devices, each once");
        }
        if secure_boot && firmware != VmFirmware::Uefi {
            return invalid("Secure Boot needs UEFI");
        }
        Ok(Self {
            id,
            os,
            cpus,
            memory,
            firmware,
            disks,
            nics,
            drives,
            boot,
            usb,
            pci,
            tpm,
            secure_boot,
            autostart,
            library,
        })
    }

    pub fn disks(&self) -> &[VmDisk] {
        &self.disks
    }

    pub fn nics(&self) -> &[VmNic] {
        &self.nics
    }

    pub fn drives(&self) -> &[VmDrive] {
        &self.drives
    }

    /// The devices it boots from, first first.
    pub fn boot(&self) -> &[VmBoot] {
        &self.boot
    }

    /// The host's USB devices it is given.
    pub fn usb(&self) -> &[VmUsbDevice] {
        &self.usb
    }

    /// The host's PCI devices it is given.
    pub fn pci(&self) -> &[PciAddress] {
        &self.pci
    }

    /// Whether it has a TPM 2.0.
    pub fn tpm(&self) -> bool {
        self.tpm
    }

    /// Whether its firmware has Secure Boot on, with Microsoft's keys.
    pub fn secure_boot(&self) -> bool {
        self.secure_boot
    }

    /// The drive in `slot`, if it has one.
    pub fn drive(&self, slot: VmDriveSlot) -> Option<&VmDrive> {
        self.drives.iter().find(|drive| drive.slot == slot)
    }

    /// The images its drives hold.
    pub fn images(&self) -> impl Iterator<Item = &VmImageId> {
        self.drives.iter().filter_map(|drive| drive.image.as_ref())
    }

    /// The locations its disks are on, each once.
    pub fn locations(&self) -> Vec<&VmStorage> {
        let mut locations: Vec<&VmStorage> = Vec::new();
        for disk in &self.disks {
            if !locations.contains(&&disk.location) {
                locations.push(&disk.location);
            }
        }
        locations
    }
}

/// [`VmSpec`] as it travels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecFields {
    id: VmId,
    os: VmOs,
    cpus: VmCpus,
    memory: VmMemory,
    firmware: VmFirmware,
    disks: Vec<VmDisk>,
    nics: Vec<VmNic>,
    drives: Vec<VmDrive>,
    boot: Vec<VmBoot>,
    #[serde(default)]
    usb: Vec<VmUsbDevice>,
    #[serde(default)]
    pci: Vec<PciAddress>,
    #[serde(default)]
    tpm: bool,
    #[serde(default)]
    secure_boot: bool,
    autostart: bool,
    library: VolumeNumber,
}

impl TryFrom<SpecFields> for VmSpec {
    type Error = InvalidVmSpec;

    fn try_from(fields: SpecFields) -> Result<Self, Self::Error> {
        let devices = VmDevices {
            disks: fields.disks,
            nics: fields.nics,
            drives: fields.drives,
            boot: fields.boot,
            usb: fields.usb,
            pci: fields.pci,
            tpm: fields.tpm,
            secure_boot: fields.secure_boot,
        };
        Self::new(
            fields.id,
            fields.os,
            fields.cpus,
            fields.memory,
            fields.firmware,
            devices,
            fields.autostart,
            fields.library,
        )
    }
}

impl From<VmSpec> for SpecFields {
    fn from(spec: VmSpec) -> Self {
        Self {
            id: spec.id,
            os: spec.os,
            cpus: spec.cpus,
            memory: spec.memory,
            firmware: spec.firmware,
            disks: spec.disks,
            nics: spec.nics,
            drives: spec.drives,
            boot: spec.boot,
            usb: spec.usb,
            pci: spec.pci,
            tpm: spec.tpm,
            secure_boot: spec.secure_boot,
            autostart: spec.autostart,
            library: spec.library,
        }
    }
}

/// What a VM is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmAction {
    Start,
    /// ACPI: the guest is asked to power off.
    Shutdown,
    /// Pulled out of power at once.
    ForceOff,
    /// ACPI too: the guest is asked to restart.
    Reboot,
}

/// A location of the VMs' storage: the dataset `<pool>/zendiloos.vms`
/// mounted at `/volume<N>/@vms`, as the agent finds its mount unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmStorage {
    pub pool: PoolName,
    pub volume: VolumeNumber,
}

/// An ISO to copy into the library: a file in a share. zendiloosd checked
/// that the administrator may read it; the copy opens it beneath the
/// share's root, never through a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmIsoSource {
    pub share: ShareLocation,
    pub path: SharePath,
}

/// An image's file in the library, as the agent finds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmImageFile {
    pub image: VmImageId,
    pub bytes: u64,
}

/// How consistent a VM's snapshot is with itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmConsistency {
    /// Taken while the VM was shut off: as consistent as it gets.
    Off,
    /// Taken while it ran, its file systems frozen by the QEMU guest agent
    /// first: what was written is on its disks.
    Frozen,
    /// Taken while it ran without the guest agent: as if the power had gone
    /// at that moment. Journaling file systems and databases recover from
    /// it as from a power cut.
    Crash,
}

impl VmConsistency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Frozen => "frozen",
            Self::Crash => "crash",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Off, Self::Frozen, Self::Crash].into_iter().find(|kind| kind.as_str() == text)
    }
}

/// The settings of a VM as zendiloOS keeps them, as a backup carries
/// them: a JSON object of at most 64 KiB that zendiloosd wrote and reads
/// again; the agent only checks that it is one and puts it into the
/// backup's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VmBackupSettings(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a VM's settings are a JSON object of at most 64 KiB")]
pub struct InvalidVmBackupSettings;

impl VmBackupSettings {
    pub const MAX_BYTES: usize = 64 * 1024;

    pub fn parse(text: &str) -> Result<Self, InvalidVmBackupSettings> {
        let object = text.len() <= Self::MAX_BYTES
            && serde_json::from_str::<serde_json::Value>(text).is_ok_and(|value| value.is_object());
        if object { Ok(Self(text.to_owned())) } else { Err(InvalidVmBackupSettings) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for VmBackupSettings {
    type Error = InvalidVmBackupSettings;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<VmBackupSettings> for String {
    fn from(settings: VmBackupSettings) -> Self {
        settings.0
    }
}

/// What a file of a VM's backup holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VmBackupPart {
    /// A disk, read whole from a snapshot: `disk<N>.raw.zst`, its raw
    /// image compressed with zstd, of `volsize` bytes once decompressed.
    Disk { slot: VmDiskSlot, volsize: u64 },
    /// The UEFI firmware's variables: `nvram.fd`.
    Nvram,
    /// The TPM's state as swtpm keeps it: `tpm2-00.permall`.
    Tpm,
}

impl VmBackupPart {
    /// Its file in the backup's folder.
    pub fn file_name(self) -> String {
        match self {
            Self::Disk { slot, .. } => format!("disk{}.raw.zst", slot.get()),
            Self::Nvram => "nvram.fd".to_owned(),
            Self::Tpm => "tpm2-00.permall".to_owned(),
        }
    }
}

/// The manifest of a VM's backup, beside its files.
pub const VM_BACKUP_MANIFEST: &str = "manifest.json";

/// A file a VM's backup wrote: what it holds, its size and SHA-256 as
/// written, checked again before and while it is restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmBackupFile {
    pub part: VmBackupPart,
    pub bytes: u64,
    pub sha256: ArchiveDigest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_twelve_lowercase_hex_digits_and_name_the_domain() {
        let id = VmId::parse("0a1b2c3d4e5f").unwrap();
        assert_eq!(id.domain(), "zendiloos-0a1b2c3d4e5f");
        assert_eq!(id.disk(VmDiskSlot::new(0).unwrap()), "0a1b2c3d4e5f-disk0");
        assert_eq!(id.disk(VmDiskSlot::new(7).unwrap()), "0a1b2c3d4e5f-disk7");
        assert_eq!(VmId::of_domain("zendiloos-0a1b2c3d4e5f"), Some(id));
        for bad in [
            "",
            "0a1b2c3d4e5",
            "0a1b2c3d4e5f0",
            "0A1B2C3D4E5F",
            "0a1b2c3d4e5g",
            "../etc/pass",
            "-a1b2c3d4e5f",
        ] {
            assert!(VmId::parse(bad).is_err(), "{bad}");
            assert!(VmImageId::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(VmId::of_domain("win10"), None);
        assert_eq!(VmId::of_domain("zendiloos-win10"), None);
        assert!(serde_json::from_str::<VmId>("\"0a1b2c3d4e5f \"").is_err());
        let image = VmImageId::parse("00000000000a").unwrap();
        assert_eq!(image.file_name(), "00000000000a.iso");
        assert_eq!(VmImageId::of_file_name("00000000000a.iso"), Some(image));
        assert_eq!(VmImageId::of_file_name(".00000000000a.iso.part"), None);
        assert_eq!(VmImageId::of_file_name("00000000000a.img"), None);
    }

    #[test]
    fn sizes_and_slots_have_their_bounds() {
        assert!(VmCpus::new(0).is_err() && VmCpus::new(1).is_ok() && VmCpus::new(257).is_err());
        assert!(VmMemory::new(127).is_err() && VmMemory::new(128).is_ok());
        assert!(VmMemory::new(VmMemory::MAX_MIB + 1).is_err());
        assert!(VmDiskSize::new(0).is_err() && VmDiskSize::new(1).is_ok());
        assert!(VmDiskSize::new(VmDiskSize::MAX_GIB + 1).is_err());
        assert_eq!(VmDiskSize::new(2).unwrap().bytes(), 2 << 30);
        assert!(serde_json::from_str::<VmCpus>("0").is_err());
        assert!(VmDiskSlot::new(7).is_ok() && VmDiskSlot::new(8).is_err());
        assert!(VmNicSlot::new(3).is_ok() && VmNicSlot::new(4).is_err());
        assert!(VmDriveSlot::new(2).is_ok() && VmDriveSlot::new(3).is_err());
        assert!(serde_json::from_str::<VmDriveSlot>("3").is_err());
    }

    const SPEC: &str = r#"{"id":"0a1b2c3d4e5f","os":"linux","cpus":2,"memory":2048,"firmware":"uefi",
        "disks":[{"slot":0,"size":20,"bus":"virtio","location":{"pool":"tank","volume":1}},
                 {"slot":1,"size":100,"bus":"sata","location":{"pool":"cold","volume":2}}],
        "nics":[{"slot":0,"model":"virtio","network":{"kind":"user"},"mac":"52:54:00:00:00:01"}],
        "drives":[{"slot":0,"image":"00000000000a"},{"slot":1,"image":null}],
        "boot":[{"drive":0},{"disk":0}],
        "autostart":false,"library":1}"#;

    #[test]
    fn a_spec_takes_nothing_it_does_not_know() {
        let parsed: VmSpec = serde_json::from_str(SPEC).unwrap();
        assert_eq!(parsed.nics()[0].network, VmNetwork::User);
        assert_eq!(parsed.boot(), [VmBoot::Drive(VmDriveSlot(0)), VmBoot::Disk(VmDiskSlot(0))]);
        assert_eq!(parsed.images().map(VmImageId::as_str).collect::<Vec<_>>(), ["00000000000a"]);
        let back: VmSpec = serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(back, parsed);
        let extra = SPEC.replace("\"autostart\":false", "\"autostart\":false,\"xml\":\"<domain/>\"");
        assert!(serde_json::from_str::<VmSpec>(&extra).is_err());
        let bridge = SPEC.replace(r#"{"kind":"user"}"#, r#"{"kind":"bridge","bridge":"br0;rm"}"#);
        assert!(serde_json::from_str::<VmSpec>(&bridge).is_err());
        let path = SPEC.replace("\"00000000000a\"", "\"../../etc/shadow\"");
        assert!(serde_json::from_str::<VmSpec>(&path).is_err());
        let pool = SPEC.replace("\"cold\"", "\"cold/../tank\"");
        assert!(serde_json::from_str::<VmSpec>(&pool).is_err());
        let locations: Vec<&str> = parsed.locations().iter().map(|l| l.pool.as_str()).collect();
        assert_eq!(locations, ["tank", "cold"]);
    }

    #[test]
    fn the_hosts_devices_a_tpm_and_secure_boot_are_part_of_the_spec() {
        for good in ["0000:01:00.0", "0000:00:1f.7", "abcd:ff:1f.3"] {
            assert!(PciAddress::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "0000:01:00",
            "0000:01:20.0",
            "0000:01:00.8",
            "0000:1:00.0",
            "0000:01:00.0 ",
            "../0:01:00.0",
            "0000:0G:00.0",
        ] {
            assert!(PciAddress::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(PciAddress::parse("0000:0a:1f.3").unwrap().parts(), (0, 10, 31, 3));
        assert_eq!(VmUsbDevice { vendor: 0x046d, product: 0xc31c }.id(), "046d:c31c");
        let text = SPEC.replace(
            r#""autostart":false"#,
            r#""usb":[{"vendor":1133,"product":49948}],"pci":["0000:01:00.0"],"tpm":true,"secure_boot":true,"autostart":false"#,
        );
        let parsed: VmSpec = serde_json::from_str(&text).unwrap();
        assert!(parsed.tpm() && parsed.secure_boot());
        assert_eq!(parsed.usb(), [VmUsbDevice { vendor: 0x046d, product: 0xc31c }]);
        assert_eq!(parsed.pci()[0].as_str(), "0000:01:00.0");
        let back: VmSpec = serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(back, parsed);
        // Left out: none of them, as before.
        let plain: VmSpec = serde_json::from_str(SPEC).unwrap();
        assert!(!plain.tpm() && !plain.secure_boot() && plain.usb().is_empty() && plain.pci().is_empty());
        let bios = text.replace(r#""firmware":"uefi""#, r#""firmware":"bios""#);
        assert!(
            serde_json::from_str::<VmSpec>(&bios).unwrap_err().to_string().contains("Secure Boot needs UEFI")
        );
        let twice = text.replace(
            r#"[{"vendor":1133,"product":49948}]"#,
            r#"[{"vendor":1133,"product":49948},{"vendor":1133,"product":49948}]"#,
        );
        assert!(serde_json::from_str::<VmSpec>(&twice).unwrap_err().to_string().contains("USB"));
        let nine = text.replace(
            r#"["0000:01:00.0"]"#,
            &format!("[{}]", (0..9).map(|n| format!("\"0000:0{n}:00.0\"")).collect::<Vec<_>>().join(",")),
        );
        assert!(serde_json::from_str::<VmSpec>(&nine).unwrap_err().to_string().contains("PCI"));
        let vendorless = text.replace(r#""vendor":1133"#, r#""vendor":0"#);
        assert!(serde_json::from_str::<VmSpec>(&vendorless).is_err());
    }

    #[test]
    fn a_backups_parts_and_settings() {
        assert_eq!(VmBackupPart::Disk { slot: VmDiskSlot(3), volsize: 1 }.file_name(), "disk3.raw.zst");
        assert_eq!(VmBackupPart::Nvram.file_name(), "nvram.fd");
        assert_eq!(VmBackupPart::Tpm.file_name(), "tpm2-00.permall");
        assert!(VmBackupSettings::parse(r#"{"name":"x"}"#).is_ok());
        for bad in ["[]", "\"x\"", "{", &format!("{{\"x\":\"{}\"}}", "y".repeat(VmBackupSettings::MAX_BYTES))]
        {
            assert!(VmBackupSettings::parse(bad).is_err());
        }
        for kind in [VmConsistency::Off, VmConsistency::Frozen, VmConsistency::Crash] {
            assert_eq!(VmConsistency::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(VmConsistency::parse("fuzzy"), None);
        let part: VmBackupPart = serde_json::from_str(r#"{"kind":"disk","slot":1,"volsize":1024}"#).unwrap();
        assert_eq!(part, VmBackupPart::Disk { slot: VmDiskSlot(1), volsize: 1024 });
        assert!(serde_json::from_str::<VmBackupPart>(r#"{"kind":"disk","slot":9,"volsize":1}"#).is_err());
    }

    #[test]
    fn cards_go_direct_on_a_card_or_on_an_internal_network_by_checked_names() {
        let direct = SPEC.replace(r#"{"kind":"user"}"#, r#"{"kind":"direct","card":"enp2s0"}"#);
        let parsed: VmSpec = serde_json::from_str(&direct).unwrap();
        assert_eq!(
            parsed.nics()[0].network,
            VmNetwork::Direct { card: InterfaceName::parse("enp2s0").unwrap() }
        );
        let internal = SPEC.replace(r#"{"kind":"user"}"#, r#"{"kind":"internal","bridge":"vmnet0"}"#);
        let parsed: VmSpec = serde_json::from_str(&internal).unwrap();
        assert!(
            matches!(&parsed.nics()[0].network, VmNetwork::Internal { bridge } if bridge.as_str() == "vmnet0")
        );
        for bad in [
            r#"{"kind":"direct","card":"enp2s0 mode='vepa'"}"#,
            r#"{"kind":"direct","bridge":"enp2s0"}"#,
            r#"{"kind":"internal","bridge":"vmnet0","card":"x"}"#,
            r#"{"kind":"internal","bridge":""}"#,
        ] {
            let text = SPEC.replace(r#"{"kind":"user"}"#, bad);
            assert!(serde_json::from_str::<VmSpec>(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_spec_is_whole_or_refused() {
        for (from, to, says) in [
            (r#"{"slot":1,"size":100"#, r#"{"slot":0,"size":100"#, "slot"),
            (r#"{"slot":1,"image":null}"#, r#"{"slot":0,"image":null}"#, "slot"),
            (r#"{"slot":1,"size":100,"bus":"sata""#, r#"{"slot":8,"size":100,"bus":"sata""#, ""),
            (r#"[{"drive":0},{"disk":0}]"#, r#"[{"drive":0},{"drive":0}]"#, "once"),
            (r#"[{"drive":0},{"disk":0}]"#, r#"[{"disk":5}]"#, "devices it has"),
            (r#"[{"drive":0},{"disk":0}]"#, "[]", "at least one"),
            (r#""autostart":false"#, r#""autostart":false,"x":[]"#, "unknown"),
        ] {
            let text = SPEC.replace(from, to);
            assert_ne!(text, SPEC, "{from} not in the spec");
            let error = serde_json::from_str::<VmSpec>(&text).unwrap_err().to_string();
            assert!(error.contains(says), "{error} does not say {says}");
        }
        let disks = &SPEC[SPEC.find(r#""disks":"#).unwrap()..SPEC.find(r#""nics":"#).unwrap()];
        let no_disks = SPEC.replace(disks.trim_end(), r#""disks":[],"#);
        assert!(serde_json::from_str::<VmSpec>(&no_disks).unwrap_err().to_string().contains("1 to 8 disks"));
        let card = r#"{"slot":0,"model":"virtio","network":{"kind":"user"},"mac":"52:54:00:00:00:01"}"#;
        let twins = SPEC.replace(card, &format!("{card},{}", card.replace("\"slot\":0", "\"slot\":1")));
        assert!(serde_json::from_str::<VmSpec>(&twins).unwrap_err().to_string().contains("MAC"));
        let five = SPEC.replace(card, &[card; 5].join(","));
        assert!(serde_json::from_str::<VmSpec>(&five).unwrap_err().to_string().contains("4 network cards"));
        let multicast = SPEC.replace("52:54:00:00:00:01", "01:00:5e:00:00:01");
        assert!(serde_json::from_str::<VmSpec>(&multicast).is_err());
    }
}
