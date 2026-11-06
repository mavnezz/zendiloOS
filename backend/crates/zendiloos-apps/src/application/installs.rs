//! Installing, running, updating and removing apps from a catalog.
//!
//! Installing records the app with its values, then a job does the work
//! on the host: the app's dataset and account, its access to the shares
//! picked for it, the agent's check against the catalog's policy, the
//! folders it mounts, its images, and the start. A step that fails leaves
//! the app *failed*: it can be tried again or removed, which undoes all of
//! it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use zendiloos_jobs::{Job, JobError, Jobs, Progress};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext};

use super::ports::{
    AppHost, AppHostError, AppShares, DockerHost, HostShare, HostValues, InstalledRepository, ListeningPorts,
    Secrets,
};
use super::{AppsError, Catalogs};
use crate::domain::{
    Access, AppVolume, CatalogApp, Containers, InstallState, InstallValues, InstalledApp, MAX_APP_ID,
    ValueRequest, docker_volume, install_values, share_and_folder,
};

/// What installing an app takes from the administrator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallRequest {
    pub source: i64,
    pub app: String,
    /// Share id → the name of the share picked.
    pub shares: BTreeMap<String, String>,
    /// Port id → host port; missing ones get their default.
    pub ports: BTreeMap<String, u16>,
    /// Setting id → value; missing ones get their default.
    pub settings: BTreeMap<String, String>,
    /// The administrator confirmed what the app may do beyond the policy:
    /// devices, capabilities, the host network.
    pub permissions_confirmed: bool,
    /// The pool its data goes on; Docker's if none.
    pub pool: Option<String>,
}

/// An installed app with what Docker makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledView {
    pub app: InstalledApp,
    /// `None` if Docker could not be asked.
    pub containers: Option<Containers>,
    /// The version its catalog offers now, if it differs.
    pub update: Option<String>,
}

