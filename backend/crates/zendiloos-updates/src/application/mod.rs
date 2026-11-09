//! Looking for updates, installing them, and saying so.

pub mod ports;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, RunningWork};
use zendiloos_notify::{Level, NewNotification, Notifier};

use self::ports::{HostError, HostPackages, ReleaseSource, UpdateStore};
use crate::domain::{
    Channel, CheckResult, Release, RunState, UpdateRun, UpdateSettings, compare_versions, newest_for,
};

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
    /// Background work runs, which the agents would see through before
    /// taking the new release; it names it.
    #[error("background work runs: {0}")]
    JobsRunning(String),
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
    /// What changed in it, in Markdown, as its package says; None for a
    /// build without.
    pub current_notes: Option<String>,
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
    /// What runs in the background, which an update waits for.
    pub work: Arc<dyn RunningWork>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct Updates {
    ports: UpdatesPorts,
    current: String,
    current_notes: Option<String>,
    phase: Arc<Mutex<Phase>>,
    /// One check or start at a time.
    busy: Arc<tokio::sync::Mutex<()>>,
    /// A check's result and a change of the channel are saved one after
    /// the other: what was found for one channel is never kept once
    /// another is set.
    saving: Arc<tokio::sync::Mutex<()>>,
}

impl Updates {
    /// `current`: the version of zendiloOS that runs.
    pub fn new(ports: UpdatesPorts, current: impl Into<String>) -> Self {
        Self {
            ports,
            current: current.into(),
            current_notes: None,
            phase: Arc::new(Mutex::new(Phase::Idle)),
            busy: Arc::new(tokio::sync::Mutex::new(())),
            saving: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// What changed in the version that runs, in Markdown, as its package
    /// brought it along.
    #[must_use]
    pub fn with_current_notes(mut self, notes: Option<String>) -> Self {
        self.current_notes = notes;
        self
    }

    fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase;
    }

    pub async fn overview(&self, request: &RequestContext) -> Result<UpdateOverview, UpdatesError> {
        let settings = self.ports.store.settings().await?;
        let latest = self.ports.host.latest(request).await?;
        let run = self.last_update(latest).await?;
        let mut check = self.ports.store.last_check().await?;
        if let Some(check) = &mut check {
            self.forget_installed(check, run.as_ref(), settings.channel);
        }
        Ok(UpdateOverview {
            current: self.current.clone(),
            current_notes: self.current_notes.clone(),
            settings,
            check,
            run,
            phase: self.phase(),
        })
    }

    /// The last update that installed: the NAS's record while it is one,
    /// otherwise as it was kept before a check replaced that record.
    async fn last_update(&self, latest: Option<UpdateRun>) -> Result<Option<UpdateRun>, UpdatesError> {
        match latest {
            Some(run) if run.installs || run.state == RunState::Running => Ok(Some(run)),
            _ => Ok(self.ports.store.last_run().await?),
        }
    }

    /// Says how a finished update went, once, and keeps it: before a check
    /// or another update replaces the NAS's record of it, and on each tick.
    async fn remember(&self, run: Option<&UpdateRun>) -> Result<(), UpdatesError> {
        if let Some(run) = run
            && run.installs
            && run.state != RunState::Running
            && self.ports.store.said("run").await?.as_deref() != Some(run.id.as_str())
        {
            self.ports.store.save_last_run(run).await?;
            self.ports.store.save_said("run", &run.id).await?;
            self.say_finished(run).await;
        }
        Ok(())
    }

    /// A release no newer than what runs is no update any more, nor one
    /// the channel does not take; after a finished update, apt's report
    /// from its end counts.
    fn forget_installed(&self, check: &mut CheckResult, run: Option<&UpdateRun>, channel: Channel) {
        if check.release.as_ref().is_some_and(|release| {
            compare_versions(&release.version, &self.current).is_le() || !channel.takes(release.channel)
        }) {
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
        let latest = self.ports.host.latest(request).await?;
        if latest.as_ref().is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        // The check replaces the NAS's record of the last run.
        self.remember(latest.as_ref()).await?;
        self.set_phase(Phase::Checking);
        let result = self.look(request).await;
        self.set_phase(Phase::Idle);
        let (mut check, releases) = result?;
        if let Some(actor) = actor {
            self.audit("system.update_check", Outcome::Success, actor, request, "").await;
        }
        {
            // The release is the channel's as it is now: an administrator
            // may have set another while apt fetched its lists.
            let _saving = self.saving.lock().await;
            let channel = self.ports.store.settings().await?.channel;
            check.release = releases
                .as_deref()
                .and_then(|releases| newest_for(channel, releases, &self.current))
                .cloned();
            self.ports.store.save_check(&check).await?;
        }
        self.say_available(&check).await;
        Ok(check)
    }

    /// What GitHub and apt say: the releases for this machine (none if
    /// GitHub could not be asked, which the result says) and the result
    /// without its release, which is picked for the channel when it is
    /// saved.
    async fn look(
        &self,
        request: &RequestContext,
    ) -> Result<(CheckResult, Option<Vec<Release>>), UpdatesError> {
        let (releases, release_error) = match self.ports.releases.releases().await {
            Ok(releases) => (Some(releases), None),
            Err(error) => (None, Some(error)),
        };
        let (host, host_error) = match self.ports.host.check(request).await {
            Ok(report) => (Some(report), None),
            Err(HostError::Refused(reason)) if reason.contains("busy") => return Err(UpdatesError::Busy),
            Err(error) => (None, Some(error.to_string())),
        };
        let check = CheckResult {
            checked_at: self.ports.clock.now(),
            release: None,
            release_error,
            host,
            host_error,
        };
        Ok((check, releases))
    }

    /// Installs what the last check found: the release is downloaded, then
    /// a run on the NAS installs Debian's packages and the release. Returns
    /// once the download began; the run is followed in the overview.
    ///
    /// Refused while background work runs: the package's upgrade restarts
    /// the agents, which first see their running operations through (a
    /// copy, a share's permissions, a pool's change), for hours maybe.
    pub async fn apply(&self, actor: &Actor, request: &RequestContext) -> Result<(), UpdatesError> {
        let busy = Arc::clone(&self.busy).try_lock_owned().map_err(|_| UpdatesError::Busy)?;
        let running = self.ports.work.running().await;
        if !running.is_empty() {
            self.audit("system.update", Outcome::Denied, actor, request, "").await;
            return Err(UpdatesError::JobsRunning(running.join(", ")));
        }
        let latest = self.ports.host.latest(request).await?;
        if latest.as_ref().is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        // This run replaces the NAS's record of the last one.
        self.remember(latest.as_ref()).await?;
        let mut check = self.ports.store.last_check().await?.ok_or(UpdatesError::NotChecked)?;
        let run = self.last_update(latest).await?;
        // The agent installs only what the channel takes, and is told it.
        let channel = self.ports.store.settings().await?.channel;
        self.forget_installed(&mut check, run.as_ref(), channel);
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
                // A job may have started while the release came.
                let running = this.ports.work.running().await;
                if !running.is_empty() {
                    return Err(HostError::Refused(format!("background work runs: {}", running.join(", "))));
                }
                this.ports.host.start(release.as_ref(), channel, &context).await
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
        {
            let _saving = self.saving.lock().await;
            let before = self.ports.store.settings().await?;
            self.ports.store.save_settings(&settings).await?;
            if before.channel != settings.channel {
                // What was found was for the other channel.
                if let Some(mut check) = self.ports.store.last_check().await? {
                    check.release = None;
                    self.ports.store.save_check(&check).await?;
                }
            }
        }
        self.audit("system.update_settings", Outcome::Success, actor, request, settings.channel.as_str())
            .await;
        Ok(settings)
    }

    /// Runs every [`TICK`]: says how a finished update went, once, and
    /// looks for updates when a day passed without.
    pub async fn tick(&self) -> Result<(), UpdatesError> {
        let request = RequestContext::default();
        let latest = self.ports.host.latest(&request).await?;
        self.remember(latest.as_ref()).await?;
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

    /// Whether an update is under way here: started, its release coming,
    /// or a run of apt installing. A reboot would cut it off.
    async fn under_way(&self) -> bool {
        if self.busy.try_lock().is_err() || self.phase() == Phase::Downloading {
            return true;
        }
        match self.ports.host.latest(&RequestContext::default()).await {
            Ok(run) => run.is_some_and(|run| run.state == RunState::Running),
            Err(error) => {
                tracing::warn!(%error, "updates: cannot tell whether a run goes on");
                false
            }
        }
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

/// An update under way is background work: reboots wait for it.
#[async_trait::async_trait]
impl RunningWork for Updates {
    async fn running(&self) -> Vec<String> {
        if self.under_way().await { vec!["system.update".to_owned()] } else { Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;
    use zendiloos_notify::testing::MemoryNotifications;

    use super::*;
    use crate::domain::HostReport;
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

    #[derive(Default)]
    struct Work(std::sync::Mutex<Vec<String>>);
    #[async_trait]
    impl RunningWork for Work {
        async fn running(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct Setup {
        updates: Updates,
        releases: Arc<FakeReleases>,
        host: Arc<FakeHost>,
        store: Arc<MemoryStore>,
        said: Arc<MemoryNotifications>,
        work: Arc<Work>,
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
        let work = Arc::new(Work::default());
        let ports = UpdatesPorts {
            releases: releases.clone(),
            host: host.clone(),
            store: store.clone(),
            notifier: Notifier::new(said.clone(), Arc::new(Fixed)),
            work: work.clone(),
            audit: Arc::new(NoAudit),
            clock: Arc::new(Fixed),
        };
        Setup { updates: Updates::new(ports, "0.1.0"), releases, host, store, said, work }
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

    /// apt as [`FakeHost`], whose check waits until `go` once it began.
    struct SlowCheck {
        host: FakeHost,
        began: tokio::sync::Notify,
        go: tokio::sync::Notify,
    }

    #[async_trait]
    impl HostPackages for SlowCheck {
        async fn check(&self, context: &RequestContext) -> Result<HostReport, HostError> {
            self.began.notify_one();
            self.go.notified().await;
            self.host.check(context).await
        }

        async fn start(
            &self,
            release: Option<&Release>,
            channel: Channel,
            context: &RequestContext,
        ) -> Result<String, HostError> {
            self.host.start(release, channel, context).await
        }

        async fn latest(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
            self.host.latest(context).await
        }
    }

    #[tokio::test]
    async fn a_check_that_ran_while_the_channel_changed_offers_what_the_new_channel_takes() {
        let s = setup();
        let host = Arc::new(SlowCheck {
            host: FakeHost::default(),
            began: tokio::sync::Notify::new(),
            go: tokio::sync::Notify::new(),
        });
        let updates = Updates::new(UpdatesPorts { host: host.clone(), ..s.updates.ports.clone() }, "0.1.0");
        let request = RequestContext::default();
        let mut settings = updates.settings().await.unwrap();
        settings.channel = Channel::Development;
        updates.set_settings(settings, &alice(), &request).await.unwrap();

        // Looked for on the development channel; back on stable while apt
        // fetches its lists.
        updates.start_check(&alice(), &request).await.unwrap();
        host.began.notified().await;
        settings.channel = Channel::Stable;
        updates.set_settings(settings, &alice(), &request).await.unwrap();
        host.go.notify_one();
        for _ in 0..200 {
            if s.store.check.lock().unwrap().as_ref().is_some_and(|check| check.host.is_some()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let overview = updates.overview(&request).await.unwrap();
        assert_eq!(overview.settings.channel, Channel::Stable);
        let offered = overview.check.and_then(|check| check.release).map(|release| release.tag);
        assert_eq!(offered.as_deref(), Some("v0.2.0"), "the stable release, not the development build");
        let said = s.said.all();
        assert_eq!(said.last().map(|n| n.params["version"].as_str()), Some("0.2.0"));
    }

    #[tokio::test]
    async fn a_release_the_channel_does_not_take_is_neither_offered_nor_installed() {
        let s = setup();
        let request = RequestContext::default();
        // Kept from before, for another channel, however it got there.
        *s.store.check.lock().unwrap() = Some(CheckResult {
            checked_at: zendiloos_kernel::Timestamp::from_unix_seconds(40),
            release: Some(release("dev", "0.3.0~rc.1+dev.1", true)),
            release_error: None,
            host: None,
            host_error: None,
        });
        let overview = s.updates.overview(&request).await.unwrap();
        assert_eq!(overview.check.unwrap().release, None);
        assert!(matches!(s.updates.apply(&alice(), &request).await, Err(UpdatesError::NothingToDo)));
        assert!(s.releases.downloaded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_overview_says_what_changed_in_the_version_that_runs_if_its_package_says() {
        let request = RequestContext::default();
        let without = setup().updates.overview(&request).await.unwrap();
        assert_eq!((without.current.as_str(), without.current_notes), ("0.1.0", None));

        let updates = setup().updates.with_current_notes(Some("- Initial package".into()));
        let with = updates.overview(&request).await.unwrap();
        assert_eq!(with.current_notes.as_deref(), Some("- Initial package"));
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
        assert_eq!(
            *s.host.channels.lock().unwrap(),
            [Channel::Stable],
            "the agent checks the release against it"
        );
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
    async fn no_update_starts_while_background_work_runs() {
        let s = setup();
        let request = RequestContext::default();
        s.updates.check(None, &request).await.unwrap();
        *s.work.0.lock().unwrap() = vec!["files.copy photos".into(), "storage.scrub tank".into()];
        match s.updates.apply(&alice(), &request).await {
            Err(UpdatesError::JobsRunning(named)) => {
                assert_eq!(named, "files.copy photos, storage.scrub tank")
            }
            other => panic!("not refused: {other:?}"),
        }
        assert!(s.releases.downloaded.lock().unwrap().is_empty());
        assert!(s.host.started.lock().unwrap().is_empty());
        assert_eq!(s.updates.overview(&request).await.unwrap().phase, Phase::Idle);

        s.work.0.lock().unwrap().clear();
        s.updates.apply(&alice(), &request).await.unwrap();
    }

    #[tokio::test]
    async fn an_update_under_way_is_background_work() {
        let s = setup();
        assert!(RunningWork::running(&s.updates).await.is_empty(), "nothing goes on");
        *s.host.run.lock().unwrap() = Some(UpdateRun {
            id: "run1".into(),
            installs: true,
            state: RunState::Running,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: None,
            error: None,
            zendiloos: None,
            report: None,
        });
        assert_eq!(RunningWork::running(&s.updates).await, ["system.update"], "apt installs");
        s.host.run.lock().unwrap().take();
        let busy = s.updates.busy.clone().try_lock_owned().unwrap();
        assert_eq!(RunningWork::running(&s.updates).await, ["system.update"], "being started");
        drop(busy);
        assert!(RunningWork::running(&s.updates).await.is_empty());
    }

    #[tokio::test]
    async fn a_job_started_while_the_release_came_stops_the_update() {
        let s = setup();
        let request = RequestContext::default();
        s.updates.check(None, &request).await.unwrap();
        // apply returns once the download began; the job comes meanwhile.
        s.updates.apply(&alice(), &request).await.unwrap();
        *s.work.0.lock().unwrap() = vec!["files.copy photos".into()];
        let mut phase = Phase::Idle;
        for _ in 0..100 {
            phase = s.updates.overview(&request).await.unwrap().phase;
            if matches!(phase, Phase::Failed(_)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(matches!(phase, Phase::Failed(ref why) if why.contains("files.copy")), "{phase:?}");
        assert!(s.host.started.lock().unwrap().is_empty(), "the run did not start");
    }

    #[tokio::test]
    async fn a_check_right_after_an_update_does_not_swallow_how_it_went() {
        let s = setup();
        let request = RequestContext::default();
        *s.host.run.lock().unwrap() = Some(UpdateRun {
            id: "run9".into(),
            installs: true,
            state: RunState::Failed,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: Some(Timestamp::from_unix_seconds(150)),
            error: Some("dpkg was interrupted".into()),
            zendiloos: None,
            report: None,
        });

        // Before the next tick: the check replaces the NAS's record.
        s.updates.check(None, &request).await.unwrap();

        let failed: Vec<_> = s.said.all().into_iter().filter(|n| n.code == "updates_failed").collect();
        assert_eq!(failed.len(), 1, "how it went is said");
        assert_eq!(failed[0].params["error"], "dpkg was interrupted");
        let run = s.updates.overview(&request).await.unwrap().run.expect("the update is still shown");
        assert_eq!((run.id.as_str(), run.state), ("run9", RunState::Failed));
        s.updates.tick().await.unwrap();
        assert_eq!(s.said.all().iter().filter(|n| n.code == "updates_failed").count(), 1, "and said once");
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
