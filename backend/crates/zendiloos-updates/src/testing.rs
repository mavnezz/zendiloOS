//! In-memory adapters for tests.

use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::ports::{HostError, HostPackages, ReleaseSource, UpdateStore};
use crate::domain::{
    Asset, BootChoice, BootReason, Channel, CheckResult, HostReport, PackageUpdate, Release, RunState,
    UpdateRun, UpdateSettings, channel_of,
};

/// A release as GitHub would list it.
pub fn release(tag: &str, version: &str, prerelease: bool) -> Release {
    let asset =
        |name: &str| Asset { name: name.into(), address: format!("https://example.org/{name}"), size: 10 };
    Release {
        tag: tag.into(),
        version: version.into(),
        channel: channel_of(tag, prerelease),
        published_at: None,
        notes: format!("What is new in {version}"),
        package: asset(&format!("zendiloos_{version}_amd64.deb")),
        sums: asset("SHA256SUMS"),
        signature: asset("SHA256SUMS.minisig"),
    }
}

#[derive(Default)]
pub struct FakeReleases {
    pub releases: Mutex<Vec<Release>>,
    pub downloaded: Mutex<Vec<String>>,
}

#[async_trait]
impl ReleaseSource for FakeReleases {
    async fn releases(&self) -> Result<Vec<Release>, String> {
        Ok(self.releases.lock().unwrap().clone())
    }

    async fn download(&self, release: &Release) -> Result<(), String> {
        self.downloaded.lock().unwrap().push(release.version.clone());
        Ok(())
    }
}

/// apt on a NAS with `packages` to upgrade; an update finishes at once.
pub struct FakeHost {
    pub packages: Mutex<Vec<PackageUpdate>>,
    /// The latest run, and the last that installed ([`FakeHost::record`]
    /// keeps both as the runner does).
    pub run: Mutex<Option<UpdateRun>>,
    pub update: Mutex<Option<UpdateRun>>,
    /// The release of each update started, and the channel it was for.
    pub started: Mutex<Vec<Option<String>>>,
    pub channels: Mutex<Vec<Channel>>,
    /// dpkg left packages half done, until an update is finished.
    pub interrupted: Mutex<bool>,
    /// How often finishing an update was started.
    pub finished: Mutex<usize>,
    /// The kernel that runs; another once the NAS restarted into the one
    /// an update brought.
    pub running: Mutex<String>,
}

impl Default for FakeHost {
    fn default() -> Self {
        let package = |name: &str, security| PackageUpdate {
            name: name.into(),
            from: Some("1.0".into()),
            to: "1.1".into(),
            security,
        };
        Self {
            packages: Mutex::new(vec![package("openssl", true), package("tzdata", false)]),
            run: Mutex::default(),
            update: Mutex::default(),
            started: Mutex::default(),
            channels: Mutex::default(),
            interrupted: Mutex::default(),
            finished: Mutex::default(),
            running: Mutex::new(KERNEL.into()),
        }
    }
}

/// The kernel the NAS runs at first.
const KERNEL: &str = "6.12.48+deb13-amd64";

impl FakeHost {
    /// Records a run as the NAS's runner does: as the latest, and one that
    /// installs as the last update too.
    pub fn record(&self, run: UpdateRun) {
        if run.installs {
            *self.update.lock().unwrap() = Some(run.clone());
        }
        *self.run.lock().unwrap() = Some(run);
    }

    fn report(&self) -> HostReport {
        HostReport {
            packages: self.packages.lock().unwrap().clone(),
            running_kernel: self.running.lock().unwrap().clone(),
            newest_kernel: Some(KERNEL.into()),
            zfs_ready: true,
            interrupted: *self.interrupted.lock().unwrap(),
            boot: Some(BootChoice { kernel: Some(KERNEL.into()), reason: BootReason::Newest, detail: None }),
        }
    }
}

