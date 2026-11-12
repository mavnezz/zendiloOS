//! Backups of a VM: its settings as zendiloOS keeps them, its disks read
//! from a snapshot as compressed raw images, its UEFI variables and its
//! TPM's state, in a folder per VM and backup (`<folder>/<id>/<stem>`) in a
//! share or on a removable drive, with a manifest. A backup is restored as
//! a new VM, or over the VM it came from while that is shut off.

use zendiloos_kernel::Timestamp;

use super::snapshots::Consistency;
use super::{Invalid, VmSettings, invalid};

/// Where a backup goes, as an administrator picks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupTarget {
    Share {
        share_id: i64,
    },
    /// A removable drive, by the id it is mounted under.
    Drive {
        drive: String,
    },
}

/// Where a backup is, found: a share by its name and volume, or a drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupPlace {
    Share { share_id: i64, share: String, volume: u16 },
    Drive { drive: String },
}

impl BackupPlace {
    /// How it is shown: the share's name, or the drive's id.
    pub fn shown(&self) -> &str {
        match self {
            Self::Share { share, .. } => share,
            Self::Drive { drive } => drive,
        }
    }
}

/// What a file of a backup holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BackupPart {
    /// A disk's compressed raw image, `volsize` bytes once decompressed.
    Disk { slot: u8, volsize: u64 },
    /// The UEFI variables.
    Nvram,
    /// The TPM's state.
    Tpm,
}

/// A file a backup wrote, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    pub part: BackupPart,
    pub bytes: u64,
    /// 64 lowercase hex digits.
    pub sha256: String,
}

/// A backup as zendiloOS records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmBackup {
    pub id: i64,
    /// The VM it came from; it may be gone since.
    pub vm: String,
    pub vm_name: String,
    pub place: BackupPlace,
    /// Its folder beneath the share's or drive's root.
    pub folder: String,
    pub taken_at: Timestamp,
    pub consistency: Consistency,
    pub files: Vec<BackupFile>,
    /// The VM's settings when it was backed up.
    pub settings: VmSettings,
}

impl VmBackup {
    /// What its files hold in all.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

/// A new backup before it is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVmBackup {
    pub vm: String,
    pub vm_name: String,
    pub place: BackupPlace,
    pub folder: String,
    pub taken_at: Timestamp,
    pub consistency: Consistency,
    pub files: Vec<BackupFile>,
    pub settings: VmSettings,
}

/// A backup's folder name: its time, `YYYYMMDD-HHMMSS` in UTC.
pub fn backup_stem(at: Timestamp) -> String {
    let t = at.utc();
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// The folder backups go into, beneath the share's or drive's root: names
/// separated by `/`, none empty, `.` or `..`, at most 255 characters each
/// and 1024 in all, without control characters; the root for an empty one.
pub fn backup_folder(text: &str) -> Result<String, Invalid> {
    let path = text.trim().trim_matches('/');
    if path.is_empty() {
        return Ok(String::new());
    }
    let fine = path.len() <= 1024
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name.len() <= 255
                && !name.chars().any(char::is_control)
        });
    if fine {
        Ok(path.to_owned())
    } else {
        Err(invalid("a backup's folder is a folder of the share or drive"))
    }
}

/// How a backup is restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreAs {
    /// A new VM of `name`, its disks on the location of `pool` (the first
    /// when empty).
    New { name: String, pool: String },
    /// Over the VM it came from, which is shut off; `confirm` is its name,
    /// typed again.
    Over { confirm: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backups_folder_and_name() {
        assert_eq!(backup_stem(Timestamp::from_unix_seconds(1_793_836_800 + 3 * 3600)), "20261105-030000");
        assert_eq!(backup_folder(" /backups/vms/ ").unwrap(), "backups/vms");
        assert_eq!(backup_folder("").unwrap(), "");
        for bad in ["a/../b", "./x", "a//b", "a\nb"] {
            assert!(backup_folder(bad).is_err(), "{bad}");
        }
        assert!(backup_folder(&"x".repeat(256)).is_err());
        let place = BackupPlace::Share { share_id: 1, share: "media".into(), volume: 1 };
        assert_eq!(place.shown(), "media");
        assert_eq!(BackupPlace::Drive { drive: "1234-ABCD".into() }.shown(), "1234-ABCD");
    }
}
