//! Volumes backed up on a schedule or by hand, and restored (docs/
//! architecture.md, 4.9). A run is a job: the containers that use the
//! volume stopped if the plan says so, the archive written into the share
//! by the privileged agent, the containers started again whatever came of
//! it, the run recorded, the oldest archives beyond the plan's number
//! removed (only those recorded, each by its identity), a failure told as
//! a notification. A restore checks the archive against its record and
//! replaces the volume's contents, the containers that use it stopped
//! meanwhile only when asked to.

use std::sync::{Arc, Mutex, PoisonError};

use zendiloos_jobs::{Job, Progress};
use zendiloos_kernel::{Actor, AuditEntry, Outcome, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification};

use super::ports::{NewBackup, SharePlace, Steps};
use super::{Containers, ContainersError, job_error};
use crate::domain::{
    Backup, BackupPlan, BackupSchedule, BackupTarget, WrittenBackup, backup_stem, beyond, is_volume_name,
    stale_failures, users,
};

/// A plan and when it runs next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    pub plan: BackupPlan,
    pub next_at: Timestamp,
}

/// A plan as an administrator sets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanInput {
    pub target: BackupTarget,
    pub schedule: BackupSchedule,
    pub keep: u32,
    pub stop_containers: bool,
}

/// Which volume a backup is restored into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTarget {
    /// The volume it was taken of; made again if it is gone, as the
    /// manifest says.
    OwnName,
    /// Another volume that exists.
    Existing(String),
}

/// The newest step not yet published, and whether a task publishes.
#[derive(Default)]
struct Pending {
    step: Option<(String, Option<u8>)>,
    publishing: bool,
}

/// A job's step and progress from a step reported without waiting: the
/// newest is published, one at a time.
struct JobSteps {
    progress: Progress,
    pending: Arc<Mutex<Pending>>,
}