#[async_trait]
impl HostPackages for FakeHost {
    /// As on the NAS, a check is a run too: its record replaces the latest.
    async fn check(&self, _: &RequestContext) -> Result<HostReport, HostError> {
        let report = self.report();
        self.record(UpdateRun {
            id: format!("check{}", self.started.lock().unwrap().len()),
            installs: false,
            finishes: false,
            state: RunState::Succeeded,
            step: "lists".into(),
            started_at: Timestamp::from_unix_seconds(300),
            finished_at: Some(Timestamp::from_unix_seconds(310)),
            error: None,
            zendiloos: None,
            report: Some(report.clone()),
        });
        Ok(report)
    }

    async fn start(
        &self,
        release: Option<&Release>,
        channel: Channel,
        _: &RequestContext,
    ) -> Result<String, HostError> {
        self.started.lock().unwrap().push(release.map(|release| release.version.clone()));
        self.channels.lock().unwrap().push(channel);
        self.packages.lock().unwrap().clear();
        let id = format!("run{}", self.started.lock().unwrap().len());
        self.record(UpdateRun {
            id: id.clone(),
            installs: true,
            finishes: false,
            state: RunState::Succeeded,
            step: "modules".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: Some(Timestamp::from_unix_seconds(200)),
            error: None,
            zendiloos: release.map(|release| release.version.clone()),
            // A new kernel with its ZFS module, which GRUB starts next.
            report: Some(HostReport {
                newest_kernel: Some("6.12.57+deb13-amd64".into()),
                boot: Some(BootChoice {
                    kernel: Some("6.12.57+deb13-amd64".into()),
                    reason: BootReason::Newest,
                    detail: None,
                }),
                ..self.report()
            }),
        });
        Ok(id)
    }

    /// Finishes at once: nothing is left half done.
    async fn finish(&self, _: &RequestContext) -> Result<String, HostError> {
        *self.interrupted.lock().unwrap() = false;
        *self.finished.lock().unwrap() += 1;
        let id = format!("finish{}", self.finished.lock().unwrap());
        self.record(UpdateRun {
            id: id.clone(),
            installs: true,
            finishes: true,
            state: RunState::Succeeded,
            step: "modules".into(),
            started_at: Timestamp::from_unix_seconds(400),
            finished_at: Some(Timestamp::from_unix_seconds(450)),
            error: None,
            zendiloos: None,
            report: Some(self.report()),
        });
        Ok(id)
    }

    async fn latest(&self, _: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
        Ok(self.run.lock().unwrap().clone())
    }

    async fn last_update(&self, _: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
        Ok(self.update.lock().unwrap().clone())
    }

    async fn running_kernel(&self, _: &RequestContext) -> Result<Option<String>, HostError> {
        Ok(Some(self.running.lock().unwrap().clone()))
    }
}

#[derive(Default)]
pub struct MemoryStore {
    pub settings: Mutex<UpdateSettings>,
    pub check: Mutex<Option<CheckResult>>,
    pub said: Mutex<std::collections::BTreeMap<String, String>>,
    pub last_run: Mutex<Option<UpdateRun>>,
}

#[async_trait]
impl UpdateStore for MemoryStore {
    async fn settings(&self) -> Result<UpdateSettings, PortError> {
        Ok(*self.settings.lock().unwrap())
    }

    async fn save_settings(&self, settings: &UpdateSettings) -> Result<(), PortError> {
        *self.settings.lock().unwrap() = *settings;
        Ok(())
    }

    async fn last_check(&self) -> Result<Option<CheckResult>, PortError> {
        Ok(self.check.lock().unwrap().clone())
    }

    async fn save_check(&self, check: &CheckResult) -> Result<(), PortError> {
        *self.check.lock().unwrap() = Some(check.clone());
        Ok(())
    }

    async fn said(&self, topic: &str) -> Result<Option<String>, PortError> {
        Ok(self.said.lock().unwrap().get(topic).cloned())
    }

    async fn save_said(&self, topic: &str, key: &str) -> Result<(), PortError> {
        self.said.lock().unwrap().insert(topic.into(), key.into());
        Ok(())
    }

    async fn last_run(&self) -> Result<Option<UpdateRun>, PortError> {
        Ok(self.last_run.lock().unwrap().clone())
    }

    async fn save_last_run(&self, run: &UpdateRun) -> Result<(), PortError> {
        *self.last_run.lock().unwrap() = Some(run.clone());
        Ok(())
    }
}
