//! Debian packages through apt. apt writes all over the system and fetches
//! over the network, so the agent never runs it in its sandbox: it starts a
//! unit of its own (`zendiloos-packages.service`), which records each run
//! in a file the agent reads back.

use serde::{Deserialize, Serialize};

/// What apt is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageTask {
    /// `docker.io` and `docker-compose` from Debian, so apps can run.
    InstallDocker,
    /// `acme.sh` from Debian, for certificates from Let's Encrypt.
    InstallAcme,
    /// apt's lists fetched again, and a [`PackageReport`] of what an
    /// update would bring; nothing installed.
    Check,
    /// Debian's packages upgraded, then zendiloOS from a release when one is
    /// given, then a [`PackageReport`].
    Update,
}

impl PackageTask {
    const ALL: [Self; 4] = [Self::InstallDocker, Self::InstallAcme, Self::Check, Self::Update];

    /// The name the runner is started with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InstallDocker => "install-docker",
            Self::InstallAcme => "install-acme",
            Self::Check => "check",
            Self::Update => "update",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.as_str() == name)
    }
}

/// A release of zendiloOS for an update: the file name of its package,
/// `zendiloos_<version>_<arch>.deb`, in the folder zendiloosd downloads
/// to, beside `SHA256SUMS` and `SHA256SUMS.minisig`. The agent trusts none
/// of the three until the signature and the checksum match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReleasePackage(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the file name of a zendiloOS package")]
pub struct InvalidReleasePackage;

impl ReleasePackage {
    pub fn parse(name: &str) -> Result<Self, InvalidReleasePackage> {
        let inner = name.strip_prefix("zendiloos_").and_then(|rest| rest.strip_suffix(".deb"));
        let valid = inner.is_some_and(|inner| {
            inner.len() <= 160
                && inner.split_once('_').is_some_and(|(version, arch)| {
                    version.starts_with(|c: char| c.is_ascii_digit())
                        && version
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '~' | '-'))
                        && !arch.is_empty()
                        && arch.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                })
        });
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidReleasePackage) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The version in the name.
    pub fn version(&self) -> &str {
        let inner = &self.0["zendiloos_".len()..self.0.len() - ".deb".len()];
        inner.rsplit_once('_').map_or(inner, |(version, _)| version)
    }
}

impl TryFrom<String> for ReleasePackage {
    type Error = InvalidReleasePackage;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ReleasePackage> for String {
    fn from(package: ReleasePackage) -> Self {
        package.0
    }
}

/// A Debian package an upgrade would bring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradablePackage {
    pub name: String,
    /// None for one that comes in new, such as a kernel.
    pub from: Option<String>,
    pub to: String,
    /// From Debian's security archive.
    pub security: bool,
}

/// What apt and the kernel say after a check or an update.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageReport {
    pub upgradable: Vec<UpgradablePackage>,
    pub running_kernel: String,
    /// The newest kernel in `/lib/modules`.
    pub newest_kernel: Option<String>,
    /// The ZFS module is built for that kernel (DKMS).
    pub zfs_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageRunState {
    Running,
    Succeeded,
    Failed,
}

/// One run of apt, as its runner records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRun {
    /// Given when the run was started, to tell runs apart.
    pub id: String,
    pub task: PackageTask,
    pub state: PackageRunState,
    /// The step running, or the last one: `lists`, `install`.
    pub step: String,
    /// Unix seconds.
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// Why it failed, with apt's last lines.
    pub error: Option<String>,
    /// The version of zendiloOS an update installs.
    #[serde(default)]
    pub release: Option<String>,
    /// After a check or an update.
    #[serde(default)]
    pub report: Option<PackageReport>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_packages_are_plain_file_names() {
        let package = ReleasePackage::parse("zendiloos_0.2.0~rc.1_amd64.deb").unwrap();
        assert_eq!(package.version(), "0.2.0~rc.1");
        let dev = ReleasePackage::parse("zendiloos_0.2.0+dev.20261106.gabc1234_arm64.deb").unwrap();
        assert_eq!(dev.version(), "0.2.0+dev.20261106.gabc1234");
        for bad in [
            "zendiloos_0.2.0_amd64.deb/../x",
            "../zendiloos_0.2.0_amd64.deb",
            "zendiloos_0.2.0_amd64.tar",
            "other_0.2.0_amd64.deb",
            "zendiloos_v0.2.0_amd64.deb",
            "zendiloos_0.2 0_amd64.deb",
            "zendiloos_0.2.0_AMD64.deb",
        ] {
            assert!(ReleasePackage::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tasks_have_names_for_the_runner() {
        assert_eq!(PackageTask::parse("update"), Some(PackageTask::Update));
        assert_eq!(PackageTask::parse("install-docker"), Some(PackageTask::InstallDocker));
        assert_eq!(PackageTask::parse("install-acme"), Some(PackageTask::InstallAcme));
        assert_eq!(PackageTask::parse("install-anything"), None);
        let json = serde_json::to_string(&PackageTask::InstallDocker).unwrap();
        assert_eq!(json, "\"install_docker\"");
    }
}
