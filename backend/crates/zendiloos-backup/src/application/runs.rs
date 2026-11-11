//! Runs: started by hand, on schedule or when a task's drive is plugged
//! in, one per task at a time, each a job; stopped on request; logged with
//! their transfer list; followed by retention; told when they fail (and
//! when they succeed, if the task says so).

use std::sync::{Arc, Mutex, PoisonError};

use zendiloos_jobs::{Job, Progress};
use zendiloos_kernel::{Actor, RequestContext};
use zendiloos_notify::{Level, NewNotification};

use super::ports::{Location, NewRun, RunEnd, RunMode, RunPlan, RunReport, Secrets, Steps};
use super::{BackupError, Backups, Claim, audited, job_error};
use crate::domain::{
    Mode, Outcome, REMOVED, Run, TargetState, Task, Trigger, beyond_retention, removed_beyond, version_name,
};

/// The newest step not yet published, and whether a task publishes.
#[derive(Default)]
struct Pending {
    step: Option<(String, Option<u8>)>,
    publishing: bool,
}

/// A job's step from a step reported without waiting: the newest is
/// published, one at a time.
pub(super) struct JobSteps {
    progress: Progress,
    pending: Arc<Mutex<Pending>>,
}

impl JobSteps {
    pub(super) fn new(progress: Progress) -> Self {
        Self { progress, pending: Arc::default() }
    }
}

impl Steps for JobSteps {
    fn step(&self, step: &str, percent: Option<u8>) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.step = Some((step.to_owned(), percent));
        if pending.publishing {
            return;
        }
        pending.publishing = true;
        let (progress, shared) = (self.progress.clone(), Arc::clone(&self.pending));
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut pending = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    match pending.step.take() {
                        Some(next) => next,
                        None => {
                            pending.publishing = false;
                            return;
                        }
                    }
                };
                progress.step(&next.0, next.1.map(|p| f32::from(p) / 100.0)).await;
            }
        });
    }
}

/// Characters of an error the log and the notification keep.
const ERROR_CHARS: usize = 600;

