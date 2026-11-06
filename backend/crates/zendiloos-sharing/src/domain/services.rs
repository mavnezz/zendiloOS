//! Which file services the NAS offers, and how Samba presents itself to
//! Windows. One set for all shares.

/// The oldest SMB version answered; SMB1 never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmbProtocol {
    /// SMB 2.0: Windows Vista, old printers and scanners.
    Smb2,
    /// SMB 2.1: Windows 7.
    Smb2_1,
    /// SMB 3: Windows 8 and every current system.
    Smb3,
}

impl SmbProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            SmbProtocol::Smb2 => "smb2",
            SmbProtocol::Smb2_1 => "smb2_1",
            SmbProtocol::Smb3 => "smb3",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [SmbProtocol::Smb2, SmbProtocol::Smb2_1, SmbProtocol::Smb3].into_iter().find(|p| p.as_str() == value)
    }
}

/// Whether SMB traffic is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmbEncryption {
    /// As each client asks.
    Default,
    /// Always; clients that cannot encrypt cannot connect.
    Required,
    Off,
}

/// Whether SMB packets are signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmbSigning {
    /// As each client asks.
    Default,
    Mandatory,
    Disabled,
}

macro_rules! named {
    ($type:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $type {
            pub fn as_str(self) -> &'static str {
                match self { $($type::$variant => $name),+ }
            }

            pub fn parse(value: &str) -> Option<Self> {
                [$($type::$variant),+].into_iter().find(|v| v.as_str() == value)
            }
        }
    };
}

named!(SmbEncryption { Default => "default", Required => "required", Off => "off" });
named!(SmbSigning { Default => "default", Mandatory => "mandatory", Disabled => "disabled" });

/// The share Macs back up to with Time Machine, and how much it may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeMachine {
    pub share: super::ShareId,
    /// `None`: as much as the share may hold.
    pub max_bytes: Option<u64>,
}

/// The longest pattern of files kept out of the shares.
pub const MAX_VETO_CHARS: usize = 512;

/// The file services as set; until someone changes them, what a fresh
/// install offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileServices {
    pub smb: bool,
    pub nfs: bool,
    /// The Windows workgroup, uppercase.
    pub workgroup: String,
    pub smb_min_protocol: SmbProtocol,
    pub smb_encryption: SmbEncryption,
    pub smb_signing: SmbSigning,
    /// Shares a user may not open are not listed to them.
    pub hide_unreadable: bool,
    /// SMB 3 over several connections at once.
    pub multichannel: bool,
    /// Clients may cache files they have open (oplocks), and SMB 2 leases.
    pub oplocks: bool,
    pub leases: bool,
    /// Open files survive a short network loss; the file system no longer
    /// sees SMB's locks, so they do not hold against NFS or the file
    /// manager.
    pub durable_handles: bool,
    /// Translation of characters macOS allows in names but Windows does
    /// not, and Apple's extensions (Samba's `catia` and `fruit`); needed for
    /// Time Machine.
    pub mac_compat: bool,
    /// Symbolic links inside a share are followed; never out of it.
    pub follow_symlinks: bool,
    /// Samba's `veto files`: `/*.tmp/Thumbs.db/` keeps such names out.
    pub veto_files: Option<String>,
    /// Folders holding only such files can be deleted.
    pub delete_veto_files: bool,
    /// Windows finds the NAS (WS-Discovery, wsdd2).
    pub wsdd: bool,
    /// Macs find the NAS and its Time Machine share (Bonjour, Avahi).
    pub bonjour: bool,
    pub time_machine: Option<TimeMachine>,
}

