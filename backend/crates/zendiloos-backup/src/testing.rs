//! In-memory adapters, for tests here and in the API crate.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::MemoryJobs;
use zendiloos_kernel::{AuditEntry, AuditLog, Clock, PortError, RequestContext, Secret, Timestamp};
use zendiloos_notify::Notifier;
use zendiloos_notify::testing::MemoryNotifications;

use crate::application::{
    BackupHost, BackupPorts, Backups, CloudAccountRepository, CloudSignIn, Crypt, Drive, Drives, HostError,
    HostFacts, HostKey, Listing, Location, NewAccount, NewRun, NewTask, PasswordChange, Readers, RestorePlan,
    RestoreReport, RunEnd, RunMode, RunPlan, RunReport, RunRepository, Secrets, SettingsRepository,
    ShareSpot, Shares, Steps, TaskRepository,
};
use crate::domain::{
    CloudAccount, Entry, EntryKind, INCOMPLETE, MARKER, Outcome, Provider, Run, Settings, Task, Transfer,
};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct MemoryTasks {
    tasks: Mutex<Vec<Task>>,
    passwords: Mutex<BTreeMap<i64, String>>,
    crypts: Mutex<BTreeMap<i64, (String, Option<String>)>>,
}

#[async_trait]
impl TaskRepository for MemoryTasks {
    async fn list(&self) -> Result<Vec<Task>, PortError> {
        Ok(lock(&self.tasks).clone())
    }

    async fn find(&self, id: i64) -> Result<Option<Task>, PortError> {
        Ok(lock(&self.tasks).iter().find(|task| task.id == id).cloned())
    }

    async fn create(&self, new: &NewTask, password: Option<&Secret>) -> Result<Option<i64>, PortError> {
        let mut tasks = lock(&self.tasks);
        let name = new.settings.name.to_lowercase();
        if tasks.iter().any(|task| task.settings.name.to_lowercase() == name) {
            return Ok(None);
        }
        let id = tasks.iter().map(|task| task.id).max().unwrap_or(0) + 1;
        tasks.push(Task {
            id,
            settings: new.settings.clone(),
            folder: new.folder.clone(),
            created_at: new.created_at,
            since: new.created_at,
            last_plug: new.last_plug.clone(),
        });
        if let Some(password) = password {
            lock(&self.passwords).insert(id, password.expose().to_owned());
        }
        Ok(Some(id))
    }

    async fn update(&self, task: &Task, password: PasswordChange) -> Result<bool, PortError> {
        let mut tasks = lock(&self.tasks);
        let name = task.settings.name.to_lowercase();
        if tasks.iter().any(|other| other.id != task.id && other.settings.name.to_lowercase() == name) {
            return Ok(false);
        }
        let Some(stored) = tasks.iter_mut().find(|other| other.id == task.id) else { return Ok(false) };
        *stored = task.clone();
        match password {
            PasswordChange::Keep => {}
            PasswordChange::Set(password) => {
                lock(&self.passwords).insert(task.id, password.expose().to_owned());
            }
            PasswordChange::Clear => {
                lock(&self.passwords).remove(&task.id);
            }
        }
        Ok(true)
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let mut tasks = lock(&self.tasks);
        let before = tasks.len();
        tasks.retain(|task| task.id != id);
        lock(&self.passwords).remove(&id);
        Ok(tasks.len() < before)
    }

    async fn password(&self, id: i64) -> Result<Option<Secret>, PortError> {
        Ok(lock(&self.passwords).get(&id).map(Secret::new))
    }

    async fn noted(&self, id: i64, since: Timestamp) -> Result<(), PortError> {
        if let Some(task) = lock(&self.tasks).iter_mut().find(|task| task.id == id) {
            task.since = since;
        }
        Ok(())
    }

    async fn plugged(&self, id: i64, plug: Option<&str>) -> Result<(), PortError> {
        if let Some(task) = lock(&self.tasks).iter_mut().find(|task| task.id == id) {
            task.last_plug = plug.map(str::to_owned);
        }
        Ok(())
    }
    async fn crypt(&self, id: i64) -> Result<Option<Crypt>, PortError> {
        Ok(lock(&self.crypts).get(&id).map(|(password, salt)| Crypt {
            password: Secret::new(password),
            salt: salt.as_deref().map(Secret::new),
        }))
    }