impl JobSteps {
    fn new(progress: Progress) -> Self {
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
        let (progress, shared) = (self.progress.clone(), self.pending.clone());
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

impl Containers {
    pub(super) fn plan_view(&self, plan: BackupPlan) -> PlanView {
        let now = self.now();
        let next_at = plan.schedule.next(now, self.ports.clock.utc_offset(now));
        PlanView { plan, next_at }
    }

    /// Marks `volume` as being worked on; refused if it is already.
    pub(super) fn claim(&self, volume: &str) -> Result<(), ContainersError> {
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        if !busy.insert(volume.to_owned()) {
            return Err(ContainersError::VolumeBusy(volume.to_owned()));
        }
        Ok(())
    }

    pub(super) fn release(&self, volume: &str) {
        self.busy.lock().unwrap_or_else(PoisonError::into_inner).remove(volume);
    }

    pub async fn backup_plan(&self, volume: &str) -> Result<Option<PlanView>, ContainersError> {
        let plans = self.ports.backups.plans().await?;
        Ok(plans.into_iter().find(|p| p.volume == volume).map(|plan| self.plan_view(plan)))
    }

    /// Makes or replaces the plan of `volume`: a local volume whose data
    /// lies in Docker's folder, into a share `actor` may write.
    pub async fn set_backup_plan(
        &self,
        volume: &str,
        input: PlanInput,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<PlanView, ContainersError> {
        let found = self.find_volume(volume, context).await?;
        if !found.backed_up_here() {
            return Err(ContainersError::Refused(format!(
                "{volume} is no local volume with its data in Docker's folder: it is not backed up here"
            )));
        }
        let places = self.ports.storage.places(actor).await?;
        if !places.iter().any(|place| place.share_id == input.target.share_id) {
            return Err(ContainersError::Denied);
        }
        let plan = BackupPlan {
            volume: found.name,
            target: BackupTarget {
                share_id: input.target.share_id,
                folder: input.target.folder.trim_matches('/').to_owned(),
            },
            schedule: input.schedule,
            keep: input.keep,
            stop_containers: input.stop_containers,
            since: self.now(),
        }
        .checked()?;
        let done = self.ports.backups.save_plan(&plan).await.map_err(Into::into);
        self.audited("containers.backup_plan", volume, actor, context, done).await?;
        Ok(self.plan_view(plan))
    }

    pub async fn delete_backup_plan(
        &self,
        volume: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let done = match self.ports.backups.delete_plan(volume).await {
            Ok(true) => Ok(()),
            Ok(false) => return Err(ContainersError::NotFound),
            Err(error) => Err(error.into()),
        };
        self.audited("containers.backup_plan_delete", volume, actor, context, done).await
    }

    /// The runs of `volume`'s backups, the newest first.
    pub async fn backups(&self, volume: &str) -> Result<Vec<Backup>, ContainersError> {
        Ok(self.ports.backups.backups(Some(volume)).await?)
    }

    /// Backs up `volume` as its plan says, now, in a job.
    pub async fn back_up_now(
        &self,
        volume: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let plan = self.backup_plan(volume).await?.ok_or(ContainersError::NotFound)?.plan;
        self.start_backup(plan, Some(actor), context).await
    }

    /// Starts the backups whose time has come; run every minute. A plan
    /// counts as run once it was started, whatever comes of it: the next
    /// comes at its time.
    pub async fn run_due_backups(&self) -> Vec<Job> {
        let now = self.now();
        let offset = self.ports.clock.utc_offset(now);
        let plans = match self.ports.backups.plans().await {
            Ok(plans) => plans,
            Err(error) => {
                tracing::warn!(%error, "backups: cannot read the plans");
                return Vec::new();
            }
        };
        let mut jobs = Vec::new();
        for plan in plans.into_iter().filter(|plan| plan.is_due(now, offset)) {
            if let Err(error) = self.ports.backups.ran(&plan.volume, now).await {
                tracing::warn!(%error, volume = plan.volume, "backups: cannot note the run; not started");
                continue;
            }
            let volume = plan.volume.clone();
            match self.start_backup(plan, None, &RequestContext::default()).await {
                Ok(job) => jobs.push(job),
                Err(error) => {
                    tracing::warn!(%error, volume, "backups: not started on schedule");
                    self.told_failure(&volume, &error.to_string()).await;
                }
            }
        }
        jobs
    }

    async fn told_failure(&self, volume: &str, reason: &str) {
        let told = NewNotification::new(Level::Error, "docker_backup_failed")
            .with("volume", volume)
            .with("reason", reason);
        self.ports.notifier.notify(told).await;
    }

    async fn start_backup(
        &self,
        plan: BackupPlan,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        self.claim(&plan.volume)?;
        let this = self.clone();
        let (who, context) = (actor.cloned(), context.clone());
        let volume = plan.volume.clone();
        let started = self
            .ports
            .jobs
            .start("containers.volume_backup", Some(volume.clone()), actor, move |progress| async move {
                let done = this.run_backup(&plan, who.as_ref(), &context, &JobSteps::new(progress)).await;
                this.release(&plan.volume);
                done.map_err(|error| job_error(&error))
            })
            .await;
        if started.is_err() {
            self.release(&volume);
        }
        Ok(started?)
    }

    /// One run, recorded, audited and told if it failed.
    async fn run_backup(
        &self,
        plan: &BackupPlan,
        actor: Option<&Actor>,
        context: &RequestContext,
        steps: &dyn Steps,
    ) -> Result<(), ContainersError> {
        let started_at = self.now();
        let new = NewBackup {
            volume: plan.volume.clone(),
            share_id: plan.target.share_id,
            started_at,
            by: actor.map(|a| a.name.clone()),
        };
        let id = match self.ports.backups.start(&new).await {
            Ok(id) => id,
            Err(error) => {
                let error = ContainersError::from(error);
                self.told_failure(&plan.volume, &error.to_string()).await;
                return Err(error);
            }
        };
        let (written, restarted) = self.backup_steps(plan, id, started_at, context, steps).await;
        let outcome: Result<&WrittenBackup, String> = written.as_ref().map_err(ToString::to_string);
        let finished = outcome.as_ref().map(|written| *written).map_err(String::as_str);
        if let Err(error) = self.ports.backups.finish(id, self.now(), finished).await {
            tracing::error!(%error, volume = plan.volume, "backups: cannot record how the run ended");
        }
        let done = written.map(|_| ()).and(restarted);
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        let mut entry = AuditEntry::new(self.now(), "containers.volume_backup", outcome, context)
            .target(plan.volume.clone());
        if let Some(actor) = actor {
            entry = entry.by(actor);
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, "containers: cannot write the audit log");
        }
        if let Err(error) = &done {
            self.told_failure(&plan.volume, &error.to_string()).await;
        } else {
            self.retain(plan, context).await;
        }
        done
    }

    /// Stops what the plan says, backs up, starts again: what was written,
    /// and whether everything stopped runs again.
    async fn backup_steps(
        &self,
        plan: &BackupPlan,
        id: i64,
        started_at: Timestamp,
        context: &RequestContext,
        steps: &dyn Steps,
    ) -> (Result<WrittenBackup, ContainersError>, Result<(), ContainersError>) {
        let ready = async {
            let volume = self.find_volume(&plan.volume, context).await.map_err(|error| match error {
                ContainersError::NotFound => {
                    ContainersError::Failed(format!("the volume {} is gone", plan.volume))
                }
                other => other,
            })?;
            if !volume.backed_up_here() {
                return Err(ContainersError::Refused(format!("{} is not backed up here", plan.volume)));
            }
            let place =
                self.ports.backup_shares.locate(plan.target.share_id).await?.ok_or_else(|| {
                    ContainersError::Failed("the share the backups go to is gone".to_owned())
                })?;
            let running: Vec<String> = if plan.stop_containers {
                let containers = self.ports.engine.containers(context).await?;
                users(&plan.volume, &containers).into_iter().filter(|u| u.running).map(|u| u.name).collect()
            } else {
                Vec::new()
            };
            Ok((place, running))
        }
        .await;
        let (place, running) = match ready {
            Ok(ready) => ready,
            Err(error) => return (Err(error), Ok(())),
        };
        let stopped = match self.stop_containers(Some(id), &running, steps, context).await {
            Ok(stopped) => stopped,
            Err((error, stopped)) => {
                return (Err(error), self.start_again(id, &stopped, steps, context).await);
            }
        };
        steps.step("backing_up", Some(0));
        let offset = self.ports.clock.utc_offset(started_at);
        let stem = backup_stem(&plan.volume, started_at, offset);
        let written = self
            .ports
            .archiver
            .back_up(&plan.volume, &place, &plan.target.folder, &stem, steps, context)
            .await
            .map_err(Into::into);
        (written, self.start_again(id, &stopped, steps, context).await)
    }

    /// Stops the containers `running`, noting them with the run of a
    /// backup first, so that they are started again even after a restart;
    /// those stopped.
    async fn stop_containers(
        &self,
        run: Option<i64>,
        running: &[String],
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Vec<String>, (ContainersError, Vec<String>)> {
        if running.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(id) = run
            && let Err(error) = self.ports.backups.stopped(id, running).await
        {
            return Err((error.into(), Vec::new()));
        }
        steps.step("stopping", None);
        let mut stopped = Vec::new();
        for name in running {
            let container = self.find_container(name, context).await.map_err(|e| (e, stopped.clone()))?;
            match self
                .ports
                .engine
                .control_container(&container.id, crate::domain::ContainerAction::Stop, context)
                .await
            {
                Ok(()) => stopped.push(name.clone()),
                Err(error) => return Err((error.into(), stopped)),
            }
        }
        Ok(stopped)
    }

    /// Starts `stopped` again, each tried even if one fails.
    async fn start_again(
        &self,
        id: i64,
        stopped: &[String],
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        if stopped.is_empty() {
            return Ok(());
        }
        steps.step("starting", None);
        let failed = self.start_containers(stopped, context).await;
        if let Err(error) = self.ports.backups.stopped(id, &[]).await {
            tracing::warn!(%error, "backups: cannot note that the containers run again");
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(ContainersError::Failed(format!("could not start again: {}", failed.join(", "))))
        }
    }

    /// Starts each container named; those that would not start.
    async fn start_containers(&self, names: &[String], context: &RequestContext) -> Vec<String> {
        let mut failed = Vec::new();
        for name in names {
            let started = match self.find_container(name, context).await {
                Ok(container) => self
                    .ports
                    .engine
                    .control_container(&container.id, crate::domain::ContainerAction::Start, context)
                    .await
                    .map_err(ContainersError::from),
                Err(error) => Err(error),
            };
            if let Err(error) = started {
                tracing::warn!(%error, container = name, "backups: cannot start the container again");
                failed.push(name.clone());
            }
        }
        failed
    }

    /// Removes the archives beyond the plan's number: only those recorded,
    /// each by the identity it had when written; and forgets old failures.
    async fn retain(&self, plan: &BackupPlan, context: &RequestContext) {
        let backups = match self.ports.backups.backups(Some(&plan.volume)).await {
            Ok(backups) => backups,
            Err(error) => return tracing::warn!(%error, "backups: cannot read the runs to keep"),
        };
        for old in beyond(&backups, plan.keep) {
            let Some(written) = old.written() else { continue };
            let removed = match self.ports.backup_shares.locate(old.share_id).await {
                // The share went, and the files with it.
                Ok(None) => Ok(()),
                Ok(Some(place)) => self.remove_files(&place, written, context).await,
                Err(error) => Err(error.into()),
            };
            match removed {
                Ok(()) => {
                    if let Err(error) = self.ports.backups.forget(old.id).await {
                        tracing::warn!(%error, "backups: cannot forget a removed backup");
                    }
                    let entry = AuditEntry::new(
                        self.now(),
                        "containers.volume_backup_remove",
                        Outcome::Success,
                        context,
                    )
                    .target(written.archive.path.clone());
                    if let Err(error) = self.ports.audit.record(entry).await {
                        tracing::error!(%error, "containers: cannot write the audit log");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, path = written.archive.path, "backups: an old archive stays")
                }
            }
        }
        for stale in stale_failures(&backups) {
            if let Err(error) = self.ports.backups.forget(stale.id).await {
                tracing::warn!(%error, "backups: cannot forget an old failure");
            }
        }
    }

    async fn remove_files(
        &self,
        place: &SharePlace,
        written: &WrittenBackup,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        for file in [&written.archive, &written.manifest] {
            // Gone, or another file now: either way not ours to remove.
            if !self.ports.archiver.remove(place, file, context).await? {
                tracing::info!(path = file.path, "backups: an old file was gone or replaced; left alone");
            }
        }
        Ok(())
    }

    /// Restores backup `id` into `target`, in a job. Running containers
    /// that use the volume are stopped meanwhile and started again after
    /// only if `stop_containers`; otherwise it is refused.
    pub async fn restore_backup(
        &self,
        id: i64,
        target: RestoreTarget,
        stop_containers: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let backup = self.ports.backups.backup(id).await?.ok_or(ContainersError::NotFound)?;
        let Some(written) = backup.written().cloned() else {
            return Err(ContainersError::Refused("only a backup that succeeded can be restored".into()));
        };
        let name = match &target {
            RestoreTarget::OwnName => backup.volume.clone(),
            RestoreTarget::Existing(name) => name.clone(),
        };
        let existing = self.ports.engine.volumes(context).await?.into_iter().find(|v| v.name == name);
        match &existing {
            None if matches!(target, RestoreTarget::Existing(_)) => return Err(ContainersError::NotFound),
            None if backup.driver != "local" || !is_volume_name(&name) => {
                return Err(ContainersError::Refused(format!("{name} cannot be made again here")));
            }
            Some(volume) if !volume.backed_up_here() => {
                return Err(ContainersError::Refused(format!(
                    "{name} is no local volume with its data in Docker's folder"
                )));
            }
            _ => {}
        }
        let containers = self.ports.engine.containers(context).await?;
        let running: Vec<String> =
            users(&name, &containers).into_iter().filter(|u| u.running).map(|u| u.name).collect();
        if !running.is_empty() && !stop_containers {
            return Err(ContainersError::VolumeInUse(running));
        }
        let place = self
            .ports
            .backup_shares
            .locate(backup.share_id)
            .await?
            .ok_or_else(|| ContainersError::Refused("the share the backup lies in is gone".into()))?;
        self.claim(&name)?;
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let audited = format!("{name} ← {}", written.archive.path);
        let volume = name.clone();
        let started = self
            .ports
            .jobs
            .start("containers.volume_restore", Some(name.clone()), Some(actor), move |progress| async move {
                let steps = JobSteps::new(progress);
                let done = async {
                    if existing.is_none() {
                        steps.step("creating", None);
                        this.ports.engine.create_volume(&name, &backup.labels, &context).await?;
                    }
                    let stopped = match this.stop_containers(None, &running, &steps, &context).await {
                        Ok(stopped) => stopped,
                        Err((error, stopped)) => {
                            this.start_containers(&stopped, &context).await;
                            return Err(error);
                        }
                    };
                    steps.step("restoring", Some(0));
                    let restored = this
                        .ports
                        .archiver
                        .restore(&name, &place, &written.archive.path, &written.sha256, &steps, &context)
                        .await
                        .map_err(ContainersError::from);
                    if !stopped.is_empty() {
                        steps.step("starting", None);
                    }
                    let failed = this.start_containers(&stopped, &context).await;
                    restored?;
                    if failed.is_empty() {
                        Ok(())
                    } else {
                        Err(ContainersError::Failed(format!(
                            "restored; could not start again: {}",
                            failed.join(", ")
                        )))
                    }
                }
                .await;
                this.release(&name);
                let done = this.audited("containers.volume_restore", &audited, &who, &context, done).await;
                done.map_err(|error| job_error(&error))
            })
            .await;
        if started.is_err() {
            self.release(&volume);
        }
        Ok(started?)
    }

    /// After a restart: runs that were under way failed, and the containers
    /// they had stopped are started again. Call once at startup; how many
    /// runs were cut off.
    pub async fn recover_backups(&self) -> usize {
        let interrupted = match self.ports.backups.interrupted(self.now()).await {
            Ok(interrupted) => interrupted,
            Err(error) => {
                tracing::warn!(%error, "backups: cannot look at the runs cut off");
                return 0;
            }
        };
        let context = RequestContext::default();
        for run in &interrupted {
            let failed = self.start_containers(&run.stopped, &context).await;
            if !failed.is_empty() {
                tracing::warn!(
                    volume = run.volume,
                    ?failed,
                    "backups: containers stopped for a run stay stopped"
                );
            }
            if let Err(error) = self.ports.backups.stopped(run.id, &[]).await {
                tracing::warn!(%error, "backups: cannot note that the containers run again");
            }
            self.told_failure(&run.volume, "zendiloosd stopped while it ran").await;
        }
        interrupted.len()
    }
}
