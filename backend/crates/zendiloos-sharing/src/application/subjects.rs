//! Permissions as one account or group sees them: what it may do on every
//! share, set in one go; and following changes to groups and accounts,
//! which the shares' files have to follow: denied members carry entries of
//! their own, disabled accounts too, and what those who may no longer
//! write own passes to root.

use std::collections::{BTreeSet, HashSet};

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};

use super::SharingUseCases;
use super::ports::Names;
use super::shares::{ShareChanges, SharingError, acl};
use crate::domain::{
    Access, AccessRule, AppAccess, Permission, Principal, Share, ShareId, ShareName, Subject, effective,
};

/// What happens to an account's access everywhere, as the Identity context
/// tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountChange {
    Disabled,
    /// Enabled again.
    Enabled,
    Deleted,
}

/// `names` as they are once `change` has happened to `account`.
fn changed(names: &Names, account: i64, change: AccountChange) -> Names {
    let mut then = names.clone();
    match change {
        AccountChange::Disabled => {
            then.disabled.insert(account);
        }
        AccountChange::Enabled => {
            then.disabled.remove(&account);
        }
        AccountChange::Deleted => {
            then.accounts.remove(&account);
            then.disabled.remove(&account);
            for members in then.members.values_mut() {
                members.retain(|id| *id != account);
            }
        }
    }
    then
}

/// The accounts that may write where `rules` apply, as `names` say: what
/// they own there stays theirs (4.4, Owners).
fn writers(rules: &[AccessRule], names: &Names) -> BTreeSet<i64> {
    names
        .accounts
        .keys()
        .copied()
        .filter(|id| !names.disabled.contains(id))
        .filter(|id| {
            let groups: Vec<i64> = names
                .members
                .iter()
                .filter(|(_, members)| members.contains(id))
                .map(|(group, _)| *group)
                .collect();
            effective(rules, *id, &groups) == Some(Access::ReadWrite)
        })
        .collect()
}

/// Whether a change of the directory from `before` to `after` leaves
/// `share`'s files owing it its rules: its ACL changes, or someone may no
/// longer write there, whose files pass to root.
fn owes(share: &Share, before: &Names, after: &Names, apps: &[AppAccess]) -> bool {
    let then = acl(&share.access, before, share.id, apps);
    let now = acl(&share.access, after, share.id, apps);
    let same = then.len() == now.len() && then.iter().all(|entry| now.contains(entry));
    !same || !writers(&share.access, before).is_subset(&writers(&share.access, after))
}

/// One share as an account or group sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectAccess {
    pub share: ShareId,
    pub name: ShareName,
    /// The subject's own rule.
    pub rule: Option<Permission>,
    /// What it may do in the end: for an account with its groups' rules
    /// (see [`effective`]), for a group what its own rule grants.
    pub effective: Option<Access>,
}

impl SharingUseCases {
    /// Every share, with `subject`'s rule and what it may do.
    pub async fn subject_access(&self, subject: Subject) -> Result<Vec<SubjectAccess>, SharingError> {
        let names = self.ports.directory.names().await?;
        let groups = match subject {
            Subject::Account(id) if names.accounts.contains_key(&id) => {
                self.ports.directory.memberships(id).await?
            }
            Subject::Group(id) if names.groups.contains_key(&id) => Vec::new(),
            _ => return Err(SharingError::UnknownSubject),
        };
        let shares = self.ports.shares.list().await?;
        Ok(shares
            .iter()
            .map(|share| {
                let rule = share.rule_of(subject);
                let effective = match subject {
                    Subject::Account(id) => effective(&share.access, id, &groups),
                    Subject::Group(_) => rule.and_then(Permission::grant),
                };
                SubjectAccess { share: share.id, name: share.name.clone(), rule, effective }
            })
            .collect())
    }