    async fn set_crypt(&self, id: i64, crypt: Option<&Crypt>) -> Result<(), PortError> {
        let mut crypts = lock(&self.crypts);
        match crypt {
            Some(crypt) => {
                let salt = crypt.salt.as_ref().map(|s| s.expose().to_owned());
                crypts.insert(id, (crypt.password.expose().to_owned(), salt));
            }
            None => {
                crypts.remove(&id);
            }
        }
        Ok(())
    }
}

/// Cloud accounts with their tokens, by id.
#[derive(Default)]
pub struct MemoryAccounts {
    accounts: Mutex<Vec<CloudAccount>>,
    tokens: Mutex<BTreeMap<i64, String>>,
}

impl MemoryAccounts {
    pub fn token(&self, id: i64) -> Option<String> {
        lock(&self.tokens).get(&id).cloned()
    }
}

#[async_trait]
impl CloudAccountRepository for MemoryAccounts {
    async fn list(&self) -> Result<Vec<CloudAccount>, PortError> {
        Ok(lock(&self.accounts).clone())
    }

    async fn find(&self, id: i64) -> Result<Option<CloudAccount>, PortError> {
        Ok(lock(&self.accounts).iter().find(|a| a.id == id).cloned())
    }

    async fn create(
        &self,
        new: &NewAccount,
        token: &Secret,
        _: Option<&Secret>,
    ) -> Result<Option<i64>, PortError> {
        let mut accounts = lock(&self.accounts);
        if accounts.iter().any(|a| a.name.eq_ignore_ascii_case(&new.name)) {
            return Ok(None);
        }
        let id = i64::try_from(accounts.len()).unwrap_or(0) + 1;
        accounts.push(CloudAccount {
            id,
            name: new.name.clone(),
            provider: new.provider,
            client_id: new.client_id.clone(),
            drive_id: new.drive_id.clone(),
            drive_type: new.drive_type.clone(),
            created_at: new.created_at,
        });
        lock(&self.tokens).insert(id, token.expose().to_owned());
        Ok(Some(id))
    }

    async fn secrets(&self, id: i64) -> Result<Option<(Secret, Option<Secret>)>, PortError> {
        Ok(lock(&self.tokens).get(&id).map(|token| (Secret::new(token), None)))
    }

    async fn set_token(&self, id: i64, token: &Secret) -> Result<(), PortError> {
        lock(&self.tokens).insert(id, token.expose().to_owned());
        Ok(())
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let mut accounts = lock(&self.accounts);
        let before = accounts.len();
        accounts.retain(|a| a.id != id);
        Ok(accounts.len() < before)
    }
}

/// OAuth pretended: the code `good` gives a token, others are refused.
#[derive(Default)]
pub struct FakeSignIn(Mutex<u32>);

#[async_trait]
impl CloudSignIn for FakeSignIn {
    fn new_state(&self) -> String {
        let mut count = lock(&self.0);
        *count += 1;
        format!("state{count}")
    }

    fn consent_url(&self, provider: Provider, client_id: Option<&str>, state: &str) -> String {
        format!(
            "https://consent.test/{}?client={}&state={state}",
            provider.as_str(),
            client_id.unwrap_or("rclone")
        )
    }

