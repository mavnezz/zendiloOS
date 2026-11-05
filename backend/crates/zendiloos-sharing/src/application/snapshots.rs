//! Snapshots of shares: by hand, on schedules, and letting go of those the
//! schedules no longer keep (docs/architecture.md, 5).

use std::cmp::Reverse;

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
    /// per share as a notification, until a snapshot works again.
    pub async fn take_due_snapshots(&self) {
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
