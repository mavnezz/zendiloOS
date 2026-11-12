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
    /// libvirt's daemon with only its QEMU driver, `virsh`, QEMU, OVMF and
    /// passt from Debian, for virtual machines; without recommended
    /// packages, so libvirt's own network (a bridge, dnsmasq and firewall
    /// rules) and its network filters never come along. On amd64 only: the
    /// runner refuses it elsewhere.
    InstallVms,
    /// rsync, the SSH client and rclone from Debian, for backup tasks;
    /// without recommended packages.
    InstallBackup,
    /// `systemd-resolved` from Debian, for taking the network over from
    /// ifupdown (systemd-networkd comes with `systemd`). The network stays
    /// as it was: installing it switches `/etc/resolv.conf` to it, which
    /// the runner takes back; the takeover switches it on.
    InstallNetworkd,
    /// apt's lists fetched again, and a [`PackageReport`] of what an
    /// update would bring; nothing installed.
    Check,
    /// Debian's packages upgraded, then zendiloOS from a release when one is
    /// given, then a [`PackageReport`].
    Update,
    /// What an update cut off left half done, finished: `dpkg --configure
    /// -a`, then `apt-get --fix-broken install`, then a [`PackageReport`].
    Finish,
}

impl PackageTask {
    const ALL: [Self; 8] = [
        Self::InstallDocker,
        Self::InstallAcme,
        Self::InstallVms,
        Self::InstallBackup,
        Self::InstallNetworkd,
        Self::Check,
        Self::Update,
        Self::Finish,
    ];

    /// The name the runner is started with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InstallDocker => "install-docker",
            Self::InstallAcme => "install-acme",
            Self::InstallVms => "install-vms",
            Self::InstallBackup => "install-backup",
            Self::InstallNetworkd => "install-networkd",
            Self::Check => "check",
            Self::Update => "update",
            Self::Finish => "finish",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.as_str() == name)
    }

    /// Whether it updates the system, finishing an update among it: the
    /// runner keeps the record of the last such run apart, which the runs
    /// after it do not replace.
    pub fn updates(self) -> bool {
        matches!(self, Self::Update | Self::Finish)
    }
}

/// Which builds of zendiloOS a NAS takes, its update channel; each takes
/// those of the steadier ones too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    /// Releases: `1.2.0`.
    Stable,
    /// Release candidates and betas: `1.2.0~rc.1`.
    Test,
    /// Builds of a commit on `main`: `1.2.0+dev.<time>.g<commit>`.
    Development,
}

impl ReleaseChannel {
    const ALL: [Self; 3] = [Self::Stable, Self::Test, Self::Development];

    /// The name the runner is started with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Test => "test",
            Self::Development => "development",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|channel| channel.as_str() == name)
    }

    /// Whether a build of `kind` may be installed on this channel.
    pub fn takes(self, kind: Self) -> bool {
        kind <= self
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

    /// The architecture in the name, Debian's word for it: `amd64`.
    pub fn arch(&self) -> &str {
        let inner = &self.0["zendiloos_".len()..self.0.len() - ".deb".len()];
        inner.rsplit_once('_').map_or("", |(_, arch)| arch)
    }

    /// Which kind of build the version names (`scripts/release-version.sh`):
    /// a release is numbers and dots (`1.2.0`), a test release that, `~`
    /// and a name (`1.2.0~rc.1`, `1.2.0~beta.2`). Everything else counts as
    /// a development build, the build of a commit
    /// (`1.2.0+dev.<time>.g<commit>`) among it.
    pub fn kind(&self) -> ReleaseChannel {
        let numbers = |text: &str| {
            !text.is_empty()
                && text.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        };
        let version = self.version();
        match version.split_once('~') {
            None if numbers(version) => ReleaseChannel::Stable,
            Some((release, name))
                if numbers(release)
                    && name.starts_with(|c: char| c.is_ascii_alphabetic())
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') =>
            {
                ReleaseChannel::Test
            }
            _ => ReleaseChannel::Development,
        }
    }
}

/// A release an update installs, and the channel of the NAS it is for: a
/// build of a kind the channel does not take is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInstall {
    pub package: ReleasePackage,
    pub channel: ReleaseChannel,
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
    /// dpkg left packages unpacked or half configured, as `dpkg --audit`
    /// lists them: an update was cut off, and [`PackageTask::Finish`]
    /// finishes what it began.
    #[serde(default)]
    pub interrupted: bool,
    /// Which kernel the NAS starts next, as the runner set it; none from a
    /// runner before zendiloOS chose.
    #[serde(default)]
    pub boot: Option<BootKernel>,
}