    async fn exchange(
        &self,
        _: Provider,
        _: Option<&str>,
        _: Option<&Secret>,
        code: &str,
    ) -> Result<Secret, HostError> {
        if code == "good" {
            Ok(Secret::new(r#"{"access_token":"a","refresh_token":"r","expiry":"2026-11-11T08:00:00Z"}"#))
        } else {
            Err(HostError::Refused("invalid_grant".into()))
        }
    }

    async fn drive(&self, provider: Provider, _: &Secret) -> Result<Option<(String, String)>, HostError> {
        Ok((provider == Provider::OneDrive).then(|| ("b!drive".to_owned(), "personal".to_owned())))
    }
}

#[derive(Default)]
pub struct MemoryRuns {
    runs: Mutex<Vec<Run>>,
    transfers: Mutex<BTreeMap<i64, Vec<Transfer>>>,
}

impl MemoryRuns {
    pub fn all(&self) -> Vec<Run> {
        lock(&self.runs).clone()
    }
}

#[async_trait]
impl RunRepository for MemoryRuns {
    async fn start(&self, run: &NewRun) -> Result<i64, PortError> {
        let mut runs = lock(&self.runs);
        let id = i64::try_from(runs.len()).unwrap_or(0) + 1;
        runs.push(Run {
            id,
            task_id: run.task_id,
            trigger: run.trigger,
            by: run.by.clone(),
            started_at: run.started_at,
            finished_at: None,
            outcome: Outcome::Running,
            files: 0,
            bytes: 0,
            version: None,
            error: None,
            transfers: 0,
            job_id: run.job_id,
        });
        Ok(id)
    }

    async fn finish(&self, id: i64, end: &RunEnd) -> Result<(), PortError> {
        if let Some(run) = lock(&self.runs).iter_mut().find(|run| run.id == id) {
            run.finished_at = Some(end.finished_at);
            run.outcome = end.outcome;
            run.files = end.files;
            run.bytes = end.bytes;
            run.version.clone_from(&end.version);
            run.error.clone_from(&end.error);
            run.transfers = end.transfers_total;
        }
        lock(&self.transfers).insert(id, end.transfers.clone());
        Ok(())
    }

    async fn find(&self, id: i64) -> Result<Option<Run>, PortError> {
        Ok(lock(&self.runs).iter().find(|run| run.id == id).cloned())
    }

    async fn list(&self, task: Option<i64>, limit: u32) -> Result<Vec<Run>, PortError> {
        Ok(lock(&self.runs)
            .iter()
            .rev()
            .filter(|run| task.is_none_or(|id| run.task_id == id))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn transfers(&self, run: i64, offset: u32, limit: u32) -> Result<Vec<Transfer>, PortError> {
        Ok(lock(&self.transfers)
            .get(&run)
            .map(|list| list.iter().skip(offset as usize).take(limit as usize).cloned().collect())
            .unwrap_or_default())
    }

    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError> {
        let mut count = 0;
        for run in lock(&self.runs).iter_mut().filter(|run| run.outcome == Outcome::Running) {
            run.outcome = Outcome::Interrupted;
            run.finished_at = Some(at);
            count += 1;
        }
        Ok(count)
    }

    async fn forget_before(&self, at: Timestamp) -> Result<u64, PortError> {
        let mut runs = lock(&self.runs);
        let before = runs.len();
        runs.retain(|run| run.started_at >= at);
        Ok((before - runs.len()) as u64)
    }
}

#[derive(Default)]
pub struct MemorySettings(Mutex<Option<Settings>>);

#[async_trait]
impl SettingsRepository for MemorySettings {
    async fn get(&self) -> Result<Settings, PortError> {
        Ok(lock(&self.0).unwrap_or_default())
    }

    async fn set(&self, settings: Settings) -> Result<(), PortError> {
        *lock(&self.0) = Some(settings);
        Ok(())
    }
}

/// Shares by id; `photos` (1) on volume 1, `backups` (2) on volume 2.
pub struct FakeShares {
    pub shares: Mutex<Vec<ShareSpot>>,
}

impl Default for FakeShares {
    fn default() -> Self {
        Self {
            shares: Mutex::new(vec![
                ShareSpot { id: 1, name: "photos".into(), volume: 1 },
                ShareSpot { id: 2, name: "backups".into(), volume: 2 },
            ]),
        }
    }
}

#[async_trait]
impl Shares for FakeShares {
    async fn locate(&self, share_id: i64) -> Result<Option<ShareSpot>, PortError> {
        Ok(lock(&self.shares).iter().find(|share| share.id == share_id).cloned())
    }

    async fn readers(&self, share_id: i64) -> Result<Option<Readers>, PortError> {
        Ok(lock(&self.shares).iter().any(|share| share.id == share_id).then(|| Readers {
            accounts: vec!["alice".into()],
            groups: vec!["family".into()],
            nfs: Vec::new(),
        }))
    }
}

/// The removable drives plugged in.
#[derive(Default)]
pub struct FakeDrives(pub Mutex<Vec<Drive>>);

pub const STICK: &str = "1A2B-3C4D";
pub const DISK: &str = "0c5f8a2e-1d3b-4c6a-9e7f-2b1d3c4e5f60";

impl FakeDrives {
    /// A drive plugged in (and mounted), its plugging in `plug`.
    pub fn plug(&self, id: &str, filesystem: &str, plug: &str) {
        let mut drives = lock(&self.0);
        drives.retain(|drive| drive.id != id);
        drives.push(Drive {
            id: id.into(),
            label: Some("BACKUP".into()),
            filesystem: filesystem.into(),
            mounted: true,
            read_only: false,
            plugged_in: Some(plug.into()),
        });
    }

    pub fn pull(&self, id: &str) {
        lock(&self.0).retain(|drive| drive.id != id);
    }
}

#[async_trait]
impl Drives for FakeDrives {
    async fn drives(&self, _: &RequestContext) -> Result<Vec<Drive>, PortError> {
        Ok(lock(&self.0).clone())
    }
}

/// What the fake host was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
    Install,
    MakeKey,
    Run(RunPlan),
    Stop(i64),
    Prune(Location, Vec<String>),
    Restore(RestorePlan),
    List(Location),
}

/// rsync on a fake host: folders are listings kept by location; a run
/// adds its version (or the mirror's copy) to its target's listing.
pub struct FakeHost {
    pub installed: Mutex<bool>,
    pub public_key: Mutex<Option<String>>,
    pub calls: Mutex<Vec<HostCall>>,
    pub listings: Mutex<BTreeMap<String, Vec<Entry>>>,
    /// Passwords the runs and listings were given.
    pub passwords: Mutex<Vec<Option<String>>>,
    /// The next run fails with this, once.
    pub fail_next: Mutex<Option<HostError>>,
    /// The next run reports this partial reason, once.
    pub partial_next: Mutex<Option<String>>,
    /// Held to keep runs waiting.
    pub gate: tokio::sync::Mutex<()>,
    /// The secrets each run was given: `password|crypt|token`.
    pub secrets_seen: Mutex<Vec<String>>,
    refreshed: Mutex<Option<String>>,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            installed: Mutex::new(true),
            public_key: Mutex::default(),
            calls: Mutex::default(),
            listings: Mutex::default(),
            passwords: Mutex::default(),
            fail_next: Mutex::default(),
            partial_next: Mutex::default(),
            gate: tokio::sync::Mutex::new(()),
            secrets_seen: Mutex::default(),
            refreshed: Mutex::default(),
        }
    }
}

/// A location as a key of the fake's listings.
pub fn key(at: &Location) -> String {
    match at {
        Location::Share { volume, share, path } => format!("/volume{volume}/{share}/{path}"),
        Location::Drive { drive, path } => format!("drive:{drive}/{path}"),
        Location::Remote { server, path } => {
            format!("{}@{}:{}/{path}", server.user, server.host, server.port)
        }
        Location::Cloud { provider, account, path } => format!("{}:{account}/{path}", provider.as_str()),
    }
}

pub fn folder(name: &str) -> Entry {
    Entry { name: name.into(), kind: EntryKind::Folder, bytes: 0, modified: None }
}

impl FakeHost {
    pub fn calls(&self) -> Vec<HostCall> {
        lock(&self.calls).clone()
    }

