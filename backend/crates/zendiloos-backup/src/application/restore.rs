//! Restores: a task's versions (or its mirror) listed and looked into,
//! and all or some of one put back where it came from or into another
//! folder, what is there skipped or replaced as asked.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext, Timestamp};

use super::ports::{Location, RestorePlan};
use super::runs::JobSteps;
use super::{BackupError, Backups, audited, job_error};
use crate::domain::{
    CURRENT, Entry, Existing, Invalid, NasPlace, Place, TargetState, Task, is_folder_path, version_name,
};

/// The most paths one restore names.
pub const MAX_RESTORE_PATHS: usize = 100;

/// What a task's folder holds to restore from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Versions {
    /// A mirror's copy is there.
    pub current: bool,
    /// Complete versions, newest first.
    pub versions: Vec<Timestamp>,
}

/// What an administrator restores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRequest {
    /// A version; none for a mirror's copy.
    pub version: Option<Timestamp>,
    /// Folders and files in it; none for all.
    pub paths: Vec<String>,
    /// Another folder on the NAS; none for where the task's source is.
    pub to: Option<NasPlace>,
    pub existing: Existing,
}

impl Backups {
    /// The task's folder in its target, as the host reaches it now.
    async fn folder(&self, task: &Task, context: &RequestContext) -> Result<Location, BackupError> {
        let target = self.reach(&task.settings.target, context).await?.0;
        Ok(target.join(&task.folder))
    }

    pub async fn versions(&self, id: i64, context: &RequestContext) -> Result<Versions, BackupError> {
        let task = self.find(id).await?;
        let folder = self.folder(&task, context).await?;
        let secrets = self.secrets(&task).await?;
        let listing = self.ports.host.list(&folder, "", &secrets, context).await?;
        self.keep_token(&task, listing.token.as_ref()).await;
        let entries = listing.entries;
        let state = TargetState::of(&entries);
        let mut versions = state.versions;
        versions.reverse();
        Ok(Versions { current: state.current, versions })
    }

    /// The entries of a folder of a version (none: the mirror's copy).
    pub async fn browse(
        &self,
        id: i64,
        version: Option<Timestamp>,
        path: &str,
        context: &RequestContext,
    ) -> Result<Vec<Entry>, BackupError> {
        if !is_folder_path(path) {
            return Err(Invalid("that is no folder of the backup".into()).into());
        }
        let task = self.find(id).await?;
        let folder = self.folder(&task, context).await?;
        let secrets = self.secrets(&task).await?;
        let inside = [version.map_or_else(|| CURRENT.to_owned(), version_name), path.to_owned()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let mut entries: Vec<Entry> = self
            .ports
            .host
            .list(&folder, &inside, &secrets, context)
            .await?
            .entries
            .into_iter()
            .filter(|e| e.name != ".")
            .collect();
        entries.sort_by(|a, b| {
            (a.kind != crate::domain::EntryKind::Folder, &a.name)
                .cmp(&(b.kind != crate::domain::EntryKind::Folder, &b.name))
        });
        Ok(entries)
    }

    /// Restores from a task's backup in a job: not while it runs, one
    /// restore of a task at a time. A server's backup (pulled) goes into a
    /// folder of the NAS that is named.
    pub async fn restore(
        &self,
        id: i64,
        request: RestoreRequest,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, BackupError> {
        let task = self.find(id).await?;
        self.require_installed(context).await?;
        if request.paths.len() > MAX_RESTORE_PATHS {
            return Err(
                Invalid(format!("a restore names at most {MAX_RESTORE_PATHS} folders and files")).into()
            );
        }
        if request.paths.iter().any(|path| path.is_empty() || !is_folder_path(path)) {
            return Err(Invalid("a path to restore is no path in the backup".into()).into());
        }
        let to = match (&request.to, &task.settings.source) {
            (Some(to), _) => {
                if !is_folder_path(to.path()) {
                    return Err(Invalid("that is no folder to restore into".into()).into());
                }
                to.clone()
            }
            (None, Place::Nas(source)) => source.clone(),
            (None, Place::Remote(_) | Place::Cloud(_)) => {
                return Err(Invalid(
                    "a server's backup is restored into a folder of the NAS: name one".into(),
                )
                .into());
            }
        };
        let claim = self.claim(id, true)?;
        let (to, drive) = self.locate(&to, context).await?;
        if drive.is_some_and(|d| d.read_only) {
            return Err(BackupError::DriveReadOnly);
        }
        let from = self
            .folder(&task, context)
            .await?
            .join(&request.version.map_or_else(|| CURRENT.to_owned(), version_name));
        let plan = RestorePlan {
            task: id,
            from,
            paths: request.paths,
            to,
            existing: request.existing,
            encrypted: task.settings.encrypted,
        };
        let secrets = self.secrets(&task).await?;
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let name = task.settings.name.clone();
        let kind = task.kind();
        let job = self
            .ports
            .jobs
            .start("backup.restore", Some(name.clone()), Some(actor), move |progress| async move {
                let _claim = claim;
                let steps = JobSteps::new(progress);
                super::Steps::step(&steps, "restoring", Some(0));
                let done = this
                    .ports
                    .host
                    .restore(&plan, &secrets, &steps, &context)
                    .await
                    .map_err(BackupError::from);
                this.audit("backup.restore", audited(&done), &name, Some(&who), &context).await;
                match done {
                    Ok(report) => {
                        tracing::info!(
                            task = id,
                            kind = kind.as_str(),
                            files = report.files,
                            "backup: restored"
                        );
                        // Some files could not be written: said, not hidden.
                        match report.partial {
                            Some(reason) => Err(zendiloos_jobs::JobError::new("partial", Some(reason))),
                            None => Ok(()),
                        }
                    }
                    Err(error) => Err(job_error(&error)),
                }
            })
            .await?;
        Ok(job)
    }
}
