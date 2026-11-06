//! Which file services run, and Samba's settings for all shares. Checked
//! while reading, like everything that reaches a configuration file.

use serde::{Deserialize, Serialize};

/// Whether SMB and NFS are offered, and how Samba presents itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileServices {
    /// Samba.
    pub smb: bool,
    /// The kernel's NFS server.
    pub nfs: bool,
    pub workgroup: Workgroup,
    /// The oldest SMB version Samba answers.
    pub smb_min_protocol: SmbProtocol,
    pub smb_encryption: SmbEncryption,
    pub smb_signing: SmbSigning,
    /// `access based share enum`: shares a user may not open are not listed.
    pub hide_unreadable: bool,
    pub multichannel: bool,
    pub oplocks: bool,
    /// SMB 2 leases; only with oplocks.
    pub leases: bool,
    /// Durable handles, which need Samba's locks kept from the kernel.
    pub durable_handles: bool,
    /// `catia` and `fruit` in every share's VFS modules.
    pub mac_compat: bool,
    pub follow_symlinks: bool,
    pub veto_files: Option<VetoFiles>,
    pub delete_veto_files: bool,
    /// WS-Discovery (wsdd2), while SMB is on.
    pub wsdd: bool,
    /// Bonjour (Avahi), and Samba announcing itself through it.
    pub bonjour: bool,
}

/// Whether SMB traffic is encrypted: as clients ask, always, or never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmbEncryption {
    Default,
    Required,
    Off,
}

impl SmbEncryption {
    /// The value of `server smb encrypt`.
    pub fn samba_name(self) -> &'static str {
        match self {
            SmbEncryption::Default => "default",
            SmbEncryption::Required => "required",
            SmbEncryption::Off => "off",
        }
    }
}

/// Whether SMB packets are signed: as clients ask, always, or never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmbSigning {
    Default,
    Mandatory,
    Disabled,
}

impl SmbSigning {
    /// The value of `server signing`.
    pub fn samba_name(self) -> &'static str {
        match self {
            SmbSigning::Default => "default",
            SmbSigning::Mandatory => "mandatory",
            SmbSigning::Disabled => "disabled",
        }
    }
}

/// Samba's `veto files`: `/*.tmp/Thumbs.db/`. Between slashes, names with
/// `*` and `?`; nothing that could end the line or the parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VetoFiles(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a pattern of names such as /*.tmp/Thumbs.db/")]
pub struct InvalidVetoFiles;

impl VetoFiles {
    pub fn parse(pattern: &str) -> Result<Self, InvalidVetoFiles> {
        let valid = (3..=512).contains(&pattern.chars().count())
            && pattern.starts_with('/')
            && pattern.ends_with('/')
            && !pattern.contains("//")
            && !pattern.chars().any(|c| c.is_control() || c == '\\' || c == '%');
        if valid { Ok(Self(pattern.to_owned())) } else { Err(InvalidVetoFiles) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for VetoFiles {
    type Error = InvalidVetoFiles;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<VetoFiles> for String {
    fn from(pattern: VetoFiles) -> Self {
        pattern.0
    }
}

/// A Windows workgroup: 1 to 15 uppercase letters, digits, `-` or `_`, as
/// NetBIOS allows and nothing a configuration line could misread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Workgroup(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a workgroup is 1 to 15 uppercase letters, digits, - or _")]
pub struct InvalidWorkgroup;

impl Workgroup {
    pub fn parse(name: &str) -> Result<Self, InvalidWorkgroup> {
        let valid = (1..=15).contains(&name.len())
            && !name.starts_with('-')
            && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidWorkgroup) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Workgroup {
    type Error = InvalidWorkgroup;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Workgroup> for String {
    fn from(workgroup: Workgroup) -> Self {
        workgroup.0
    }
}

/// The oldest SMB version answered. SMB1 is not among them: it is unsafe,
/// and Samba turned it off long ago.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmbProtocol {
    /// SMB 2.0: Windows Vista, old printers and scanners.
    Smb2,
    /// SMB 2.1: Windows 7.
    Smb2_1,
    /// SMB 3: Windows 8 and every current system; encryption.
    Smb3,
}

impl SmbProtocol {
    /// The value of Samba's `server min protocol`.
    pub fn samba_name(self) -> &'static str {
        match self {
            SmbProtocol::Smb2 => "SMB2_02",
            SmbProtocol::Smb2_1 => "SMB2_10",
            SmbProtocol::Smb3 => "SMB3",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workgroups_are_netbios_names() {
        for name in ["WORKGROUP", "HOME", "MY-NET", "LAB_2", "A"] {
            assert!(Workgroup::parse(name).is_ok(), "{name}");
        }
        for name in ["", "workgroup", "TOO-LONG-FOR-NETBIOS", "A B", "-X", "X\n[x]", "X=Y", "ÄÖÜ"] {
            assert!(Workgroup::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn protocols_are_read_by_name() {
        let services: FileServices = serde_json::from_str(
            r#"{"smb":true,"nfs":false,"workgroup":"HOME","smb_min_protocol":"smb2_1","smb_encryption":"default",
                "smb_signing":"mandatory","hide_unreadable":true,"multichannel":true,"oplocks":true,"leases":true,
                "durable_handles":false,"mac_compat":true,"follow_symlinks":true,"veto_files":"/*.tmp/",
                "delete_veto_files":false,"wsdd":true,"bonjour":false}"#,
        )
        .unwrap();
        assert_eq!(services.smb_min_protocol.samba_name(), "SMB2_10");
        assert!(serde_json::from_str::<SmbProtocol>(r#""smb1""#).is_err());
        assert_eq!(services.smb_signing.samba_name(), "mandatory");
        for bad in ["*.tmp", "/a//b/", "/x\ny/", "/a\\b/", "/%U/"] {
            assert!(VetoFiles::parse(bad).is_err(), "{bad:?}");
        }
        assert!(VetoFiles::parse("/*.tmp/Thumbs.db/.DS_Store/").is_ok());
    }
}
