//! Long-running operations, such as creating a pool or replacing a disk.
//!
//! A job is recorded before its work starts and updated as it progresses;
//! every change is published to subscribers (the WebSocket topic `jobs`).
//! After a restart, jobs that were running are marked as interrupted: the
//! work does not survive the process, and nobody should wait for it.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_trait::async_trait;
use tokio::sync::broadcast;
use zendiloos_kernel::{Actor, Clock, PortError, RunningWork, Timestamp, WorkHeld, WorkHold};

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
    /// What a program the work runs wrote, for a person to follow (Docker
    /// Compose deploying a project): the last [`OUTPUT_LINES`] lines, each
    /// cleaned by [`clean_line`]. Empty for most jobs.
    pub output: Vec<String>,
}

/// Lines of output a job keeps; older ones go.
pub const OUTPUT_LINES: usize = 200;

/// Characters of one line of output a job keeps; the rest of it goes.
pub const OUTPUT_LINE_CHARS: usize = 300;

/// How often a job's output is saved and published at most while lines
/// come; a line waits that long at most.
const OUTPUT_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// What a line reports the progress of, if it is one such line: a layer
/// of an image being pulled (`76a9dff22e25 Downloading [==>  ] …`) or a
/// container, network or volume Compose works on (`Container web-1
/// Created`). A newer line of the same thing takes the older one's place,
/// as a terminal shows it, instead of a line each step.
pub fn progress_key(line: &str) -> Option<&str> {
    const LAYER_STATES: [&str; 9] = [
        "Pulling fs layer",
        "Waiting",
        "Downloading",
        "Verifying Checksum",
        "Download complete",
        "Extracting",
        "Pull complete",
        "Already exists",
        "Retrying",
    ];
    let line = line.trim_start();
    let (first, rest) = line.split_once(' ')?;
    if first.len() == 12 && first.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        let rest = rest.trim_start();
        return LAYER_STATES.iter().any(|state| rest.starts_with(state)).then_some(first);
    }
    if matches!(first, "Container" | "Network" | "Volume") {
        let name_end = rest.find(char::is_whitespace)?;
        let thing = &line[..first.len() + 1 + name_end];
        return (!line[thing.len()..].trim().is_empty()).then_some(thing);
    }
    None
}

/// A line of output as a job keeps it: terminal escape sequences (colours,
/// cursor moves) and other control characters out, tabs as spaces, at most
/// [`OUTPUT_LINE_CHARS`] characters; `None` if nothing is left.
pub fn clean_line(line: &str) -> Option<String> {
    let mut clean = String::with_capacity(line.len().min(OUTPUT_LINE_CHARS));
    let mut chars = line.chars().peekable();
    let mut kept = 0;
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte.
                Some('[') => {
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \.
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => {
                clean.push(' ');
                kept += 1;
            }
            c if c.is_control() => {}
            c => {
                clean.push(c);
                kept += 1;
            }
        }
        if kept >= OUTPUT_LINE_CHARS {
            break;
        }
    }
    let clean = clean.trim_end();
    (!clean.trim().is_empty()).then(|| clean.to_owned())
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
    /// The jobs this process runs now, as kind and target, from the moment
    /// they are asked to start, in that order.
    running: Mutex<Running>,
    /// What keeps new jobs from starting, once set ([`Jobs::hold_while`]).
    hold: OnceLock<Arc<dyn WorkHold>>,
}

#[derive(Default)]
struct Running {
    next: u64,
    jobs: BTreeMap<u64, (String, Option<String>)>,
}

/// Takes a job off [`Inner::running`] once its work ended, or it did not
/// start, also when the work panicked.
struct Ended {
    jobs: Jobs,
    turn: u64,
}

impl Drop for Ended {
    fn drop(&mut self) {
        self.jobs.inner.running.lock().unwrap_or_else(PoisonError::into_inner).jobs.remove(&self.turn);
    }
}

/// Lets running work report its step and progress.
#[derive(Clone)]
pub struct Progress {
    jobs: Jobs,
    job: Arc<Mutex<Job>>,
    /// Saves go one at a time, each of the job as it is then: a slow save
    /// of an older state never lands after a newer one (the end).
    order: Arc<tokio::sync::Mutex<()>>,
    /// Whether a save of new output waits, and when output was saved last.
    output: Arc<Mutex<(bool, Option<std::time::Instant>)>>,
}

