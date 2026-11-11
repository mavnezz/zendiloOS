//! Virtual machines through libvirt (docs/architecture.md, 4.13): what
//! zendiloosd may ask of the agent about them. Every value is checked here,
//! while the request is read; the agent builds the domain's XML from these
//! types alone, never from text a client sent.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{InterfaceName, MacAddress, PoolName, ShareLocation, SharePath, VolumeNumber};

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
    /// libvirt starts it with the machine.
    pub autostart: bool,
    /// The volume of the location that holds the image library, where the
    /// images in its drives are.
    pub library: VolumeNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidVmSpec(String);

/// The devices of a [`VmSpec`], checked together by [`VmSpec::new`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VmDevices {
    pub disks: Vec<VmDisk>,
    pub nics: Vec<VmNic>,
    pub drives: Vec<VmDrive>,
    pub boot: Vec<VmBoot>,
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
        let VmDevices { disks, nics, drives, boot } = devices;
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
        Ok(Self { id, os, cpus, memory, firmware, disks, nics, drives, boot, autostart, library })
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
    autostart: bool,
    library: VolumeNumber,
}

impl TryFrom<SpecFields> for VmSpec {
    type Error = InvalidVmSpec;

    fn try_from(fields: SpecFields) -> Result<Self, Self::Error> {
        let devices =
            VmDevices { disks: fields.disks, nics: fields.nics, drives: fields.drives, boot: fields.boot };
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