/// Which kernel GRUB starts next, and why (docs/architecture.md, 4.10):
/// the newest one with its ZFS module, never a newer one without.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootKernel {
    /// The kernel GRUB starts by default; none where zendiloOS does not
    /// choose it ([`BootReason::NotGrub`], [`BootReason::NotSaved`],
    /// [`BootReason::Failed`]).
    pub kernel: Option<String>,
    pub reason: BootReason,
    /// What stood in the way, where something did.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootReason {
    /// The newest kernel, which has its ZFS module.
    Newest,
    /// The newest kernel has no ZFS module (DKMS could not build it): GRUB
    /// keeps starting the newest one that has.
    NewestLacksZfs,
    /// No kernel has a ZFS module: GRUB starts the newest, as Debian would.
    NoZfs,
    /// GRUB's configuration does not start the entry zendiloOS saves: it
    /// was written before `GRUB_DEFAULT=saved`, or another setting wins.
    /// `update-grub` writes it anew.
    NotSaved,
    /// The machine does not start through GRUB, or GRUB's configuration
    /// names no kernel of Debian's: nothing chosen.
    NotGrub,
    /// Saving the entry failed.
    Failed,
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
        assert_eq!((package.version(), package.arch()), ("0.2.0~rc.1", "amd64"));
        let dev = ReleasePackage::parse("zendiloos_0.2.0+dev.20261106.gabc1234_arm64.deb").unwrap();
        assert_eq!((dev.version(), dev.arch()), ("0.2.0+dev.20261106.gabc1234", "arm64"));
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
    fn a_version_tells_which_channels_may_take_it() {
        let kind =
            |version: &str| ReleasePackage::parse(&format!("zendiloos_{version}_amd64.deb")).unwrap().kind();
        assert_eq!(kind("0.3.0"), ReleaseChannel::Stable);
        assert_eq!(kind("10.20.30"), ReleaseChannel::Stable);
        assert_eq!(kind("0.3.1~rc.1"), ReleaseChannel::Test);
        assert_eq!(kind("0.3.1~beta.2"), ReleaseChannel::Test);
        assert_eq!(kind("0.1.1~vmtest1"), ReleaseChannel::Test);
        assert_eq!(kind("0.3.0+dev.20261109120000.gabc1234"), ReleaseChannel::Development);
        assert_eq!(kind("0.3.1~rc.1+dev.20261109120000.gabc1234"), ReleaseChannel::Development);
        // Whatever CI does not make counts as the least steady.
        for odd in ["0.3.0-1", "0.3.0~", "0.3.0~1", "0.3..0", "0.3.0.", "0.3.0~rc-1"] {
            assert_eq!(kind(odd), ReleaseChannel::Development, "{odd}");
        }

        use ReleaseChannel::{Development, Stable, Test};
        assert!(Stable.takes(Stable) && !Stable.takes(Test) && !Stable.takes(Development));
        assert!(Test.takes(Stable) && Test.takes(Test) && !Test.takes(Development));
        assert!(Development.takes(Stable) && Development.takes(Test) && Development.takes(Development));
        assert_eq!(ReleaseChannel::parse("development"), Some(Development));
        assert_eq!(ReleaseChannel::parse("nightly"), None);
        assert_eq!(serde_json::to_string(&Test).unwrap(), "\"test\"");
    }

    #[test]
    fn tasks_have_names_for_the_runner() {
        assert_eq!(PackageTask::parse("update"), Some(PackageTask::Update));
        assert_eq!(PackageTask::parse("install-docker"), Some(PackageTask::InstallDocker));
        assert_eq!(PackageTask::parse("install-acme"), Some(PackageTask::InstallAcme));
        assert_eq!(PackageTask::parse("install-vms"), Some(PackageTask::InstallVms));
        assert_eq!(PackageTask::parse("install-networkd"), Some(PackageTask::InstallNetworkd));
        assert_eq!(PackageTask::parse("finish"), Some(PackageTask::Finish));
        assert!(PackageTask::Update.updates() && PackageTask::Finish.updates());
        assert!(!PackageTask::Check.updates() && !PackageTask::InstallDocker.updates());
        assert_eq!(PackageTask::parse("install-anything"), None);
        let json = serde_json::to_string(&PackageTask::InstallDocker).unwrap();
        assert_eq!(json, "\"install_docker\"");
    }
}
