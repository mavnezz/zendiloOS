//! What apt reports about the NAS, and a run of the update.

use zendiloos_kernel::Timestamp;

/// A Debian package that would be upgraded, or newly installed for one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUpdate {
    pub name: String,
    /// None for a package that comes in new (a kernel, say).
    pub from: Option<String>,
    pub to: String,
    /// From Debian's security archive.
    pub security: bool,
}

/// What apt and the kernel say about the NAS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostReport {
    pub packages: Vec<PackageUpdate>,
    pub running_kernel: String,
    /// The newest kernel installed; another than the running one needs a
    /// restart.
    pub newest_kernel: Option<String>,
    /// The ZFS module is built for the newest kernel; without it the pools
    /// would not import after a restart.
    pub zfs_ready: bool,
    /// dpkg left packages half installed or half configured: an update was
    /// cut off, and finishing it comes before anything else apt does.
    pub interrupted: bool,
}

impl HostReport {
    pub fn restart_needed(&self) -> bool {
        self.newest_kernel.as_ref().is_some_and(|newest| *newest != self.running_kernel)
    }

    pub fn security_updates(&self) -> usize {
        self.packages.iter().filter(|package| package.security).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Succeeded,
    Failed,
}

/// A run of the update on the NAS: apt in a unit of its own, which outlives
/// a restart of zendiloOS while it installs zendiloOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRun {
    pub id: String,
    /// `update` installs, and so does finishing one; `check` only looks.
    pub installs: bool,
    /// It finishes an update that was cut off rather than updating.
    pub finishes: bool,
    pub state: RunState,
    /// `lists`, `debian`, `zendiloos`, `modules`; finishing: `configure`,
    /// `repair`, `modules`.
    pub step: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub error: Option<String>,
    /// The zendiloOS version it installs, if any.
    pub zendiloos: Option<String>,
    /// What apt said at the end.
    pub report: Option<HostReport>,
}
