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
    /// Ends the open SMB connections of a share whose access was taken
    /// away, and has every smbd read the configuration again first.
    pub smbcontrol: PathBuf,
    pub useradd: PathBuf,
    pub userdel: PathBuf,
    pub groupadd: PathBuf,
    pub groupdel: PathBuf,
    pub gpasswd: PathBuf,
    pub smbpasswd: PathBuf,
    pub busctl: PathBuf,
    /// Run in a transient unit, in the host's network namespace: deletes
    /// the link of a bridge, bond or VLAN zendiloOS made.
    pub ip: PathBuf,
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
    /// Starts a terminal's program as the leader of its own session
    /// (util-linux): `docker exec` for a terminal in a container.
    pub setsid: PathBuf,
    /// Reads when an issued certificate ends.
    pub openssl: PathBuf,
    /// Checks the web server's settings (`nginx -t`), in a transient unit.
    pub nginx: PathBuf,
    /// Run in a transient unit as acme.sh's account, to remove its files.
    pub rm: PathBuf,
    /// Checks the firewall's rules (`nft -c`), in a transient unit; the
    /// unit zendiloos-firewall.service puts them in force.
    pub nft: PathBuf,
    /// libvirt's client, for virtual machines; may be missing until the
    /// plugin is installed, as the two below.
    pub virsh: PathBuf,
    /// Only looked for: libvirt's daemon and the QEMU it runs.
    pub libvirtd: PathBuf,
    pub qemu: PathBuf,
    /// Run in a transient unit: makes the folder of the VMs' consoles.
    pub systemd_tmpfiles: PathBuf,
    /// The backup plugin's: rsync and the SSH client run in a backup's
    /// unit, ssh-keygen makes the NAS's key, ssh-keyscan reads a server's;
    /// all may be missing until the plugin is installed.
    pub rsync: PathBuf,
    pub ssh: PathBuf,
    pub ssh_keygen: PathBuf,
    pub ssh_keyscan: PathBuf,
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
            smbcontrol: PathBuf::from("/usr/bin/smbcontrol"),
            useradd: PathBuf::from("/usr/sbin/useradd"),
            userdel: PathBuf::from("/usr/sbin/userdel"),
            groupadd: PathBuf::from("/usr/sbin/groupadd"),
            groupdel: PathBuf::from("/usr/sbin/groupdel"),
            gpasswd: PathBuf::from("/usr/bin/gpasswd"),
            smbpasswd: PathBuf::from("/usr/bin/smbpasswd"),
            busctl: PathBuf::from("/usr/bin/busctl"),
            ip: PathBuf::from("/usr/sbin/ip"),
            timedatectl: PathBuf::from("/usr/bin/timedatectl"),
            sshd: PathBuf::from("/usr/sbin/sshd"),
            systemd_run: PathBuf::from("/usr/bin/systemd-run"),
            systemd_mount: PathBuf::from("/usr/bin/systemd-mount"),
            umount: PathBuf::from("/usr/bin/umount"),
            rmdir: PathBuf::from("/usr/bin/rmdir"),
            docker: PathBuf::from("/usr/bin/docker"),
            dockerd: PathBuf::from("/usr/sbin/dockerd"),
            setsid: PathBuf::from("/usr/bin/setsid"),
            openssl: PathBuf::from("/usr/bin/openssl"),
            nginx: PathBuf::from("/usr/sbin/nginx"),
            rm: PathBuf::from("/usr/bin/rm"),
            nft: PathBuf::from("/usr/sbin/nft"),
            virsh: PathBuf::from("/usr/bin/virsh"),
            libvirtd: PathBuf::from("/usr/sbin/libvirtd"),
            qemu: PathBuf::from("/usr/bin/qemu-system-x86_64"),
            systemd_tmpfiles: PathBuf::from("/usr/bin/systemd-tmpfiles"),
            rsync: PathBuf::from("/usr/bin/rsync"),
            ssh: PathBuf::from("/usr/bin/ssh"),
            ssh_keygen: PathBuf::from("/usr/bin/ssh-keygen"),
            ssh_keyscan: PathBuf::from("/usr/bin/ssh-keyscan"),
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
