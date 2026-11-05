//! Changes to pools. Everything that erases disks goes through a plan:
//! the plan names the disks, is shown to the user, and is confirmed with a
//! single-use token. Right before it runs, the plan is made again from the
//! disks as they are now; if anything differs, nothing happens.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zendiloos_jobs::{Job, JobError, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext, Timestamp};

use crate::application::StorageError;
use crate::application::ports::{AdminError, BlockDeviceSource, NewPool, PlanTokens, PoolAdmin, ZfsSource};
use crate::application::queries::inventory;
use crate::domain::{self, PoolLayout, PoolName, PoolPlan, ReplacePlan};

/// How long a plan can be confirmed.
pub const PLAN_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
enum Planned {
    Pool { request: Vec<String>, plan: PoolPlan },
    Replace(ReplacePlan),
}

struct Stored {
    planned: Planned,
    account_id: i64,
    expires_at: Timestamp,
}

/// Shared by the commands of this module.
pub(crate) struct Context {
    pub(crate) devices: Arc<dyn BlockDeviceSource>,
    pub(crate) zfs: Arc<dyn ZfsSource>,
    pub(crate) admin: Arc<dyn PoolAdmin>,
    pub(crate) jobs: Jobs,
    pub(crate) audit: Arc<dyn AuditLog>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) tokens: Arc<dyn PlanTokens>,
    plans: Mutex<HashMap<String, Stored>>,
    /// Storage jobs run one at a time; later ones wait.
    changes: Arc<tokio::sync::Mutex<()>>,
}

impl Context {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        zfs: Arc<dyn ZfsSource>,
        admin: Arc<dyn PoolAdmin>,
        jobs: Jobs,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        tokens: Arc<dyn PlanTokens>,
    ) -> Self {
        Self {
            devices,
            zfs,
            admin,
            jobs,
            audit,
            clock,
            tokens,
            plans: Mutex::new(HashMap::new()),
            changes: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn store(&self, planned: Planned, actor: &Actor) -> (String, Timestamp) {
        let now = self.clock.now();
        let expires_at = now.saturating_add(PLAN_LIFETIME);
        let token = self.tokens.token();
        let mut plans = self.plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, stored| stored.expires_at > now);
        plans.insert(token.clone(), Stored { planned, account_id: actor.account_id, expires_at });
        (token, expires_at)
    }

    /// The plan behind `token`; only for whoever made it.
    fn peek(&self, token: &str, actor: &Actor) -> Result<Planned, StorageError> {
        let plans = self.plans.lock().unwrap_or_else(PoisonError::into_inner);
        let stored = plans.get(token).ok_or(StorageError::PlanExpired)?;
        if stored.account_id != actor.account_id || stored.expires_at <= self.clock.now() {
            return Err(StorageError::PlanExpired);
        }
        Ok(stored.planned.clone())
    }

    /// Uses the token up; a plan is carried out at most once.
    fn take(&self, token: &str) {
        self.plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token);
    }

    async fn plan_pool_now(
        &self,
        name: &str,
        layout: PoolLayout,
        disks: &[String],
        request: &RequestContext,
    ) -> Result<PoolPlan, StorageError> {
        let (devices, pools) = inventory(self.devices.as_ref(), self.zfs.as_ref(), request).await?;
        let disks_now = domain::disks(&devices, &pools);
        Ok(domain::plan_pool(name, layout, disks, &disks_now, &pools)?)
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    /// Runs `work` as a job, after other storage jobs, and audits its end.
    async fn job<F, Fut>(
        self: &Arc<Self>,
        kind: &'static str,
        target: String,
        actor: &Actor,
        request: &RequestContext,
        work: F,
    ) -> Result<Job, StorageError>
    where
        F: FnOnce(Arc<dyn PoolAdmin>, RequestContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AdminError>> + Send + 'static,
    {
        let context = Arc::clone(self);
        let (actor_owned, request_owned, target_owned) = (actor.clone(), request.clone(), target.clone());
        let job = self
            .jobs
            .start(kind, Some(target), Some(actor), move |progress| async move {
                let _one_at_a_time = context.changes.lock().await;
                progress.step("running", None).await;
                let outcome = work(Arc::clone(&context.admin), request_owned.clone()).await;
                let audit_outcome = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
                context.audit(kind, audit_outcome, &target_owned, &actor_owned, &request_owned).await;
                outcome.map_err(job_error)
            })
            .await?;
        Ok(job)
    }
}

fn job_error(error: AdminError) -> JobError {
    match error {
        AdminError::Refused(reason) => JobError::new("refused", Some(reason)),
        AdminError::Conflict => JobError::new("name_taken", None),
        AdminError::Port(error) => {
            let mut detail = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                detail = format!("{detail}: {cause}");
                source = cause.source();
            }
            JobError::new("agent_failed", Some(detail))
        }
    }
}

