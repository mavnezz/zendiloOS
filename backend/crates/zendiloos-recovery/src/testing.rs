//! In-memory adapters for this context's ports, for tests here and in the
//! API crate.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, PortError, RequestContext, Timestamp};

use crate::application::{
    Archive, BackupFolders, Operators, PlanRepository, Restart, RestoreError, Restored, Restorer, Snapshots,
};
use crate::domain::{FORMAT, LastRun, Manifest, Passphrase, Plan, Target, archive_name};

/// The schema the fakes call this release's.
pub const SUPPORTED_SCHEMA: i64 = 55;

fn manifest(host: &str, at: i64) -> Manifest {
    Manifest {
        format: FORMAT,
        version: "0.1.0".into(),
        schema: SUPPORTED_SCHEMA,
        host_name: host.into(),
        created_at: Timestamp::from_unix_seconds(at),
    }
}

/// Makes `backup:<passphrase>` as the archive, named after `nas` and a
/// time that goes up by an hour each time.
#[derive(Default)]
pub struct FakeSnapshots {
    made: AtomicUsize,
    pub fail: Mutex<Option<String>>,
}

#[async_trait]
impl Snapshots for FakeSnapshots {
    async fn create(&self, passphrase: &Passphrase, _: &RequestContext) -> Result<Archive, PortError> {
        if let Some(reason) = self.fail.lock().unwrap().clone() {
            return Err(PortError::new("database", reason));
        }
        let at = 1_800_000_000 + 3600 * self.made.fetch_add(1, Ordering::SeqCst) as i64;
        Ok(Archive {
            name: archive_name("nas", Timestamp::from_unix_seconds(at)),
            manifest: manifest("nas", at),
            bytes: format!("backup:{}", passphrase.expose()).into_bytes(),
        })
    }
}

/// Opens what [`FakeSnapshots`] makes, with the passphrase it was made
/// with; `too-new:…` is a backup of a newer schema, anything else is no
/// backup. Keeps the audit entries written into the restored database.
#[derive(Default)]
pub struct FakeRestorer {
    pub restored: Mutex<Vec<Vec<u8>>>,
    pub audit: Mutex<Vec<AuditEntry>>,
}

#[async_trait]
impl Restorer for FakeRestorer {
    async fn restore(
        &self,
        archive: Vec<u8>,
        passphrase: &Passphrase,
        audit: AuditEntry,
        _: &RequestContext,
    ) -> Result<Restored, RestoreError> {
        let text = String::from_utf8_lossy(&archive).into_owned();
        let (kind, with) = text.split_once(':').ok_or_else(|| RestoreError::NotABackup("not age".into()))?;
        if with != passphrase.expose() {
            return Err(RestoreError::WrongPassphrase);
        }
        match kind {
            "backup" => {}
            "too-new" => {
                return Err(RestoreError::TooNew {
                    found: SUPPORTED_SCHEMA + 1,
                    supported: SUPPORTED_SCHEMA,
                });
            }
            "format" => return Err(RestoreError::UnsupportedFormat(FORMAT + 1)),
            "taken" => return Err(RestoreError::Refused("UID 2001 of anna is eve's here".into())),
            _ => return Err(RestoreError::NotABackup("no manifest".into())),
        }
        self.restored.lock().unwrap().push(archive);
        self.audit.lock().unwrap().push(audit);
        Ok(Restored {
            manifest: manifest("nas", 1_800_000_000),
            warnings: vec![],
            https_port: Some(443),
            network: vec!["05-zendiloos-enp1s0.network".into()],
        })
    }
}

#[derive(Default)]
pub struct MemoryPlans(pub Mutex<Plan>);

#[async_trait]
impl PlanRepository for MemoryPlans {
    async fn get(&self) -> Result<Plan, PortError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn save(&self, plan: &Plan) -> Result<(), PortError> {
        let mut kept = self.0.lock().unwrap();
        let last = kept.last.clone();
        *kept = Plan { last, ..plan.clone() };
        Ok(())
    }

    async fn record(&self, run: &LastRun) -> Result<(), PortError> {
        self.0.lock().unwrap().last = Some(run.clone());
        Ok(())
    }
}

/// Folders by their target; `fail` refuses writing.
#[derive(Default)]
pub struct MemoryFolders {
    pub files: Mutex<BTreeMap<String, Vec<u8>>>,
    pub writers: Mutex<Vec<String>>,
    pub fail: Mutex<bool>,
}

fn key(target: &Target) -> String {
    match target {
        Target::Share { share_id, folder } => format!("share{share_id}:{folder}"),
        Target::Drive { drive, folder } => format!("{drive}:{folder}"),
    }
}

#[async_trait]
impl BackupFolders for MemoryFolders {
    async fn write(
        &self,
        target: &Target,
        user: &str,
        name: &str,
        bytes: Vec<u8>,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        if *self.fail.lock().unwrap() {
            return Err(PortError::new("files agent", "No space left on device"));
        }
        self.writers.lock().unwrap().push(user.to_owned());
        self.files.lock().unwrap().insert(format!("{}/{name}", key(target)), bytes);
        Ok(())
    }

