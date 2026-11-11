//! Virtual machines as administrators make and change them: a name, an OS
//! preset that fills in the rest, disks, network cards and CD drives with
//! images of the library, a boot order; settings held to what the machine
//! has, and hardware that changes only while a VM is off.

use std::collections::BTreeSet;

use zendiloos_kernel::Timestamp;

/// A rule a request broke, in words for the person who sent it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

fn invalid(text: impl Into<String>) -> Invalid {
    Invalid(text.into())
}

/// The longest name a VM is shown by.
pub const NAME_MAX: usize = 48;
/// The longest name an image of the library is shown by.
pub const IMAGE_NAME_MAX: usize = 128;
/// The least memory a VM gets: what the agent takes, and enough for a
/// firmware's menu.
pub const MEMORY_MIN_MIB: u32 = 128;
/// The largest disk: 64 TiB.
pub const DISK_MAX_GIB: u32 = 64 * 1024;
/// Virtual CPUs a VM gets at most, whatever the machine has.
pub const CPUS_MAX: u16 = 256;
/// Disks a VM has at most.
pub const DISKS_MAX: usize = 8;
/// Network cards a VM has at most.
pub const NICS_MAX: usize = 4;
/// CD drives a VM has at most: an installer, its drivers, and one more.
pub const DRIVES_MAX: usize = 3;
/// The largest image uploaded from a browser: 32 GiB.
pub const IMAGE_UPLOAD_MAX: u64 = 32 << 30;

/// A name as people type it: trimmed, 1 to `max` characters, no control
/// characters; `what` says what is named.
fn check_text(name: &str, max: usize, what: &str) -> Result<String, Invalid> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(format!("{what} needs a name")));
    }
    if name.chars().count() > max {
        return Err(invalid(format!("a name has at most {max} characters")));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a name holds no control characters"));
    }
    Ok(name.to_owned())
}

/// The name a VM is shown by: what the administrator typed, trimmed, 1 to
/// [`NAME_MAX`] characters, no control characters. It is kept by zendiloOS
/// only; libvirt knows the VM by its id.
pub fn check_name(name: &str) -> Result<String, Invalid> {
    check_text(name, NAME_MAX, "a virtual machine")
}

/// The name an image is shown by, 1 to [`IMAGE_NAME_MAX`] characters; its
/// file is named after its id.
pub fn check_image_name(name: &str) -> Result<String, Invalid> {
    check_text(name, IMAGE_NAME_MAX, "an image")
}

/// What a VM runs; it chooses the defaults, and the devices that are not
/// chosen one by one (video, memory balloon, clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsPreset {
    Linux,
    Windows,
    Other,
}

/// What a preset fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    pub cpus: u16,
    pub memory_mib: u32,
    pub disk_gib: u32,
    pub firmware: Firmware,
    pub disk_bus: DiskBus,
    pub nic_model: NicModel,
}

impl OsPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Linux, Self::Windows, Self::Other].into_iter().find(|preset| preset.as_str() == text)
    }

    /// Enough for a usual installation: a Linux server, Windows 10 or 11
    /// (64 GB as it asks for), and a small machine for anything else, with
    /// the BIOS older systems need. virtio for Linux; SATA and an e1000e
    /// card for the others, which may bring no virtio drivers.
    pub fn defaults(self) -> Defaults {
        let (disk_bus, nic_model) = match self {
            Self::Linux => (DiskBus::Virtio, NicModel::Virtio),
            Self::Windows | Self::Other => (DiskBus::Sata, NicModel::E1000e),
        };
        let (cpus, memory_mib, disk_gib, firmware) = match self {
            Self::Linux => (2, 2048, 20, Firmware::Uefi),
            Self::Windows => (2, 4096, 64, Firmware::Uefi),
            Self::Other => (1, 1024, 16, Firmware::Bios),
        };
        Defaults { cpus, memory_mib, disk_gib, firmware, disk_bus, nic_model }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firmware {
    /// OVMF, Secure Boot off.
    Uefi,
    /// SeaBIOS.
    Bios,
}

impl Firmware {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uefi => "uefi",
            Self::Bios => "bios",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Uefi, Self::Bios].into_iter().find(|firmware| firmware.as_str() == text)
    }
}

/// How a disk is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskBus {
    Virtio,
    Sata,
}

impl DiskBus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Virtio => "virtio",
            Self::Sata => "sata",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Virtio, Self::Sata].into_iter().find(|bus| bus.as_str() == text)
    }
}

/// The network card a VM sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicModel {
    Virtio,
    E1000e,
}

impl NicModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Virtio => "virtio",
            Self::E1000e => "e1000e",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Virtio, Self::E1000e].into_iter().find(|model| model.as_str() == text)
    }
}

/// How a network card reaches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// passt: out to the LAN and the internet from the NAS's address;
    /// nothing reaches the VM.
    User,
    /// A bridge the machine has: the VM is on the LAN as a machine of its own.
    Bridge(String),
}

/// A disk: a sparse zvol of `size_gib`, in its slot (0 to 7) for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    pub slot: u8,
    pub size_gib: u32,
    pub bus: DiskBus,
}