    /// Sets `subject`'s rule on the shares named, `None` for no rule;
    /// other shares and other subjects keep theirs. Each share that
    /// changes gets its permissions at once, its contents in a job.
    pub async fn set_subject_access(
        &self,
        subject: Subject,
        rules: &[(ShareId, Option<Permission>)],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<Job>, SharingError> {
        let shares = self.ports.shares.list().await?;
        let mut seen = HashSet::new();
        for (id, _) in rules {
            if !seen.insert(*id) {
                return Err(SharingError::Duplicate);
            }
            if !shares.iter().any(|share| share.id == *id) {
                return Err(SharingError::ShareNotFound);
            }
        }
        let mut jobs = Vec::new();
        for (id, permission) in rules {
            let Some(share) = shares.iter().find(|share| share.id == *id) else { continue };
            if share.rule_of(subject) == *permission {
                continue;
            }
            let mut access: Vec<AccessRule> =
                share.access.iter().filter(|rule| rule.subject != subject).cloned().collect();
            if let Some(permission) = permission {
                access.push(AccessRule { subject, permission: *permission });
            }
            let changes = ShareChanges { access: Some(access), ..ShareChanges::default() };
            let (_, job) = self.update(*id, changes, actor, request).await?;
            jobs.extend(job);
        }
        Ok(jobs)
    }

    /// The members of `group` are about to become `after`; nothing has
    /// changed yet. The shares whose files will have to follow are marked
    /// owing them their rules first, so that whatever stops the change
    /// halfway, nothing of it is lost. Returns their names.
    pub async fn group_members_changing(
        &self,
        group: i64,
        after: &[i64],
    ) -> Result<Vec<ShareName>, SharingError> {
        let _changes = self.changes.lock().await;
        let now = self.ports.directory.names().await?;
        let mut then = now.clone();
        then.members.insert(group, after.to_vec());
        self.mark_owing(&now, &then).await
    }

    /// The members of `group` changed from `before`; `owing` are the
    /// shares [`Self::group_members_changing`] named. Where the group is
    /// denied, members who left lose their entry and new ones get one;
    /// where it may write, those who left hand what they own over (4.4,
    /// Owners) and new members get their recycle bins; where it reads, the
    /// group entry covers them already. Samba follows the group itself.
    /// See [`Self::follow`]: returns the shares whose files follow in the
    /// background.
    pub async fn group_members_changed(
        &self,
        group: i64,
        before: &[i64],
        owing: &[ShareName],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<ShareName>, SharingError> {
        let _changes = self.changes.lock().await;
        let now = self.ports.directory.names().await?;
        let mut then = now.clone();
        then.members.insert(group, before.to_vec());
        self.follow(&then, &now, owing, actor, request).await
    }

    /// `account` is about to be disabled, enabled again or deleted;
    /// nothing has changed yet. Marks the shares whose files will have to
    /// follow, as [`Self::group_members_changing`] does. A disabled account
    /// may do nothing anywhere, as if denied, and what it owns passes to
    /// root where it wrote.
    pub async fn account_changing(
        &self,
        account: i64,
        change: AccountChange,
    ) -> Result<Vec<ShareName>, SharingError> {
        let _changes = self.changes.lock().await;
        let now = self.ports.directory.names().await?;
        let then = changed(&now, account, change);
        self.mark_owing(&now, &then).await
    }

    /// `account` was disabled, enabled again or deleted; `owing` are the
    /// shares [`Self::account_changing`] named. See [`Self::follow`].
    pub async fn account_changed(
        &self,
        account: i64,
        change: AccountChange,
        owing: &[ShareName],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<ShareName>, SharingError> {
        let _changes = self.changes.lock().await;
        let now = self.ports.directory.names().await?;
        // How it was before. A deleted account went with its rules: what
        // it could do is not known any more, and the shares named before
        // follow.
        let then = match change {
            AccountChange::Disabled => changed(&now, account, AccountChange::Enabled),
            AccountChange::Enabled => changed(&now, account, AccountChange::Disabled),
            AccountChange::Deleted => now.clone(),
        };
        self.follow(&then, &now, owing, actor, request).await
    }

    /// Marks the shares a change of the directory from `before` to `after`
    /// leaves owing their files their rules, and names them.
    async fn mark_owing(&self, before: &Names, after: &Names) -> Result<Vec<ShareName>, SharingError> {
        let shares = self.ports.shares.list().await?;
        let apps = self.ports.shares.app_access().await?;
        let owing: Vec<&Share> = shares.iter().filter(|share| owes(share, before, after, &apps)).collect();
        if !owing.is_empty() {
            let ids: Vec<ShareId> = owing.iter().map(|share| share.id).collect();
            self.ports.shares.owe_access(&ids).await?;
        }
        Ok(owing.into_iter().map(|share| share.name.clone()).collect())
    }

    /// After a change of the directory from `before` to `after`, for every
    /// share it leaves owing its files their rules, and those in `owing`:
    /// marked again (a walk that read the directory before the change must
    /// not settle it); its folder gets the rules at once, unless a walk is
    /// under way, which would give the folder back the rules it read (the
    /// walk queued next gives the folder its rules first); its files in a
    /// walk. Where the change makes new writers, the folder is set again,
    /// which makes their recycle bins: a writer whose bin is not made first
    /// could have one made by another writer, who would then see what
    /// lands in it. A share that cannot be changed now does not keep the
    /// others from it: it stays owed and is walked again later (see
    /// [`Self::reconcile_access`]). Returns the shares whose files follow
    /// in the background.
    async fn follow(
        &self,
        before: &Names,
        after: &Names,
        owing: &[ShareName],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<ShareName>, SharingError> {
        let shares = self.ports.shares.list().await?;
        let apps = self.ports.shares.app_access().await?;
        let owed: HashSet<ShareId> = shares
            .iter()
            .filter(|share| owing.contains(&share.name) || owes(share, before, after, &apps))
            .map(|share| share.id)
            .collect();
        if !owed.is_empty() {
            self.ports.shares.owe_access(&owed.iter().copied().collect::<Vec<_>>()).await?;
        }
        let mut following = Vec::new();
        for share in &shares {
            let gains = !writers(&share.access, after).is_subset(&writers(&share.access, before));
            let mut owes = owed.contains(&share.id);
            if !owes && !gains {
                continue;
            }
            let now = acl(&share.access, after, share.id, &apps);
            let removed: Vec<Principal> = acl(&share.access, before, share.id, &apps)
                .into_iter()
                .map(|(principal, _)| principal)
                .filter(|principal| !now.iter().any(|(entry, _)| entry == principal))
                .collect();
            let set = if self.walks.is_under_way(share.id) {
                tracing::info!(
                    share = share.name.as_str(),
                    "a walk is under way: the next gives the folder its rules"
                );
                false
            } else {
                let set = self
                    .ports
                    .host
                    .set_access(share.volume, &share.name, &now, &removed, false, request)
                    .await;
                if let Err(error) = &set {
                    tracing::warn!(%error, share = share.name.as_str(), "the share's folder follows in a walk");
                }
                set.is_ok()
            };
            // The folder (and the new writers' bins) is the walk's then.
            if !set && !owes {
                if let Err(error) = self.ports.shares.owe_access(&[share.id]).await {
                    tracing::warn!(%error, share = share.name.as_str(), "cannot mark the share's files as owing");
                }
                owes = true;
            }
            if !owes {
                continue;
            }
            if let Err(error) = self.access_job(share, removed, Some(actor), request).await {
                tracing::warn!(%error, share = share.name.as_str(), "cannot walk the share now; it is walked later");
            }
            following.push(share.name.clone());
        }
        Ok(following)
    }

    /// `group` is about to be deleted: its rules go first, while its
    /// members are still known, and with them the entries of members it
    /// denied.
    pub async fn group_deleting(
        &self,
        group: i64,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<Job>, SharingError> {
        let subject = Subject::Group(group);
        let rules: Vec<(ShareId, Option<Permission>)> = self
            .ports
            .shares
            .list()
            .await?
            .iter()
            .filter(|share| share.rule_of(subject).is_some())
            .map(|share| (share.id, None))
            .collect();
        self.set_subject_access(subject, &rules, actor, request).await
    }
}
