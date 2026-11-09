//! Snapshots of shares: by hand, on schedules, and letting go of those the
//! schedules no longer keep (docs/architecture.md, 5).

use std::cmp::Reverse;

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, Outcome, RequestContext};
use zendiloos_notify::{Level, NewNotification};

use super::ports::HostError;
use super::{SharingError, SharingUseCases};
use crate::domain::{self, Share, ShareId, Snapshot, SnapshotSchedule};

impl SharingUseCases {
    /// The share's snapshots, newest first, and its schedules.
    pub async fn snapshots(
        &self,
        id: ShareId,
        request: &RequestContext,
    ) -> Result<(Vec<Snapshot>, Vec<SnapshotSchedule>), SharingError> {
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        let all = self.ports.host.snapshots(request).await?;
        Ok((of_share(&share, all), self.ports.shares.snapshot_schedules(id).await?))
    }

    /// Takes a snapshot now, by hand; schedules never delete it.
    pub async fn take_snapshot(
        &self,
        id: ShareId,
        note: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Snapshot, SharingError> {
        let note = domain::note(note)?;
        let _changes = self.changes.lock().await;
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        let now = self.ports.clock.now();
        let label = domain::label_at(now);
        let host = &self.ports.host;
        let created = host
            .create_snapshot(&share.pool, share.volume, &share.name, &label, &[], note.as_deref(), request)
            .await;
        let target = format!("{}@{label}", share.name);
        let outcome = if created.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("sharing.take_snapshot", outcome, &target, actor, request).await;
        match created {
            Ok(()) => Ok(Snapshot {
                pool: share.pool,
                share: share.name,
                label,
                taken_at: now,
                schedules: Vec::new(),
                note,
                used_bytes: 0,
                referenced_bytes: 0,
            }),
            Err(HostError::Conflict) => Err(SharingError::SnapshotTaken),
            Err(error) => Err(error.into()),
        }
    }

    /// Destroys one snapshot of the share; the share's data stays.
    pub async fn delete_snapshot(
        &self,
        id: ShareId,
        label: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        let snapshots = of_share(&share, self.ports.host.snapshots(request).await?);
        if !domain::is_label(label) || !snapshots.iter().any(|s| s.label == label) {
            return Err(SharingError::SnapshotNotFound);
        }
        let destroyed =
            self.ports.host.destroy_snapshot(&share.pool, share.volume, &share.name, label, request).await;
        let outcome = if destroyed.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("sharing.delete_snapshot", outcome, &format!("{}@{label}", share.name), actor, request)
            .await;
        Ok(destroyed?)
    }

    /// Returns the share to snapshot `label`: everything changed since is
    /// gone, and so are the snapshots taken after it. `confirm` must be the
    /// share's name. Refused while the share's walk to share-level access
    /// is queued or under way.
    ///
    /// The rules hold as recorded now, whatever the files say: Samba and
    /// the file manager enforce them. The share's folder gets what every
    /// share's has again. A snapshot from before the share's walk brings
    /// back the entries per person of then: the share owes its walk again,
    /// and the job is returned with the labels of the snapshots that went.
    pub async fn rollback_snapshot(
        &self,
        id: ShareId,
        label: &str,
        confirm: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(Vec<String>, Option<Job>), SharingError> {
        let _changes = self.changes.lock().await;
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        if confirm != share.name.as_str() {
            return Err(SharingError::ConfirmationMismatch);
        }
        let snapshots = of_share(&share, self.ports.host.snapshots(request).await?);
        let Some(target) = snapshots.iter().find(|s| domain::is_label(label) && s.label == label) else {
            return Err(SharingError::SnapshotNotFound);
        };
        self.not_migrating(&share)?;
        let later: Vec<String> =
            snapshots.iter().filter(|s| s.taken_at > target.taken_at).map(|s| s.label.clone()).collect();
        // Owed before the files change, should the snapshot be from before
        // the walk: whatever stops this halfway, the walk comes.
        let migrated = self.ports.shares.migrated_at(share.id).await?;
        // Older than its walk (or than the share, made with share-level access).
        let older = migrated.is_none_or(|at| target.taken_at < at);
        if older {
            self.ports.shares.set_migrated(share.id, None).await?;
        }
        let host = &self.ports.host;
        let rolled = host.rollback_snapshot(&share.pool, share.volume, &share.name, label, request).await;
        let outcome = if rolled.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("sharing.rollback_snapshot", outcome, &format!("{}@{label}", share.name), actor, request)
            .await;
        rolled?;

        if let Err(error) = host.prepare_share(share.volume, &share.name, request).await {
            tracing::warn!(%error, share = share.name.as_str(), "after a rollback the share's folder is not as every share's");
        }
        if !older {
            return Ok((later, None));
        }
        let walk = match self.migration_job(&share, Some(actor), request).await {
            Ok(job) => job.map(|(job, _)| job),
            Err(error) => {
                tracing::warn!(%error, share = share.name.as_str(), "cannot walk the share after a rollback now; it is walked later");
                None
            }
        };
        Ok((later, walk))
    }

    /// Replaces how often the share is snapshotted and how many to keep.
    /// Snapshots of a schedule that is removed stay until deleted by hand.
    pub async fn set_snapshot_schedules(
        &self,
        id: ShareId,
        schedules: Vec<SnapshotSchedule>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<SnapshotSchedule>, SharingError> {
        let schedules = domain::schedules(schedules)?;
        let _changes = self.changes.lock().await;
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        self.ports.shares.set_snapshot_schedules(id, &schedules).await?;
        self.audit("sharing.snapshot_schedules", Outcome::Success, share.name.as_str(), actor, request).await;
        Ok(schedules)
    }

    /// Takes the snapshots that are due and lets go of those the schedules
    /// no longer keep; call every few minutes. A failure is reported once
    /// per share as a notification, until a snapshot works again. While an
    /// update of the system installs, what is due waits for the next call.
    pub async fn take_due_snapshots(&self) {
        if let Some(why) = self.ports.jobs.held().await {
            return tracing::info!(why, "snapshots: what is due waits until the update is done");
        }
        let request = RequestContext::default();
        let (shares, all) = match (self.ports.shares.list().await, self.ports.host.snapshots(&request).await)
        {
            (Ok(shares), Ok(all)) => (shares, all),
            (Err(error), _) => return tracing::warn!(%error, "snapshots: cannot read the shares"),
            (_, Err(error)) => return tracing::warn!(%error, "snapshots: cannot read them from the host"),
        };
        for share in shares {
            let schedules = match self.ports.shares.snapshot_schedules(share.id).await {
                Ok(schedules) if !schedules.is_empty() => schedules,
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(%error, share = share.name.as_str(), "snapshots: cannot read the schedules");
                    continue;
                }
            };
            self.keep_up(&share, &schedules, of_share(&share, all.clone()), &request).await;
        }
    }

    async fn keep_up(
        &self,
        share: &Share,
        schedules: &[SnapshotSchedule],
        mut taken: Vec<Snapshot>,
        request: &RequestContext,
    ) {
        let _changes = self.changes.lock().await;
        let now = self.ports.clock.now();
        let due = domain::due(schedules, &taken, now);
        if !due.is_empty() {
            let label = domain::label_at(now);
            let host = &self.ports.host;
            match host
                .create_snapshot(&share.pool, share.volume, &share.name, &label, &due, None, request)
                .await
            {
                Ok(()) => {
                    tracing::info!(share = share.name.as_str(), label, "snapshots: taken on schedule");
                    self.snapshot_failures
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&share.id);
                    taken.push(Snapshot {
                        pool: share.pool.clone(),
                        share: share.name.clone(),
                        label,
                        taken_at: now,
                        schedules: due,
                        note: None,
                        used_bytes: 0,
                        referenced_bytes: 0,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, share = share.name.as_str(), "snapshots: cannot take one on schedule");
                    let first = self
                        .snapshot_failures
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(share.id);
                    if first {
                        let notice = NewNotification::new(Level::Warning, "snapshot_failed")
                            .with("share", share.name.as_str());
                        self.ports.notifier.notify(notice).await;
                    }
                    return;
                }
            }
        }
        for old in domain::prunable(schedules, &taken) {
            if let Err(error) = self
                .ports
                .host
                .destroy_snapshot(&share.pool, share.volume, &share.name, &old.label, request)
                .await
            {
                tracing::warn!(%error, share = share.name.as_str(), label = old.label, "snapshots: cannot let one go");
            }
        }
    }
}

/// Those of `all` that belong to `share`, newest first.
fn of_share(share: &Share, all: Vec<Snapshot>) -> Vec<Snapshot> {
    let mut mine: Vec<Snapshot> =
        all.into_iter().filter(|s| s.pool == share.pool && s.share == share.name).collect();
    mine.sort_by_key(|s| Reverse(s.taken_at));
    mine
}