/// A network card in its slot (0 to 3). Its MAC address is given once,
/// when the card is added (empty until then), and stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nic {
    pub slot: u8,
    pub model: NicModel,
    pub network: Network,
    pub mac: String,
}

/// A CD drive in its slot (0 to 2), with an image of the library or empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    pub slot: u8,
    pub image: Option<String>,
}

/// A device a VM boots from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BootDevice {
    Disk(u8),
    Drive(u8),
}

impl BootDevice {
    /// `disk:0`, `drive:1`: how the boot order is kept.
    pub fn to_text(self) -> String {
        match self {
            Self::Disk(slot) => format!("disk:{slot}"),
            Self::Drive(slot) => format!("drive:{slot}"),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (kind, slot) = text.split_once(':')?;
        let slot = slot.parse().ok()?;
        match kind {
            "disk" => Some(Self::Disk(slot)),
            "drive" => Some(Self::Drive(slot)),
            _ => None,
        }
    }
}

/// What the machine has to give.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostLimits {
    /// Threads.
    pub cores: u32,
    pub memory_bytes: u64,
    /// Its own Linux bridges.
    pub bridges: Vec<String>,
}

/// What an administrator chose for a VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSettings {
    pub name: String,
    pub os: OsPreset,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: Firmware,
    pub disks: Vec<Disk>,
    pub nics: Vec<Nic>,
    pub drives: Vec<Drive>,
    /// The devices it boots from, first first; others it never boots from.
    pub boot: Vec<BootDevice>,
    pub autostart: bool,
}

/// Whether each of `items` has a key of its own.
fn unique<T, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> bool {
    let mut seen = BTreeSet::new();
    items.iter().all(|item| seen.insert(key(item)))
}

/// Whether `mac` is a MAC address as zendiloOS gives them: six pairs of
/// lowercase hexadecimal digits, unicast, not all zero.
pub fn is_mac(mac: &str) -> bool {
    let parts: Vec<&str> = mac.split(':').collect();
    let hex = parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
    hex && u8::from_str_radix(parts[0], 16).is_ok_and(|first| first & 1 == 0)
        && parts.iter().any(|part| *part != "00")
}

impl VmSettings {
    /// A preset's settings under `name`: one disk, one card through the
    /// NAS, one empty drive, booted from the drive first, no autostart.
    pub fn preset(name: &str, os: OsPreset) -> Self {
        let defaults = os.defaults();
        Self {
            name: name.to_owned(),
            os,
            cpus: defaults.cpus,
            memory_mib: defaults.memory_mib,
            firmware: defaults.firmware,
            disks: vec![Disk { slot: 0, size_gib: defaults.disk_gib, bus: defaults.disk_bus }],
            nics: vec![Nic {
                slot: 0,
                model: defaults.nic_model,
                network: Network::User,
                mac: String::new(),
            }],
            drives: vec![Drive { slot: 0, image: None }],
            boot: vec![BootDevice::Drive(0), BootDevice::Disk(0)],
            autostart: false,
        }
    }

    /// The images its drives hold.
    pub fn images(&self) -> impl Iterator<Item = &str> {
        self.drives.iter().filter_map(|drive| drive.image.as_deref())
    }

    /// The bridges its cards are on.
    pub fn bridges(&self) -> impl Iterator<Item = &str> {
        self.nics.iter().filter_map(|nic| match &nic.network {
            Network::Bridge(bridge) => Some(bridge.as_str()),
            Network::User => None,
        })
    }

    /// Checks the settings against the machine, the names other VMs have
    /// (compared without case) and the images of the library that are
    /// ready, and gives back the name as kept. A VM may have as many
    /// virtual CPUs as the machine has threads and as much memory as it has
    /// in all: it shares them with the NAS and other VMs, which the
    /// administrator weighs. Changing a VM (`before`), its disks only grow,
    /// and a card it had on a bridge the machine no longer has may stay.
    pub fn check(
        &self,
        host: &HostLimits,
        taken: &[&str],
        images: &[&str],
        before: Option<&VmSettings>,
    ) -> Result<String, Invalid> {
        let name = check_name(&self.name)?;
        if taken.iter().any(|other| other.to_lowercase() == name.to_lowercase()) {
            return Err(invalid(format!("another virtual machine is called {name}")));
        }
        let cores = u16::try_from(host.cores.max(1)).unwrap_or(CPUS_MAX).min(CPUS_MAX);
        if !(1..=cores).contains(&self.cpus) {
            return Err(invalid(format!("a virtual machine has 1 to {cores} virtual CPUs here")));
        }
        let total_mib = u32::try_from(host.memory_bytes / (1024 * 1024)).unwrap_or(u32::MAX);
        if self.memory_mib < MEMORY_MIN_MIB || self.memory_mib > total_mib {
            return Err(invalid(format!(
                "a virtual machine has {MEMORY_MIN_MIB} to {total_mib} MiB of memory here"
            )));
        }
        self.check_disks(before)?;
        self.check_nics(host, before)?;
        self.check_drives(images)?;
        let exists = |device: &BootDevice| match device {
            BootDevice::Disk(slot) => self.disks.iter().any(|disk| disk.slot == *slot),
            BootDevice::Drive(slot) => self.drives.iter().any(|drive| drive.slot == *slot),
        };
        if self.boot.is_empty() || !unique(&self.boot, |device| *device) || !self.boot.iter().all(exists) {
            return Err(invalid("the boot order names devices the virtual machine has, each once"));
        }
        Ok(name)
    }

