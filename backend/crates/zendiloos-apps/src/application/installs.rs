//! Apps installed from a catalog the earlier way, each with a dataset and
//! an account of its own and its Compose file kept by the agent. Apps are
//! no longer installed so (the App Center makes administrators' projects
//! from templates now, see [`super::Templates`]); those there are keep
//! running: they are started, stopped and removed here, which undoes all
//! of it.

use std::sync::Arc;

use zendiloos_jobs::{Job, JobError, Jobs, Progress};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext};

use super::AppsError;
use super::ports::{AppHost, AppHostError, AppShares, InstalledRepository};
use crate::domain::{AppVolume, Containers, InstallState, InstalledApp};

/// An installed app with what Docker makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledView {
    pub app: InstalledApp,
    /// `None` if Docker could not be asked.
    pub containers: Option<Containers>,
}

#[derive(Clone)]
pub struct InstallPorts {
    pub installed: Arc<dyn InstalledRepository>,
    pub host: Arc<dyn AppHost>,
    pub shares: Arc<dyn AppShares>,
    pub jobs: Jobs,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct Installs {
    ports: InstallPorts,
    /// Changing an app's state, one at a time.
    changes: Arc<tokio::sync::Mutex<()>>,
}

impl From<AppHostError> for AppsError {
    fn from(error: AppHostError) -> Self {
        match error {
            AppHostError::Refused(reason) => AppsError::Refused(reason),
            AppHostError::Failed(reason) => AppsError::HostFailed(reason),
            AppHostError::Port(error) => AppsError::Port(error),
        }
    }
}

fn job_error(error: &AppHostError) -> JobError {
    let code = match error {
        AppHostError::Refused(_) => "refused",
        AppHostError::Failed(_) | AppHostError::Port(_) => "agent_failed",
    };
    let detail = match error {
        AppHostError::Refused(reason) | AppHostError::Failed(reason) => reason.clone(),
        AppHostError::Port(error) => error.to_string(),
    };
    JobError::new(code, Some(detail))
}

impl Installs {
    pub fn new(ports: InstallPorts) -> Self {
        Self { ports, changes: Arc::new(tokio::sync::Mutex::new(())) }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry =
            AuditEntry::new(self.ports.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    /// Apps a job worked on when zendiloosd stopped: the job was
    /// interrupted, so they are failed, to be removed.
    pub async fn recover(&self) -> Result<usize, AppsError> {
        let mut recovered = 0;
        for mut app in self.ports.installed.list().await? {
            if app.state.is_busy() {
                app.state = InstallState::Failed;
                app.updated_at = self.ports.clock.now();
                self.ports.installed.update(&app).await?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    async fn find(&self, id: &str) -> Result<InstalledApp, AppsError> {
        self.ports.installed.find(id).await?.ok_or(AppsError::NotInstalled)
    }

    /// The installed apps by id, with their containers.
    pub async fn list(&self, request: &RequestContext) -> Result<Vec<InstalledView>, AppsError> {
        let installed = self.ports.installed.list().await?;
        if installed.is_empty() {
            return Ok(Vec::new());
        }
        let status = match self.ports.host.status(request).await {
            Ok(status) => Some(status),
            Err(error) => {
                tracing::warn!(%error, "apps: cannot ask Docker about the apps");
                None
            }
        };
        Ok(installed
            .into_iter()
            .map(|app| {
                let containers =
                    status.as_ref().map(|status| status.get(&app.id).copied().unwrap_or_default());
                InstalledView { app, containers }
            })
            .collect())
    }

    pub async fn get(&self, id: &str, request: &RequestContext) -> Result<InstalledView, AppsError> {
        self.list(request).await?.into_iter().find(|view| view.app.id == id).ok_or(AppsError::NotInstalled)
    }

    pub async fn start(&self, id: &str, actor: &Actor, context: &RequestContext) -> Result<(), AppsError> {
        self.run_or_stop(id, true, actor, context).await
    }

    pub async fn stop(&self, id: &str, actor: &Actor, context: &RequestContext) -> Result<(), AppsError> {
        self.run_or_stop(id, false, actor, context).await
    }

    async fn run_or_stop(
        &self,
        id: &str,
        start: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), AppsError> {
        let app = self.find(id).await?;
        if app.state != InstallState::Installed {
            return Err(AppsError::Busy);
        }
        let done = if start {
            self.ports.host.start(id, context).await
        } else {
            self.ports.host.stop(id, context).await
        };
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit(if start { "apps.start" } else { "apps.stop" }, outcome, id, actor, context).await;
        Ok(done?)
    }

    pub async fn logs(&self, id: &str, lines: u32, context: &RequestContext) -> Result<String, AppsError> {
        self.find(id).await?;
        Ok(self.ports.host.logs(id, lines, context).await?)
    }

    /// Removes the app with all its data, as a job. `confirm` must repeat
    /// the app's id.
    pub async fn uninstall(
        &self,
        id: &str,
        confirm: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, AppsError> {
        let changes = self.changes.lock().await;
        let mut app = self.find(id).await?;
        if confirm.trim() != app.id {
            return Err(AppsError::ConfirmationMismatch);
        }
        if app.state.is_busy() {
            return Err(AppsError::Busy);
        }
        app.state = InstallState::Removing;
        app.updated_at = self.ports.clock.now();
        self.ports.installed.update(&app).await?;
        drop(changes);

        let this = self.clone();
        let owned = context.clone();
        let job = self
            .ports
            .jobs
            .start("apps.uninstall", Some(app.id.clone()), Some(actor), move |progress| async move {
                let outcome = this.take_away(&app, &progress, &owned).await;
                let recorded = match &outcome {
                    Ok(()) => this.ports.installed.delete(&app.id).await,
                    Err(_) => {
                        let mut failed = app.clone();
                        failed.state = InstallState::Failed;
                        failed.updated_at = this.ports.clock.now();
                        this.ports.installed.update(&failed).await
                    }
                };
                if let Err(error) = recorded {
                    tracing::error!(%error, app = app.id, "apps: cannot record the removal");
                }
                outcome.map_err(|error| job_error(&error))
            })
            .await?;
        self.audit("apps.uninstall", Outcome::Success, id, actor, context).await;
        Ok(job)
    }

    async fn take_away(
        &self,
        app: &InstalledApp,
        progress: &Progress,
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let volume = AppVolume { pool: app.pool.clone(), number: app.volume };
        progress.step("removing", Some(0.1)).await;
        self.ports.host.remove(&app.id, &volume, context).await?;
        // The share permissions name the account: they go before it.
        progress.step("shares", Some(0.6)).await;
        self.ports.shares.grant(&app.account(), &[], context).await?;
        progress.step("account", Some(0.9)).await;
        self.ports.host.delete_account(&app.id, context).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use zendiloos_jobs::JobState;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_kernel::{PortError, Timestamp};

    use super::*;
    use crate::application::ports::InstalledRepository;
    use crate::domain::{Access, InstallValues, Share};
    use crate::testing::{FakeAppHost, FakeShares, MemoryInstalled, app};

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_793_836_800)
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<String>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(format!("{} {}", entry.action, entry.target.unwrap_or_default()));
            Ok(())
        }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    struct Setup {
        installs: Installs,
        installed: Arc<MemoryInstalled>,
        host: Arc<FakeAppHost>,
        shares: Arc<FakeShares>,
        jobs: Jobs,
    }

    fn setup() -> Setup {
        let installed = Arc::new(MemoryInstalled::default());
        let host = Arc::new(FakeAppHost::default());
        let shares = Arc::new(FakeShares::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let installs = Installs::new(InstallPorts {
            installed: installed.clone(),
            host: host.clone(),
            shares: shares.clone(),
            jobs: jobs.clone(),
            audit: Arc::new(Audit::default()),
            clock: Arc::new(Fixed),
        });
        Setup { installs, installed, host, shares, jobs }
    }

    /// Immich as installed the earlier way, with the share `family`.
    async fn immich(s: &Setup, state: InstallState) {
        let mut catalog = app("immich", "Immich");
        catalog.shares = vec![Share {
            id: "photos".into(),
            label: "Photos".into(),
            description: None,
            access: Access::ReadWrite,
        }];
        let mut values = InstallValues::default();
        values.shares.insert("photos".into(), "family".into());
        let at = Timestamp::from_unix_seconds(1_793_000_000);
        let installed = InstalledApp {
            id: "immich".into(),
            source_id: 1,
            app: catalog,
            pool: "tank".into(),
            volume: 1,
            values,
            state,
            installed_at: at,
            updated_at: at,
        };
        assert!(s.installed.create(&installed).await.unwrap());
        s.shares.grant("app-immich", &[("family".into(), true)], &RequestContext::default()).await.unwrap();
    }

    async fn finished(jobs: &Jobs, job: Job) -> Job {
        for _ in 0..200 {
            let current = jobs.find(job.id).await.unwrap().unwrap();
            if current.finished_at.is_some() {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the job did not finish");
    }

    #[tokio::test]
    async fn an_app_installed_before_keeps_running() {
        let s = setup();
        let context = RequestContext::default();
        assert!(s.installs.list(&context).await.unwrap().is_empty());
        immich(&s, InstallState::Installed).await;

        s.installs.start("immich", &alice(), &context).await.unwrap();
        assert_eq!(s.installs.get("immich", &context).await.unwrap().containers.unwrap().running, 1);
        s.installs.stop("immich", &alice(), &context).await.unwrap();
        let view = s.installs.get("immich", &context).await.unwrap();
        assert_eq!((view.containers.unwrap().running, view.app.state), (0, InstallState::Installed));
        assert_eq!(s.installs.logs("immich", 100, &context).await.unwrap(), "immich: 100 lines\n");
        assert!(matches!(s.installs.get("jellyfin", &context).await, Err(AppsError::NotInstalled)));
        assert_eq!(s.host.calls(), ["start immich", "stop immich"]);
    }

    #[tokio::test]
    async fn a_failed_app_is_only_removed() {
        let s = setup();
        let context = RequestContext::default();
        immich(&s, InstallState::Installing).await;
        assert_eq!(s.installs.recover().await.unwrap(), 1);
        assert_eq!(s.installs.get("immich", &context).await.unwrap().app.state, InstallState::Failed);

        let started = s.installs.start("immich", &alice(), &context).await;
        assert!(matches!(started, Err(AppsError::Busy)), "{started:?}");
        let job = s.installs.uninstall("immich", "immich", &alice(), &context).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
    }

    #[tokio::test]
    async fn removing_takes_the_data_then_the_shares_then_the_account() {
        let s = setup();
        let context = RequestContext::default();
        immich(&s, InstallState::Installed).await;

        let wrong = s.installs.uninstall("immich", "Immich", &alice(), &context).await;
        assert!(matches!(wrong, Err(AppsError::ConfirmationMismatch)));
        let job = s.installs.uninstall("immich", "immich", &alice(), &context).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.host.calls(), ["remove immich", "delete_account immich"]);
        assert!(s.shares.granted("app-immich").is_empty());
        assert!(matches!(s.installs.get("immich", &context).await, Err(AppsError::NotInstalled)));
    }

    #[tokio::test]
    async fn a_removal_that_fails_leaves_the_app_failed() {
        let s = setup();
        let context = RequestContext::default();
        immich(&s, InstallState::Installed).await;
        s.host.fail_at(Some("remove"));

        let job = s.installs.uninstall("immich", "immich", &alice(), &context).await.unwrap();
        let job = finished(&s.jobs, job).await;
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().detail.as_deref(), Some("remove went wrong"));
        assert_eq!(s.installs.get("immich", &context).await.unwrap().app.state, InstallState::Failed);
        assert_eq!(s.shares.granted("app-immich"), [("family".to_owned(), true)], "nothing taken yet");
    }
}
