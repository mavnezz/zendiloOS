//! Use cases of the Recovery context and the ports they need.

mod ports;

use std::sync::Arc;

use zendiloos_jobs::{Job, JobError, Jobs};
use zendiloos_kernel::{
    Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, RunningWork, Secret, Timestamp,
};
use zendiloos_notify::{Level, NewNotification, Notifier};

pub use ports::{
    Archive, BackupFolders, Operators, PlanRepository, Restart, RestoreError, Restored, Restorer, Snapshots,
};

use crate::domain::{InvalidPassphrase, InvalidPlan, LastRun, Passphrase, Plan, Schedule, Target, to_remove};

/// An error with what caused it, for the administrator: a port's error
/// names only what failed, its source why.
fn reason(error: &RecoveryError) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// What the use cases need.
#[derive(Clone)]
pub struct RecoveryPorts {
    pub snapshots: Arc<dyn Snapshots>,
    pub restorer: Arc<dyn Restorer>,
    pub plans: Arc<dyn PlanRepository>,
    pub folders: Arc<dyn BackupFolders>,
    pub operators: Arc<dyn Operators>,
    pub restart: Arc<dyn Restart>,
    /// What runs in the background, which a restore would cut off.
    pub work: Arc<dyn RunningWork>,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Passphrase(#[from] InvalidPassphrase),
    #[error(transparent)]
    Plan(#[from] InvalidPlan),
    /// The passphrase does not open the backup.
    #[error("the passphrase does not open this backup")]
    WrongPassphrase,
    #[error("not a configuration backup of zendiloOS: {0}")]
    NotABackup(String),
    #[error("a backup of another layout ({0}) than this release reads")]
    UnsupportedFormat(u32),
    #[error(
        "the backup's database (schema {found}) is newer than this release knows (schema {supported}): \
         update zendiloOS first"
    )]
    TooNew { found: i64, supported: i64 },
    /// The host would not take the backup back: a name or ID taken.
    #[error("{0}")]
    Refused(String),
    /// Background jobs would be cut off by the restart a restore needs.
    #[error("{}", .0.join(", "))]
    JobsRunning(Vec<String>),
    #[error("no backup is planned: choose a folder and a passphrase first")]
    NotPlanned,
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<RestoreError> for RecoveryError {
    fn from(error: RestoreError) -> Self {
        match error {
            RestoreError::WrongPassphrase => Self::WrongPassphrase,
            RestoreError::NotABackup(why) => Self::NotABackup(why),
            RestoreError::UnsupportedFormat(format) => Self::UnsupportedFormat(format),
            RestoreError::TooNew { found, supported } => Self::TooNew { found, supported },
            RestoreError::Refused(why) => Self::Refused(why),
            RestoreError::Port(error) => Self::Port(error),
        }
    }
}

/// The plan as administrators see it: no passphrase, whether one is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    pub enabled: bool,
    pub schedule: Schedule,
    pub target: Option<Target>,
    pub keep: u8,
    pub passphrase_set: bool,
    /// Who set it up; the backups are written as that account.
    pub account: Option<String>,
    pub next_at: Option<Timestamp>,
    pub last: Option<LastRun>,
}

/// A change of the plan; `passphrase` none keeps the one kept.
#[derive(Debug, Clone)]
pub struct PlanChange {
    pub enabled: bool,
    pub schedule: Schedule,
    pub target: Option<Target>,
    pub keep: u8,
    pub passphrase: Option<String>,
}

/// Backing the configuration up and restoring it.
#[derive(Clone)]
pub struct Recovery {
    ports: RecoveryPorts,
}

impl Recovery {
    pub fn new(ports: RecoveryPorts) -> Self {
        Self { ports }
    }

    async fn audit(&self, entry: AuditEntry) {
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
    }

    fn entry(&self, action: &'static str, outcome: Outcome, request: &RequestContext) -> AuditEntry {
        AuditEntry::new(self.ports.clock.now(), action, outcome, request)
    }

