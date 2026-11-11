//! Virtual machines as administrators make them: a name, an OS preset that
//! fills in the rest, and settings held to what the machine has.

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
/// The least memory a VM gets: what the agent takes, and enough for a
/// firmware's menu.
pub const MEMORY_MIN_MIB: u32 = 128;
/// The largest system disk: 64 TiB.
pub const DISK_MAX_GIB: u32 = 64 * 1024;
/// Virtual CPUs a VM gets at most, whatever the machine has.
pub const CPUS_MAX: u16 = 256;

/// The name a VM is shown by: what the administrator typed, trimmed, 1 to
/// [`NAME_MAX`] characters, no control characters. It is kept by zendiloOS
/// only; libvirt knows the VM by its id.
pub fn check_name(name: &str) -> Result<String, Invalid> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a virtual machine needs a name"));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(format!("a name has at most {NAME_MAX} characters")));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a name holds no control characters"));
    }
    Ok(name.to_owned())
}

/// What a VM runs; it chooses the defaults and the devices (virtio for
/// Linux, SATA and an e1000e card for Windows and anything else, which may
/// bring no virtio drivers).
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
    /// the BIOS older systems need.
    pub fn defaults(self) -> Defaults {
        match self {
            Self::Linux => Defaults { cpus: 2, memory_mib: 2048, disk_gib: 20, firmware: Firmware::Uefi },
            Self::Windows => Defaults { cpus: 2, memory_mib: 4096, disk_gib: 64, firmware: Firmware::Uefi },
            Self::Other => Defaults { cpus: 1, memory_mib: 1024, disk_gib: 16, firmware: Firmware::Bios },
        }
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

/// How a VM reaches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// passt: out to the LAN and the internet from the NAS's address;
    /// nothing reaches the VM.
    User,
    /// A bridge the machine has: the VM is on the LAN as a machine of its own.
    Bridge(String),
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
    pub disk_gib: u32,
    pub network: Network,
    pub autostart: bool,
}

impl VmSettings {
    /// A preset's settings under `name`, the user-mode network, no autostart.
    pub fn preset(name: &str, os: OsPreset) -> Self {
        let defaults = os.defaults();
        Self {
            name: name.to_owned(),
            os,
            cpus: defaults.cpus,
            memory_mib: defaults.memory_mib,
            firmware: defaults.firmware,
            disk_gib: defaults.disk_gib,
            network: Network::User,
            autostart: false,
        }
    }

    /// Checks the settings against the machine and the names other VMs
    /// have (compared without case), and gives back the name as kept. A VM
    /// may have as many virtual CPUs as the machine has threads and as much
    /// memory as it has in all: it shares them with the NAS and other VMs,
    /// which the administrator weighs.
    pub fn check(&self, host: &HostLimits, taken: &[&str]) -> Result<String, Invalid> {
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
        if !(1..=DISK_MAX_GIB).contains(&self.disk_gib) {
            return Err(invalid(format!("a disk has 1 to {DISK_MAX_GIB} GiB")));
        }
        if let Network::Bridge(bridge) = &self.network
            && !host.bridges.contains(bridge)
        {
            return Err(invalid(format!("{bridge} is no bridge of this machine")));
        }
        Ok(name)
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
        let file = path.rsplit('/').next().unwrap_or_default();
        if path.is_empty() || path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(invalid("an ISO is a file in a share"));
        }
        if !file.to_lowercase().ends_with(".iso") || file.len() <= 4 {
            return Err(invalid("only an .iso file can be the VM's installation medium"));
        }
        Ok(())
    }
}

/// Where an ISO came from, kept to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoOrigin {
    pub share: String,
    pub path: String,
}