impl Progress {
    fn new(jobs: Jobs, job: Job) -> Self {
        Self {
            jobs,
            job: Arc::new(Mutex::new(job)),
            order: Arc::default(),
            output: Arc::new(Mutex::new((false, None))),
        }
    }

    fn change(&self, change: impl FnOnce(&mut Job)) {
        let mut job = self.job.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut job);
        job.updated_at = self.jobs.inner.clock.now();
    }

    /// Stores and publishes the job as it is now.
    async fn publish(&self) {
        let _turn = self.order.lock().await;
        let job = self.job.lock().unwrap_or_else(PoisonError::into_inner).clone();
        self.jobs.save(&job).await;
    }

    /// Records the step and publishes the change; a failure to store it is
    /// logged, not fatal to the work.
    pub async fn step(&self, step: &str, progress: Option<f32>) {
        self.change(|job| {
            job.step = Some(step.to_owned());
            job.progress = progress.map(|p| p.clamp(0.0, 1.0));
        });
        self.publish().await;
    }

    /// Adds a line a program wrote to the job's output ([`clean_line`],
    /// the last [`OUTPUT_LINES`] kept). Lines come fast: they are published
    /// together, at most every quarter of a second, in the background.
    pub fn line(&self, line: &str) {
        let Some(line) = clean_line(line) else { return };
        self.change(|job| {
            if let Some(key) = progress_key(&line) {
                let older = job.output.iter_mut().rev().find(|old| progress_key(old) == Some(key));
                if let Some(older) = older {
                    *older = line;
                    return;
                }
            }
            job.output.push(line);
            let excess = job.output.len().saturating_sub(OUTPUT_LINES);
            job.output.drain(..excess);
        });
        let wait = {
            let mut output = self.output.lock().unwrap_or_else(PoisonError::into_inner);
            if output.0 {
                return;
            }
            output.0 = true;
            output.1.map_or(std::time::Duration::ZERO, |last| OUTPUT_EVERY.saturating_sub(last.elapsed()))
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            *this.output.lock().unwrap_or_else(PoisonError::into_inner) =
                (false, Some(std::time::Instant::now()));
            this.publish().await;
        });
    }

    pub fn id(&self) -> JobId {
        self.job.lock().unwrap_or_else(PoisonError::into_inner).id
    }
}

impl Jobs {
    pub fn new(repository: Arc<dyn JobRepository>, clock: Arc<dyn Clock>) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                repository,
                clock,
                events,
                running: Mutex::default(),
                hold: OnceLock::new(),
            }),
        }
    }

    /// No job starts while `hold` holds new work back (an update of the
    /// system installing): [`Jobs::start`] refuses with [`WorkHeld`]. Set
    /// once, after both are made; a later call changes nothing.
    pub fn hold_while(&self, hold: Arc<dyn WorkHold>) {
        let _ = self.inner.hold.set(hold);
    }

    /// What holds new jobs back now, if anything: a schedule that comes due
    /// meanwhile waits rather than be refused.
    pub async fn held(&self) -> Option<String> {
        match self.inner.hold.get() {
            Some(hold) => hold.holding().await,
            None => None,
        }
    }

    /// Marks jobs of a previous run as interrupted. Call once at startup.
    pub async fn recover(&self) -> Result<u64, PortError> {
        self.inner.repository.interrupt_running(self.inner.clock.now()).await
    }

    /// Records a job and runs `work` in the background. Returns the job as
    /// recorded; its outcome arrives through [`Jobs::subscribe`]. Refused,
    /// with [`WorkHeld`] as the error's source, while something holds new
    /// work back ([`Jobs::hold_while`]).
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
        // Counted as running before the hold is asked: an update starting
        // meanwhile sees it, or this one sees the update.
        let turn = {
            let mut running = self.inner.running.lock().unwrap_or_else(PoisonError::into_inner);
            running.next += 1;
            let turn = running.next;
            running.jobs.insert(turn, (kind.to_owned(), target.clone()));
            turn
        };
        let ended = Ended { jobs: self.clone(), turn };
        if let Some(why) = self.held().await {
            return Err(PortError::new("background work", WorkHeld(why)));
        }
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
        let progress = Progress::new(self.clone(), job.clone());
        let jobs = self.clone();
        tokio::spawn(async move {
            let outcome = work(progress.clone()).await;
            drop(ended);
            progress.change(|job| {
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
                job.finished_at = Some(jobs.inner.clock.now());
            });
            progress.publish().await;
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
        self.inner.running.lock().unwrap_or_else(PoisonError::into_inner).jobs.values().cloned().collect()
    }
}

