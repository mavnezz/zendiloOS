//! Long-running operations, such as creating a pool or replacing a disk.
//!
//! A job is recorded before its work starts and updated as it progresses;
//! every change is published to subscribers (the WebSocket topic `jobs`).
//! After a restart, jobs that were running are marked as interrupted: the
//! work does not survive the process, and nobody should wait for it.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use tokio::sync::broadcast;
use zendiloos_kernel::{Actor, Clock, PortError, RunningWork, Timestamp};

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(i64);

impl JobId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    /// zendiloosd stopped while it ran; its outcome is unknown.
    Interrupted,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "failed" => JobState::Failed,
            "interrupted" => JobState::Interrupted,
            _ => return None,
        })
    }
}

/// Why a job failed: a stable code for the interface and a detail for
/// administrators. Never secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    pub code: String,
    pub detail: Option<String>,
}

impl JobError {
    pub fn new(code: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self { code: code.into(), detail: detail.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: JobId,
    /// Dotted name, e.g. `storage.create_pool`.
    pub kind: String,
    /// What it works on, e.g. a pool name.
    pub target: Option<String>,
    pub state: JobState,
    /// The current step, as a stable code, e.g. `partitioning`.
    pub step: Option<String>,
    /// 0 to 1, if the work can tell.
    pub progress: Option<f32>,
    pub error: Option<JobError>,
    pub account_id: Option<i64>,
    pub actor: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub kind: String,
    pub target: Option<String>,
    pub account_id: Option<i64>,
    pub actor: Option<String>,
    pub created_at: Timestamp,
}

#[async_trait]
pub trait JobRepository: Send + Sync {
    /// Stores a running job.
    async fn create(&self, job: NewJob) -> Result<Job, PortError>;

    async fn update(&self, job: &Job) -> Result<(), PortError>;

    async fn find(&self, id: JobId) -> Result<Option<Job>, PortError>;

    /// The most recent jobs, newest first.
    async fn recent(&self, limit: u32) -> Result<Vec<Job>, PortError>;

    /// The most recent jobs started by the account `account`, newest first.
    async fn recent_of(&self, account: i64, limit: u32) -> Result<Vec<Job>, PortError>;

    /// Marks every running job as interrupted; returns how many.
    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError>;
}

/// Starts jobs and tells subscribers about every change. Cheap to clone.
#[derive(Clone)]
pub struct Jobs {
    inner: Arc<Inner>,
}

struct Inner {
    repository: Arc<dyn JobRepository>,
    clock: Arc<dyn Clock>,
    events: broadcast::Sender<Job>,
    /// The jobs this process runs now, as kind and target.
    running: Mutex<BTreeMap<JobId, (String, Option<String>)>>,
}

/// Takes a job off [`Inner::running`] once its work ended, also when the
/// work panicked.
struct Ended {
    jobs: Jobs,
    id: JobId,
}

impl Drop for Ended {
    fn drop(&mut self) {
        self.jobs.inner.running.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.id);
    }
}

/// Lets running work report its step and progress.
#[derive(Clone)]
pub struct Progress {
    jobs: Jobs,
    job: Arc<Mutex<Job>>,
}

impl Progress {
    /// Records the step and publishes the change; a failure to store it is
    /// logged, not fatal to the work.
    pub async fn step(&self, step: &str, progress: Option<f32>) {
        let job = {
            let mut job = self.job.lock().unwrap_or_else(PoisonError::into_inner);
            job.step = Some(step.to_owned());
            job.progress = progress.map(|p| p.clamp(0.0, 1.0));
            job.updated_at = self.jobs.inner.clock.now();
            job.clone()
        };
        self.jobs.save(&job).await;
    }

    pub fn id(&self) -> JobId {
        self.job.lock().unwrap_or_else(PoisonError::into_inner).id
    }
}

impl Jobs {
    pub fn new(repository: Arc<dyn JobRepository>, clock: Arc<dyn Clock>) -> Self {
        let (events, _) = broadcast::channel(64);
        Self { inner: Arc::new(Inner { repository, clock, events, running: Mutex::new(BTreeMap::new()) }) }
    }

    /// Marks jobs of a previous run as interrupted. Call once at startup.
    pub async fn recover(&self) -> Result<u64, PortError> {
        self.inner.repository.interrupt_running(self.inner.clock.now()).await
    }

