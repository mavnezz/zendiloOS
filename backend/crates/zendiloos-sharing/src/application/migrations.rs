//! The walk to share-level access (docs/architecture.md, 4.4), once per
//! share: shares made before it came had their rules written into every
//! file as entries per person and group. Each such share's folder gets
//! what every share's has, and every file the entries of the group
//! `zendiloos-files` in place of those per person; apps' entries and the
//! owners stay. Until a share's walk is done, its rules hold all the same:
//! Samba and the file manager enforce them, whatever the files say, so
//! they may change meanwhile; only NFS clients and apps, which reach the
//! files through that group, wait for it.
//!
//! The walks run one at a time on the whole NAS, a share at a time, each as
//! a job, so a long one never keeps the machine busy on several at once. A
//! share whose walk was cut off (zendiloosd stopped, an entry could not be
//! changed) stays owed and is walked again: when zendiloosd starts and
//! every few minutes. A share rolled back to a snapshot from before its
//! walk owes it again.

use std::collections::HashSet;
use std::sync::PoisonError;

use zendiloos_jobs::{Job, JobError, Progress};
use zendiloos_kernel::{Actor, RequestContext};

use super::SharingUseCases;
use super::shares::SharingError;
use crate::domain::{Share, ShareId};

/// The walks: one at a time, and which shares have one queued or under way.
#[derive(Default)]
pub(super) struct Migrations {
    /// The one walk going through a share now.
    walking: tokio::sync::Mutex<()>,
    /// One run through the shares that owe their walk at a time.
    run: tokio::sync::Mutex<()>,
    /// Shares whose walk is queued or under way.
    pending: std::sync::Mutex<HashSet<ShareId>>,
}

impl Migrations {
    fn is_pending(&self, share: ShareId) -> bool {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).contains(&share)
    }
}

/// A share's walk queued or under way, until the guard goes.
struct Pending<'a> {
    migrations: &'a Migrations,
    share: ShareId,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.migrations.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.share);
    }
}

impl SharingUseCases {
    /// Refused while the share's walk is queued or under way: what changes
    /// all its files at once (a rollback, deleting it) waits for it.
    pub(super) fn not_migrating(&self, share: &Share) -> Result<(), SharingError> {
        if self.migrations.is_pending(share.id) {
            return Err(SharingError::Refused(format!(
                "share {} is being given share-level permissions; try again once that has ended",
                share.name.as_str()
            )));
        }
        Ok(())
    }

    /// Walks each share that owes its walk, one after another, each as a
    /// job, unless its volume is not mounted (it waits for the next run) or
    /// its walk is queued already. A run under way already is left to go on.
    /// zendiloosd calls it when it starts and every few minutes. Returns
    /// how many shares were walked without errors.
    pub async fn migrate_permissions(&self, request: &RequestContext) -> Result<usize, SharingError> {
        let Ok(_run) = self.migrations.run.try_lock() else { return Ok(0) };
        let owed = self.ports.shares.migrations_owed().await?;
        if owed.is_empty() {
            return Ok(0);
        }
        let volumes = self.ports.volumes.volumes(request).await?;
        let mut walked = 0;
        for id in owed {
            let Some(share) = self.ports.shares.find(id).await? else { continue };
            let mounted = volumes.iter().any(|volume| volume.pool == share.pool && volume.mounted);
            if !mounted {
                tracing::info!(
                    share = share.name.as_str(),
                    "the share's volume is not mounted: walked later"
                );
                continue;
            }
            let Some((_, done)) = self.migration_job(&share, None, request).await? else { continue };
            // One share after another: the next job starts once this one ended.
            if done.await.unwrap_or(false) {
                walked += 1;
            }
        }
        Ok(walked)
    }

    /// Starts the job walking `share`, unless one is queued or under way;
    /// with it, what says whether it ended without errors once it has.
    pub(super) async fn migration_job(
        &self,
        share: &Share,
        actor: Option<&Actor>,
        request: &RequestContext,
    ) -> Result<Option<(Job, tokio::sync::oneshot::Receiver<bool>)>, SharingError> {
        {
            let mut pending = self.migrations.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if !pending.insert(share.id) {
                return Ok(None);
            }
        }
        let (finished, done) = tokio::sync::oneshot::channel();
        let (this, request, id) = (self.clone(), request.clone(), share.id);
        let started = self
            .ports
            .jobs
            .start("sharing.migrate_permissions", Some(share.name.to_string()), actor, move |progress| async move {
                let _pending = Pending { migrations: &this.migrations, share: id };
                progress.step("waiting", None).await;
                let walked = {
                    let _walking = this.migrations.walking.lock().await;
                    this.migrate_share(id, &progress, &request).await
                };
                if let Err(error) = &walked {
                    tracing::warn!(%error, share = id.get(), "the share's files did not get share-level access; walked again later");
                }
                let _ = finished.send(walked.is_ok());
                walked.map_err(|error| JobError::new("agent_failed", Some(error.to_string())))
            })
            .await;
        match started {
            Ok(job) => Ok(Some((job, done))),
            Err(error) => {
                self.migrations.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&share.id);
                Err(error.into())
            }
        }
    }

    /// The walk of the share `id` as it is recorded when its turn comes:
    /// one gone, being deleted or walked meanwhile is left alone.
    async fn migrate_share(
        &self,
        id: ShareId,
        progress: &Progress,
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let Some(share) = self.ports.shares.find(id).await? else { return Ok(()) };
        if self.ports.shares.migrated_at(id).await?.is_some() {
            return Ok(());
        }
        progress.step("walking", None).await;
        tracing::info!(share = share.name.as_str(), "giving the share's files share-level access");
        let lines = |line: &str| progress.line(line);
        self.ports.host.migrate_share(share.volume, &share.name, &lines, request).await?;
        // The first second wholly after the walk: a snapshot taken in the
        // second it ended may hold files it had not got to.
        let after = self.ports.clock.now().saturating_add(std::time::Duration::from_secs(1));
        self.ports.shares.set_migrated(id, Some(after)).await?;
        tracing::info!(share = share.name.as_str(), "the share's files have share-level access");
        Ok(())
    }
}
