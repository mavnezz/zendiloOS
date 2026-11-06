//! zendiloOS's releases on GitHub and the channels they belong to.

use std::cmp::Ordering;

use zendiloos_kernel::Timestamp;

use super::version::compare_versions;

/// Which releases an administrator wants: each channel takes those of the
/// steadier ones too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// Releases (`v1.2.0`).
    Stable,
    /// Pre-releases for testing (`v1.2.0-rc.1`, `v1.2.0-beta.1`), and stable ones.
    Test,
    /// The build of the latest commit on `main` (the release `dev`), and all
    /// others.
    Development,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Test => "test",
            Self::Development => "development",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Stable, Self::Test, Self::Development].into_iter().find(|channel| channel.as_str() == name)
    }

    /// Whether a release of `kind` is offered on this channel.
    pub fn takes(self, kind: Channel) -> bool {
        kind <= self
    }
}

/// The tag of the rolling development build.
pub const DEVELOPMENT_TAG: &str = "dev";

/// A file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// Where it is downloaded from (https).
    pub address: String,
    pub size: u64,
}

/// One release of zendiloOS, with the files an update needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    /// The package's version, from its file name (`zendiloos_<version>_<arch>.deb`).
    pub version: String,
    pub channel: Channel,
    pub published_at: Option<Timestamp>,
    /// What changed, as written on GitHub (Markdown).
    pub notes: String,
    pub package: Asset,
    /// `SHA256SUMS`, and its minisign signature `SHA256SUMS.minisig`.
    pub sums: Asset,
    pub signature: Asset,
}

/// The channel of a release, by its tag and GitHub's pre-release mark.
pub fn channel_of(tag: &str, prerelease: bool) -> Channel {
    if tag == DEVELOPMENT_TAG {
        Channel::Development
    } else if prerelease {
        Channel::Test
    } else {
        Channel::Stable
    }
}

/// The version in a package's file name, for `arch`:
/// `zendiloos_0.2.0~rc.1_amd64.deb` → `0.2.0~rc.1`.
pub fn package_version<'a>(file: &'a str, arch: &str) -> Option<&'a str> {
    let version = file.strip_prefix("zendiloos_")?.strip_suffix(&format!("_{arch}.deb"))?;
    let valid = !version.is_empty()
        && version.len() <= 128
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '~' | '-' | ':'));
    valid.then_some(version)
}

/// The newest release on `channel` that is newer than `current`.
pub fn newest_for<'a>(channel: Channel, releases: &'a [Release], current: &str) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|release| channel.takes(release.channel))
        .filter(|release| compare_versions(&release.version, current) == Ordering::Greater)
        .max_by(|a, b| compare_versions(&a.version, &b.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, version: &str, channel: Channel) -> Release {
        let asset = |name: &str| Asset { name: name.into(), address: format!("https://x/{name}"), size: 1 };
        Release {
            tag: tag.into(),
            version: version.into(),
            channel,
            published_at: None,
            notes: String::new(),
            package: asset(&format!("zendiloos_{version}_amd64.deb")),
            sums: asset("SHA256SUMS"),
            signature: asset("SHA256SUMS.minisig"),
        }
    }

    #[test]
    fn each_channel_takes_the_steadier_ones_too() {
        let releases = [
            release("v0.2.0", "0.2.0", Channel::Stable),
            release("v0.3.0-rc.1", "0.3.0~rc.1", Channel::Test),
            release("dev", "0.3.0~rc.1+dev.20261106.gabc", Channel::Development),
            release("v0.1.0", "0.1.0", Channel::Stable),
        ];
        let newest = |channel, current| newest_for(channel, &releases, current).map(|r| r.tag.as_str());
        assert_eq!(newest(Channel::Stable, "0.1.0"), Some("v0.2.0"));
        assert_eq!(newest(Channel::Test, "0.1.0"), Some("v0.3.0-rc.1"));
        assert_eq!(newest(Channel::Development, "0.1.0"), Some("dev"));
        assert_eq!(newest(Channel::Stable, "0.2.0"), None);
        // Back from testing: nothing older is offered.
        assert_eq!(newest(Channel::Stable, "0.3.0~rc.1"), None);
    }

    #[test]
    fn tags_and_files_tell_channel_and_version() {
        assert_eq!(channel_of("v1.0.0", false), Channel::Stable);
        assert_eq!(channel_of("v1.0.0-rc.1", true), Channel::Test);
        assert_eq!(channel_of("dev", true), Channel::Development);
        assert_eq!(package_version("zendiloos_0.2.0~rc.1_amd64.deb", "amd64"), Some("0.2.0~rc.1"));
        assert_eq!(package_version("zendiloos_0.2.0_arm64.deb", "amd64"), None);
        assert_eq!(package_version("zendiloos_0.2.0 x_amd64.deb", "amd64"), None);
        assert_eq!(package_version("other_0.2.0_amd64.deb", "amd64"), None);
        assert!(Channel::Development.takes(Channel::Stable) && !Channel::Stable.takes(Channel::Test));
        assert_eq!(Channel::parse("test"), Some(Channel::Test));
    }
}