    fn check_disks(&self, before: Option<&VmSettings>) -> Result<(), Invalid> {
        if self.disks.is_empty() || self.disks.len() > DISKS_MAX {
            return Err(invalid(format!("a virtual machine has 1 to {DISKS_MAX} disks")));
        }
        if !unique(&self.disks, |disk| disk.slot)
            || self.disks.iter().any(|disk| usize::from(disk.slot) >= DISKS_MAX)
        {
            return Err(invalid("each disk has a slot of its own, 0 to 7"));
        }
        if let Some(disk) = self.disks.iter().find(|disk| !(1..=DISK_MAX_GIB).contains(&disk.size_gib)) {
            return Err(invalid(format!("disk {} has 1 to {DISK_MAX_GIB} GiB", disk.slot)));
        }
        for disk in &self.disks {
            let had = before.and_then(|before| before.disks.iter().find(|old| old.slot == disk.slot));
            if let Some(had) = had
                && disk.size_gib < had.size_gib
            {
                return Err(invalid(format!("disk {} has {} GiB and only grows", disk.slot, had.size_gib)));
            }
        }
        Ok(())
    }

    fn check_nics(&self, host: &HostLimits, before: Option<&VmSettings>) -> Result<(), Invalid> {
        if self.nics.len() > NICS_MAX {
            return Err(invalid(format!("a virtual machine has at most {NICS_MAX} network cards")));
        }
        if !unique(&self.nics, |nic| nic.slot)
            || self.nics.iter().any(|nic| usize::from(nic.slot) >= NICS_MAX)
        {
            return Err(invalid("each network card has a slot of its own, 0 to 3"));
        }
        for nic in &self.nics {
            let Network::Bridge(bridge) = &nic.network else { continue };
            let kept = before.is_some_and(|before| {
                before.nics.iter().any(|old| old.slot == nic.slot && old.network == nic.network)
            });
            if !kept && !host.bridges.contains(bridge) {
                return Err(invalid(format!("{bridge} is no bridge of this machine")));
            }
        }
        let given: Vec<&Nic> = self.nics.iter().filter(|nic| !nic.mac.is_empty()).collect();
        if given.iter().any(|nic| !is_mac(&nic.mac)) || !unique(&given, |nic| nic.mac.clone()) {
            return Err(invalid("each network card has a MAC address of its own"));
        }
        Ok(())
    }

    fn check_drives(&self, images: &[&str]) -> Result<(), Invalid> {
        if self.drives.len() > DRIVES_MAX {
            return Err(invalid(format!("a virtual machine has at most {DRIVES_MAX} CD drives")));
        }
        if !unique(&self.drives, |drive| drive.slot)
            || self.drives.iter().any(|drive| usize::from(drive.slot) >= DRIVES_MAX)
        {
            return Err(invalid("each CD drive has a slot of its own, 0 to 2"));
        }
        if let Some(image) = self.images().find(|image| !images.contains(image)) {
            return Err(invalid(format!("the image {image} is not in the library")));
        }
        Ok(())
    }

    /// Whether the hardware differs from `other`'s: everything but the
    /// name, autostart and the images in the drives, which change while a
    /// VM runs.
    pub fn hardware_differs(&self, other: &VmSettings) -> bool {
        let slots = |drives: &[Drive]| drives.iter().map(|drive| drive.slot).collect::<BTreeSet<_>>();
        self.os != other.os
            || self.cpus != other.cpus
            || self.memory_mib != other.memory_mib
            || self.firmware != other.firmware
            || self.disks != other.disks
            || self.nics != other.nics
            || slots(&self.drives) != slots(&other.drives)
            || self.boot != other.boot
    }

    /// The drives, of those it had `before`, whose image is another now.
    pub fn changed_media(&self, before: &VmSettings) -> Vec<u8> {
        self.drives
            .iter()
            .filter(|drive| {
                before.drives.iter().any(|old| old.slot == drive.slot && old.image != drive.image)
            })
            .map(|drive| drive.slot)
            .collect()
    }

    /// The disks it had `before` and has no longer.
    pub fn removed_disks(&self, before: &VmSettings) -> Vec<u8> {
        before
            .disks
            .iter()
            .filter(|old| !self.disks.iter().any(|disk| disk.slot == old.slot))
            .map(|old| old.slot)
            .collect()
    }

