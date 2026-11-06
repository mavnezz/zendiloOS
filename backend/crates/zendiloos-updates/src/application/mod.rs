//! Looking for updates, installing them, and saying so.

pub mod ports;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};
use zendiloos_notify::{Level, NewNotification, Notifier};

use self::ports::{HostError, HostPackages, ReleaseSource, UpdateStore};
use crate::domain::{CheckResult, RunState, UpdateRun, UpdateSettings, compare_versions, newest_for};

/// How often [`Updates::tick`] wants to run; a daily check comes after a
/// day without one.
pub const TICK: Duration = Duration::from_secs(10 * 60);
const DAY_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, thiserror::Error)]
pub enum UpdatesError {
    #[error("nothing was looked for yet")]
    NotChecked,
    #[error("there is nothing to update")]
    NothingToDo,
    /// A check or an update is going on.
    #[error("an update or a check is going on")]
    Busy,
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// What zendiloosd itself is doing before a run starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking,
    /// The release is being downloaded.
    Downloading,
    /// Starting the update failed, with why.
    Failed(String),
}

/// Everything the update page shows.
#[derive(Debug, Clone)]
pub struct UpdateOverview {
    /// The version of zendiloOS that runs.
    pub current: String,
    pub settings: UpdateSettings,
    /// What the last check found, without what is installed since.
    pub check: Option<CheckResult>,
    /// The latest run that installs, or any run going on.
    pub run: Option<UpdateRun>,
    pub phase: Phase,
}

