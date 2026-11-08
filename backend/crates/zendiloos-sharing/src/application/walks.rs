//! Walks: changes of everything in a share, which may take hours; and what
//! makes the files of every share end up with the share's rules
//! (docs/architecture.md, 4.4).
//!
//! Whatever the files have to follow (new rules, members of a group a
//! share names, an account disabled, enabled again or deleted, a snapshot
//! rolled back) marks the share first, in the database
//! ([`ShareRepository::owe_access`](super::ports::ShareRepository::owe_access)).
//! Only a walk that read the rules after the latest mark and ended without
//! errors settles it. A share that still owes its files their rules is
//! walked again: when zendiloosd starts, after each walk and every few
//! minutes ([`SharingUseCases::reconcile_access`]), so a walk that failed,
//! stopped with zendiloosd or never started is made up.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, RequestContext, Timestamp};

use super::SharingUseCases;
use super::shares::{SharingError, acl};
use crate::domain::{Principal, Share, ShareId};

/// How long the next walk of a share whose walk failed waits; each further
/// failure doubles it, up to [`MAX_RETRY`]. A share that keeps failing
/// (folders nested too deeply, a pool not imported) is not walked through
/// again and again.
const RETRY: Duration = Duration::from_secs(5 * 60);
const MAX_RETRY: Duration = Duration::from_secs(24 * 3600);

/// The walks of each share: one at a time, those under way, those queued,
/// and those that failed.
#[derive(Default)]
pub(super) struct Walks {
    queues: std::sync::Mutex<HashMap<ShareId, Arc<tokio::sync::Mutex<()>>>>,
    /// Shares whose walk has read the rules it gives and not ended.
    under_way: std::sync::Mutex<HashSet<ShareId>>,
    /// Walks queued or under way, for each share.
    pending: std::sync::Mutex<HashMap<ShareId, usize>>,
    /// Shares whose last walks failed: how many in a row, and when the
    /// next may start on its own.
    failed: std::sync::Mutex<HashMap<ShareId, (u32, Timestamp)>>,
}

impl Walks {
    pub(super) fn queue(&self, share: ShareId) -> Arc<tokio::sync::Mutex<()>> {
        let mut queues = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(queues.entry(share).or_default())
    }

    pub(super) fn is_under_way(&self, share: ShareId) -> bool {
        self.under_way.lock().unwrap_or_else(PoisonError::into_inner).contains(&share)
    }

    /// Marks a walk of `share` under way until the guard goes.
    fn start(&self, share: ShareId) -> UnderWay<'_> {
        self.under_way.lock().unwrap_or_else(PoisonError::into_inner).insert(share);
        UnderWay { walks: self, share }
    }

    /// A walk of `share` queued, until the guard goes.
    pub(super) fn pending(self: &Arc<Self>, share: ShareId) -> Pending {
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner).entry(share).or_default() += 1;
        Pending { walks: Arc::clone(self), share }
    }

    /// A walk of `share` queued, unless one is queued or under way already.
    fn pending_unless_queued(self: &Arc<Self>, share: ShareId) -> Option<Pending> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.get(&share).is_some_and(|count| *count > 0) {
            return None;
        }
        *pending.entry(share).or_default() += 1;
        Some(Pending { walks: Arc::clone(self), share })
    }

    fn succeeded(&self, share: ShareId) {
        self.failed.lock().unwrap_or_else(PoisonError::into_inner).remove(&share);
    }

    fn failed(&self, share: ShareId, at: Timestamp) {
        let mut failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        let times = failed.get(&share).map_or(0, |(times, _)| *times).saturating_add(1);
        let wait = RETRY.saturating_mul(1_u32 << times.saturating_sub(1).min(16)).min(MAX_RETRY);
        failed.insert(share, (times, at.saturating_add(wait)));
    }

    /// Whether a walk of `share` may start on its own at `now`: not before
    /// its wait after failures is over.
    fn may_retry(&self, share: ShareId, now: Timestamp) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&share)
            .is_none_or(|(_, next)| *next <= now)
    }
}

struct UnderWay<'a> {
    walks: &'a Walks,
    share: ShareId,
}

impl Drop for UnderWay<'_> {
    fn drop(&mut self) {
        self.walks.under_way.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.share);
    }
}

/// A walk queued or under way, until it ends.
pub(super) struct Pending {
    walks: Arc<Walks>,
    share: ShareId,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut pending = self.walks.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = pending.get_mut(&self.share) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pending.remove(&self.share);
            }
        }
    }
}

impl SharingUseCases {
    /// Refused while a walk gives the share's files rules it read before:
    /// it would undo the change on them, and on the share's folder too.
    /// Called with `changes` held, before anything changes.
    pub(super) fn not_walking(&self, share: &Share) -> Result<(), SharingError> {
        if self.walks.is_under_way(share.id) {
            return Err(SharingError::Refused(format!(
                "the permissions of share {} are being applied to its files; try again once that has ended",
                share.name.as_str()
            )));
        }
        Ok(())
    }

