//! The language of updates: versions, channels, releases, what apt reports.

mod host;
mod release;
mod version;

pub use host::{HostReport, PackageUpdate, RunState, UpdateRun};
pub use release::{Asset, Channel, DEVELOPMENT_TAG, Release, channel_of, newest_for, package_version};
pub use version::compare_versions;

use zendiloos_kernel::Timestamp;

/// How the NAS looks for updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSettings {
    pub channel: Channel,
    /// Look once a day and say what there is; installing stays a click.
    pub check_daily: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { channel: Channel::Stable, check_daily: true }
    }
}

/// What the last look found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub checked_at: Timestamp,
    /// The newest release on the channel, if newer than what runs.
    pub release: Option<Release>,
    /// Why GitHub could not be asked.
    pub release_error: Option<String>,
    pub host: Option<HostReport>,
    /// Why apt could not be asked.
    pub host_error: Option<String>,
}

impl CheckResult {
    /// Something to install.
    pub fn has_updates(&self) -> bool {
        self.release.is_some() || self.host.as_ref().is_some_and(|host| !host.packages.is_empty())
    }

    /// What tells one finding from another, to say each only once.
    pub fn key(&self) -> String {
        let release = self.release.as_ref().map_or("", |release| release.version.as_str());
        let mut packages: Vec<String> = self
            .host
            .iter()
            .flat_map(|host| host.packages.iter().map(|p| format!("{}={}", p.name, p.to)))
            .collect();
        packages.sort();
        format!("{release}|{}", packages.join(","))
    }
}