/// A virtual machine as zendiloOS keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vm {
    /// Twelve lowercase hexadecimal digits; libvirt's domain is
    /// `zendiloos-<id>`.
    pub id: String,
    pub settings: VmSettings,
    pub iso: Option<IsoOrigin>,
    /// The pool its disk is on, with the VMs' storage.
    pub pool: String,
    pub volume: u16,
    pub created_at: Timestamp,
    /// Defined in libvirt: false while it is being made.
    pub ready: bool,
}

/// Whether an id is one zendiloOS makes.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostLimits {
        HostLimits { cores: 2, memory_bytes: 2 << 30, bridges: vec!["br0".into()] }
    }

    #[test]
    fn names_are_trimmed_bounded_and_plain() {
        assert_eq!(check_name("  Debian test ").unwrap(), "Debian test");
        assert_eq!(check_name("Büro-PC (Windows 11)").unwrap(), "Büro-PC (Windows 11)");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(NAME_MAX + 1)).is_err());
        assert!(check_name(&"ä".repeat(NAME_MAX)).is_ok(), "characters, not bytes");
        assert!(check_name("a\nb").is_err() && check_name("a\u{7}").is_err());
    }

    #[test]
    fn presets_fill_in_what_their_system_needs() {
        assert_eq!(
            OsPreset::Linux.defaults(),
            Defaults { cpus: 2, memory_mib: 2048, disk_gib: 20, firmware: Firmware::Uefi }
        );
        assert_eq!(OsPreset::Windows.defaults().memory_mib, 4096);
        assert_eq!(OsPreset::Windows.defaults().disk_gib, 64);
        assert_eq!(OsPreset::Other.defaults().firmware, Firmware::Bios);
        for preset in [OsPreset::Linux, OsPreset::Windows, OsPreset::Other] {
            assert_eq!(OsPreset::parse(preset.as_str()), Some(preset));
        }
        assert_eq!(OsPreset::parse("macos"), None);
        assert_eq!(Firmware::parse("uefi"), Some(Firmware::Uefi));
        assert_eq!(Firmware::parse("coreboot"), None);
    }

    #[test]
    fn settings_are_held_to_the_machine() {
        let ok = VmSettings {
            cpus: 1,
            memory_mib: 256,
            disk_gib: 1,
            ..VmSettings::preset("test", OsPreset::Linux)
        };
        assert_eq!(ok.check(&host(), &[]).unwrap(), "test");
        let too = |settings: VmSettings, says: &str| {
            let error = settings.check(&host(), &[]).unwrap_err();
            assert!(error.0.contains(says), "{error} does not say {says}");
        };
        too(VmSettings { cpus: 3, ..ok.clone() }, "1 to 2 virtual CPUs");
        too(VmSettings { cpus: 0, ..ok.clone() }, "1 to 2 virtual CPUs");
        too(VmSettings { memory_mib: 2049, ..ok.clone() }, "128 to 2048 MiB");
        too(VmSettings { memory_mib: 64, ..ok.clone() }, "128 to 2048 MiB");
        too(VmSettings { disk_gib: 0, ..ok.clone() }, "1 to 65536 GiB");
        too(VmSettings { disk_gib: DISK_MAX_GIB + 1, ..ok.clone() }, "GiB");
        too(VmSettings { network: Network::Bridge("br1".into()), ..ok.clone() }, "br1 is no bridge");
        assert!(
            VmSettings { network: Network::Bridge("br0".into()), ..ok.clone() }.check(&host(), &[]).is_ok()
        );
        let taken = ok.check(&host(), &["Other", "TEST"]).unwrap_err();
        assert!(taken.0.contains("another virtual machine is called test"), "{taken}");
        // The whole machine is allowed: the administrator weighs it.
        assert!(VmSettings { cpus: 2, memory_mib: 2048, ..ok }.check(&host(), &[]).is_ok());
    }

    #[test]
    fn only_iso_files_in_a_share_are_installation_media() {
        let iso = |path: &str| IsoChoice { share_id: 1, path: path.into() };
        assert!(iso("isos/debian-13.1.0-amd64-netinst.iso").check().is_ok());
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
}