    async fn names(&self, target: &Target, _: &str, _: &RequestContext) -> Result<Vec<String>, PortError> {
        let prefix = format!("{}/", key(target));
        let files = self.files.lock().unwrap();
        Ok(files.keys().filter_map(|path| path.strip_prefix(&prefix).map(str::to_owned)).collect())
    }

    async fn remove(
        &self,
        target: &Target,
        _: &str,
        name: &str,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        self.files.lock().unwrap().remove(&format!("{}/{name}", key(target)));
        Ok(())
    }
}

/// Accounts by ID, with whether they may manage the system.
#[derive(Default)]
pub struct FixedOperators(pub Mutex<BTreeMap<i64, (String, bool)>>);

#[async_trait]
impl Operators for FixedOperators {
    async fn name(&self, account_id: i64) -> Result<Option<String>, PortError> {
        Ok(self.0.lock().unwrap().get(&account_id).map(|(name, _)| name.clone()))
    }

    async fn administrator(&self, account_id: i64) -> Result<Option<String>, PortError> {
        Ok(self.0.lock().unwrap().get(&account_id).filter(|(_, admin)| *admin).map(|(name, _)| name.clone()))
    }
}

/// Counts the restarts asked for.
#[derive(Default)]
pub struct RecordingRestart(pub AtomicUsize);

impl Restart for RecordingRestart {
    fn restart(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A clock a test sets.
pub struct SetClock(pub std::sync::atomic::AtomicI64);

impl zendiloos_kernel::Clock for SetClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
pub struct MemoryAudit(pub Mutex<Vec<AuditEntry>>);

#[async_trait]
impl zendiloos_kernel::AuditLog for MemoryAudit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry);
        Ok(())
    }
}

impl MemoryAudit {
    pub fn actions(&self) -> Vec<(&'static str, zendiloos_kernel::Outcome)> {
        self.0.lock().unwrap().iter().map(|entry| (entry.action, entry.outcome)).collect()
    }
}

/// Background work a test says runs.
#[derive(Default)]
pub struct SomeWork(pub Mutex<Vec<String>>);

#[async_trait]
impl zendiloos_kernel::RunningWork for SomeWork {
    async fn running(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Every fake, for a test to look at.
pub struct Fakes {
    pub snapshots: std::sync::Arc<FakeSnapshots>,
    pub restorer: std::sync::Arc<FakeRestorer>,
    pub plans: std::sync::Arc<MemoryPlans>,
    pub folders: std::sync::Arc<MemoryFolders>,
    pub operators: std::sync::Arc<FixedOperators>,
    pub restart: std::sync::Arc<RecordingRestart>,
    pub work: std::sync::Arc<SomeWork>,
    pub jobs: std::sync::Arc<zendiloos_jobs::testing::MemoryJobs>,
    pub notifications: std::sync::Arc<zendiloos_notify::testing::MemoryNotifications>,
    pub audit: std::sync::Arc<MemoryAudit>,
    pub clock: std::sync::Arc<SetClock>,
}

/// 2027-01-15 08:00 UTC.
pub const NOW: i64 = 1_800_000_000;

/// The use cases on fakes; account 1 is the administrator alice, 2 the
/// user bob.
pub fn recovery() -> (crate::application::Recovery, Fakes) {
    use std::sync::Arc;
    let clock = Arc::new(SetClock(std::sync::atomic::AtomicI64::new(NOW)));
    let jobs = Arc::new(zendiloos_jobs::testing::MemoryJobs::default());
    let notifications = Arc::new(zendiloos_notify::testing::MemoryNotifications::default());
    let operators = Arc::new(FixedOperators::default());
    operators.0.lock().unwrap().extend([(1, ("alice".to_owned(), true)), (2, ("bob".to_owned(), false))]);
    let fakes = Fakes {
        snapshots: Arc::default(),
        restorer: Arc::default(),
        plans: Arc::default(),
        folders: Arc::default(),
        operators,
        restart: Arc::default(),
        work: Arc::default(),
        jobs: jobs.clone(),
        notifications: notifications.clone(),
        audit: Arc::default(),
        clock: clock.clone(),
    };
    let ports = crate::application::RecoveryPorts {
        snapshots: fakes.snapshots.clone(),
        restorer: fakes.restorer.clone(),
        plans: fakes.plans.clone(),
        folders: fakes.folders.clone(),
        operators: fakes.operators.clone(),
        restart: fakes.restart.clone(),
        work: fakes.work.clone(),
        jobs: zendiloos_jobs::Jobs::new(jobs, clock.clone()),
        notifier: zendiloos_notify::Notifier::new(notifications, clock.clone()),
        audit: fakes.audit.clone(),
        clock,
    };
    (crate::application::Recovery::new(ports), fakes)
}