    /// Gives each card without a MAC address one: the one the card in its
    /// slot had `before`, else a new one from `fresh` that no card of any
    /// VM (`taken`) has.
    pub fn give_macs(&mut self, before: &[Nic], taken: &BTreeSet<String>, mut fresh: impl FnMut() -> String) {
        let mut used: BTreeSet<String> = taken.clone();
        used.extend(self.nics.iter().filter(|nic| !nic.mac.is_empty()).map(|nic| nic.mac.clone()));
        for nic in &mut self.nics {
            if !nic.mac.is_empty() {
                continue;
            }
            if let Some(old) = before.iter().find(|old| old.slot == nic.slot) {
                nic.mac = old.mac.clone();
                continue;
            }
            // A clash is all but impossible; a few tries make it so.
            let mut mac = fresh();
            for _ in 0..16 {
                if !used.contains(&mac) {
                    break;
                }
                mac = fresh();
            }
            used.insert(mac.clone());
            nic.mac = mac;
        }
    }
}

/// An ISO in a share, as an administrator picked it: `share/folder/file.iso`
/// in the interface, here the share's id and the path in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoChoice {
    pub share_id: i64,
    pub path: String,
}

impl IsoChoice {
    /// Only a file whose name ends in `.iso`, and a path that stays in the
    /// share; the file manager checks the rest.
    pub fn check(&self) -> Result<(), Invalid> {
        let path = self.path.trim_matches('/');
        let file = self.file();
        if path.is_empty() || path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(invalid("an ISO is a file in a share"));
        }
        if !file.to_lowercase().ends_with(".iso") || file.len() <= 4 {
            return Err(invalid("only an .iso file goes into the library"));
        }
        Ok(())
    }

    /// The file's name: what the image is called unless another name is given.
    pub fn file(&self) -> &str {
        self.path.trim_matches('/').rsplit('/').next().unwrap_or_default()
    }
}

/// Where an image came from, kept to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageOrigin {
    /// Copied from a file in a share.
    Share { share: String, path: String },
    /// Uploaded from a browser; the file's name then.
    Upload { file: String },
}

/// An ISO in the library: kept once, attached to as many VMs as want it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmImage {
    /// Twelve lowercase hexadecimal digits: its file is `<id>.iso`.
    pub id: String,
    pub name: String,
    /// Its size; unknown for an ISO a VM had before the library, until
    /// looked at.
    pub bytes: Option<u64>,
    pub origin: ImageOrigin,
    /// The volume the VMs' storage, and so its file, is on.
    pub volume: u16,
    pub added_at: Timestamp,
    /// In the library: false while it is being copied or uploaded.
    pub ready: bool,
}

/// A virtual machine as zendiloOS keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vm {
    /// Twelve lowercase hexadecimal digits; libvirt's domain is
    /// `zendiloos-<id>`.
    pub id: String,
    pub settings: VmSettings,
    /// The pool its disks are on, with the VMs' storage.
    pub pool: String,
    pub volume: u16,
    pub created_at: Timestamp,
    /// Defined in libvirt: false while it is being made.
    pub ready: bool,
}

/// Whether an id is one zendiloOS makes, of a VM or an image.
pub fn is_vm_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What libvirt says a VM does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Running,
    Paused,
    ShuttingDown,
    ShutOff,
    Crashed,
    /// Suspended by the guest itself.
    Suspended,
    /// Being made: not defined in libvirt yet.
    Creating,
    /// libvirt cannot be asked, or does not know it.
    Unknown,
}

impl PowerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::ShuttingDown => "shutting_down",
            Self::ShutOff => "shut_off",
            Self::Crashed => "crashed",
            Self::Suspended => "suspended",
            Self::Creating => "creating",
            Self::Unknown => "unknown",
        }
    }

    /// As `virsh` words it: `running`, `shut off`, `in shutdown`, …
    pub fn from_virsh(text: &str) -> Self {
        match text.trim() {
            "running" | "idle" | "blocked" => Self::Running,
            "paused" => Self::Paused,
            "in shutdown" => Self::ShuttingDown,
            "shut off" => Self::ShutOff,
            "crashed" | "dying" => Self::Crashed,
            "pmsuspended" => Self::Suspended,
            _ => Self::Unknown,
        }
    }

    /// Whether QEMU runs, and so the console answers.
    pub fn is_on(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::ShuttingDown | Self::Suspended)
    }
}

/// What a VM is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmAction {
    Start,
    Shutdown,
    ForceOff,
    Restart,
}

impl VmAction {
    /// The audit log's word.
    pub fn audit(self) -> &'static str {
        match self {
            Self::Start => "vms.start",
            Self::Shutdown => "vms.shutdown",
            Self::ForceOff => "vms.force_off",
            Self::Restart => "vms.restart",
        }
    }
}

/// What libvirt counts of a running VM, as `virsh domstats` tells it: what
/// it does not tell is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmCounters {
    /// CPU time of all its vCPUs, in nanoseconds.
    pub cpu_time_ns: Option<u64>,
    /// Memory in KiB: what it was given, what the guest sees in all, can
    /// use without swapping and has not used at all (these three only with
    /// a balloon driver), and what QEMU holds of the host's.
    pub memory_current_kib: Option<u64>,
    pub memory_available_kib: Option<u64>,
    pub memory_usable_kib: Option<u64>,
    pub memory_unused_kib: Option<u64>,
    pub memory_rss_kib: Option<u64>,
    /// Bytes its cards received and sent, as the guest sees them. A card
    /// through passt has no device on the host, and so no counters.
    pub received_bytes: Option<u64>,
    pub sent_bytes: Option<u64>,
}

