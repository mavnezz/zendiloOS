//! Permissions as one account or group sees them: what it may do on every
//! share, set in one go; and following changes to groups and accounts.
//! Access is the share's as a whole: Samba follows a group's members
//! itself, and nothing in the files changes. What follows a change is the
//! servers' configuration written again and, where someone lost access,
//! the share's open SMB connections ended, so the next connection holds
//! the new rules.

use std::collections::{BTreeMap, HashSet};

use zendiloos_kernel::{Actor, RequestContext};

use super::SharingUseCases;
use super::ports::Names;
use super::shares::{ShareChanges, SharingError, narrowed, reach};
use crate::domain::{Access, AccessRule, Permission, Share, ShareId, ShareName, Subject, effective};

/// What happens to an account's access everywhere, as the Identity context
/// tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountChange {
    Disabled,
    /// Enabled again.
    Enabled,
    Deleted,
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

/// The shares where someone may do less after the directory went from
/// `before` to `after`.
fn narrowed_shares(shares: &[Share], before: &Names, after: &Names) -> Vec<ShareName> {
    shares
        .iter()
        .filter(|share| narrowed(&reach(share, before), &reach(share, after)))
        .map(|share| share.name.clone())
        .collect()
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
    /// other shares and other subjects keep theirs. Each share that changes
    /// holds its new rules at once (see [`Self::update`]). Returns the
    /// shares that changed.
    pub async fn set_subject_access(
        &self,
        subject: Subject,
        rules: &[(ShareId, Option<Permission>)],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<ShareName>, SharingError> {
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
        let mut changed = Vec::new();
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
            changed.push(self.update(*id, changes, actor, request).await?.name);
        }
        Ok(changed)
    }

    /// The members of `group` changed from `before` to what the directory
    /// says now. Samba follows the group itself; where a member who left
    /// (or one who came, to a group that is denied) may do less now, the
    /// share's open connections end. Returns those shares.
    pub async fn group_members_changed(
        &self,
        group: i64,
        before: &[i64],
        request: &RequestContext,
    ) -> Result<Vec<ShareName>, SharingError> {
        let _changes = self.changes.lock().await;
        let now = self.ports.directory.names().await?;
        let mut then = now.clone();
        then.members.insert(group, before.to_vec());
        let shares = self.ports.shares.list().await?;
        let close = narrowed_shares(&shares, &then, &now);
        self.apply_locked(&close, request).await?;
        Ok(close)
    }

    /// The shares `account` may use now, over SMB: those whose connections
    /// end when it is disabled or deleted. Asked before the change, while
    /// the account, its rules and groups are still there.
    pub async fn account_changing(
        &self,
        account: i64,
        change: AccountChange,
    ) -> Result<Vec<ShareName>, SharingError> {
        if change == AccountChange::Enabled {
            return Ok(Vec::new());
        }
        let names = self.ports.directory.names().await?;
        let shares = self.ports.shares.list().await?;
        Ok(shares
            .iter()
            .filter(|share| reach(share, &names).contains_key(&account))
            .map(|share| share.name.clone())
            .collect())
    }

    /// `account` was disabled, enabled again or deleted; `reached` are the
    /// shares [`Self::account_changing`] named. Samba's sections deny a
    /// disabled account by name (and Samba has it disabled besides); a
    /// deleted one is named no more. The connections of `reached` end.
    pub async fn account_changed(
        &self,
        account: i64,
        change: AccountChange,
        reached: &[ShareName],
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        let close: Vec<ShareName> = match change {
            AccountChange::Enabled => Vec::new(),
            AccountChange::Disabled | AccountChange::Deleted => {
                let shares: BTreeMap<ShareName, ()> =
                    self.ports.shares.list().await?.into_iter().map(|share| (share.name, ())).collect();
                reached.iter().filter(|name| shares.contains_key(name)).cloned().collect()
            }
        };
        tracing::debug!(account, ?change, shares = close.len(), "sharing follows an account");
        self.apply_locked(&close, request).await
    }

    /// `group` is about to be deleted: its rules go first, as a change of
    /// each share's rules, while its members are still known.
    pub async fn group_deleting(
        &self,
        group: i64,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), SharingError> {
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
        self.set_subject_access(subject, &rules, actor, request).await?;
        Ok(())
    }
}
