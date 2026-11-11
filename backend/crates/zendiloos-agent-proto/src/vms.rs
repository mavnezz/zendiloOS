//! Virtual machines through libvirt (docs/architecture.md, 4.13): what
//! zendiloosd may ask of the agent about them. Every value is checked here,
//! while the request is read; the agent builds the domain's XML from these
//! types alone, never from text a client sent.

use serde::{Deserialize, Serialize};

use crate::{InterfaceName, PoolName, ShareLocation, SharePath, VolumeNumber};

/// Every domain zendiloOS defines is named so; the agent touches no other.
pub const VM_DOMAIN_PREFIX: &str = "zendiloos-";
/// The VMs' dataset below a pool: `<pool>/zendiloos.vms`. The dot keeps it
/// from ever being a share, as for Docker's.
pub const VMS_DATASET: &str = "zendiloos.vms";
/// Where it is mounted on the volume: `/volume<N>/@vms`.
pub const VMS_DIRECTORY: &str = "@vms";
/// Where QEMU puts each VM's console, a VNC server on a Unix socket:
/// `<this>/<id>.vnc`.
pub const VM_CONSOLES: &str = "/run/zendiloos-vms";

/// A VM's id: twelve lowercase hexadecimal digits, made by zendiloosd. It
/// names the domain (`zendiloos-<id>`), its disk (`<id>-disk0`), its ISO's
/// copy and its console.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VmId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a VM id")]
pub struct InvalidVmId;

impl VmId {
    pub const LENGTH: usize = 12;

    pub fn parse(id: &str) -> Result<Self, InvalidVmId> {
        let valid =
            id.len() == Self::LENGTH && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if valid { Ok(Self(id.to_owned())) } else { Err(InvalidVmId) }
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

    /// The system disk's zvol below the VMs' dataset: `<id>-disk0`.
    pub fn disk(&self) -> String {
        format!("{}-disk0", self.0)
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

/// What a VM runs, which decides its devices: Windows has no virtio
/// drivers of its own, so its disk is SATA and its network card an e1000e.
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

/// The system disk's size in GiB, 1 to 64 TiB; a sparse zvol takes only
/// what the VM writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// How a VM reaches the network.
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
}

/// A VM as the agent defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmSpec {
    pub id: VmId,
    pub os: VmOs,
    pub cpus: VmCpus,
    pub memory: VmMemory,
    pub firmware: VmFirmware,
    pub disk: VmDiskSize,
    pub network: VmNetwork,
    /// The copy of an ISO ([`crate::Request::CopyVmIso`]) is attached
    /// read-only and booted first.
    pub iso: bool,
    /// libvirt starts it with the machine.
    pub autostart: bool,
    /// The pool the VMs' dataset is on; its disk goes there.
    pub pool: PoolName,
    pub volume: VolumeNumber,
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

/// Where the VMs' dataset is, as the agent finds its mount unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmStorage {
    pub pool: PoolName,
    pub volume: VolumeNumber,
}

/// An ISO to copy for a VM: a file in a share. zendiloosd checked that the
/// administrator may read it; the copy opens it beneath the share's root,
/// never through a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmIsoSource {
    pub share: ShareLocation,
    pub path: SharePath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_twelve_lowercase_hex_digits_and_name_the_domain() {
        let id = VmId::parse("0a1b2c3d4e5f").unwrap();
        assert_eq!(id.domain(), "zendiloos-0a1b2c3d4e5f");
        assert_eq!(id.disk(), "0a1b2c3d4e5f-disk0");
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
        }
        assert_eq!(VmId::of_domain("win10"), None);
        assert_eq!(VmId::of_domain("zendiloos-win10"), None);
        assert!(serde_json::from_str::<VmId>("\"0a1b2c3d4e5f \"").is_err());
    }

    #[test]
    fn sizes_have_their_bounds() {
        assert!(VmCpus::new(0).is_err() && VmCpus::new(1).is_ok() && VmCpus::new(257).is_err());
        assert!(VmMemory::new(127).is_err() && VmMemory::new(128).is_ok());
        assert!(VmMemory::new(VmMemory::MAX_MIB + 1).is_err());
        assert!(VmDiskSize::new(0).is_err() && VmDiskSize::new(1).is_ok());
        assert!(VmDiskSize::new(VmDiskSize::MAX_GIB + 1).is_err());
        assert!(serde_json::from_str::<VmCpus>("0").is_err());
    }

    #[test]
    fn a_spec_takes_nothing_it_does_not_know() {
        let spec = r#"{"id":"0a1b2c3d4e5f","os":"linux","cpus":2,"memory":2048,"firmware":"uefi","disk":20,
            "network":{"kind":"user"},"iso":false,"autostart":false,"pool":"tank","volume":1}"#;
        let parsed: VmSpec = serde_json::from_str(spec).unwrap();
        assert_eq!(parsed.network, VmNetwork::User);
        let extra = spec.replace("\"iso\":false", "\"iso\":false,\"xml\":\"<domain/>\"");
        assert!(serde_json::from_str::<VmSpec>(&extra).is_err());
        let bridge = spec.replace(r#"{"kind":"user"}"#, r#"{"kind":"bridge","bridge":"br0;rm"}"#);
        assert!(serde_json::from_str::<VmSpec>(&bridge).is_err());
    }
}