    /// A backup of the configuration as it is now, for the browser to
    /// download.
    pub async fn download(
        &self,
        passphrase: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Archive, RecoveryError> {
        let passphrase = Passphrase::new(passphrase)?;
        let made = self.ports.snapshots.create(&passphrase, request).await;
        let outcome = if made.is_ok() { Outcome::Success } else { Outcome::Failure };
        let mut entry = self.entry("configuration.download", outcome, request).by(actor);
        if let Ok(archive) = &made {
            entry = entry.target(archive.name.clone());
        }
        self.audit(entry).await;
        Ok(made?)
    }

    pub async fn plan(&self) -> Result<PlanView, RecoveryError> {
        let plan = self.ports.plans.get().await?;
        let account = match plan.account_id {
            Some(id) => self.ports.operators.name(id).await?,
            None => None,
        };
        let offsets = |at| self.ports.clock.utc_offset(at);
        let next_at = plan.enabled.then(|| plan.schedule.next(self.ports.clock.now(), &offsets));
        Ok(PlanView {
            enabled: plan.enabled,
            schedule: plan.schedule,
            target: plan.target,
            keep: plan.keep,
            passphrase_set: plan.passphrase.is_some(),
            account,
            next_at,
            last: plan.last,
        })
    }

    /// Sets the plan; the backups are written as `actor` from now on.
    pub async fn set_plan(
        &self,
        change: PlanChange,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<PlanView, RecoveryError> {
        let before = self.ports.plans.get().await?;
        let passphrase = match change.passphrase.as_deref().filter(|p| !p.is_empty()) {
            Some(given) => Some(Secret::new(Passphrase::new(given)?.expose())),
            None => before.passphrase,
        };
        let plan = Plan {
            enabled: change.enabled,
            schedule: change.schedule,
            target: change.target,
            keep: change.keep,
            account_id: Some(actor.account_id),
            passphrase,
            since: self.ports.clock.now(),
            last: before.last,
        };
        plan.check()?;
        let saved = self.ports.plans.save(&plan).await;
        let outcome = if saved.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = if plan.enabled { "on" } else { "off" };
        self.audit(self.entry("configuration.backup_settings", outcome, request).by(actor).target(target))
            .await;
        saved?;
        self.plan().await
    }

    /// Backs up into the plan's folder now, in a job.
    pub async fn run_now(&self, actor: &Actor, request: &RequestContext) -> Result<Job, RecoveryError> {
        let plan = self.ports.plans.get().await?;
        if plan.target.is_none() || plan.passphrase.is_none() || plan.account_id.is_none() {
            return Err(RecoveryError::NotPlanned);
        }
        self.audit(self.entry("configuration.backup", Outcome::Success, request).by(actor)).await;
        Ok(self.start(plan, Some(actor)).await?)
    }

    /// A backup whose time came (a missed one, the NAS was off, at once);
    /// zendiloosd looks every minute.
    pub async fn run_due(&self) -> Option<Job> {
        let plan = match self.ports.plans.get().await {
            Ok(plan) => plan,
            Err(error) => {
                tracing::warn!(%error, "configuration backup: cannot read the plan");
                return None;
            }
        };
        let offsets = |at| self.ports.clock.utc_offset(at);
        if !plan.enabled || !plan.schedule.is_due(plan.since, self.ports.clock.now(), &offsets) {
            return None;
        }
        match self.start(plan, None).await {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::warn!(%error, "configuration backup: cannot start");
                None
            }
        }
    }

    async fn start(&self, plan: Plan, actor: Option<&Actor>) -> Result<Job, PortError> {
        // Started: not due again before its next time, also if it fails.
        let started = Plan { since: self.ports.clock.now(), ..plan.clone() };
        self.ports.plans.save(&started).await?;
        let this = self.clone();
        let target = plan.target.as_ref().map(|target| target.folder().to_owned());
        self.ports
            .jobs
            .start("configuration.backup", target, actor, move |progress| async move {
                progress.step("backing_up", Some(0.1)).await;
                let done = this.back_up(&plan).await;
                let last = LastRun {
                    at: this.ports.clock.now(),
                    succeeded: done.is_ok(),
                    file: done.as_ref().ok().cloned(),
                    error: done.as_ref().err().map(reason),
                };
                if let Err(error) = this.ports.plans.record(&last).await {
                    tracing::warn!(%error, "configuration backup: cannot note the run");
                }
                match done {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        let reason = reason(&error);
                        this.ports
                            .notifier
                            .notify(
                                NewNotification::new(Level::Error, "configuration_backup_failed")
                                    .with("reason", &reason),
                            )
                            .await;
                        Err(JobError::new("backup_failed", Some(reason)))
                    }
                }
            })
            .await
    }

    /// Writes a backup into the plan's folder, then removes the oldest
    /// beyond those kept. The file written.
    async fn back_up(&self, plan: &Plan) -> Result<String, RecoveryError> {
        let (Some(target), Some(secret), Some(account)) = (&plan.target, &plan.passphrase, plan.account_id)
        else {
            return Err(RecoveryError::NotPlanned);
        };
        let Some(user) = self.ports.operators.administrator(account).await? else {
            return Err(RecoveryError::Port(PortError::new(
                "configuration backup",
                "the administrator who set it up is gone or no longer one: set it up again",
            )));
        };
        let context = RequestContext::default();
        let passphrase = Passphrase::new(secret.expose())?;
        let archive = self.ports.snapshots.create(&passphrase, &context).await?;
        self.ports.folders.write(target, &user, &archive.name, archive.bytes, &context).await?;
        let names = self.ports.folders.names(target, &user, &context).await?;
        for old in to_remove(&names, usize::from(plan.keep)) {
            if let Err(error) = self.ports.folders.remove(target, &user, &old, &context).await {
                tracing::warn!(%error, file = old, "configuration backup: cannot remove an old one");
            }
        }
        tracing::info!(file = archive.name, "configuration backup: written");
        Ok(archive.name)
    }

    /// Restores a backup on this installation: checked whole first (the
    /// passphrase, the layout, a database not newer than this release
    /// knows), then the host's part put back and the database staged for
    /// zendiloosd's next start, which follows at once. `actor` is none
    /// during the first setup, which the caller has allowed.
    pub async fn restore(
        &self,
        archive: Vec<u8>,
        passphrase: &str,
        actor: Option<&Actor>,
        request: &RequestContext,
    ) -> Result<Restored, RecoveryError> {
        let passphrase = Passphrase::new(passphrase)?;
        let running = self.ports.work.running().await;
        if !running.is_empty() {
            return Err(RecoveryError::JobsRunning(running));
        }
        let entry = |outcome| {
            let entry = self.entry("configuration.restore", outcome, request);
            match actor {
                Some(actor) => entry.by(actor),
                None => entry,
            }
        };
        // Into the restored database too: it replaces the one written here.
        let restored =
            self.ports.restorer.restore(archive, &passphrase, entry(Outcome::Success), request).await;
        let mut written = entry(if restored.is_ok() { Outcome::Success } else { Outcome::Failure });
        if let Ok(restored) = &restored {
            written = written.target(restored.manifest.host_name.clone());
        }
        self.audit(written).await;
        let restored = restored?;
        tracing::warn!(
            host = restored.manifest.host_name,
            version = restored.manifest.version,
            "configuration restored: zendiloosd restarts with it"
        );
        self.ports.restart.restart();
        Ok(restored)
    }
}

#[cfg(test)]
mod tests;
