//! The file manager: share contents as the signed-in user.
//!
//! Every operation runs on the host with the user's own credentials, so
//! what is allowed is decided where it is decided for SMB and NFS: by the
//! file system's permissions. The checks here keep users out of shares that
//! are not theirs and refuse early what the share's rules forbid anyway.

use std::sync::Arc;

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, PortError, RequestContext};

use super::SharingPorts;
use super::ports::FileHostError;
use crate::domain::{
    Access, FileLocation, FilePath, InvalidFilePath, Listing, Share, ShareId, Subject, is_new_file_name,
};

/// Most entries one copy, move or delete takes.
pub const MAX_TRANSFER_ENTRIES: usize = 1000;
/// Prefix of the hidden files uploads are written to.
const UPLOAD_PREFIX: &str = ".zendiloos-upload-";

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// The share does not exist or is not shared with the user.
    #[error("there is no such share")]
    ShareNotFound,
    #[error("no such file or folder")]
    NotFound,
    #[error("permission denied")]
    Denied,
    #[error("the name is taken")]
    Exists,
    #[error("not a valid name: no control characters, no spaces at either end, no dot at the end")]
    InvalidName,
    #[error(transparent)]
    InvalidPath(#[from] InvalidFilePath),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<FileHostError> for FilesError {
    fn from(error: FileHostError) -> Self {
        match error {
            FileHostError::NotFound => FilesError::NotFound,
            FileHostError::Denied => FilesError::Denied,
            FileHostError::Exists => FilesError::Exists,
            FileHostError::Invalid(reason) => FilesError::Invalid(reason),
            FileHostError::Port(error) => FilesError::Port(error),
        }
    }
}