#[derive(Clone)]
pub struct InstallPorts {
    pub catalogs: Catalogs,
    pub installed: Arc<dyn InstalledRepository>,
    pub host: Arc<dyn AppHost>,
    pub docker: Arc<dyn DockerHost>,
    pub shares: Arc<dyn AppShares>,
    pub listening: Arc<dyn ListeningPorts>,
    pub secrets: Arc<dyn Secrets>,
    pub jobs: Jobs,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct Installs {
    ports: InstallPorts,
    /// Checking values and recording apps, one at a time: two apps never
    /// get the same port.
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

/// Whether `new` asks for something `old` did not.
fn asks_more(new: &CatalogApp, old: &CatalogApp) -> bool {
    let (new, old) = (&new.permissions, &old.permissions);
    new.devices.iter().any(|d| !old.devices.contains(d))
        || new.capabilities.iter().any(|c| !old.capabilities.contains(c))
        || (new.host_network && !old.host_network)
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
    /// interrupted, so they are failed, to be tried again or removed.
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

    /// The installed apps by id, with their containers and updates.
    pub async fn list(&self, request: &RequestContext) -> Result<Vec<InstalledView>, AppsError> {
        let installed = self.ports.installed.list().await?;
        let status = match self.ports.host.status(request).await {
            Ok(status) => Some(status),
            Err(error) => {
                tracing::warn!(%error, "apps: cannot ask Docker about the apps");
                None
            }
        };
        let offered: BTreeMap<(i64, String), (String, String)> = match self.ports.catalogs.apps().await {
            Ok(listed) => {
                listed.into_iter().map(|l| ((l.source.id, l.app.id), (l.app.digest, l.app.version))).collect()
            }
            Err(error) => {
                tracing::warn!(%error, "apps: cannot read the catalogs");
                BTreeMap::new()
            }
        };
        Ok(installed
            .into_iter()
            .map(|app| {
                let containers =
                    status.as_ref().map(|status| status.get(&app.id).copied().unwrap_or_default());
                let update = offered
                    .get(&(app.source_id, app.id.clone()))
                    .filter(|(digest, _)| *digest != app.app.digest)
                    .map(|(_, version)| version.clone());
                InstalledView { app, containers, update }
            })
            .collect())
    }

    pub async fn get(&self, id: &str, request: &RequestContext) -> Result<InstalledView, AppsError> {
        self.list(request).await?.into_iter().find(|view| view.app.id == id).ok_or(AppsError::NotInstalled)
    }

    /// Where a new app's data goes: `pool`, one zendiloOS made, or the pool
    /// Docker's data is on. Docker runs either way.
    async fn install_volume(
        &self,
        pool: Option<&str>,
        request: &RequestContext,
    ) -> Result<AppVolume, AppsError> {
        let docker = self.apps_volume(request).await?;
        let Some(pool) = pool.filter(|p| *p != docker.pool) else { return Ok(docker) };
        let volumes = self.ports.docker.volumes(request).await?;
        volumes.into_iter().find(|v| v.pool == pool).ok_or(AppsError::PoolNotFound)
    }

    /// The volume Docker's data is on: where apps go.
    async fn apps_volume(&self, request: &RequestContext) -> Result<AppVolume, AppsError> {
        let state = self.ports.docker.state(request).await?;
        let number = state.data_root.as_deref().and_then(docker_volume).filter(|_| state.running);
        let volumes = self.ports.docker.volumes(request).await?;
        number.and_then(|n| volumes.into_iter().find(|v| v.number == n)).ok_or(AppsError::DockerNotReady)
    }

    /// The values for `app` from what was given, against what the NAS has.
    async fn values(
        &self,
        app: &CatalogApp,
        request: &InstallRequest,
        before: Option<&InstalledApp>,
    ) -> Result<InstallValues, AppsError> {
        let existing_shares: BTreeSet<String> =
            self.ports.shares.shares().await?.into_iter().map(|share| share.name).collect();
        let other_apps: BTreeMap<u16, String> = self
            .ports
            .installed
            .list()
            .await?
            .into_iter()
            .filter(|other| other.id != app.id)
            .flat_map(|other| {
                let id = other.id.clone();
                other.values.ports.into_values().map(move |port| (port, id.clone()))
            })
            .collect();
        let busy = match self.ports.listening.listening().await {
            Ok(busy) => busy,
            Err(error) => {
                tracing::warn!(%error, "apps: cannot tell which ports are in use");
                BTreeSet::new()
            }
        };
        let given = ValueRequest {
            shares: &request.shares,
            ports: &request.ports,
            settings: &request.settings,
            before: before.map(|b| &b.values),
            existing_shares: &existing_shares,
            other_apps: &other_apps,
            busy: &busy,
        };
        Ok(install_values(app, &given, || self.ports.secrets.generate())?)
    }

    /// Records the app and starts the job that installs it.
    pub async fn install(
        &self,
        request: InstallRequest,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(InstalledApp, Job), AppsError> {
        let changes = self.changes.lock().await;
        let listed = self.ports.catalogs.app(request.source, &request.app).await?;
        let app = listed.app;
        if app.id.len() > MAX_APP_ID {
            return Err(AppsError::IdTooLong);
        }
        if !app.permissions.is_empty() && !request.permissions_confirmed {
            return Err(AppsError::PermissionsNotConfirmed);
        }
        if self.ports.installed.find(&app.id).await?.is_some() {
            return Err(AppsError::AlreadyInstalled);
        }
        let volume = self.install_volume(request.pool.as_deref(), context).await?;
        let values = self.values(&app, &request, None).await?;
        let now = self.ports.clock.now();
        let installed = InstalledApp {
            id: app.id.clone(),
            source_id: listed.source.id,
            app,
            pool: volume.pool,
            volume: volume.number,
            values,
            state: InstallState::Installing,
            installed_at: now,
            updated_at: now,
        };
        if !self.ports.installed.create(&installed).await? {
            return Err(AppsError::AlreadyInstalled);
        }
        drop(changes);
        let job = self.run_install("apps.install", installed.clone(), actor, context).await?;
        self.audit("apps.install", Outcome::Success, &installed.id, actor, context).await;
        Ok((installed, job))
    }

    /// Installs a failed app again, with the values it has.
    pub async fn retry(&self, id: &str, actor: &Actor, context: &RequestContext) -> Result<Job, AppsError> {
        let changes = self.changes.lock().await;
        let mut app = self.find(id).await?;
        if app.state != InstallState::Failed {
            return Err(AppsError::Busy);
        }
        app.state = InstallState::Installing;
        app.updated_at = self.ports.clock.now();
        self.ports.installed.update(&app).await?;
        drop(changes);
        let job = self.run_install("apps.install", app, actor, context).await?;
        self.audit("apps.retry", Outcome::Success, id, actor, context).await;
        Ok(job)
    }

    /// Brings the app to the version its catalog offers now, with the
    /// values it has; new ones get their default or are generated. What
    /// the new version may do beyond the old must be confirmed.
    pub async fn update(
        &self,
        id: &str,
        permissions_confirmed: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, AppsError> {
        let changes = self.changes.lock().await;
        let mut installed = self.find(id).await?;
        if installed.state.is_busy() {
            return Err(AppsError::Busy);
        }
        let new = self.ports.catalogs.app(installed.source_id, id).await?.app;
        if new.digest == installed.app.digest {
            return Err(AppsError::UpToDate);
        }
        if asks_more(&new, &installed.app) && !permissions_confirmed {
            return Err(AppsError::PermissionsNotConfirmed);
        }
        let request = InstallRequest {
            shares: installed
                .values
                .shares
                .iter()
                .filter(|(share, _)| new.shares.iter().any(|s| &s.id == *share))
                .map(|(share, name)| (share.clone(), name.clone()))
                .collect(),
            ports: installed
                .values
                .ports
                .iter()
                .filter(|(port, _)| new.ports.iter().any(|p| &p.id == *port && !p.fixed))
                .map(|(port, number)| (port.clone(), *number))
                .collect(),
            ..InstallRequest::default()
        };
        installed.values = self.values(&new, &request, Some(&installed)).await?;
        installed.app = new;
        installed.state = InstallState::Installing;
        installed.updated_at = self.ports.clock.now();
        self.ports.installed.update(&installed).await?;
        drop(changes);
        let version = installed.app.version.clone();
        let job = self.run_install("apps.update", installed, actor, context).await?;
        self.audit("apps.update", Outcome::Success, &format!("{id} {version}"), actor, context).await;
        Ok(job)
    }

    async fn run_install(
        &self,
        kind: &str,
        app: InstalledApp,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, AppsError> {
        let this = self.clone();
        let context = context.clone();
        let job = self
            .ports
            .jobs
            .start(kind, Some(app.id.clone()), Some(actor), move |progress| async move {
                let outcome = this.carry_out(&app, &progress, &context).await;
                let mut done = app;
                done.state = if outcome.is_ok() { InstallState::Installed } else { InstallState::Failed };
                done.updated_at = this.ports.clock.now();
                if let Err(error) = this.ports.installed.update(&done).await {
                    tracing::error!(%error, app = done.id, "apps: cannot record how the installation ended");
                }
                outcome.map_err(|error| job_error(&error))
            })
            .await?;
        Ok(job)
    }

    async fn carry_out(
        &self,
        app: &InstalledApp,
        progress: &Progress,
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let host = &self.ports.host;
        let volume = AppVolume { pool: app.pool.clone(), number: app.volume };
        progress.step("preparing", Some(0.05)).await;
        host.prepare(&app.id, &volume, context).await?;

        progress.step("shares", Some(0.15)).await;
        let grants: Vec<(String, bool)> = app
            .app
            .shares
            .iter()
            .filter_map(|share| {
                let (name, _) = share_and_folder(app.values.shares.get(&share.id)?);
                Some((name.to_owned(), share.access == Access::ReadWrite))
            })
            .collect();
        self.ports.shares.grant(&app.account(), &grants, context).await?;

        progress.step("checking", Some(0.3)).await;
        let shares = self.ports.shares.shares().await?;
        let mut values = HostValues {
            shares: BTreeMap::new(),
            ports: app.values.ports.clone(),
            settings: app.values.settings.clone(),
        };
        for (id, picked) in &app.values.shares {
            let (name, folder) = share_and_folder(picked);
            let share = shares
                .iter()
                .find(|share| share.name == name)
                .ok_or_else(|| AppHostError::Refused(format!("the share {name} is gone")))?;
            let host =
                HostShare { volume: share.volume, name: share.name.clone(), folder: folder.to_owned() };
            values.shares.insert(id.clone(), host);
        }
        let folders = host
            .configure(&app.id, app.volume, &app.app.manifest, &app.app.compose, &values, context)
            .await?;

        progress.step("folders", Some(0.4)).await;
        host.make_folders(&app.id, app.volume, &folders, context).await?;
        progress.step("pulling", Some(0.5)).await;
        host.pull(&app.id, context).await?;
        progress.step("starting", Some(0.9)).await;
        host.start(&app.id, context).await
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
    use crate::domain::{OFFICIAL_INDEX, Permissions, Share};
    use crate::testing::{
        CountingSecrets, FakeAppHost, FakeDocker, FakeListening, FakeReader, FakeShares, MemoryInstalled,
        MemorySources, app,
    };

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
        catalogs: Catalogs,
        reader: Arc<FakeReader>,
        host: Arc<FakeAppHost>,
        shares: Arc<FakeShares>,
        jobs: Jobs,
    }

    fn immich() -> CatalogApp {
        let mut immich = app("immich", "Immich");
        immich.shares = vec![Share {
            id: "photos".into(),
            label: "Photos".into(),
            description: None,
            access: Access::ReadWrite,
        }];
        immich
    }

    fn setup(docker: FakeDocker) -> Setup {
        let reader = Arc::new(FakeReader::default());
        reader.serve(OFFICIAL_INDEX, vec![app("jellyfin", "Jellyfin"), immich()]);
        let catalogs = Catalogs::new(
            Arc::new(MemorySources::default()),
            reader.clone(),
            Arc::new(Audit::default()),
            Arc::new(Fixed),
        );
        let host = Arc::new(FakeAppHost::default());
        let shares = Arc::new(FakeShares::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let installs = Installs::new(InstallPorts {
            catalogs: catalogs.clone(),
            installed: Arc::new(MemoryInstalled::default()),
            host: host.clone(),
            docker: Arc::new(docker),
            shares: shares.clone(),
            listening: Arc::new(FakeListening),
            secrets: Arc::new(CountingSecrets::default()),
            jobs: jobs.clone(),
            audit: Arc::new(Audit::default()),
            clock: Arc::new(Fixed),
        });
        Setup { installs, catalogs, reader, host, shares, jobs }
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

    fn request(id: &str) -> InstallRequest {
        InstallRequest { source: 1, app: id.into(), ..InstallRequest::default() }
    }

    #[tokio::test]
    async fn an_app_goes_on_the_pool_chosen() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        s.catalogs.refresh().await.unwrap();

        let mut given = InstallRequest { pool: Some("backup".into()), ..request("immich") };
        given.shares.insert("photos".into(), "family".into());
        let (app, job) = s.installs.install(given, &alice(), &context).await.unwrap();

        assert_eq!((app.pool.as_str(), app.volume), ("backup", 2));
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.host.calls()[0], "prepare immich backup/2");
    }

    #[tokio::test]
    async fn an_app_may_get_a_folder_in_a_share() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        s.catalogs.refresh().await.unwrap();

        let mut given = request("immich");
        given.shares.insert("photos".into(), "family/Photos/2024".into());
        let (app, job) = s.installs.install(given, &alice(), &context).await.unwrap();

        assert_eq!(app.values.shares["photos"], "family/Photos/2024");
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(
            s.host.calls()[1],
            "configure immich 1 [photos=/volume1/family in Photos/2024] [web=8096]"
        );
        // The account may use the share; the container sees the folder only.
        assert_eq!(s.shares.granted("app-immich"), [("family".to_owned(), true)]);
    }

    #[tokio::test]
    async fn installing_takes_every_step_on_the_host() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        s.catalogs.refresh().await.unwrap();

        let mut given = request("immich");
        given.shares.insert("photos".into(), "family".into());
        let (app, job) = s.installs.install(given, &alice(), &context).await.unwrap();
        assert_eq!(app.state, InstallState::Installing);
        assert_eq!(app.values.ports["web"], 8096);
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(
            s.host.calls(),
            [
                "prepare immich tank/1",
                "configure immich 1 [photos=/volume1/family] [web=8096]",
                "folders immich config",
                "pull immich",
                "start immich",
            ]
        );
        assert_eq!(s.shares.granted("app-immich"), [("family".to_owned(), true)]);

        let view = s.installs.get("immich", &context).await.unwrap();
        assert_eq!(view.app.state, InstallState::Installed);
        assert_eq!(view.containers.unwrap().running, 1);
        assert_eq!(view.update, None);

        let again = s.installs.install(request("immich"), &alice(), &context).await;
        assert!(matches!(again, Err(AppsError::AlreadyInstalled)), "{again:?}");
        let nowhere = InstallRequest { pool: Some("elsewhere".into()), ..request("jellyfin") };
        let nowhere = s.installs.install(nowhere, &alice(), &context).await;
        assert!(matches!(nowhere, Err(AppsError::PoolNotFound)), "{nowhere:?}");
        // Jellyfin wants 8096 too.
        let taken = s.installs.install(request("jellyfin"), &alice(), &context).await;
        assert!(
            matches!(&taken, Err(AppsError::InvalidValue(v)) if v.to_string() == "ports.web: port 8096 is used by immich"),
            "{taken:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_step_can_be_tried_again() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        s.catalogs.refresh().await.unwrap();
        s.host.fail_at(Some("pull"));

        let (_, job) = s.installs.install(request("jellyfin"), &alice(), &context).await.unwrap();
        let job = finished(&s.jobs, job).await;
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().detail.as_deref(), Some("pull went wrong"));
        assert_eq!(s.installs.get("jellyfin", &context).await.unwrap().app.state, InstallState::Failed);
        assert!(matches!(s.installs.start("jellyfin", &alice(), &context).await, Err(AppsError::Busy)));

        s.host.fail_at(None);
        let job = s.installs.retry("jellyfin", &alice(), &context).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.installs.get("jellyfin", &context).await.unwrap().app.state, InstallState::Installed);

        s.installs.stop("jellyfin", &alice(), &context).await.unwrap();
        assert_eq!(s.installs.get("jellyfin", &context).await.unwrap().containers.unwrap().running, 0);
        assert_eq!(s.installs.logs("jellyfin", 100, &context).await.unwrap(), "jellyfin: 100 lines\n");
    }

    #[tokio::test]
    async fn removing_takes_the_data_then_the_shares_then_the_account() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        s.catalogs.refresh().await.unwrap();
        let mut given = request("immich");
        given.shares.insert("photos".into(), "family".into());
        let (_, job) = s.installs.install(given, &alice(), &context).await.unwrap();
        finished(&s.jobs, job).await;

        let wrong = s.installs.uninstall("immich", "Immich", &alice(), &context).await;
        assert!(matches!(wrong, Err(AppsError::ConfirmationMismatch)));
        let job = s.installs.uninstall("immich", "immich", &alice(), &context).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(&s.host.calls()[5..], ["remove immich", "delete_account immich"]);
        assert!(s.shares.granted("app-immich").is_empty());
        assert!(matches!(s.installs.get("immich", &context).await, Err(AppsError::NotInstalled)));
    }

    #[tokio::test]
    async fn updates_keep_the_values_and_confirm_new_permissions() {
        let s = setup(FakeDocker::enabled());
        let context = RequestContext::default();
        let mut jellyfin = app("jellyfin", "Jellyfin");
        jellyfin.settings = vec![crate::domain::Setting {
            id: "SECRET".into(),
            label: "Secret".into(),
            description: None,
            kind: crate::domain::SettingKind::Password,
            default: None,
            required: true,
            generate: true,
        }];
        s.reader.serve(OFFICIAL_INDEX, vec![jellyfin.clone()]);
        s.catalogs.refresh().await.unwrap();
        let mut given = request("jellyfin");
        given.ports.insert("web".into(), 8097);
        let (_, job) = s.installs.install(given, &alice(), &context).await.unwrap();
        finished(&s.jobs, job).await;
        let up_to_date = s.installs.update("jellyfin", false, &alice(), &context).await;
        assert!(matches!(up_to_date, Err(AppsError::UpToDate)));

        jellyfin.version = "2.0".into();
        jellyfin.digest = "sha256:jellyfin-2".into();
        jellyfin.permissions = Permissions { devices: vec!["/dev/dri".into()], ..Permissions::default() };
        s.reader.serve(OFFICIAL_INDEX, vec![jellyfin]);
        s.catalogs.refresh().await.unwrap();
        assert_eq!(s.installs.get("jellyfin", &context).await.unwrap().update.as_deref(), Some("2.0"));

        let unconfirmed = s.installs.update("jellyfin", false, &alice(), &context).await;
        assert!(matches!(unconfirmed, Err(AppsError::PermissionsNotConfirmed)));
        let job = s.installs.update("jellyfin", true, &alice(), &context).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        let view = s.installs.get("jellyfin", &context).await.unwrap();
        assert_eq!(view.app.app.version, "2.0");
        assert_eq!(view.app.values.ports["web"], 8097, "the port stays");
        assert_eq!(view.app.values.settings["SECRET"], "secret-1", "the secret stays");
        assert_eq!(view.update, None);
    }

    #[tokio::test]
    async fn nothing_is_installed_without_docker_or_confirmed_permissions() {
        let s = setup(FakeDocker::default());
        let context = RequestContext::default();
        let mut tun = app("vpn", "VPN");
        tun.permissions.devices = vec!["/dev/net/tun".into()];
        s.reader.serve(OFFICIAL_INDEX, vec![app("jellyfin", "Jellyfin"), tun]);
        s.catalogs.refresh().await.unwrap();

        let no_docker = s.installs.install(request("jellyfin"), &alice(), &context).await;
        assert!(matches!(no_docker, Err(AppsError::DockerNotReady)), "{no_docker:?}");
        let unconfirmed = s.installs.install(request("vpn"), &alice(), &context).await;
        assert!(matches!(unconfirmed, Err(AppsError::PermissionsNotConfirmed)), "{unconfirmed:?}");
        assert!(s.host.calls().is_empty());
    }
}