#[async_trait]
impl RunningWork for Jobs {
    /// Each as `kind target`, e.g. `sharing.migrate_permissions photos`.
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

    #[test]
    fn progress_lines_are_told_by_what_they_report_on() {
        assert_eq!(progress_key("76a9dff22e25 Downloading [==>   ]  2.21MB/42.67MB"), Some("76a9dff22e25"));
        assert_eq!(progress_key(" 76a9dff22e25 Pull complete"), Some("76a9dff22e25"));
        assert_eq!(progress_key("Container web-1  Recreated"), Some("Container web-1"));
        assert_eq!(progress_key(" Network tesla_default  Created"), Some("Network tesla_default"));
        // Not progress: said once.
        assert_eq!(progress_key("jellyfin Pulling"), None);
        assert_eq!(progress_key("76a9dff22e25 is a layer"), None);
        assert_eq!(progress_key("Container"), None);
        assert_eq!(progress_key("Container web-1"), None);
        assert_eq!(progress_key("abcdefabcdeg Downloading"), None);
    }

    #[tokio::test]
    async fn a_newer_progress_line_takes_the_older_ones_place() {
        let (jobs, _) = jobs();
        let mut events = jobs.subscribe();
        jobs.start("containers.project_deploy", None, None, |progress| async move {
            for line in [
                " jellyfin Pulling",
                "76a9dff22e25 Pulling fs layer",
                "6dd3dbc22433 Pulling fs layer",
                "76a9dff22e25 Downloading [>     ]  1MB/42MB",
                "76a9dff22e25 Downloading [===>  ]  20MB/42MB",
                "6dd3dbc22433 Download complete",
                "76a9dff22e25 Pull complete",
                " jellyfin Pulled",
                "Container jellyfin-1  Creating",
                "Container jellyfin-1  Started",
            ] {
                progress.line(line);
            }
            Ok(())
        })
        .await
        .unwrap();
        let done = loop {
            let job = next(&mut events).await;
            if job.state != JobState::Running {
                break job;
            }
        };
        assert_eq!(
            done.output,
            [
                " jellyfin Pulling",
                "76a9dff22e25 Pull complete",
                "6dd3dbc22433 Download complete",
                " jellyfin Pulled",
                "Container jellyfin-1  Started",
            ]
        );
    }

    #[test]
    fn output_lines_are_cleaned_of_what_a_terminal_would_act_on_and_cut() {
        assert_eq!(
            clean_line("\u{1b}[32m✔\u{1b}[0m Container web\tStarted  ").as_deref(),
            Some("✔ Container web Started")
        );
        assert_eq!(clean_line("\u{1b}]0;title\u{7}pulled").as_deref(), Some("pulled"));
        assert_eq!(clean_line("\u{1b}]8;;https://x\u{1b}\\link").as_deref(), Some("link"));
        assert_eq!(clean_line("a\u{0}b\u{8}c<b>").as_deref(), Some("abc<b>"), "markup is text, kept as such");
        assert_eq!(clean_line(" \u{1b}[2K "), None);
        assert_eq!(clean_line(&"é".repeat(1000)).unwrap().chars().count(), OUTPUT_LINE_CHARS);
    }

