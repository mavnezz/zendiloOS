//! Looking for updates, installing them, and saying so.

pub mod ports;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{
    Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, RunningWork, WorkHold,
};
use zendiloos_notify::{Level, NewNotification, Notifier};

use self::ports::{HostError, HostPackages, ReleaseSource, UpdateStore};
use crate::domain::{
    Channel, CheckResult, HostReport, Release, RunState, UpdateRun, UpdateSettings, compare_versions,
    newest_for,
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
    /// The update going on, else the last one. Never a check or a
    /// plugin's installation: they run apt too, but the page would show
    /// their steps as an update's.
    pub run: Option<UpdateRun>,
    // The check's report and the run's name the kernel that runs now, not
    // the one that ran when they were read: a restart since started the
    // kernel it was needed for.
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
    /// An update or finishing one is decided and being started: new work
    /// is held back from then on ([`WorkHold`]), and once its run is on
    /// the NAS, for as long as that runs.
    starting: Arc<Mutex<bool>>,
}

/// Holds new work back while it lives, from before an update looks at what
/// runs until its run is on the NAS (or did not start).
struct Starting(Arc<Mutex<bool>>);

impl Starting {
    fn new(flag: &Arc<Mutex<bool>>) -> Self {
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        Self(Arc::clone(flag))
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }
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
            starting: Arc::new(Mutex::new(false)),
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
        let (latest, update) = self.runs(request).await?;
        let mut run = self.shown(latest, update).await?;
        let mut check = self.ports.store.last_check().await?;
        if let Some(check) = &mut check {
            self.forget_installed(check, run.as_ref(), settings.channel);
        }
        let reports = check.as_mut().and_then(|check| check.host.as_mut());
        let reports = reports.into_iter().chain(run.as_mut().and_then(|run| run.report.as_mut()));
        self.now_running(reports, request).await?;
        Ok(UpdateOverview {
            current: self.current.clone(),
            current_notes: self.current_notes.clone(),
            settings,
            check,
            run,
            phase: self.phase(),
        })
    }

    /// The NAS's latest run of apt, whatever it did, and its last update.
    async fn runs(
        &self,
        request: &RequestContext,
    ) -> Result<(Option<UpdateRun>, Option<UpdateRun>), UpdatesError> {
        let latest = self.ports.host.latest(request).await?;
        let update = self.ports.host.last_update(request).await?;
        Ok((latest, update))
    }

    /// The run the page shows: an update going on, else the last update,
    /// as the NAS keeps it or, from before it kept its own, as kept here.
    /// A check going on is the phase's to say (`Phase::Checking`), and a
    /// plugin being installed is its job's.
    async fn shown(
        &self,
        latest: Option<UpdateRun>,
        update: Option<UpdateRun>,
    ) -> Result<Option<UpdateRun>, UpdatesError> {
        match (latest, update) {
            (Some(run), _) if run.state == RunState::Running && run.installs => Ok(Some(run)),
            (_, Some(update)) => Ok(Some(update)),
            (Some(run), None) if run.installs => Ok(Some(run)),
            _ => Ok(self.ports.store.last_run().await?),
        }
    }

    /// Says how a finished update went, once, and keeps it: when a check or
    /// another update starts, and on each tick. Returns whether it told of
    /// one that failed.
    async fn remember(&self, run: Option<&UpdateRun>) -> Result<bool, UpdatesError> {
        if let Some(run) = run
            && run.installs
            && run.state != RunState::Running
            && self.ports.store.said("run").await?.as_deref() != Some(run.id.as_str())
        {
            self.ports.store.save_last_run(run).await?;
            self.ports.store.save_said("run", &run.id).await?;
            // Told after a restart, it needs none.
            let mut told = run.clone();
            self.now_running(told.report.as_mut(), &RequestContext::default()).await?;
            self.say_finished(&told).await;
            return Ok(run.state == RunState::Failed);
        }
        Ok(false)
    }

    /// Names the kernel that runs now in reports read before: after a
    /// restart another may run, the one the restart was needed for. Where
    /// the NAS does not say, they stay as they were read.
    async fn now_running<'a>(
        &self,
        reports: impl IntoIterator<Item = &'a mut HostReport>,
        request: &RequestContext,
    ) -> Result<(), UpdatesError> {
        if let Some(kernel) = self.ports.host.running_kernel(request).await? {
            for report in reports {
                report.running_kernel.clone_from(&kernel);
            }
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
        let (latest, update) = self.runs(request).await?;
        if latest.as_ref().is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        self.remember(updated(latest.as_ref(), update.as_ref())).await?;
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
        // Held before looking: a job asked to start meanwhile is seen here
        // or sees the hold.
        let starting = Starting::new(&self.starting);
        let running = self.ports.work.running().await;
        if !running.is_empty() {
            self.audit("system.update", Outcome::Denied, actor, request, "").await;
            return Err(UpdatesError::JobsRunning(running.join(", ")));
        }
        let (latest, update) = self.runs(request).await?;
        if latest.as_ref().is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        // This run replaces the NAS's record of the last one.
        self.remember(updated(latest.as_ref(), update.as_ref())).await?;
        let mut check = self.ports.store.last_check().await?.ok_or(UpdatesError::NotChecked)?;
        let run = self.shown(latest, update).await?;
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
                    this.until_recorded(&id, starting).await;
                }
                Err(error) => {
                    tracing::warn!(%error, "updates: the update could not start");
                    this.set_phase(Phase::Failed(error.to_string()));
                }
            }
        });
        Ok(())
    }

    /// Finishes an update that was cut off (a power cut while dpkg
    /// configured, say): dpkg configures what it unpacked, apt completes
    /// what is missing, in a run on the NAS like an update's, followed in
    /// the overview. Returns once it began. Refused while background work
    /// runs, as an update is: zendiloOS's own package may be among what
    /// is configured.
    pub async fn finish(&self, actor: &Actor, request: &RequestContext) -> Result<(), UpdatesError> {
        let _busy = self.busy.try_lock().map_err(|_| UpdatesError::Busy)?;
        let starting = Starting::new(&self.starting);
        let running = self.ports.work.running().await;
        if !running.is_empty() {
            self.audit("system.update_finish", Outcome::Denied, actor, request, "").await;
            return Err(UpdatesError::JobsRunning(running.join(", ")));
        }
        let (latest, update) = self.runs(request).await?;
        if latest.as_ref().is_some_and(|run| run.state == RunState::Running) {
            return Err(UpdatesError::Busy);
        }
        self.remember(updated(latest.as_ref(), update.as_ref())).await?;
        let started = self.ports.host.finish(request).await;
        let outcome = if started.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("system.update_finish", outcome, actor, request, "").await;
        let id = started?;
        tracing::info!(run = id, "updates: finishing the update that was cut off");
        let this = self.clone();
        tokio::spawn(async move { this.until_recorded(&id, starting).await });
        Ok(())
    }

    /// Keeps `starting` until the run `id` is on the NAS's record (a moment
    /// after systemd started it, a minute at most), which holds new work
    /// back from then on while it runs.
    async fn until_recorded(&self, id: &str, starting: Starting) {
        for _ in 0..300 {
            match self.ports.host.latest(&RequestContext::default()).await {
                Ok(Some(run)) if run.id == id => break,
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        drop(starting);
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
    /// looks for updates when a day passed without. After an update that
    /// failed it looks at once: whether dpkg was left half done (a run cut
    /// off by a restart) is what the page then offers to finish.
    pub async fn tick(&self) -> Result<(), UpdatesError> {
        let request = RequestContext::default();
        let (latest, update) = self.runs(&request).await?;
        let failed = self.remember(updated(latest.as_ref(), update.as_ref())).await?;
        let settings = self.ports.store.settings().await?;
        let last = self.ports.store.last_check().await?.map(|check| check.checked_at);
        let due =
            last.is_none_or(|at| self.ports.clock.now().unix_seconds() - at.unix_seconds() >= DAY_SECONDS);
        if (settings.check_daily && due) || failed {
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

/// The last update on the NAS: its own record of it, or, on a NAS whose
/// last update a runner made before it kept that record apart, the latest
/// run while it is that update.
fn updated<'a>(latest: Option<&'a UpdateRun>, update: Option<&'a UpdateRun>) -> Option<&'a UpdateRun> {
    update.or(latest.filter(|run| run.installs))
}

/// While an update is started or installs, no new job starts: the
/// package's upgrade restarts the agents, which see their operations
/// through first, and zendiloosd, which cuts its own jobs off; Debian's
/// updates may restart Docker and Samba under them.
#[async_trait::async_trait]
impl WorkHold for Updates {
    async fn holding(&self) -> Option<String> {
        if *self.starting.lock().unwrap_or_else(PoisonError::into_inner) {
            return Some("system.update".to_owned());
        }
        match self.ports.host.latest(&RequestContext::default()).await {
            Ok(Some(run)) if run.installs && run.state == RunState::Running => {
                Some("system.update".to_owned())
            }
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(%error, "updates: cannot tell whether an update installs");
                None
            }
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

        async fn finish(&self, context: &RequestContext) -> Result<String, HostError> {
            self.host.finish(context).await
        }

        async fn latest(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
            self.host.latest(context).await
        }

        async fn last_update(&self, context: &RequestContext) -> Result<Option<UpdateRun>, HostError> {
            self.host.last_update(context).await
        }

        async fn running_kernel(&self, context: &RequestContext) -> Result<Option<String>, HostError> {
            self.host.running_kernel(context).await
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
    async fn a_restart_into_the_kernel_an_update_brought_needs_no_check_to_be_seen() {
        let s = setup();
        let request = RequestContext::default();
        s.updates.check(None, &request).await.unwrap();
        s.updates.apply(&alice(), &request).await.unwrap();
        for _ in 0..100 {
            if s.updates.overview(&request).await.unwrap().run.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let before = s.updates.overview(&request).await.unwrap();
        assert!(before.check.unwrap().host.unwrap().restart_needed());
        assert!(before.run.unwrap().report.unwrap().restart_needed());

        // Restarted before anything was looked for or said again.
        *s.host.running.lock().unwrap() = "6.12.57+deb13-amd64".into();
        let after = s.updates.overview(&request).await.unwrap();
        let host = after.check.unwrap().host.unwrap();
        assert_eq!(host.running_kernel, "6.12.57+deb13-amd64");
        assert!(!host.restart_needed());
        assert!(!after.run.unwrap().report.unwrap().restart_needed(), "the run offers no restart");
        let kept = s.store.check.lock().unwrap().clone().unwrap().host.unwrap();
        assert_eq!(kept.running_kernel, "6.12.48+deb13-amd64", "what was read is kept as read");

        s.updates.tick().await.unwrap();
        let finished: Vec<_> = s.said.all().into_iter().filter(|n| n.code == "updates_installed").collect();
        assert_eq!(finished[0].params["restart"], "false", "told after the restart");
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

    /// GitHub as [`FakeReleases`], whose download waits until `go` once it
    /// began.
    struct SlowDownload {
        releases: Arc<FakeReleases>,
        began: tokio::sync::Notify,
        go: tokio::sync::Notify,
    }

    #[async_trait]
    impl ReleaseSource for SlowDownload {
        async fn releases(&self) -> Result<Vec<Release>, String> {
            self.releases.releases().await
        }

        async fn download(&self, release: &Release) -> Result<(), String> {
            self.began.notify_one();
            self.go.notified().await;
            self.releases.download(release).await
        }
    }

    #[tokio::test]
    async fn new_work_waits_from_the_decision_to_update_until_the_update_installed() {
        let s = setup();
        let request = RequestContext::default();
        let releases = Arc::new(SlowDownload {
            releases: s.releases.clone(),
            began: tokio::sync::Notify::new(),
            go: tokio::sync::Notify::new(),
        });
        let updates =
            Updates::new(UpdatesPorts { releases: releases.clone(), ..s.updates.ports.clone() }, "0.1.0");
        assert_eq!(WorkHold::holding(&updates).await, None, "nothing holds");

        updates.check(None, &request).await.unwrap();
        updates.apply(&alice(), &request).await.unwrap();
        releases.began.notified().await;
        assert_eq!(
            WorkHold::holding(&updates).await.as_deref(),
            Some("system.update"),
            "while the release comes"
        );
        releases.go.notify_one();
        for _ in 0..200 {
            if WorkHold::holding(&updates).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*s.host.started.lock().unwrap(), [Some("0.2.0".to_owned())]);
        assert_eq!(WorkHold::holding(&updates).await, None, "the fake run ended at once");

        // A run that installs holds new work back until it ends, finishing
        // an update too; a check does not.
        let run = |id: &str, installs: bool, finishes: bool, state: RunState| UpdateRun {
            id: id.into(),
            installs,
            finishes,
            state,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: None,
            error: None,
            zendiloos: None,
            report: None,
        };
        s.host.record(run("run7", true, false, RunState::Running));
        assert_eq!(WorkHold::holding(&updates).await.as_deref(), Some("system.update"));
        s.host.record(run("finish1", true, true, RunState::Running));
        assert_eq!(WorkHold::holding(&updates).await.as_deref(), Some("system.update"));
        s.host.record(run("check9", false, false, RunState::Running));
        assert_eq!(WorkHold::holding(&updates).await, None);
    }

    #[tokio::test]
    async fn an_update_refused_for_running_work_holds_nothing_back() {
        let s = setup();
        let request = RequestContext::default();
        s.updates.check(None, &request).await.unwrap();
        *s.work.0.lock().unwrap() = vec!["containers.volume_backup db".into()];
        assert!(matches!(s.updates.apply(&alice(), &request).await, Err(UpdatesError::JobsRunning(_))));
        assert!(matches!(s.updates.finish(&alice(), &request).await, Err(UpdatesError::JobsRunning(_))));
        assert_eq!(WorkHold::holding(&s.updates).await, None);
    }

    #[tokio::test]
    async fn an_update_under_way_is_background_work() {
        let s = setup();
        assert!(RunningWork::running(&s.updates).await.is_empty(), "nothing goes on");
        s.host.record(UpdateRun {
            id: "run1".into(),
            installs: true,
            finishes: false,
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

    /// A runner from before the last update's record was kept apart (the
    /// one that installed this version): a check replaces its only record.
    #[tokio::test]
    async fn a_check_right_after_an_update_does_not_swallow_how_it_went() {
        let s = setup();
        let request = RequestContext::default();
        *s.host.run.lock().unwrap() = Some(UpdateRun {
            id: "run9".into(),
            installs: true,
            finishes: false,
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
    async fn a_check_or_a_plugin_going_on_is_not_shown_as_an_update() {
        let s = setup();
        let request = RequestContext::default();
        let run = |id: &str, installs: bool, state: RunState| UpdateRun {
            id: id.into(),
            installs,
            finishes: false,
            state,
            step: "lists".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: (state != RunState::Running).then(|| Timestamp::from_unix_seconds(150)),
            error: None,
            zendiloos: None,
            report: None,
        };
        s.host.record(run("run9", true, RunState::Succeeded));

        // A check (or Docker, NUT, rsync being installed) runs apt: the
        // last update stays what the page shows.
        s.host.record(run("look1", false, RunState::Running));
        let shown = s.updates.overview(&request).await.unwrap().run.expect("the last update");
        assert_eq!((shown.id.as_str(), shown.state), ("run9", RunState::Succeeded));
        // Nothing was ever updated: nothing is shown.
        let fresh = setup();
        fresh.host.record(run("look1", false, RunState::Running));
        assert!(fresh.updates.overview(&request).await.unwrap().run.is_none());

        // An update going on is.
        s.host.record(run("run10", true, RunState::Running));
        let shown = s.updates.overview(&request).await.unwrap().run.expect("the update going on");
        assert_eq!((shown.id.as_str(), shown.state), ("run10", RunState::Running));
    }

    #[tokio::test]
    async fn a_plugin_installed_right_after_an_update_leaves_how_the_update_went() {
        let s = setup();
        let request = RequestContext::default();
        let run = |id: &str, installs: bool, state: RunState, error: Option<&str>| UpdateRun {
            id: id.into(),
            installs,
            finishes: false,
            state,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: Some(Timestamp::from_unix_seconds(150)),
            error: error.map(str::to_owned),
            zendiloos: None,
            report: None,
        };
        s.host.record(run("run9", true, RunState::Failed, Some("dpkg was interrupted")));
        // Before the next tick, Docker is installed: a run of its own.
        s.host.record(run("dock1", false, RunState::Succeeded, None));

        s.updates.tick().await.unwrap();
        let failed: Vec<_> = s.said.all().into_iter().filter(|n| n.code == "updates_failed").collect();
        assert_eq!(failed.len(), 1, "how the update went is said");
        assert_eq!(failed[0].params["error"], "dpkg was interrupted");
        let shown = s.updates.overview(&request).await.unwrap().run.expect("the update is shown");
        assert_eq!((shown.id.as_str(), shown.state), ("run9", RunState::Failed));
        s.updates.tick().await.unwrap();
        assert_eq!(s.said.all().iter().filter(|n| n.code == "updates_failed").count(), 1, "once");
    }

    #[tokio::test]
    async fn an_update_cut_off_while_dpkg_configured_is_found_and_finished() {
        let s = setup();
        let request = RequestContext::default();
        s.releases.releases.lock().unwrap().clear();
        // A power cut while DKMS built ZFS for the new kernel: apt sees
        // nothing to upgrade, dpkg lists what it did not configure.
        s.host.packages.lock().unwrap().clear();
        *s.host.interrupted.lock().unwrap() = true;
        s.host.record(UpdateRun {
            id: "run9".into(),
            installs: true,
            finishes: false,
            state: RunState::Failed,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(10),
            finished_at: None,
            error: Some("the run stopped before it finished (was the NAS restarted?)".into()),
            zendiloos: None,
            report: None,
        });

        // The tick tells of it and looks at once.
        s.updates.tick().await.unwrap();
        let overview = s.updates.overview(&request).await.unwrap();
        assert!(
            overview
                .check
                .as_ref()
                .and_then(|check| check.host.as_ref())
                .is_some_and(|host| host.interrupted)
        );
        assert!(matches!(s.updates.apply(&alice(), &request).await, Err(UpdatesError::NothingToDo)));

        *s.work.0.lock().unwrap() = vec!["storage.scrub tank".into()];
        let refused = s.updates.finish(&alice(), &request).await;
        assert!(matches!(refused, Err(UpdatesError::JobsRunning(_))), "{refused:?}");
        s.work.0.lock().unwrap().clear();

        s.updates.finish(&alice(), &request).await.unwrap();
        assert_eq!(*s.host.finished.lock().unwrap(), 1);
        let overview = s.updates.overview(&request).await.unwrap();
        let run = overview.run.unwrap();
        assert!(run.finishes && run.state == RunState::Succeeded, "{run:?}");
        let host = overview.check.unwrap().host.unwrap();
        assert!(!host.interrupted, "the end of the run says dpkg is done");

        s.updates.tick().await.unwrap();
        let installed = s.said.all().into_iter().filter(|n| n.code == "updates_installed").count();
        assert_eq!(installed, 1, "how it went is said");
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