/// A plan ready for confirmation.
#[derive(Debug, Clone)]
pub struct Confirmable<T> {
    pub token: String,
    pub expires_at: Timestamp,
    pub plan: T,
}

/// Command: plans a new pool. Erases nothing.
#[derive(Clone)]
pub struct PlanPool {
    context: Arc<Context>,
}

impl PlanPool {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        name: &str,
        layout: PoolLayout,
        disks: Vec<String>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Confirmable<PoolPlan>, StorageError> {
        let plan = self.context.plan_pool_now(name.trim(), layout, &disks, request).await?;
        let (token, expires_at) =
            self.context.store(Planned::Pool { request: disks, plan: plan.clone() }, actor);
        Ok(Confirmable { token, expires_at, plan })
    }
}

/// Command: creates the pool of a confirmed plan, as a job.
#[derive(Clone)]
pub struct CreatePool {
    context: Arc<Context>,
}

impl CreatePool {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// `confirm` must repeat the pool's name.
    pub async fn execute(
        &self,
        token: &str,
        confirm: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, StorageError> {
        let c = &self.context;
        let Planned::Pool { request: disks, plan } = c.peek(token, actor)? else {
            return Err(StorageError::PlanExpired);
        };
        if confirm.trim() != plan.name.as_str() {
            return Err(StorageError::ConfirmationMismatch);
        }
        c.take(token);
        // The same request against the disks as they are now. Kernel names
        // may have moved; the disks are compared by by-id link and serial.
        let now = c.plan_pool_now(plan.name.as_str(), plan.layout.clone(), &disks, request).await;
        let unchanged = now.as_ref().is_ok_and(|now| {
            now.volume == plan.volume
                && now.disks.len() == plan.disks.len()
                && now.disks.iter().zip(&plan.disks).all(|(a, b)| {
                    (&a.id_link, &a.serial, a.size_bytes, &a.erases)
                        == (&b.id_link, &b.serial, b.size_bytes, &b.erases)
                })
        });
        if !unchanged {
            return Err(StorageError::PlanChanged);
        }

        let new = NewPool {
            name: plan.name.clone(),
            layout: plan.layout.clone(),
            disks: plan.disks.iter().map(|d| d.id_link.clone()).collect(),
            volume: plan.volume,
        };
        c.job(
            "storage.create_pool",
            plan.name.to_string(),
            actor,
            request,
            move |admin, request| async move { admin.create_pool(&new, &request).await },
        )
        .await
    }
}

/// Command: destroys a pool zendiloos created, as a job. Its data is lost.
#[derive(Clone)]
pub struct DestroyPool {
    context: Arc<Context>,
}

impl DestroyPool {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// `confirm` must repeat the pool's name.
    pub async fn execute(
        &self,
        pool: &str,
        confirm: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, StorageError> {
        let c = &self.context;
        if confirm.trim() != pool {
            return Err(StorageError::ConfirmationMismatch);
        }
        let pools = c.zfs.pools(request).await?;
        let found = pools.iter().find(|p| p.name == pool).ok_or(StorageError::PoolNotFound)?;
        let volume = found.volume.ok_or(StorageError::Plan(domain::PlanError::PoolNotManaged))?;
        let name =
            PoolName::parse(pool).map_err(|_| StorageError::Plan(domain::PlanError::PoolNotManaged))?;
        c.job("storage.destroy_pool", pool.to_owned(), actor, request, move |admin, request| async move {
            admin.destroy_pool(&name, volume, &request).await
        })
        .await
    }
}

/// Command: plans replacing a device of a pool with another disk.
#[derive(Clone)]
pub struct PlanReplace {
    context: Arc<Context>,
}

impl PlanReplace {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        pool: &str,
        device: &str,
        disk: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Confirmable<ReplacePlan>, StorageError> {
        let c = &self.context;
        let (devices, pools) = inventory(c.devices.as_ref(), c.zfs.as_ref(), request).await?;
        let disks = domain::disks(&devices, &pools);
        let plan = domain::plan_replace(pool, device, disk, &disks, &pools)?;
        let (token, expires_at) = c.store(Planned::Replace(plan.clone()), actor);
        Ok(Confirmable { token, expires_at, plan })
    }
}

/// Command: carries out a confirmed replacement, as a job. The resilver
/// then runs in ZFS; the pool shows its progress.
#[derive(Clone)]
pub struct ReplaceDisk {
    context: Arc<Context>,
}

impl ReplaceDisk {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        token: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, StorageError> {
        let c = &self.context;
        let Planned::Replace(plan) = c.peek(token, actor)? else {
            return Err(StorageError::PlanExpired);
        };
        c.take(token);
        let (devices, pools) = inventory(c.devices.as_ref(), c.zfs.as_ref(), request).await?;
        let disks = domain::disks(&devices, &pools);
        // The disk by its by-id link, wherever the kernel put it now.
        let current = disks.iter().find(|d| d.id_link.as_deref() == Some(plan.disk.id_link.as_str()));
        let now = current
            .map(|disk| domain::plan_replace(plan.pool.as_str(), &plan.device, &disk.name, &disks, &pools));
        let unchanged = matches!(&now, Some(Ok(now))
            if now.disk.serial == plan.disk.serial && now.disk.size_bytes == plan.disk.size_bytes && now.disk.erases == plan.disk.erases);
        if !unchanged {
            return Err(StorageError::PlanChanged);
        }
        let (pool, device, disk) = (plan.pool.clone(), plan.device.clone(), plan.disk.id_link.clone());
        c.job(
            "storage.replace_disk",
            plan.pool.to_string(),
            actor,
            request,
            move |admin, request| async move { admin.replace_disk(&pool, &device, &disk, &request).await },
        )
        .await
    }
}

/// Command: starts a scrub. It runs in ZFS; the pool shows its progress.
#[derive(Clone)]
pub struct ScrubPool {
    context: Arc<Context>,
}

impl ScrubPool {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        pool: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), StorageError> {
        let c = &self.context;
        let pools = c.zfs.pools(request).await?;
        if !pools.iter().any(|p| p.name == pool) {
            return Err(StorageError::PoolNotFound);
        }
        let name =
            PoolName::parse(pool).map_err(|_| StorageError::Plan(domain::PlanError::PoolNotManaged))?;
        let outcome = c.admin.scrub(&name, request).await;
        let audit = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
        c.audit("storage.scrub", audit, pool, actor, request).await;
        outcome.map_err(StorageError::from)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use zendiloos_jobs::JobState;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_kernel::PortError;

