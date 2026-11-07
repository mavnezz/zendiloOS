//! Removable drives: USB sticks and disks, memory cards. They are named by
//! the UUID of their file system, which is also the name of the place they
//! are mounted at, `/run/zendiloos-removable/<id>`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Where removable drives are mounted, one folder per drive: root's,
/// outside the volumes, so no share, Samba or NFS export ever reaches one;
/// on a tmpfs, so a place left from before a restart is gone with it.
pub const REMOVABLE_PLACE: &str = "/run/zendiloos-removable";

/// The UUID of a removable drive's file system, as `lsblk` and
/// `/dev/disk/by-uuid` name it: `1A2B-3C4D` (FAT, exFAT),
/// `0123456789ABCDEF` (NTFS), `2f6b…-…` (ext4). Letters, digits and `-`,
/// not starting with `-`, at most 64. It names the drive's place too.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RemovableId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the UUID of a removable drive's file system")]
pub struct InvalidRemovableId;

impl RemovableId {
    pub fn parse(value: &str) -> Result<Self, InvalidRemovableId> {
        let valid = (1..=64).contains(&value.len())
            && !value.starts_with('-')
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidRemovableId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `/run/zendiloos-removable/<id>`.
    pub fn place(&self) -> String {
        format!("{REMOVABLE_PLACE}/{}", self.0)
    }
}

impl fmt::Display for RemovableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RemovableId {
    type Error = InvalidRemovableId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RemovableId> for String {
    fn from(id: RemovableId) -> Self {
        id.0
    }
}

/// The file systems a removable drive may carry; anything else is left
/// alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovableFilesystem {
    /// FAT12, FAT16 or FAT32 (`vfat`).
    Fat,
    Exfat,
    /// Through the kernel's `ntfs3`.
    Ntfs,
    Ext4,
}

impl RemovableFilesystem {
    /// From the signature `lsblk` reports (`FSTYPE`).
    pub fn from_signature(signature: &str) -> Option<Self> {
        match signature {
            "vfat" => Some(Self::Fat),
            "exfat" => Some(Self::Exfat),
            "ntfs" => Some(Self::Ntfs),
            "ext4" => Some(Self::Ext4),
            _ => None,
        }
    }

    /// The kernel's name for it, as `mount -t` takes it.
    pub fn kernel_type(self) -> &'static str {
        match self {
            Self::Fat => "vfat",
            Self::Exfat => "exfat",
            Self::Ntfs => "ntfs3",
            Self::Ext4 => "ext4",
        }
    }
}

/// Signatures of a device that belongs to something bigger: a disk with
/// one of them anywhere is never a removable drive.
pub const MEMBER_SIGNATURES: [&str; 6] =
    ["zfs_member", "linux_raid_member", "LVM2_member", "crypto_LUKS", "bcache", "ddf_raid_member"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_file_system_uuids_and_nothing_a_path_could_hide_in() {
        for id in ["1A2B-3C4D", "0123456789ABCDEF", "2f6b0c4e-8d1a-4f7e-9c3b-5a6d7e8f9a0b"] {
            assert_eq!(RemovableId::parse(id).unwrap().place(), format!("/run/zendiloos-removable/{id}"));
        }
        for id in ["", "-o", "../x", "a/b", "a b", "a.b", &"a".repeat(65)] {
            assert!(RemovableId::parse(id).is_err(), "{id:?}");
        }
        let decoded: Result<RemovableId, _> = serde_json::from_str("\"..\"");
        assert!(decoded.is_err());
    }

    #[test]
    fn file_systems_by_signature() {
        assert_eq!(RemovableFilesystem::from_signature("vfat"), Some(RemovableFilesystem::Fat));
        assert_eq!(RemovableFilesystem::from_signature("ntfs").map(|f| f.kernel_type()), Some("ntfs3"));
        for other in ["btrfs", "xfs", "zfs_member", "iso9660", "swap", "ext3"] {
            assert_eq!(RemovableFilesystem::from_signature(other), None, "{other}");
        }
    }
}