    /// Gives the share's folder and everything in it the rules the
    /// database has for the share when this gets to run, never rules read
    /// earlier: walks of a share run one after another, each reads the
    /// rules once it is its turn. So the last walk gives the last rules,
    /// whatever order the jobs ran in. `removed` lose their entries, unless
    /// the rules name them again by now; the files agent also takes away
    /// whatever else the rules do not grant. Ended without errors, it
    /// settles what the share owed up to the mark it read with the rules. A
    /// share gone or being deleted is left alone.
    pub(super) async fn walk(
        &self,
        share: ShareId,
        removed: &[Principal],
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let walked = self.walk_once(share, removed, request).await;
        match &walked {
            Ok(()) => self.walks.succeeded(share),
            Err(error) => {
                tracing::warn!(%error, share = share.get(), "the share's files did not get its rules; tried again later");
                self.walks.failed(share, self.ports.clock.now());
            }
        }
        walked
    }

    async fn walk_once(
        &self,
        share: ShareId,
        removed: &[Principal],
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let queue = self.walks.queue(share);
        let _turn = queue.lock().await;
        let (found, now, mark, _under_way) = {
            let _changes = self.changes.lock().await;
            let Some(found) = self.ports.shares.find(share).await? else { return Ok(()) };
            let owed = self.ports.shares.access_owed().await?;
            let mark = owed.iter().find(|(id, _)| *id == share).map_or(0, |(_, mark)| *mark);
            let names = self.ports.directory.names().await?;
            let apps = self.ports.shares.app_access().await?;
            let now = acl(&found.access, &names, found.id, &apps);
            (found, now, mark, self.walks.start(share))
        };
        let removed: Vec<Principal> =
            removed.iter().filter(|gone| !now.iter().any(|(p, _)| p == *gone)).cloned().collect();
        self.ports.host.set_access(found.volume, &found.name, &now, &removed, true, request).await?;
        if mark > 0 {
            self.ports.shares.access_settled(share, mark).await?;
        }
        Ok(())
    }

    /// A job that gives everything in the share the permissions its
    /// directory got already, as they are recorded when it runs (see
    /// [`Self::walk`]). The share is marked owing them already.
    pub(super) async fn access_job(
        &self,
        share: &Share,
        removed: Vec<Principal>,
        actor: Option<&Actor>,
        request: &RequestContext,
    ) -> Result<Job, SharingError> {
        let pending = self.walks.pending(share.id);
        self.walk_job(share, removed, actor, request, pending).await
    }

    async fn walk_job(
        &self,
        share: &Share,
        removed: Vec<Principal>,
        actor: Option<&Actor>,
        request: &RequestContext,
        pending: Pending,
    ) -> Result<Job, SharingError> {
        let this = self.clone();
        let request = request.clone();
        let id = share.id;
        Ok(self
            .ports
            .jobs
            .start("sharing.apply_access", Some(share.name.to_string()), actor, move |progress| async move {
                progress.step("existing_files", None).await;
                let walked = this.walk(id, &removed, &request).await;
                drop(pending);
                this.reconcile_after_walk(request).await;
                walked.map_err(|error| JobError::new("agent_failed", Some(error.to_string())))
            })
            .await?)
    }

    /// Queues a walk of every share whose files still owe it its rules,
    /// unless one is queued or under way, its volume is not mounted, or its
    /// last walks failed not long ago (see [`RETRY`]). zendiloosd calls it
    /// when it starts and every few minutes; every walk ends with it.
    /// Returns the walks queued.
    pub async fn reconcile_access(&self, request: &RequestContext) -> Result<Vec<Job>, SharingError> {
        let owed = self.ports.shares.access_owed().await?;
        if owed.is_empty() {
            return Ok(Vec::new());
        }
        let shares = self.ports.shares.list().await?;
        let volumes = self.ports.volumes.volumes(request).await?;
        let now = self.ports.clock.now();
        let mut jobs = Vec::new();
        for (id, _) in owed {
            let Some(share) = shares.iter().find(|share| share.id == id) else { continue };
            let mounted = volumes.iter().any(|volume| volume.pool == share.pool && volume.mounted);
            if !mounted || !self.walks.may_retry(id, now) {
                continue;
            }
            let Some(pending) = self.walks.pending_unless_queued(id) else { continue };
            tracing::info!(share = share.name.as_str(), "the share's files owe it its rules: walked again");
            jobs.push(self.walk_job(share, Vec::new(), None, request, pending).await?);
        }
        Ok(jobs)
    }

    /// [`Self::reconcile_access`] after a walk, boxed: it starts walks,
    /// whose jobs end with it.
    fn reconcile_after_walk(&self, request: RequestContext) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let this = self.clone();
        Box::pin(async move {
            if let Err(error) = this.reconcile_access(&request).await {
                tracing::warn!(%error, "cannot look for shares whose files owe them their rules");
            }
        })
    }
}