    use super::*;
    use crate::application::StorageUseCases;
    use crate::testing::FakeStorage;

    #[derive(Default)]
    struct Audit(StdMutex<Vec<AuditEntry>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(entry);
            Ok(())
        }
    }

    struct Setup {
        storage: StorageUseCases,
        fake: Arc<FakeStorage>,
        jobs: Jobs,
        audit: Arc<Audit>,
    }

    fn setup() -> Setup {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let audit = Arc::new(Audit::default());
        let (ports, fake) = FakeStorage::ports(jobs.clone(), audit.clone(), Arc::new(FixedClock));
        Setup { storage: StorageUseCases::new(ports), fake, jobs, audit }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    async fn finished(jobs: &Jobs, job: Job) -> Job {
        let mut events = jobs.subscribe();
        loop {
            if let Some(current) = jobs.find(job.id).await.unwrap()
                && current.state != zendiloos_jobs::JobState::Running
            {
                return current;
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        }
    }

    fn disks(names: &[&str]) -> Vec<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    #[tokio::test]
    async fn a_confirmed_plan_creates_the_pool_once() {
        let s = setup();
        let request = RequestContext::default();
        let planned = s
            .storage
            .plan_pool
            .execute("media", PoolLayout::Mirror, disks(&["sdd", "sde"]), &alice(), &request)
            .await
            .unwrap();
        assert_eq!(planned.plan.volume.get(), 2, "tank has volume 1");

        let wrong = s.storage.create_pool.execute(&planned.token, "medai", &alice(), &request).await;
        assert!(matches!(wrong, Err(StorageError::ConfirmationMismatch)), "{wrong:?}");
        let job = s.storage.create_pool.execute(&planned.token, "media", &alice(), &request).await.unwrap();
        assert_eq!(job.kind, "storage.create_pool");
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.fake.calls(), ["create media Mirror ata-TEST_HDD_T4,ata-TEST_HDD_T5 2"]);
        let entry = s.audit.0.lock().unwrap().pop().unwrap();
        assert_eq!((entry.action, entry.target.as_deref()), ("storage.create_pool", Some("media")));

        let again = s.storage.create_pool.execute(&planned.token, "media", &alice(), &request).await;
        assert!(matches!(again, Err(StorageError::PlanExpired)), "{again:?}");
    }

    #[tokio::test]
    async fn a_plan_is_void_if_the_disks_changed_or_someone_else_confirms() {
        let s = setup();
        let request = RequestContext::default();
        let planned = s
            .storage
            .plan_pool
            .execute("media", PoolLayout::Mirror, disks(&["sdd", "sde"]), &alice(), &request)
            .await
            .unwrap();

        let bob = Actor { account_id: 2, name: "bob".into() };
        let foreign = s.storage.create_pool.execute(&planned.token, "media", &bob, &request).await;
        assert!(matches!(foreign, Err(StorageError::PlanExpired)), "{foreign:?}");

        // Another disk now answers to the name sdd.
        *s.fake.sdd_serial.lock().unwrap() = "T9".into();
        let changed = s.storage.create_pool.execute(&planned.token, "media", &alice(), &request).await;
        assert!(matches!(changed, Err(StorageError::PlanChanged)), "{changed:?}");
        assert!(s.fake.calls().is_empty(), "nothing erased");
    }

    #[tokio::test]
    async fn plans_refuse_disks_in_use() {
        let s = setup();
        let result = s
            .storage
            .plan_pool
            .execute(
                "media",
                PoolLayout::Mirror,
                disks(&["sdc", "sdd"]),
                &alice(),
                &RequestContext::default(),
            )
            .await;
        assert!(
            matches!(result, Err(StorageError::Plan(domain::PlanError::DiskUnusable { .. }))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn destroys_replaces_and_scrubs_managed_pools() {
        let s = setup();
        let request = RequestContext::default();

        let wrong = s.storage.destroy_pool.execute("tank", "tnak", &alice(), &request).await;
        assert!(matches!(wrong, Err(StorageError::ConfirmationMismatch)));
        let job = s.storage.destroy_pool.execute("tank", "tank", &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);

        let planned = s
            .storage
            .plan_replace
            .execute("tank", "4933705814018298948", "sde", &alice(), &request)
            .await
            .unwrap();
        let job = s.storage.replace_disk.execute(&planned.token, &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);

        s.storage.scrub_pool.execute("tank", &alice(), &request).await.unwrap();
        let missing = s.storage.scrub_pool.execute("nope", &alice(), &request).await;
        assert!(matches!(missing, Err(StorageError::PoolNotFound)));

        assert_eq!(
            s.fake.calls(),
            ["destroy tank 1", "replace tank 4933705814018298948 ata-TEST_HDD_T5", "scrub tank"]
        );
    }
}