#[derive(Clone)]
pub struct UpdatesPorts {
    pub releases: Arc<dyn ReleaseSource>,
    pub host: Arc<dyn HostPackages>,
    pub store: Arc<dyn UpdateStore>,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct Updates {
    ports: UpdatesPorts,
    current: String,
    phase: Arc<Mutex<Phase>>,
    /// One check or start at a time.
    busy: Arc<tokio::sync::Mutex<()>>,
}

impl Updates {
    /// `current`: the version of zendiloOS that runs.
    pub fn new(ports: UpdatesPorts, current: impl Into<String>) -> Self {
        Self {
            ports,
            current: current.into(),
            phase: Arc::new(Mutex::new(Phase::Idle)),
            busy: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase;
    }

    pub async fn overview(&self, request: &RequestContext) -> Result<UpdateOverview, UpdatesError> {
        let settings = self.ports.store.settings().await?;
        let run = self.ports.host.latest(request).await?;
        let mut check = self.ports.store.last_check().await?;
        if let Some(check) = &mut check {
            self.forget_installed(check, run.as_ref());
        }
        let run = run.filter(|run| run.installs || run.state == RunState::Running);
        Ok(UpdateOverview { current: self.current.clone(), settings, check, run, phase: self.phase() })
    }

    /// A release no newer than what runs is no update any more; after a
    /// finished update, apt's report from its end counts.
    fn forget_installed(&self, check: &mut CheckResult, run: Option<&UpdateRun>) {
        if check
            .release
            .as_ref()
            .is_some_and(|release| compare_versions(&release.version, &self.current).is_le())
        {
            check.release = None;
        }
        if let Some(run) = run
            && run.installs
            && run.state == RunState::Succeeded
            && let (Some(report), Some(finished)) = (&run.report, run.finished_at)
            && finished >= check.checked_at
        {
            check.host = Some(report.clone());
            check.host_error = None;
        }
    }

    /// Looks for a newer release on the channel and for Debian's updates.
    pub async fn check(
        &self,
        actor: Option<&Actor>,
        request: &RequestContext,
    ) -> Result<CheckResult, UpdatesError> {
        let _busy = self.busy.try_lock().map_err(|_| UpdatesError::Busy)?;
        self.check_now(actor, request).await
    }

    /// As [`Updates::check`], in the background: returns once it began; the
    /// overview says when it is done.
    pub async fn start_check(&self, actor: &Actor, request: &RequestContext) -> Result<(), UpdatesError> {
        let busy = Arc::clone(&self.busy).try_lock_owned().map_err(|_| UpdatesError::Busy)?;
        if self.ports.host.latest(request).await?.is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        self.set_phase(Phase::Checking);
        let (this, actor, request) = (self.clone(), actor.clone(), request.clone());
        tokio::spawn(async move {
            let _busy = busy;
            if let Err(error) = this.check_now(Some(&actor), &request).await {
                tracing::warn!(%error, "updates: the check failed");
                this.set_phase(Phase::Failed(error.to_string()));
            }
        });
        Ok(())
    }

    async fn check_now(
        &self,
        actor: Option<&Actor>,
        request: &RequestContext,
    ) -> Result<CheckResult, UpdatesError> {
        if self.ports.host.latest(request).await?.is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        self.set_phase(Phase::Checking);
        let result = self.look(request).await;
        self.set_phase(Phase::Idle);
        let check = result?;
        if let Some(actor) = actor {
            self.audit("system.update_check", Outcome::Success, actor, request, "").await;
        }
        self.ports.store.save_check(&check).await?;
        self.say_available(&check).await;
        Ok(check)
    }

    async fn look(&self, request: &RequestContext) -> Result<CheckResult, UpdatesError> {
        let settings = self.ports.store.settings().await?;
        let (release, release_error) = match self.ports.releases.releases().await {
            Ok(releases) => (newest_for(settings.channel, &releases, &self.current).cloned(), None),
            Err(error) => (None, Some(error)),
        };
        let (host, host_error) = match self.ports.host.check(request).await {
            Ok(report) => (Some(report), None),
            Err(HostError::Refused(reason)) if reason.contains("busy") => return Err(UpdatesError::Busy),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(CheckResult { checked_at: self.ports.clock.now(), release, release_error, host, host_error })
    }

    /// Installs what the last check found: the release is downloaded, then
    /// a run on the NAS installs Debian's packages and the release. Returns
    /// once the download began; the run is followed in the overview.
    pub async fn apply(&self, actor: &Actor, request: &RequestContext) -> Result<(), UpdatesError> {
        let busy = Arc::clone(&self.busy).try_lock_owned().map_err(|_| UpdatesError::Busy)?;
        if self.ports.host.latest(request).await?.is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        let mut check = self.ports.store.last_check().await?.ok_or(UpdatesError::NotChecked)?;
        let run = self.ports.host.latest(request).await?;
        self.forget_installed(&mut check, run.as_ref());
        if !check.has_updates() {
            return Err(UpdatesError::NothingToDo);
        }
        let release = check.release.clone();
        let target = release.as_ref().map_or_else(String::new, |release| release.version.clone());
        self.set_phase(Phase::Downloading);
        let (this, owned_actor, context) = (self.clone(), actor.clone(), request.clone());
        tokio::spawn(async move {
            let _busy = busy;
            let started = async {
                if let Some(release) = &release {
                    this.ports.releases.download(release).await.map_err(HostError::Failed)?;
                }
                this.ports.host.start(release.as_ref(), &context).await
            }
            .await;
            let outcome = if started.is_ok() { Outcome::Success } else { Outcome::Failure };
            this.audit("system.update", outcome, &owned_actor, &context, &target).await;
            match started {
                Ok(id) => {
                    tracing::info!(run = id, release = target, "updates: the update runs");
                    this.set_phase(Phase::Idle);
                }
                Err(error) => {
                    tracing::warn!(%error, "updates: the update could not start");
                    this.set_phase(Phase::Failed(error.to_string()));
                }
            }
        });
        Ok(())
    }

    pub async fn settings(&self) -> Result<UpdateSettings, UpdatesError> {
        Ok(self.ports.store.settings().await?)
    }

    pub async fn set_settings(
        &self,
        settings: UpdateSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<UpdateSettings, UpdatesError> {
        let before = self.ports.store.settings().await?;
        self.ports.store.save_settings(&settings).await?;
        self.audit("system.update_settings", Outcome::Success, actor, request, settings.channel.as_str())
            .await;
        if before.channel != settings.channel {
            // What was found was for the other channel.
            if let Some(mut check) = self.ports.store.last_check().await? {
                check.release = None;
                self.ports.store.save_check(&check).await?;
            }
        }
        Ok(settings)
    }

    /// Runs every [`TICK`]: says how a finished update went, once, and
    /// looks for updates when a day passed without.
    pub async fn tick(&self) -> Result<(), UpdatesError> {
        let request = RequestContext::default();
        if let Some(run) = self.ports.host.latest(&request).await?
            && run.installs
            && run.state != RunState::Running
            && self.ports.store.said("run").await?.as_deref() != Some(run.id.as_str())
        {
            self.ports.store.save_said("run", &run.id).await?;
            self.say_finished(&run).await;
        }
        let settings = self.ports.store.settings().await?;
        let last = self.ports.store.last_check().await?.map(|check| check.checked_at);
        let due =
            last.is_none_or(|at| self.ports.clock.now().unix_seconds() - at.unix_seconds() >= DAY_SECONDS);
        if settings.check_daily && due {
            match self.check(None, &request).await {
                Ok(_) | Err(UpdatesError::Busy) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn say_available(&self, check: &CheckResult) {
        if !check.has_updates() {
            return;
        }
        let key = check.key();
        match self.ports.store.said("check").await {
            Ok(Some(said)) if said == key => return,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "updates: cannot read what was said");
                return;
            }
        }
        let packages = check.host.as_ref().map_or(0, |host| host.packages.len());
        let security = check.host.as_ref().map_or(0, |host| host.security_updates());
        let mut notification = NewNotification::new(Level::Info, "updates_available")
            .with("packages", packages.to_string())
            .with("security", security.to_string());
        if let Some(release) = &check.release {
            notification = notification.with("version", release.version.clone());
        }
        self.ports.notifier.notify(notification).await;
        if let Err(error) = self.ports.store.save_said("check", &key).await {
            tracing::warn!(%error, "updates: cannot keep what was said");
        }
    }

    async fn say_finished(&self, run: &UpdateRun) {
        let notification = match run.state {
            RunState::Succeeded => {
                let restart = run.report.as_ref().is_some_and(|report| report.restart_needed());
                let zfs_ready = run.report.as_ref().is_none_or(|report| report.zfs_ready);
                let level = if zfs_ready { Level::Success } else { Level::Error };
                NewNotification::new(level, "updates_installed")
                    .with("version", run.zendiloos.clone().unwrap_or_default())
                    .with("restart", restart.to_string())
                    .with("zfs_ready", zfs_ready.to_string())
            }
            _ => NewNotification::new(Level::Error, "updates_failed")
                .with("error", run.error.clone().unwrap_or_default()),
        };
        self.ports.notifier.notify(notification).await;
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        actor: &Actor,
        request: &RequestContext,
        target: &str,
    ) {
        let mut entry = AuditEntry::new(self.ports.clock.now(), action, outcome, request).by(actor);
        if !target.is_empty() {
            entry = entry.target(target);
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;
    use zendiloos_notify::testing::MemoryNotifications;

    use super::*;
    use crate::domain::Channel;
    use crate::testing::{FakeHost, FakeReleases, MemoryStore, release};

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(50)
        }
    }

    struct NoAudit;
    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    struct Setup {
        updates: Updates,
        releases: Arc<FakeReleases>,
        host: Arc<FakeHost>,
        store: Arc<MemoryStore>,
        said: Arc<MemoryNotifications>,
    }

    fn setup() -> Setup {
        let releases = Arc::new(FakeReleases::default());
        *releases.releases.lock().unwrap() = vec![
            release("dev", "0.3.0~rc.1+dev.1", true),
            release("v0.3.0-rc.1", "0.3.0~rc.1", true),
            release("v0.2.0", "0.2.0", false),
            release("v0.1.0", "0.1.0", false),
        ];
        let host = Arc::new(FakeHost::default());
        let store = Arc::new(MemoryStore::default());
        let said = Arc::new(MemoryNotifications::default());
        let ports = UpdatesPorts {
            releases: releases.clone(),
            host: host.clone(),
            store: store.clone(),
            notifier: Notifier::new(said.clone(), Arc::new(Fixed)),
            audit: Arc::new(NoAudit),
            clock: Arc::new(Fixed),
        };
        Setup { updates: Updates::new(ports, "0.1.0"), releases, host, store, said }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn a_check_finds_the_channels_release_and_debians_packages_and_says_so_once() {
        let s = setup();
        let request = RequestContext::default();
        let check = s.updates.check(Some(&alice()), &request).await.unwrap();
        assert_eq!(check.release.as_ref().map(|r| r.version.as_str()), Some("0.2.0"));
        assert_eq!(check.host.as_ref().unwrap().packages.len(), 2);
        let said = s.said.all();
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].code, "updates_available");
        assert_eq!(said[0].params["version"], "0.2.0");
        assert_eq!(said[0].params["security"], "1");

        s.updates.check(None, &request).await.unwrap();
        assert_eq!(s.said.all().len(), 1, "the same finding is said once");

        let mut settings = s.updates.settings().await.unwrap();
        settings.channel = Channel::Development;
        s.updates.set_settings(settings, &alice(), &request).await.unwrap();
        assert_eq!(s.store.check.lock().unwrap().as_ref().unwrap().release, None);
        let check = s.updates.check(None, &request).await.unwrap();
        assert_eq!(check.release.unwrap().tag, "dev");
    }

    #[tokio::test]
    async fn an_update_downloads_the_release_and_runs_on_the_nas() {
        let s = setup();
        let request = RequestContext::default();
        assert!(matches!(s.updates.apply(&alice(), &request).await, Err(UpdatesError::NotChecked)));
        s.updates.check(None, &request).await.unwrap();
        s.updates.apply(&alice(), &request).await.unwrap();
        for _ in 0..100 {
            if s.updates.overview(&request).await.unwrap().run.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*s.releases.downloaded.lock().unwrap(), ["0.2.0"]);
        assert_eq!(*s.host.started.lock().unwrap(), [Some("0.2.0".to_owned())]);
        let overview = s.updates.overview(&request).await.unwrap();
        assert_eq!(overview.phase, Phase::Idle);
        let run = overview.run.unwrap();
        assert_eq!((run.state, run.zendiloos.as_deref()), (RunState::Succeeded, Some("0.2.0")));
        // apt's report from the end of the run replaces what was found before.
        assert!(overview.check.unwrap().host.unwrap().packages.is_empty());

        s.updates.tick().await.unwrap();
        s.updates.tick().await.unwrap();
        let finished: Vec<_> = s.said.all().into_iter().filter(|n| n.code == "updates_installed").collect();
        assert_eq!(finished.len(), 1, "a finished update is said once");
        assert_eq!(finished[0].params["restart"], "true");
    }

    #[tokio::test]
    async fn nothing_is_installed_when_nothing_is_new() {
        let s = setup();
        let request = RequestContext::default();
        s.releases.releases.lock().unwrap().clear();
        s.host.packages.lock().unwrap().clear();
        let check = s.updates.check(None, &request).await.unwrap();
        assert!(!check.has_updates());
        assert!(s.said.all().is_empty());
        assert!(matches!(s.updates.apply(&alice(), &request).await, Err(UpdatesError::NothingToDo)));
    }
}