    pub fn runs(&self) -> Vec<RunPlan> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                HostCall::Run(plan) => Some(plan),
                _ => None,
            })
            .collect()
    }

    /// What the next run, listing, retention or restore hands back as a
    /// refreshed token.
    pub fn refresh_token(&self, token: &str) {
        *lock(&self.refreshed) = Some(token.to_owned());
    }

    fn refreshed(&self) -> Option<Secret> {
        lock(&self.refreshed).take().map(Secret::new)
    }

    pub fn put(&self, at: &Location, entries: Vec<Entry>) {
        lock(&self.listings).insert(key(at), entries);
    }

    pub fn listing(&self, at: &Location) -> Vec<Entry> {
        lock(&self.listings).get(&key(at)).cloned().unwrap_or_default()
    }

    fn call(&self, call: HostCall) {
        lock(&self.calls).push(call);
    }
}

#[async_trait]
impl BackupHost for FakeHost {
    async fn facts(&self, _: &RequestContext) -> Result<HostFacts, PortError> {
        let installed = *lock(&self.installed);
        Ok(HostFacts {
            rsync: installed,
            ssh: installed,
            rclone: installed,
            public_key: lock(&self.public_key).clone(),
        })
    }

    async fn install(&self, _: &RequestContext) -> Result<(), HostError> {
        self.call(HostCall::Install);
        *lock(&self.installed) = true;
        Ok(())
    }