fn shortened(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= ERROR_CHARS {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(ERROR_CHARS).collect();
    short.push('…');
    short
}

impl Backups {
    /// "Back up now".
    pub async fn run_now(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, BackupError> {
        let task = self.find(id).await?;
        self.require_installed(context).await?;
        self.start(task, Trigger::Manual, Some(actor), context).await
    }

    /// Stops a task's run: rsync is ended, what it wrote stays (a version
    /// keeps its `.incomplete` name, which the next run goes on from).
    pub async fn stop(&self, id: i64, actor: &Actor, context: &RequestContext) -> Result<(), BackupError> {
        let task = self.find(id).await?;
        {
            let mut busy = self.lock();
            let slot = busy.runs.get_mut(&id).ok_or(BackupError::NotRunning)?;
            slot.stopping = true;
        }
        let done = self.ports.host.stop(id, context).await;
        self.audit("backup.stop", audited(&done), task.name(), Some(actor), context).await;
        Ok(done?)
    }

    /// Starts what is due; run every minute. A schedule whose time came
    /// starts its task once, counted as run from then whatever comes of
    /// it; one that came while the task ran waits until it ended. A task
    /// whose drive was plugged in (another plugging in than the one it
    /// last backed up for) starts as well. While an update of the system
    /// installs, nothing starts and what is due stays due.
    pub async fn run_due(&self) -> Vec<Job> {
        if let Some(why) = self.ports.jobs.held().await {
            tracing::info!(why, "backup: what is due waits until the update is done");
            return Vec::new();
        }
        let context = RequestContext::default();
        let tasks = match self.ports.tasks.list().await {
            Ok(tasks) => tasks,
            Err(error) => {
                tracing::warn!(%error, "backup: cannot read the tasks");
                return Vec::new();
            }
        };
        let drives = if tasks.iter().any(|task| task.settings.on_plug) {
            self.ports.drives.drives(&context).await.unwrap_or_else(|error| {
                tracing::info!(%error, "backup: cannot look at the removable drives");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let now = self.now();
        let offsets = |at| self.ports.clock.utc_offset(at);
        let mut jobs = Vec::new();
        for task in tasks {
            if self.running(task.id) {
                continue;
            }
            let plugged = task.settings.on_plug.then(|| task.target_drive()).flatten().and_then(|drive| {
                drives.iter().find(|d| d.id == drive && d.mounted).and_then(|d| d.plugged_in.clone())
            });
            if let Some(plug) = plugged.filter(|plug| task.last_plug.as_ref() != Some(plug)) {
                if let Err(error) = self.ports.tasks.plugged(task.id, Some(&plug)).await {
                    tracing::warn!(%error, task = task.id, "backup: cannot note the drive plugged in; not started");
                    continue;
                }
                tracing::info!(task = task.id, "backup: its drive was plugged in");
                self.start_quietly(task, Trigger::Plug, &context, &mut jobs).await;
                continue;
            }
            let Some(schedule) = task.settings.schedule else { continue };
            if task.since > now {
                // Noted while the clock was ahead: due from now on.
                if let Err(error) = self.ports.tasks.noted(task.id, now).await {
                    tracing::warn!(%error, task = task.id, "backup: cannot note the task anew");
                }
                continue;
            }
            if !schedule.is_due(task.since, now, &offsets) {
                continue;
            }
            if let Err(error) = self.ports.tasks.noted(task.id, now).await {
                tracing::warn!(%error, task = task.id, "backup: cannot note the run; not started");
                continue;
            }
            // A drive that is not there: its plugging in starts the task,
            // if it is set so; otherwise the run fails and is told.
            if task.settings.on_plug
                && let Some(drive) = task.target_drive()
                && !drives.iter().any(|d| d.id == drive && d.mounted)
            {
                tracing::info!(task = task.id, "backup: its drive is not plugged in; it backs up once it is");
                continue;
            }
            self.start_quietly(task, Trigger::Schedule, &context, &mut jobs).await;
        }
        jobs
    }

    async fn start_quietly(
        &self,
        task: Task,
        trigger: Trigger,
        context: &RequestContext,
        jobs: &mut Vec<Job>,
    ) {
        let (id, name) = (task.id, task.settings.name.clone());
        match self.start(task, trigger, None, context).await {
            Ok(job) => jobs.push(job),
            Err(error) => {
                tracing::warn!(%error, task = id, "backup: not started");
                // Recorded and told as a run that failed at once.
                self.failed_to_start(id, &name, trigger, &error).await;
            }
        }
    }

    async fn failed_to_start(&self, id: i64, name: &str, trigger: Trigger, error: &BackupError) {
        let now = self.now();
        let run = NewRun { task_id: id, trigger, by: None, started_at: now, job_id: None };
        let end = RunEnd {
            finished_at: now,
            outcome: Outcome::Failed,
            files: 0,
            bytes: 0,
            version: None,
            error: Some(shortened(&error.to_string())),
            transfers: Vec::new(),
            transfers_total: 0,
        };
        match self.ports.runs.start(&run).await {
            Ok(run) => {
                if let Err(error) = self.ports.runs.finish(run, &end).await {
                    tracing::warn!(%error, "backup: cannot log a run that did not start");
                }
            }
            Err(error) => tracing::warn!(%error, "backup: cannot log a run that did not start"),
        }
        self.tell(name, Outcome::Failed, end.error.as_deref(), 0, 0).await;
    }

    /// Where a task's run goes, as the host reaches it now.
    async fn plan(&self, task: &Task, context: &RequestContext) -> Result<RunPlan, BackupError> {
        let (source, _) = self.place(&task.settings.source, context).await?;
        let (target, drive) = self.place(&task.settings.target, context).await?;
        if let Some(drive) = drive {
            if drive.read_only {
                return Err(BackupError::DriveReadOnly);
            }
            if matches!(task.settings.mode, Mode::Versioned { .. })
                && !task.settings.rclone()
                && !drive.links()
            {
                return Err(BackupError::Refused(format!(
                    "versions need hard links, which {} has not: back up as a mirror, or format the drive as ext4",
                    drive.filesystem
                )));
            }
        }
        let bwlimit_kib = match task.settings.bwlimit_kib {
            Some(limit) => limit,
            None => self.ports.settings.get().await?.bwlimit_kib,
        };
        Ok(RunPlan {
            task: task.id,
            source,
            target: target.join(&task.folder),
            mode: match task.settings.mode {
                Mode::Mirror { keep_changed_days } => {
                    RunMode::Mirror { keep_changed: keep_changed_days.is_some() }
                }
                Mode::Versioned { .. } => RunMode::Versioned,
            },
            stamp: version_name(self.now()),
            filters: task.settings.filters.clone(),
            bwlimit_kib,
            encrypted: task.settings.encrypted,
        })
    }

    async fn place(
        &self,
        place: &crate::domain::Place,
        context: &RequestContext,
    ) -> Result<(Location, Option<super::Drive>), BackupError> {
        self.reach(place, context).await
    }

    /// Claims the task, plans its run and starts it as a job.
    async fn start(
        &self,
        task: Task,
        trigger: Trigger,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) -> Result<Job, BackupError> {
        let claim = self.claim(task.id, false)?;
        let plan = self.plan(&task, context).await?;
        let secrets = self.secrets(&task).await?;
        let this = self.clone();
        let (who, context) = (actor.cloned(), context.clone());
        let name = task.settings.name.clone();
        let job = self
            .ports
            .jobs
            .start("backup.run", Some(name), actor, move |progress| async move {
                let job_id = progress.id().get();
                let steps = JobSteps::new(progress);
                this.carry_out(&task, &plan, secrets, trigger, who.as_ref(), job_id, &steps, &context, claim)
                    .await
            })
            .await?;
        Ok(job)
    }

    /// One run: logged, carried out, retention after a complete one,
    /// audited and told.
    #[allow(clippy::too_many_arguments)]
    async fn carry_out(
        &self,
        task: &Task,
        plan: &RunPlan,
        secrets: Secrets,
        trigger: Trigger,
        actor: Option<&Actor>,
        job_id: i64,
        steps: &JobSteps,
        context: &RequestContext,
        claim: Claim,
    ) -> Result<(), zendiloos_jobs::JobError> {
        let started = NewRun {
            task_id: task.id,
            trigger,
            by: actor.map(|a| a.name.clone()),
            started_at: self.now(),
            job_id: Some(job_id),
        };
        let run = self.ports.runs.start(&started).await.map_err(|e| job_error(&e.into()))?;
        steps.step("transferring", Some(0));
        let done = self.ports.host.run(plan, &secrets, steps, context).await.map_err(BackupError::from);
        if let Ok(report) = &done {
            self.keep_token(task, report.token.as_ref()).await;
        }
        let stopping = self.lock().runs.get(&task.id).is_some_and(|slot| slot.stopping);
        let versioned = matches!(plan.mode, RunMode::Versioned);
        let end = match &done {
            Ok(report) => RunEnd {
                finished_at: self.now(),
                outcome: if report.partial.is_some() { Outcome::Partial } else { Outcome::Succeeded },
                files: report.files,
                bytes: report.bytes,
                version: versioned.then(|| plan.stamp.clone()),
                error: report.partial.as_deref().map(shortened),
                transfers: report.transfers.iter().take(crate::domain::TRANSFERS_KEPT).cloned().collect(),
                transfers_total: report.transfers_total,
            },
            Err(error) => RunEnd {
                finished_at: self.now(),
                outcome: if stopping { Outcome::Stopped } else { Outcome::Failed },
                files: 0,
                bytes: 0,
                version: None,
                error: Some(shortened(&error.to_string())),
                transfers: Vec::new(),
                transfers_total: 0,
            },
        };
        if let Ok(report) = &done {
            steps.step("pruning", None);
            self.retain(task, plan, report, &secrets, context).await;
        }
        if let Err(error) = self.ports.runs.finish(run, &end).await {
            tracing::error!(%error, task = task.id, "backup: cannot log how the run ended");
        }
        drop(claim);
        let action_outcome = if end.outcome.complete() {
            zendiloos_kernel::Outcome::Success
        } else {
            zendiloos_kernel::Outcome::Failure
        };
        self.audit("backup.run", action_outcome, task.name(), actor, context).await;
        if end.outcome != Outcome::Stopped
            && (end.outcome != Outcome::Succeeded || task.settings.notify_success)
        {
            self.tell(task.name(), end.outcome, end.error.as_deref(), end.files, end.bytes).await;
        }
        match done {
            Ok(_) => Ok(()),
            Err(error) if stopping => Err(zendiloos_jobs::JobError::new("stopped", Some(error.to_string()))),
            Err(error) => Err(job_error(&error)),
        }
    }

    /// Removes what retention lets go after a complete run: versions beyond
    /// it and the folders of versions cut off before (versioned), kept
    /// changes older than their days (mirror). A failure is logged; the
    /// next run tries again.
    async fn retain(
        &self,
        task: &Task,
        plan: &RunPlan,
        report: &RunReport,
        secrets: &Secrets,
        context: &RequestContext,
    ) {
        let state = TargetState::of(&report.entries);
        let now = self.now();
        let mut names: Vec<String> = Vec::new();
        match task.settings.mode {
            Mode::Versioned { keep_last, keep_days } => {
                names.extend(
                    beyond_retention(&state.versions, keep_last, keep_days, now)
                        .into_iter()
                        .map(version_name),
                );
                names.extend(state.incomplete.iter().cloned());
            }
            Mode::Mirror { keep_changed_days: Some(days) } => {
                let removed = self.kept_changes(task, &plan.target, secrets, context).await;
                names.extend(
                    removed_beyond(&removed, days, now)
                        .into_iter()
                        .map(|at| format!("{REMOVED}/{}", version_name(at))),
                );
            }
            Mode::Mirror { keep_changed_days: None } => {}
        }
        if names.is_empty() {
            return;
        }
        let pruned = self.ports.host.prune(task.id, &plan.target, &names, secrets, context).await;
        match pruned {
            Ok(token) => {
                self.keep_token(task, token.as_ref()).await;
                tracing::info!(
                    task = task.id,
                    removed = names.len(),
                    "backup: retention removed old versions"
                )
            }
            Err(error) => {
                tracing::warn!(%error, task = task.id, "backup: retention could not remove old versions")
            }
        }
    }

    /// When a mirror's kept changes were made, from the folders' names.
    async fn kept_changes(
        &self,
        task: &Task,
        target: &Location,
        secrets: &Secrets,
        context: &RequestContext,
    ) -> Vec<zendiloos_kernel::Timestamp> {
        match self.ports.host.list(target, REMOVED, secrets, context).await {
            Ok(listing) => {
                self.keep_token(task, listing.token.as_ref()).await;
                let entries = listing.entries;
                let mut times: Vec<_> = entries
                    .iter()
                    .filter(|e| e.kind == crate::domain::EntryKind::Folder)
                    .filter_map(|e| crate::domain::parse_version(&e.name))
                    .collect();
                times.sort();
                times
            }
            Err(error) => {
                tracing::warn!(%error, "backup: cannot list what a mirror kept");
                Vec::new()
            }
        }
    }

    /// A run that did not succeed (or did, if the task wants to know).
    async fn tell(&self, task: &str, outcome: Outcome, reason: Option<&str>, files: u64, bytes: u64) {
        let notification = match outcome {
            Outcome::Succeeded => NewNotification::new(Level::Success, "backup_succeeded")
                .with("task", task)
                .with("files", files.to_string())
                .with("bytes", bytes.to_string()),
            Outcome::Partial => NewNotification::new(Level::Warning, "backup_partial")
                .with("task", task)
                .with("reason", reason.unwrap_or_default()),
            _ => NewNotification::new(Level::Error, "backup_failed")
                .with("task", task)
                .with("reason", reason.unwrap_or_default()),
        };
        self.ports.notifier.notify(notification).await;
    }

    /// Runs of one task or of all, newest first.
    pub async fn runs(&self, task: Option<i64>, limit: u32) -> Result<Vec<Run>, BackupError> {
        if let Some(id) = task {
            self.find(id).await?;
        }
        Ok(self.ports.runs.list(task, limit.clamp(1, 500)).await?)
    }

    /// A run's transfer list, as kept.
    pub async fn transfers(
        &self,
        run: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(Run, Vec<crate::domain::Transfer>), BackupError> {
        let found = self.ports.runs.find(run).await?.ok_or(BackupError::NotFound)?;
        let transfers = self.ports.runs.transfers(run, offset, limit.clamp(1, 1000)).await?;
        Ok((found, transfers))
    }

    /// Runs that were going when zendiloosd stopped are interrupted; once,
    /// at the start.
    pub async fn recover(&self) -> u64 {
        match self.ports.runs.interrupt_running(self.now()).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(%error, "backup: cannot mark runs cut off");
                0
            }
        }
    }

    /// Forgets runs older than the settings keep; daily.
    pub async fn purge_log(&self) -> u64 {
        let settings = match self.ports.settings.get().await {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(%error, "backup: cannot read the settings");
                return 0;
            }
        };
        match self.ports.runs.forget_before(settings.keep_since(self.now())).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(%error, "backup: cannot forget old runs");
                0
            }
        }
    }
}
