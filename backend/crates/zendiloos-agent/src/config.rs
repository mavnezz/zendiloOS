//! Static configuration of the agent, `/etc/zendiloos/agent.toml`.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

use crate::pools::Erasable;

pub const DEFAULT_PATH: &str = "/etc/zendiloos/agent.toml";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Socket to create when not started through systemd socket activation.
    pub socket: PathBuf,
    /// The only account allowed to connect: the one zendiloosd runs as.
    pub client_user: String,
    /// `tracing` filter directive; `RUST_LOG` takes precedence.
    pub log_filter: String,
    pub tools: Tools,
    pub storage: StorageConfig,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// Which disks may be erased for pools: `"unused"` (any disk that passes
    /// the checks), `"none"`, or a list of `/dev/disk/by-id` names.
    pub erasable: Erasable,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket: PathBuf::from("/run/zendiloos/agent.sock"),
            client_user: "zendiloos".to_owned(),
            log_filter: "info".to_owned(),
            tools: Tools::default(),
            storage: StorageConfig::default(),
        }
    }
}

/// Absolute paths of the programs the agent may run. Nothing else is run.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    pub systemctl: PathBuf,
    pub lsblk: PathBuf,
    pub smartctl: PathBuf,
    pub zpool: PathBuf,
    pub zfs: PathBuf,
    pub sgdisk: PathBuf,
    pub wipefs: PathBuf,
    pub udevadm: PathBuf,
    pub testparm: PathBuf,
    pub exportfs: PathBuf,
    pub setfacl: PathBuf,
    pub useradd: PathBuf,
    pub userdel: PathBuf,
    pub groupadd: PathBuf,
    pub groupdel: PathBuf,
    pub gpasswd: PathBuf,
    pub smbpasswd: PathBuf,
    pub busctl: PathBuf,
    pub timedatectl: PathBuf,
    /// May be missing: then the SSH server is not installed.
    pub sshd: PathBuf,
    pub systemd_run: PathBuf,
    /// Mounts removable drives in transient units.
    pub systemd_mount: PathBuf,
    /// Run in a transient unit, to unmount removable drives.
    pub umount: PathBuf,
    /// Run in a transient unit, to remove a removable drive's place.
    pub rmdir: PathBuf,
    /// May be missing: then Docker is not installed.
    pub docker: PathBuf,
    /// Docker's daemon, from the `docker.io` package; the client above
    /// comes in a package of its own (`docker-cli`) and may stay without it.
    pub dockerd: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            systemctl: PathBuf::from("/usr/bin/systemctl"),
            lsblk: PathBuf::from("/usr/bin/lsblk"),
            smartctl: PathBuf::from("/usr/sbin/smartctl"),
            zpool: PathBuf::from("/usr/sbin/zpool"),
            zfs: PathBuf::from("/usr/sbin/zfs"),
            sgdisk: PathBuf::from("/usr/sbin/sgdisk"),
            wipefs: PathBuf::from("/usr/sbin/wipefs"),
            udevadm: PathBuf::from("/usr/bin/udevadm"),
            testparm: PathBuf::from("/usr/bin/testparm"),
            exportfs: PathBuf::from("/usr/sbin/exportfs"),
            setfacl: PathBuf::from("/usr/bin/setfacl"),
            useradd: PathBuf::from("/usr/sbin/useradd"),
            userdel: PathBuf::from("/usr/sbin/userdel"),
            groupadd: PathBuf::from("/usr/sbin/groupadd"),
            groupdel: PathBuf::from("/usr/sbin/groupdel"),
            gpasswd: PathBuf::from("/usr/bin/gpasswd"),
            smbpasswd: PathBuf::from("/usr/bin/smbpasswd"),
            busctl: PathBuf::from("/usr/bin/busctl"),
            timedatectl: PathBuf::from("/usr/bin/timedatectl"),
            sshd: PathBuf::from("/usr/sbin/sshd"),
            systemd_run: PathBuf::from("/usr/bin/systemd-run"),
            systemd_mount: PathBuf::from("/usr/bin/systemd-mount"),
            umount: PathBuf::from("/usr/bin/umount"),
            rmdir: PathBuf::from("/usr/bin/rmdir"),
            docker: PathBuf::from("/usr/bin/docker"),
            dockerd: PathBuf::from("/usr/sbin/dockerd"),
        }
    }
}

impl Config {
    /// Loads `explicit` if given (it must exist), otherwise [`DEFAULT_PATH`]
    /// if it exists, otherwise the defaults.
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let path = explicit.unwrap_or(Path::new(DEFAULT_PATH));
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid {}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound && explicit.is_none() => {
                Ok(Self::default())
            }
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_all_fields_and_rejects_unknown_ones() {
        let config: Config = toml::from_str(
            r#"
            socket = "/tmp/agent.sock"
            client_user = "tester"
            log_filter = "debug"

            [tools]
            systemctl = "/bin/systemctl"
            smartctl = "/sbin/smartctl"

            [storage]
            erasable = ["ata-TEST_DISK_1", "ata-TEST_DISK_2"]
            "#,
        )
        .unwrap();
        assert_eq!(config.client_user, "tester");
        assert_eq!(config.tools.systemctl, Path::new("/bin/systemctl"));
        assert_eq!(config.tools.smartctl, Path::new("/sbin/smartctl"));
        assert_eq!(config.tools.lsblk, Path::new("/usr/bin/lsblk"));
        assert_eq!(
            config.storage.erasable,
            Erasable::Only(vec!["ata-TEST_DISK_1".to_owned(), "ata-TEST_DISK_2".to_owned()])
        );
        let none: Config = toml::from_str("[storage]\nerasable = \"none\"").unwrap();
        assert_eq!(none.storage.erasable, Erasable::Policy(crate::pools::ErasePolicy::None));
        assert!(toml::from_str::<Config>("[storage]\nerasable = \"all\"").is_err());
        assert!(toml::from_str::<Config>("[tools]\nsh = \"/bin/sh\"").is_err());
    }

    #[test]
    fn a_file_given_must_be_there_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("agent.toml");
        let missing = Config::load(Some(&file)).unwrap_err();
        assert!(missing.to_string().contains("cannot read"), "{missing}");
        std::fs::write(&file, "client_user = \"tester\"\n").unwrap();
        assert_eq!(Config::load(Some(&file)).unwrap().client_user, "tester");
        std::fs::write(&file, "colour = \"red\"\n").unwrap();
        let invalid = Config::load(Some(&file)).unwrap_err();
        assert!(invalid.to_string().contains("invalid"), "{invalid}");
    }

    #[test]
    fn without_a_file_the_defaults_hold() {
        // On a machine with zendiloOS installed the file is there and read.
        if !Path::new(DEFAULT_PATH).exists() {
            let config = Config::load(None).unwrap();
            assert_eq!(config.tools.systemctl, Tools::default().systemctl);
        }
    }
}