    #[tokio::test]
    async fn output_is_kept_bounded_and_published_while_the_work_runs() {
        let (jobs, repository) = jobs();
        let mut events = jobs.subscribe();
        jobs.start("containers.project_deploy", Some("tesla".into()), None, |progress| async move {
            for n in 0..OUTPUT_LINES + 5 {
                progress.line(&format!("line {n}"));
            }
            progress.line("\u{1b}[2K");
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(next(&mut events).await.state, JobState::Running, "started");
        let during = next(&mut events).await;
        assert_eq!(during.state, JobState::Running);
        assert_eq!(during.output.len(), OUTPUT_LINES);
        assert_eq!(during.output[0], "line 5", "the oldest went");
        let done = next(&mut events).await;
        assert_eq!(done.state, JobState::Succeeded);
        assert_eq!(done.output.last().map(String::as_str), Some("line 204"));
        assert_eq!(repository.all()[0], done);
    }

    #[tokio::test]
    async fn the_end_is_published_last_whatever_lines_came_just_before() {
        let (jobs, repository) = jobs();
        let mut events = jobs.subscribe();
        jobs.start("containers.project_deploy", None, None, |progress| async move {
            progress.line("last words");
            Ok(())
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut last = None;
        while let Ok(job) = events.try_recv() {
            last = Some(job);
        }
        let last = last.unwrap();
        assert_eq!((last.state, last.output.clone()), (JobState::Succeeded, vec!["last words".to_owned()]));
        assert_eq!(repository.all()[0], last, "stored as published last");
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
        jobs.start("sharing.migrate_permissions", Some("photos".into()), None, |_| async move {
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
        assert_eq!(RunningWork::running(&jobs).await, ["sharing.migrate_permissions photos"]);

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

    /// Holds new work back while `held` is set; tells when it was asked.
    #[derive(Default)]
    struct Gate {
        held: Mutex<Option<String>>,
        asked: tokio::sync::Notify,
        go: tokio::sync::Notify,
        wait: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl WorkHold for Gate {
        async fn holding(&self) -> Option<String> {
            if self.wait.load(std::sync::atomic::Ordering::SeqCst) {
                self.asked.notify_one();
                self.go.notified().await;
            }
            self.held.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn no_job_starts_while_an_update_holds_new_work_back() {
        let (jobs, repository) = jobs();
        assert_eq!(jobs.held().await, None, "nothing holds without a hold");
        let gate = Arc::new(Gate::default());
        jobs.hold_while(gate.clone());
        *gate.held.lock().unwrap() = Some("system.update".into());

        let refused =
            jobs.start("containers.volume_backup", Some("db".into()), None, |_| async { Ok(()) }).await;
        let error = refused.unwrap_err();
        let why = std::error::Error::source(&error).and_then(|source| source.downcast_ref::<WorkHeld>());
        assert_eq!(why, Some(&WorkHeld("system.update".into())));
        assert!(repository.all().is_empty(), "nothing recorded");
        assert!(jobs.running().is_empty());
        assert_eq!(jobs.held().await.as_deref(), Some("system.update"));

        *gate.held.lock().unwrap() = None;
        jobs.start("storage.scrub", Some("tank".into()), None, |_| std::future::pending()).await.unwrap();
        assert_eq!(repository.all().len(), 1);
    }

    #[tokio::test]
    async fn a_job_counts_as_running_while_it_asks_whether_it_may_start() {
        let (jobs, _) = jobs();
        let gate = Arc::new(Gate::default());
        gate.wait.store(true, std::sync::atomic::Ordering::SeqCst);
        jobs.hold_while(gate.clone());
        let starting = {
            let jobs = jobs.clone();
            tokio::spawn(async move {
                jobs.start("files.copy", Some("photos".into()), None, |_| std::future::pending()).await
            })
        };
        gate.asked.notified().await;
        // An update deciding now sees it.
        assert_eq!(RunningWork::running(&jobs).await, ["files.copy photos"]);
        gate.go.notify_one();
        starting.await.unwrap().unwrap();
        assert_eq!(RunningWork::running(&jobs).await, ["files.copy photos"]);
    }

    #[tokio::test]
    async fn a_restart_interrupts_running_jobs() {
        let (jobs, repository) = jobs();
        jobs.start("storage.create_pool", None, None, |_| std::future::pending()).await.unwrap();
        assert_eq!(jobs.recover().await.unwrap(), 1);
        assert_eq!(repository.all()[0].state, JobState::Interrupted);
    }
}