fn job_error(error: FileHostError) -> JobError {
    let code = match &error {
        FileHostError::NotFound => "not_found",
        FileHostError::Denied => "denied",
        FileHostError::Exists => "name_taken",
        FileHostError::Invalid(_) => "invalid_operation",
        FileHostError::Port(_) => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

/// A share the user may use, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsableShare {
    pub share: Share,
    pub access: Access,
}

/// The broadest access the rules give `account`, directly or by a group.
fn access_of(share: &Share, account: i64, groups: &[i64]) -> Option<Access> {
    share
        .access
        .iter()
        .filter(|rule| match rule.subject {
            Subject::Account(id) => id == account,
            Subject::Group(id) => groups.contains(&id),
        })
        .map(|rule| rule.access)
        .max_by_key(|access| *access == Access::ReadWrite)
}

fn at(share: &Share, path: FilePath) -> FileLocation {
    FileLocation { volume: share.volume, share: share.name.clone(), path }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

/// An upload in progress: written to a hidden file in the target folder,
/// renamed when complete.
#[derive(Debug)]
pub struct Upload {
    pub file: tokio::fs::File,
    pub ticket: UploadTicket,
}

#[derive(Debug)]
pub struct UploadTicket {
    temporary: FileLocation,
    target: FileLocation,
    replace: bool,
}

/// All use cases of the file manager. Cheap to clone.
#[derive(Clone)]
pub struct FilesUseCases {
    ports: SharingPorts,
}

impl FilesUseCases {
    pub fn new(ports: SharingPorts) -> Self {
        Self { ports }
    }

    /// Shares whose rules name the user or one of their groups, by name.
    pub async fn shares(&self, actor: &Actor) -> Result<Vec<UsableShare>, FilesError> {
        let groups = self.ports.directory.memberships(actor.account_id).await?;
        let shares = self.ports.shares.list().await?;
        Ok(shares
            .into_iter()
            .filter_map(|share| {
                let access = access_of(&share, actor.account_id, &groups)?;
                Some(UsableShare { share, access })
            })
            .collect())
    }

    async fn share(&self, actor: &Actor, id: ShareId) -> Result<UsableShare, FilesError> {
        let share = self.ports.shares.find(id).await?.ok_or(FilesError::ShareNotFound)?;
        let groups = self.ports.directory.memberships(actor.account_id).await?;
        let access = access_of(&share, actor.account_id, &groups).ok_or(FilesError::ShareNotFound)?;
        Ok(UsableShare { share, access })
    }

    /// The share, if the user may change things in it.
    async fn writable_share(&self, actor: &Actor, id: ShareId) -> Result<Share, FilesError> {
        let usable = self.share(actor, id).await?;
        if usable.access != Access::ReadWrite {
            return Err(FilesError::Denied);
        }
        Ok(usable.share)
    }

    pub async fn list(
        &self,
        actor: &Actor,
        id: ShareId,
        path: FilePath,
        context: &RequestContext,
    ) -> Result<Listing, FilesError> {
        let share = self.share(actor, id).await?.share;
        Ok(self.ports.files.list(&actor.name, &at(&share, path), context).await?)
    }

    /// A file for download, and its size.
    pub async fn open(
        &self,
        actor: &Actor,
        id: ShareId,
        path: FilePath,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FilesError> {
        let share = self.share(actor, id).await?.share;
        Ok(self.ports.files.open(&actor.name, &at(&share, path), context).await?)
    }

    /// The folder `path` as a tar archive being written, its exact length,
    /// and its name: the folder's, or the share's for its top.
    pub async fn archive(
        &self,
        actor: &Actor,
        id: ShareId,
        path: FilePath,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64, String), FilesError> {
        let share = self.share(actor, id).await?.share;
        let name = path.name().unwrap_or(share.name.as_str()).to_owned();
        let (file, size) = self.ports.files.archive(&actor.name, &at(&share, path), context).await?;
        Ok((file, size, name))
    }

    /// Starts an upload of `name` into `directory`. The caller writes the
    /// file, then calls [`FilesUseCases::finish_upload`], or
    /// [`FilesUseCases::abort_upload`] if anything failed.
    pub async fn begin_upload(
        &self,
        actor: &Actor,
        id: ShareId,
        directory: FilePath,
        name: &str,
        replace: bool,
        context: &RequestContext,
    ) -> Result<Upload, FilesError> {
        if !is_new_file_name(name) || name.starts_with(UPLOAD_PREFIX) {
            return Err(FilesError::InvalidName);
        }
        let share = self.writable_share(actor, id).await?;
        let target = at(&share, directory.join(name)?);
        let folder = at(&share, directory.clone());
        let (file, temporary) = self.ports.files.create_upload(&actor.name, &folder, context).await?;
        let temporary = at(&share, directory.join(&temporary)?);
        Ok(Upload { file, ticket: UploadTicket { temporary, target, replace } })
    }

    /// Gives the uploaded file its name. On failure, the upload is removed.
    pub async fn finish_upload(
        &self,
        actor: &Actor,
        ticket: UploadTicket,
        context: &RequestContext,
    ) -> Result<FilePath, FilesError> {
        let files = &self.ports.files;
        match files.move_entry(&actor.name, &ticket.temporary, &ticket.target, ticket.replace, context).await
        {
            Ok(()) => {
                tracing::info!(user = actor.name, share = ticket.target.share.as_str(), "files: uploaded");
                Ok(ticket.target.path)
            }
            Err(error) => {
                self.abort_upload(actor, ticket, context).await;
                Err(error.into())
            }
        }
    }

    pub async fn abort_upload(&self, actor: &Actor, ticket: UploadTicket, context: &RequestContext) {
        if let Err(error) = self.ports.files.remove(&actor.name, &ticket.temporary, false, context).await {
            tracing::warn!(%error, share = ticket.temporary.share.as_str(), "cannot remove an aborted upload");
        }
    }

    pub async fn make_directory(
        &self,
        actor: &Actor,
        id: ShareId,
        path: FilePath,
        context: &RequestContext,
    ) -> Result<(), FilesError> {
        if !path.name().is_some_and(is_new_file_name) {
            return Err(FilesError::InvalidName);
        }
        let share = self.writable_share(actor, id).await?;
        Ok(self.ports.files.make_directory(&actor.name, &at(&share, path), context).await?)
    }

    /// Gives `path` the name `name` in the same folder.
    pub async fn rename(
        &self,
        actor: &Actor,
        id: ShareId,
        path: FilePath,
        name: &str,
        context: &RequestContext,
    ) -> Result<FilePath, FilesError> {
        let parent = path.parent().ok_or(FilesError::Invalid("the share itself cannot be renamed".into()))?;
        if !is_new_file_name(name) {
            return Err(FilesError::InvalidName);
        }
        let target = parent.join(name).map_err(|_| FilesError::InvalidName)?;
        let share = self.writable_share(actor, id).await?;
        let (from, to) = (at(&share, path), at(&share, target.clone()));
        self.ports.files.move_entry(&actor.name, &from, &to, false, context).await?;
        Ok(target)
    }

    /// Copies or moves `paths` of share `id` into the folder `directory` of
    /// share `target`, one after the other, as a job.
    #[allow(clippy::too_many_arguments)]
    pub async fn transfer(
        &self,
        actor: &Actor,
        mode: Transfer,
        id: ShareId,
        paths: Vec<FilePath>,
        target: ShareId,
        directory: FilePath,
        context: &RequestContext,
    ) -> Result<Job, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES || paths.iter().any(FilePath::is_root) {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let source = match mode {
            Transfer::Copy => self.share(actor, id).await?.share,
            Transfer::Move => self.writable_share(actor, id).await?,
        };
        let destination = self.writable_share(actor, target).await?;
        let mut moves = Vec::with_capacity(paths.len());
        for path in paths {
            if source.id == destination.id && directory.starts_with(&path) {
                return Err(FilesError::Invalid("a folder cannot go into itself".into()));
            }
            let name = path.name().unwrap_or_default().to_owned();
            moves.push((at(&source, path), at(&destination, directory.join(&name)?)));
        }

        let (kind, step) = match mode {
            Transfer::Copy => ("files.copy", "copying"),
            Transfer::Move => ("files.move", "moving"),
        };
        let files = Arc::clone(&self.ports.files);
        let (user, context) = (actor.name.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start(kind, Some(destination.name.to_string()), Some(actor), move |progress| async move {
                let total = moves.len();
                for (done, (from, to)) in moves.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    progress.step(step, Some(done as f32 / total as f32)).await;
                    match mode {
                        Transfer::Copy => files.copy_entry(&user, from, to, &context).await.map(drop),
                        Transfer::Move => files.move_entry(&user, from, to, false, &context).await,
                    }
                    .map_err(job_error)?;
                }
                tracing::info!(user, entries = total, kind = step, "files: transferred");
                Ok(())
            })
            .await?;
        Ok(job)
    }

    /// Moves `paths` out of the user's recycle bin in share `id` back to
    /// where they were deleted from, or beside it if the name is taken.
    /// Returns where each went; stops at the first that fails.
    pub async fn restore(
        &self,
        actor: &Actor,
        id: ShareId,
        paths: Vec<FilePath>,
        context: &RequestContext,
    ) -> Result<Vec<FilePath>, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let share = self.writable_share(actor, id).await?;
        let mut restored = Vec::with_capacity(paths.len());
        for path in paths {
            restored.push(self.ports.files.restore(&actor.name, &at(&share, path), context).await?);
        }
        tracing::info!(user = actor.name, entries = restored.len(), "files: restored");
        Ok(restored)
    }

    /// Moves `paths` into the user's recycle bin if the share has one,
    /// otherwise deletes them for good; as a job.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: ShareId,
        paths: Vec<FilePath>,
        context: &RequestContext,
    ) -> Result<Job, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES || paths.iter().any(FilePath::is_root) {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let share = self.writable_share(actor, id).await?;
        let recycle = share.recycle_bin;
        let locations: Vec<FileLocation> = paths.into_iter().map(|path| at(&share, path)).collect();
        let files = Arc::clone(&self.ports.files);
        let (user, context) = (actor.name.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("files.delete", Some(share.name.to_string()), Some(actor), move |progress| async move {
                let total = locations.len();
                for (done, location) in locations.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    progress.step("deleting", Some(done as f32 / total as f32)).await;
                    files.remove(&user, location, recycle, &context).await.map_err(job_error)?;
                }
                tracing::info!(user, entries = total, recycle, "files: deleted");
                Ok(())
            })
            .await?;
        Ok(job)
    }
}