impl Default for FileServices {
    /// As `/etc/samba/smb.conf` of the package sets it, Samba's defaults
    /// otherwise; shares nobody may open are hidden.
    fn default() -> Self {
        Self {
            smb: true,
            nfs: true,
            workgroup: "WORKGROUP".to_owned(),
            smb_min_protocol: SmbProtocol::Smb2_1,
            smb_encryption: SmbEncryption::Default,
            smb_signing: SmbSigning::Default,
            hide_unreadable: true,
            multichannel: true,
            oplocks: true,
            leases: true,
            durable_handles: false,
            mac_compat: true,
            follow_symlinks: true,
            veto_files: None,
            delete_veto_files: false,
            wsdd: true,
            bonjour: true,
            time_machine: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidFileServices {
    #[error("a workgroup is 1 to 15 letters, digits, - or _, not starting with -")]
    Workgroup,
    #[error(
        "files to keep out are written /pattern/pattern/, at most {MAX_VETO_CHARS} characters, without line breaks"
    )]
    VetoFiles,
    #[error("Time Machine needs the macOS compatibility of SMB")]
    TimeMachineNeedsMac,
    #[error("a Time Machine share holds at least 1 GiB")]
    TimeMachineTooSmall,
}

impl FileServices {
    /// The settings tidied (workgroup uppercase, an empty pattern none,
    /// leases only with oplocks), if they hold up. Whether the Time Machine
    /// share exists is for the caller to check.
    pub fn checked(mut self) -> Result<Self, InvalidFileServices> {
        self.workgroup = self.workgroup.trim().to_ascii_uppercase();
        let valid = (1..=15).contains(&self.workgroup.len())
            && !self.workgroup.starts_with('-')
            && self
                .workgroup
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b"-_".contains(&b));
        if !valid {
            return Err(InvalidFileServices::Workgroup);
        }
        self.veto_files = self.veto_files.map(|pattern| pattern.trim().to_owned()).filter(|p| !p.is_empty());
        if let Some(pattern) = &self.veto_files {
            let valid = pattern.len() >= 3
                && pattern.chars().count() <= MAX_VETO_CHARS
                && pattern.starts_with('/')
                && pattern.ends_with('/')
                && !pattern.contains("//")
                && !pattern.chars().any(|c| c.is_control() || c == '\\' || c == '%');
            if !valid {
                return Err(InvalidFileServices::VetoFiles);
            }
        }
        if !self.oplocks {
            self.leases = false;
        }
        if let Some(time_machine) = self.time_machine {
            if !self.mac_compat {
                return Err(InvalidFileServices::TimeMachineNeedsMac);
            }
            if time_machine.max_bytes.is_some_and(|bytes| bytes < super::MIN_QUOTA_BYTES) {
                return Err(InvalidFileServices::TimeMachineTooSmall);
            }
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_time_machine_and_leases_are_checked() {
        let with = |change: fn(&mut FileServices)| {
            let mut services = FileServices::default();
            change(&mut services);
            services.checked()
        };
        assert!(with(|s| s.veto_files = Some("/*.tmp/Thumbs.db/".into())).is_ok());
        assert_eq!(with(|s| s.veto_files = Some("  ".into())).unwrap().veto_files, None);
        for bad in ["*.tmp", "/a//b/", "/x\ny/", "/a\\b/", "/%U/"] {
            let services = FileServices { veto_files: Some(bad.into()), ..FileServices::default() };
            assert_eq!(services.checked(), Err(InvalidFileServices::VetoFiles), "{bad:?}");
        }
        assert!(!with(|s| s.oplocks = false).unwrap().leases, "leases need oplocks");
        let time_machine = Some(TimeMachine { share: crate::domain::ShareId::new(1), max_bytes: Some(1024) });
        let mut services = FileServices { time_machine, ..FileServices::default() };
        assert_eq!(services.clone().checked(), Err(InvalidFileServices::TimeMachineTooSmall));
        services.mac_compat = false;
        assert_eq!(services.checked(), Err(InvalidFileServices::TimeMachineNeedsMac));
    }

    fn with(workgroup: &str) -> Result<FileServices, InvalidFileServices> {
        FileServices { workgroup: workgroup.to_owned(), ..FileServices::default() }.checked()
    }

    #[test]
    fn workgroups_are_tidied_and_checked() {
        assert_eq!(with(" home ").unwrap().workgroup, "HOME");
        assert_eq!(with("lab_2").unwrap().workgroup, "LAB_2");
        for bad in ["", "TOO-LONG-FOR-NETBIOS", "a b", "-x", "x=y", "Küche"] {
            assert_eq!(with(bad), Err(InvalidFileServices::Workgroup), "{bad:?}");
        }
    }

    #[test]
    fn protocols_are_stored_by_name() {
        for protocol in [SmbProtocol::Smb2, SmbProtocol::Smb2_1, SmbProtocol::Smb3] {
            assert_eq!(SmbProtocol::parse(protocol.as_str()), Some(protocol));
        }
        assert_eq!(SmbProtocol::parse("smb1"), None);
        assert_eq!(SmbEncryption::parse("required"), Some(SmbEncryption::Required));
        assert_eq!(SmbSigning::parse("mandatory"), Some(SmbSigning::Mandatory));
    }
}