    async fn make_key(&self, _: &RequestContext) -> Result<String, HostError> {
        self.call(HostCall::MakeKey);
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv1zDdNJ6P8b8HFqUvNiOXIdk9JKLy6vJwDGP1vbTqL zendiloos-backup".to_owned();
        *lock(&self.public_key) = Some(key.clone());
        Ok(key)
    }

    async fn host_keys(&self, host: &str, _: u16, _: &RequestContext) -> Result<Vec<HostKey>, HostError> {
        if host == "silent.lan" {
            return Ok(Vec::new());
        }
        Ok(vec![HostKey {
            line: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl".into(),
            fingerprint: "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU".into(),
        }])
    }

    async fn list(
        &self,
        at: &Location,
        inside: &str,
        secrets: &Secrets,
        _: &RequestContext,
    ) -> Result<Listing, HostError> {
        let at = &at.join(inside);
        self.call(HostCall::List(at.clone()));
        lock(&self.passwords).push(secrets.password.as_ref().map(|p| p.expose().to_owned()));
        if let Location::Remote { server, .. } = at
            && server.host == "down.lan"
        {
            return Err(HostError::Failed("rsync: failed to connect to down.lan: Connection refused".into()));
        }
        Ok(Listing { entries: self.listing(at), token: self.refreshed() })
    }