    /// Records a job and runs `work` in the background. Returns the job as
    /// recorded; its outcome arrives through [`Jobs::subscribe`].
    pub async fn start<F, Fut>(
        &self,
        kind: &str,
        target: Option<String>,
        actor: Option<&Actor>,
        work: F,
    ) -> Result<Job, PortError>
    where
        F: FnOnce(Progress) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        let job = self
            .inner
            .repository
            .create(NewJob {
                kind: kind.to_owned(),
                target,
                account_id: actor.map(|a| a.account_id),
                actor: actor.map(|a| a.name.clone()),
                created_at: self.inner.clock.now(),
            })
            .await?;
        let _ = self.inner.events.send(job.clone());
        self.inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job.id, (job.kind.clone(), job.target.clone()));
        let ended = Ended { jobs: self.clone(), id: job.id };
        let progress = Progress { jobs: self.clone(), job: Arc::new(Mutex::new(job.clone())) };
        let jobs = self.clone();
        tokio::spawn(async move {
            let outcome = work(progress.clone()).await;
            drop(ended);
            let finished = {
                let mut job = progress.job.lock().unwrap_or_else(PoisonError::into_inner);
                let now = jobs.inner.clock.now();
                match outcome {
                    Ok(()) => {
                        job.state = JobState::Succeeded;
                        job.progress = Some(1.0);
                    }
                    Err(error) => {
                        tracing::warn!(job = job.id.get(), kind = job.kind, code = error.code, detail = ?error.detail, "job failed");
                        job.state = JobState::Failed;
                        job.error = Some(error);
                    }
                }
                job.updated_at = now;
                job.finished_at = Some(now);
                job.clone()
            };
            jobs.save(&finished).await;
        });
        Ok(job)
    }

    async fn save(&self, job: &Job) {
        if let Err(error) = self.inner.repository.update(job).await {
            tracing::warn!(job = job.id.get(), %error, "cannot store the job's progress");
        }
        let _ = self.inner.events.send(job.clone());
    }

    /// Every change to a job from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Job> {
        self.inner.events.subscribe()
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<Job>, PortError> {
        self.inner.repository.recent(limit.clamp(1, 200)).await
    }

    /// As [`Jobs::recent`], of one account's own: by its id, as a name may
    /// be another account's later.
    pub async fn recent_of(&self, account: i64, limit: u32) -> Result<Vec<Job>, PortError> {
        self.inner.repository.recent_of(account, limit.clamp(1, 200)).await
    }

    pub async fn find(&self, id: JobId) -> Result<Option<Job>, PortError> {
        self.inner.repository.find(id).await
    }

    /// The jobs this process runs now, oldest first, as kind and target.
    pub fn running(&self) -> Vec<(String, Option<String>)> {
        self.inner.running.lock().unwrap_or_else(PoisonError::into_inner).values().cloned().collect()
    }
}

#[async_trait]
impl RunningWork for Jobs {
    /// Each as `kind target`, e.g. `sharing.apply_access photos`.
    async fn running(&self) -> Vec<String> {
        Jobs::running(self)
            .into_iter()
            .map(|(kind, target)| match target {
                Some(target) => format!("{kind} {target}"),
                None => kind,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::testing::{FixedClock, MemoryJobs};

    fn jobs() -> (Jobs, Arc<MemoryJobs>) {
        let repository = Arc::new(MemoryJobs::default());
        (Jobs::new(repository.clone(), Arc::new(FixedClock)), repository)
    }

    async fn next(events: &mut broadcast::Receiver<Job>) -> Job {
        tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn records_progress_and_success() {
        let (jobs, repository) = jobs();
        let mut events = jobs.subscribe();
        let actor = Actor { account_id: 1, name: "alice".into() };

        let job = jobs
            .start("storage.create_pool", Some("tank".into()), Some(&actor), |progress| async move {
                progress.step("partitioning", Some(0.5)).await;
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!((job.state, job.actor.as_deref()), (JobState::Running, Some("alice")));
        assert_eq!(next(&mut events).await.state, JobState::Running);
        let step = next(&mut events).await;
        assert_eq!((step.step.as_deref(), step.progress), (Some("partitioning"), Some(0.5)));
        let done = next(&mut events).await;
        assert_eq!((done.state, done.progress), (JobState::Succeeded, Some(1.0)));
        assert!(done.finished_at.is_some());
        assert_eq!(repository.all()[0], done, "stored as published");
    }

    #[tokio::test]
    async fn records_failures_with_their_code() {
        let (jobs, _) = jobs();
        let mut events = jobs.subscribe();
        jobs.start("storage.destroy_pool", None, None, |_| async {
            Err(JobError::new("agent_failed", Some("zpool: pool is busy".to_owned())))
        })
        .await
        .unwrap();
        next(&mut events).await;
        let failed = next(&mut events).await;
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.error.unwrap().code, "agent_failed");
    }

    #[tokio::test]
    async fn knows_what_runs_until_it_ended() {
        let (jobs, _) = jobs();
        let mut events = jobs.subscribe();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        jobs.start("sharing.apply_access", Some("photos".into()), None, |_| async move {
            let _ = finished.await;
            Ok(())
        })
        .await
        .unwrap();
        jobs.start("storage.scrub", None, None, |_| async { Ok(()) }).await.unwrap();
        // Both starts, then the scrub's end.
        for _ in 0..3 {
            next(&mut events).await;
        }
        assert_eq!(RunningWork::running(&jobs).await, ["sharing.apply_access photos"]);

        finish.send(()).unwrap();
        assert_eq!(next(&mut events).await.state, JobState::Succeeded);
        assert!(RunningWork::running(&jobs).await.is_empty());
    }

    #[tokio::test]
    async fn a_job_whose_work_panicked_runs_no_more() {
        let (jobs, _) = jobs();
        jobs.start("storage.scrub", None, None, |_| async { panic!("broken") }).await.unwrap();
        for _ in 0..100 {
            if jobs.running().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("still listed as running");
    }

    #[tokio::test]
    async fn a_restart_interrupts_running_jobs() {
        let (jobs, repository) = jobs();
        jobs.start("storage.create_pool", None, None, |_| std::future::pending()).await.unwrap();
        assert_eq!(jobs.recover().await.unwrap(), 1);
        assert_eq!(repository.all()[0].state, JobState::Interrupted);
    }
}
