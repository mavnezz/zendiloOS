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
    /// Which kernel the NAS starts next; none from a NAS that did not say.
    pub boot: Option<BootChoice>,
}

/// Which kernel GRUB starts next, and why: the newest one with its ZFS
/// module, never a newer one without (docs/architecture.md, 4.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootChoice {
    /// None where zendiloOS does not choose it.
    pub kernel: Option<String>,
    pub reason: BootReason,
    /// What stood in the way, where something did.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootReason {
    /// The newest kernel, which has its ZFS module.
    Newest,
    /// The newest kernel has no ZFS module yet: GRUB keeps starting the
    /// newest one that has.
    NewestLacksZfs,
    /// No kernel has a ZFS module: the newest starts.
    NoZfs,
    /// GRUB's configuration does not start the entry zendiloOS saves.
    NotSaved,
    /// The machine does not start through GRUB: nothing chosen.
    NotGrub,
    /// Saving the entry failed.
    Failed,
}

impl BootReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::NewestLacksZfs => "newest_lacks_zfs",
            Self::NoZfs => "no_zfs",
            Self::NotSaved => "not_saved",
            Self::NotGrub => "not_grub",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Newest, Self::NewestLacksZfs, Self::NoZfs, Self::NotSaved, Self::NotGrub, Self::Failed]
            .into_iter()
            .find(|reason| reason.as_str() == text)
    }
}

impl HostReport {
    /// Whether a restart brings another kernel: the one GRUB starts where
    /// zendiloOS chose it, else the newest installed.
    pub fn restart_needed(&self) -> bool {
        let next = self.boot.as_ref().and_then(|boot| boot.kernel.as_ref()).or(self.newest_kernel.as_ref());
        next.is_some_and(|next| *next != self.running_kernel)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(newest: &str, boot: Option<&str>) -> HostReport {
        HostReport {
            running_kernel: "6.12.48+deb13-amd64".into(),
            newest_kernel: Some(newest.into()),
            boot: boot.map(|kernel| BootChoice {
                kernel: Some(kernel.into()),
                reason: BootReason::NewestLacksZfs,
                detail: None,
            }),
            ..HostReport::default()
        }
    }

    #[test]
    fn a_restart_is_waiting_when_it_starts_another_kernel() {
        let (running, newer) = ("6.12.48+deb13-amd64", "6.12.57+deb13-amd64");
        assert!(report(newer, None).restart_needed(), "without a choice, the newest");
        assert!(!report(running, None).restart_needed());
        assert!(!report(newer, Some(running)).restart_needed(), "GRUB keeps the running one");
        assert!(report(newer, Some("6.12.50+deb13-amd64")).restart_needed());
        let unchosen = HostReport {
            boot: Some(BootChoice { kernel: None, reason: BootReason::NotGrub, detail: None }),
            ..report(newer, None)
        };
        assert!(unchosen.restart_needed(), "not chosen: the newest");
    }

    #[test]
    fn boot_reasons_are_kept_by_name() {
        for reason in [
            BootReason::Newest,
            BootReason::NewestLacksZfs,
            BootReason::NoZfs,
            BootReason::NotSaved,
            BootReason::NotGrub,
            BootReason::Failed,
        ] {
            assert_eq!(BootReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(BootReason::parse("later"), None);
    }
}