    async fn run(
        &self,
        plan: &RunPlan,
        secrets: &Secrets,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<RunReport, HostError> {
        self.call(HostCall::Run(plan.clone()));
        lock(&self.passwords).push(secrets.password.as_ref().map(|p| p.expose().to_owned()));
        lock(&self.secrets_seen).push(format!(
            "{}|{}|{}",
            secrets.password.as_ref().map_or("", |p| p.expose()),
            secrets.crypt.as_ref().map_or("", |c| c.password.expose()),
            secrets.cloud.as_ref().map_or("", |c| c.token.expose()),
        ));
        steps.step("transferring", Some(50));
        let _gate = self.gate.lock().await;
        if let Some(error) = lock(&self.fail_next).take() {
            // What a cut off version leaves.
            if plan.mode == RunMode::Versioned {
                let mut entries = self.listing(&plan.target);
                entries.push(folder(&format!("{}{INCOMPLETE}", plan.stamp)));
                self.put(&plan.target, entries);
            }
            return Err(error);
        }
        let mut entries = self.listing(&plan.target);
        if !entries.iter().any(|e| e.name == MARKER) {
            entries.push(Entry { name: MARKER.into(), kind: EntryKind::File, bytes: 2, modified: None });
        }
        match plan.mode {
            RunMode::Versioned => entries.push(folder(&plan.stamp)),
            RunMode::Mirror { .. } if !entries.iter().any(|e| e.name == "current") => {
                entries.push(folder("current"))
            }
            RunMode::Mirror { .. } => {}
        }
        self.put(&plan.target, entries.clone());
        Ok(RunReport {
            files: 3,
            bytes: 300,
            partial: lock(&self.partial_next).take(),
            transfers: vec![Transfer { change: ">f+++++++++".into(), path: "a.jpg".into() }],
            transfers_total: 3,
            entries,
            token: self.refreshed(),
        })
    }

    async fn stop(&self, task: i64, _: &RequestContext) -> Result<(), HostError> {
        self.call(HostCall::Stop(task));
        // The run waiting at the gate ends as stopped.
        *lock(&self.fail_next) = Some(HostError::Failed("rsync was stopped".into()));
        Ok(())
    }

    async fn prune(
        &self,
        _: i64,
        at: &Location,
        names: &[String],
        _: &Secrets,
        _: &RequestContext,
    ) -> Result<Option<Secret>, HostError> {
        self.call(HostCall::Prune(at.clone(), names.to_vec()));
        let mut entries = self.listing(at);
        entries.retain(|entry| !names.contains(&entry.name));
        self.put(at, entries);
        Ok(self.refreshed())
    }

    async fn restore(
        &self,
        plan: &RestorePlan,
        _: &Secrets,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<RestoreReport, HostError> {
        self.call(HostCall::Restore(plan.clone()));
        steps.step("restoring", Some(100));
        Ok(RestoreReport { files: 2, bytes: 20, partial: None, token: self.refreshed() })
    }
}

/// A clock that can be set, at UTC (no offset).
pub struct SetClock(pub AtomicI64);

impl SetClock {
    pub fn at(seconds: i64) -> Self {
        Self(AtomicI64::new(seconds))
    }

    pub fn set(&self, seconds: i64) {
        self.0.store(seconds, Ordering::SeqCst);
    }
}

impl Clock for SetClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
pub struct MemoryAudit(pub Mutex<Vec<AuditEntry>>);

impl MemoryAudit {
    pub fn actions(&self) -> Vec<(&'static str, zendiloos_kernel::Outcome)> {
        lock(&self.0).iter().map(|entry| (entry.action, entry.outcome)).collect()
    }
}

#[async_trait]
impl AuditLog for MemoryAudit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        lock(&self.0).push(entry);
        Ok(())
    }
}

/// The fakes behind a [`Backups`].
pub struct Fakes {
    pub tasks: Arc<MemoryTasks>,
    pub runs: Arc<MemoryRuns>,
    pub settings: Arc<MemorySettings>,
    pub host: Arc<FakeHost>,
    pub shares: Arc<FakeShares>,
    pub drives: Arc<FakeDrives>,
    pub accounts: Arc<MemoryAccounts>,
    pub jobs: Arc<MemoryJobs>,
    pub notifications: Arc<MemoryNotifications>,
    pub audit: Arc<MemoryAudit>,
    pub clock: Arc<SetClock>,
}

/// 2026-11-11 07:00 UTC.
pub const NOW: i64 = 1_794_380_400;

/// Backups on fakes, rsync installed, the clock at [`NOW`].
pub fn backups() -> (Backups, Fakes) {
    let fakes = Fakes {
        tasks: Arc::default(),
        runs: Arc::default(),
        settings: Arc::default(),
        host: Arc::default(),
        shares: Arc::default(),
        drives: Arc::default(),
        accounts: Arc::default(),
        jobs: Arc::default(),
        notifications: Arc::default(),
        audit: Arc::default(),
        clock: Arc::new(SetClock::at(NOW)),
    };
    let jobs = Jobs::new(fakes.jobs.clone(), fakes.clock.clone());
    let notifier = Notifier::new(fakes.notifications.clone(), fakes.clock.clone());
    let backups = Backups::new(BackupPorts {
        tasks: fakes.tasks.clone(),
        runs: fakes.runs.clone(),
        settings: fakes.settings.clone(),
        host: fakes.host.clone(),
        shares: fakes.shares.clone(),
        drives: fakes.drives.clone(),
        accounts: fakes.accounts.clone(),
        sign_in: Arc::new(FakeSignIn::default()),
        jobs,
        notifier,
        audit: fakes.audit.clone(),
        clock: fakes.clock.clone(),
    });
    (backups, fakes)
}