impl VmCounters {
    /// The memory the VM uses: as its guest says (in all, less what it can
    /// use without swapping, caches being such), else what QEMU holds of
    /// the host's, at most what it was given.
    pub fn memory_used_bytes(&self) -> Option<u64> {
        let guest = match (self.memory_available_kib, self.memory_usable_kib, self.memory_unused_kib) {
            (Some(available), Some(usable), _) => Some(available.saturating_sub(usable)),
            (Some(available), None, Some(unused)) => Some(available.saturating_sub(unused)),
            _ => None,
        };
        let held =
            self.memory_rss_kib.map(|rss| self.memory_current_kib.map_or(rss, |current| rss.min(current)));
        guest.or(held).map(|kib| kib.saturating_mul(1024))
    }
}

/// What a running VM uses, from two looks at its counters.
#[derive(Debug, Clone, PartialEq)]
pub struct VmLoad {
    pub id: String,
    /// Busy share of its vCPUs, 0 to 100; none on the first look.
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    /// What it was given.
    pub memory_bytes: u64,
    pub received_bytes_per_second: Option<u64>,
    pub sent_bytes_per_second: Option<u64>,
}

/// A rate from two counters `seconds` apart; none if either is missing or
/// the counter started again (the VM restarted).
fn rate(before: Option<u64>, now: Option<u64>, seconds: f64) -> Option<f64> {
    let (before, now) = (before?, now?);
    (now >= before && seconds > 0.0).then(|| (now - before) as f64 / seconds)
}

impl VmLoad {
    /// The load of a VM with `cpus` vCPUs and `memory_mib`, from `now`'s
    /// counters and those `seconds` before.
    pub fn of(
        id: &str,
        cpus: u16,
        memory_mib: u32,
        before: Option<&VmCounters>,
        now: &VmCounters,
        seconds: f64,
    ) -> Self {
        let per_second = |pick: fn(&VmCounters) -> Option<u64>| {
            rate(before.and_then(pick), pick(now), seconds).map(|rate| rate.round() as u64)
        };
        let cpu = rate(before.and_then(|b| b.cpu_time_ns), now.cpu_time_ns, seconds).map(|ns_per_second| {
            (ns_per_second / 1e9 / f64::from(cpus.max(1)) * 100.0).clamp(0.0, 100.0) as f32
        });
        Self {
            id: id.to_owned(),
            cpu_percent: cpu,
            memory_used_bytes: now.memory_used_bytes(),
            memory_bytes: u64::from(memory_mib) << 20,
            received_bytes_per_second: per_second(|c| c.received_bytes),
            sent_bytes_per_second: per_second(|c| c.sent_bytes),
        }
    }
}

/// What the VMs use now, every few seconds while someone watches.
#[derive(Debug, Clone, PartialEq)]
pub struct VmStatsSample {
    pub at: Timestamp,
    /// Each running VM's load.
    pub vms: Vec<VmLoad>,
    pub running: usize,
    pub total: usize,
    /// The share of the machine's threads the VMs keep busy, 0 to 100.
    pub cpu_percent: f32,
    /// What the running VMs were given, against what the machine has.
    pub memory_assigned_bytes: u64,
    pub host_memory_bytes: u64,
    pub host_cores: u32,
}

impl VmStatsSample {
    /// The machine's share the loads add up to: each VM's busy vCPUs
    /// against the machine's threads.
    pub fn host_cpu_percent(loads: &[(u16, &VmLoad)], cores: u32) -> f32 {
        let busy: f32 = loads
            .iter()
            .filter_map(|(cpus, load)| load.cpu_percent.map(|percent| percent / 100.0 * f32::from(*cpus)))
            .sum();
        (busy / cores.max(1) as f32 * 100.0).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostLimits {
        HostLimits { cores: 2, memory_bytes: 2 << 30, bridges: vec!["br0".into()] }
    }

    fn small() -> VmSettings {
        VmSettings {
            cpus: 1,
            memory_mib: 256,
            disks: vec![Disk { slot: 0, size_gib: 1, bus: DiskBus::Virtio }],
            ..VmSettings::preset("test", OsPreset::Linux)
        }
    }

    fn too(settings: VmSettings, says: &str) {
        let error = settings.check(&host(), &[], &["00000000000a"], None).unwrap_err();
        assert!(error.0.contains(says), "{error} does not say {says}");
    }

    #[test]
    fn names_are_trimmed_bounded_and_plain() {
        assert_eq!(check_name("  Debian test ").unwrap(), "Debian test");
        assert_eq!(check_name("Büro-PC (Windows 11)").unwrap(), "Büro-PC (Windows 11)");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(NAME_MAX + 1)).is_err());
        assert!(check_name(&"ä".repeat(NAME_MAX)).is_ok(), "characters, not bytes");
        assert!(check_name("a\nb").is_err() && check_name("a\u{7}").is_err());
        assert_eq!(
            check_image_name(" debian-13.1.0-amd64-netinst.iso ").unwrap(),
            "debian-13.1.0-amd64-netinst.iso"
        );
        assert!(check_image_name(&"x".repeat(IMAGE_NAME_MAX)).is_ok());
        assert!(check_image_name(&"x".repeat(IMAGE_NAME_MAX + 1)).is_err());
        assert!(check_image_name("").unwrap_err().0.contains("an image needs a name"));
    }

    #[test]
    fn presets_fill_in_what_their_system_needs() {
        let linux = OsPreset::Linux.defaults();
        assert_eq!(
            (linux.cpus, linux.memory_mib, linux.disk_gib, linux.firmware),
            (2, 2048, 20, Firmware::Uefi)
        );
        assert_eq!((linux.disk_bus, linux.nic_model), (DiskBus::Virtio, NicModel::Virtio));
        let windows = OsPreset::Windows.defaults();
        assert_eq!((windows.memory_mib, windows.disk_gib), (4096, 64));
        assert_eq!((windows.disk_bus, windows.nic_model), (DiskBus::Sata, NicModel::E1000e));
        assert_eq!(OsPreset::Other.defaults().firmware, Firmware::Bios);
        for preset in [OsPreset::Linux, OsPreset::Windows, OsPreset::Other] {
            assert_eq!(OsPreset::parse(preset.as_str()), Some(preset));
        }
        assert_eq!(OsPreset::parse("macos"), None);
        assert_eq!(Firmware::parse("uefi"), Some(Firmware::Uefi));
        assert_eq!(Firmware::parse("coreboot"), None);
        assert_eq!((DiskBus::parse("sata"), DiskBus::parse("ide")), (Some(DiskBus::Sata), None));
        assert_eq!((NicModel::parse("e1000e"), NicModel::parse("rtl8139")), (Some(NicModel::E1000e), None));
        let preset = VmSettings::preset("x", OsPreset::Windows);
        assert_eq!((preset.disks.len(), preset.nics.len(), preset.drives.len()), (1, 1, 1));
        assert_eq!(preset.boot, [BootDevice::Drive(0), BootDevice::Disk(0)]);
        for device in [BootDevice::Disk(7), BootDevice::Drive(2)] {
            assert_eq!(BootDevice::parse(&device.to_text()), Some(device));
        }
        assert_eq!(BootDevice::parse("nic:0"), None);
    }

    #[test]
    fn settings_are_held_to_the_machine() {
        let ok = small();
        assert_eq!(ok.check(&host(), &[], &[], None).unwrap(), "test");
        too(VmSettings { cpus: 3, ..ok.clone() }, "1 to 2 virtual CPUs");
        too(VmSettings { cpus: 0, ..ok.clone() }, "1 to 2 virtual CPUs");
        too(VmSettings { memory_mib: 2049, ..ok.clone() }, "128 to 2048 MiB");
        too(VmSettings { memory_mib: 64, ..ok.clone() }, "128 to 2048 MiB");
        let taken = ok.check(&host(), &["Other", "TEST"], &[], None).unwrap_err();
        assert!(taken.0.contains("another virtual machine is called test"), "{taken}");
        // The whole machine is allowed: the administrator weighs it.
        assert!(VmSettings { cpus: 2, memory_mib: 2048, ..ok }.check(&host(), &[], &[], None).is_ok());
    }

    #[test]
    fn disks_have_slots_of_their_own_and_sizes_within_bounds() {
        let disk = |slot, size_gib| Disk { slot, size_gib, bus: DiskBus::Sata };
        let with = |disks: Vec<Disk>| VmSettings { disks, ..small() };
        too(with(vec![]), "1 to 8 disks");
        too(with((0..9).map(|slot| disk(slot, 1)).collect()), "1 to 8 disks");
        too(with(vec![disk(0, 1), disk(0, 2)]), "slot of its own");
        too(with(vec![disk(8, 1)]), "slot of its own");
        too(with(vec![disk(0, 0)]), "disk 0 has 1 to 65536 GiB");
        too(with(vec![disk(0, DISK_MAX_GIB + 1)]), "GiB");
        let eight = VmSettings {
            boot: vec![BootDevice::Disk(7)],
            ..with((0..8).map(|slot| disk(slot, 1)).collect())
        };
        assert!(eight.check(&host(), &[], &[], None).is_ok());
    }

    #[test]
    fn cards_are_on_bridges_the_machine_has_with_macs_of_their_own() {
        let nic = |slot, network, mac: &str| Nic { slot, model: NicModel::Virtio, network, mac: mac.into() };
        let with = |nics: Vec<Nic>| VmSettings { nics, ..small() };
        assert!(with(vec![]).check(&host(), &[], &[], None).is_ok(), "a VM without network");
        too(with((0..5).map(|slot| nic(slot, Network::User, "")).collect()), "at most 4 network cards");
        too(with(vec![nic(0, Network::User, ""), nic(0, Network::User, "")]), "slot of its own");
        too(with(vec![nic(4, Network::User, "")]), "slot of its own");
        too(with(vec![nic(0, Network::Bridge("br1".into()), "")]), "br1 is no bridge");
        too(
            with(vec![
                nic(0, Network::User, "52:54:00:00:00:01"),
                nic(1, Network::User, "52:54:00:00:00:01"),
            ]),
            "MAC",
        );
        too(with(vec![nic(0, Network::User, "01:00:5e:00:00:01")]), "MAC");
        too(with(vec![nic(0, Network::User, "52:54:00:00:00:0G")]), "MAC");
        let bridged = with(vec![nic(0, Network::Bridge("br0".into()), "52:54:00:00:00:01")]);
        assert!(bridged.check(&host(), &[], &[], None).is_ok());
        // A bridge that went since stays with the card that was on it.
        let gone = with(vec![nic(1, Network::Bridge("br9".into()), "52:54:00:00:00:02")]);
        assert!(gone.check(&host(), &[], &[], Some(&gone)).is_ok());
        let moved = with(vec![nic(0, Network::Bridge("br9".into()), "52:54:00:00:00:02")]);
        assert!(moved.check(&host(), &[], &[], Some(&gone)).is_err(), "not onto it");
        assert!(is_mac("52:54:00:ab:cd:ef") && !is_mac("52:54:00:AB:CD:EF") && !is_mac("00:00:00:00:00:00"));
    }

    #[test]
    fn drives_hold_images_of_the_library_and_the_boot_order_its_devices() {
        let drive = |slot, image: Option<&str>| Drive { slot, image: image.map(Into::into) };
        let with = |drives: Vec<Drive>| VmSettings { drives, boot: vec![BootDevice::Disk(0)], ..small() };
        assert!(with(vec![]).check(&host(), &[], &[], None).is_ok());
        let three =
            with(vec![drive(0, Some("00000000000a")), drive(1, None), drive(2, Some("00000000000a"))]);
        assert!(three.check(&host(), &[], &["00000000000a"], None).is_ok());
        too(with((0..4).map(|slot| drive(slot, None)).collect()), "at most 3 CD drives");
        too(with(vec![drive(3, None)]), "slot of its own");
        too(with(vec![drive(0, Some("00000000000b"))]), "the image 00000000000b is not in the library");
        let boot = |boot: Vec<BootDevice>| VmSettings { boot, ..small() };
        too(boot(vec![]), "boot order");
        too(boot(vec![BootDevice::Disk(0), BootDevice::Disk(0)]), "boot order");
        too(boot(vec![BootDevice::Disk(1)]), "boot order");
        too(boot(vec![BootDevice::Drive(2)]), "boot order");
        assert!(boot(vec![BootDevice::Disk(0)]).check(&host(), &[], &[], None).is_ok());
    }

    #[test]
    fn a_change_tells_hardware_from_what_changes_while_a_vm_runs() {
        let before = small();
        let renamed = VmSettings { name: "other".into(), autostart: true, ..before.clone() };
        assert!(!renamed.hardware_differs(&before));
        let inserted = VmSettings {
            drives: vec![Drive { slot: 0, image: Some("00000000000a".into()) }],
            ..before.clone()
        };
        assert!(!inserted.hardware_differs(&before), "a medium is no hardware");
        assert_eq!(inserted.changed_media(&before), [0]);
        assert!(inserted.changed_media(&inserted).is_empty());
        for changed in [
            VmSettings { cpus: 2, ..before.clone() },
            VmSettings { memory_mib: 512, ..before.clone() },
            VmSettings { firmware: Firmware::Bios, ..before.clone() },
            VmSettings { drives: vec![], boot: vec![BootDevice::Disk(0)], ..before.clone() },
            VmSettings { boot: vec![BootDevice::Disk(0), BootDevice::Drive(0)], ..before.clone() },
            VmSettings { nics: vec![], ..before.clone() },
            VmSettings { disks: vec![Disk { slot: 0, size_gib: 2, bus: DiskBus::Virtio }], ..before.clone() },
        ] {
            assert!(changed.hardware_differs(&before), "{changed:?}");
        }
        let grown = VmSettings {
            disks: vec![
                Disk { slot: 0, size_gib: 2, bus: DiskBus::Virtio },
                Disk { slot: 3, size_gib: 5, bus: DiskBus::Sata },
            ],
            ..before.clone()
        };
        assert!(grown.check(&host(), &[], &[], Some(&before)).is_ok());
        assert!(grown.removed_disks(&before).is_empty());
        let shrunk = VmSettings {
            disks: vec![Disk { slot: 3, size_gib: 4, bus: DiskBus::Sata }],
            boot: vec![BootDevice::Disk(3)],
            ..before.clone()
        };
        let error = shrunk.check(&host(), &[], &[], Some(&grown)).unwrap_err();
        assert!(error.0.contains("disk 3 has 5 GiB and only grows"), "{error}");
        assert_eq!(shrunk.removed_disks(&grown), [0]);
    }

    #[test]
    fn a_card_keeps_its_mac_and_a_new_one_gets_one_no_card_has() {
        let nic =
            |slot, mac: &str| Nic { slot, model: NicModel::Virtio, network: Network::User, mac: mac.into() };
        let before = [nic(0, "52:54:00:00:00:01")];
        let mut settings =
            VmSettings { nics: vec![nic(0, ""), nic(1, ""), nic(2, "52:54:00:00:00:09")], ..small() };
        let taken: BTreeSet<String> = ["52:54:00:00:00:02".to_owned()].into();
        let mut fresh =
            ["52:54:00:00:00:02", "52:54:00:00:00:09", "52:54:00:00:00:03"].into_iter().map(str::to_owned);
        settings.give_macs(&before, &taken, || fresh.next().unwrap());
        let macs: Vec<&str> = settings.nics.iter().map(|nic| nic.mac.as_str()).collect();
        assert_eq!(macs, ["52:54:00:00:00:01", "52:54:00:00:00:03", "52:54:00:00:00:09"]);
    }

    #[test]
    fn only_iso_files_in_a_share_go_into_the_library() {
        let iso = |path: &str| IsoChoice { share_id: 1, path: path.into() };
        assert!(iso("isos/debian-13.1.0-amd64-netinst.iso").check().is_ok());
        assert_eq!(iso("/isos/debian.iso/").file(), "debian.iso");
        assert!(iso("Win11_24H2_German_x64.ISO").check().is_ok());
        for bad in
            ["", "isos", "isos/readme.txt", "../etc/x.iso", "isos/../x.iso", "a//b.iso", ".iso", "isos/.iso"]
        {
            assert!(iso(bad).check().is_err(), "{bad}");
        }
    }

    #[test]
    fn virsh_words_become_states() {
        assert_eq!(PowerState::from_virsh("running"), PowerState::Running);
        assert_eq!(PowerState::from_virsh("shut off"), PowerState::ShutOff);
        assert_eq!(PowerState::from_virsh("in shutdown"), PowerState::ShuttingDown);
        assert_eq!(PowerState::from_virsh("pmsuspended"), PowerState::Suspended);
        assert_eq!(PowerState::from_virsh("whatever"), PowerState::Unknown);
        assert!(PowerState::Running.is_on() && !PowerState::ShutOff.is_on() && !PowerState::Creating.is_on());
        assert!(is_vm_id("0a1b2c3d4e5f") && !is_vm_id("0A1B2C3D4E5F") && !is_vm_id("zendiloos-1"));
    }

    #[test]
    fn loads_come_from_two_looks_at_the_counters() {
        let at = |cpu: u64, rx: u64| VmCounters {
            cpu_time_ns: Some(cpu),
            memory_current_kib: Some(1 << 20),
            memory_available_kib: Some(1_000_000),
            memory_usable_kib: Some(600_000),
            memory_rss_kib: Some(900_000),
            received_bytes: Some(rx),
            sent_bytes: Some(rx / 2),
            ..VmCounters::default()
        };
        let first = VmLoad::of("a", 2, 1024, None, &at(10_000_000_000, 1000), 3.0);
        assert_eq!(first.cpu_percent, None, "a rate needs two looks");
        assert_eq!(first.memory_used_bytes, Some(400_000 * 1024));
        assert_eq!(first.memory_bytes, 1 << 30);
        // 3 s later: 3 s of CPU time over 2 vCPUs, 3000 bytes in.
        let load = VmLoad::of("a", 2, 1024, Some(&at(10_000_000_000, 1000)), &at(13_000_000_000, 4000), 3.0);
        assert_eq!(load.cpu_percent, Some(50.0));
        assert_eq!((load.received_bytes_per_second, load.sent_bytes_per_second), (Some(1000), Some(500)));
        // Started again: counters from zero say nothing.
        let restarted = VmLoad::of("a", 2, 1024, Some(&at(13_000_000_000, 4000)), &at(1_000, 10), 3.0);
        assert_eq!((restarted.cpu_percent, restarted.received_bytes_per_second), (None, None));
        // Without a balloon driver: what QEMU holds, at most what it was given.
        let windows = VmCounters {
            memory_current_kib: Some(1000),
            memory_rss_kib: Some(1200),
            ..VmCounters::default()
        };
        assert_eq!(windows.memory_used_bytes(), Some(1000 * 1024));
        assert_eq!(VmCounters::default().memory_used_bytes(), None);
        let unused = VmCounters {
            memory_available_kib: Some(100),
            memory_unused_kib: Some(40),
            ..VmCounters::default()
        };
        assert_eq!(unused.memory_used_bytes(), Some(60 * 1024));
        let a = VmLoad { cpu_percent: Some(50.0), ..first.clone() };
        let b = VmLoad { cpu_percent: Some(100.0), ..first };
        assert_eq!(VmStatsSample::host_cpu_percent(&[(2, &a), (1, &b)], 4), 50.0);
        assert_eq!(VmStatsSample::host_cpu_percent(&[], 0), 0.0);
    }
}
