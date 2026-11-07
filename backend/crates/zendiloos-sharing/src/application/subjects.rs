//! Permissions as one account or group sees them: what it may do on every
//! share, set in one go; and following changes to groups, whose denied
//! members carry entries of their own.

use std::collections::HashSet;

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};

use super::SharingUseCases;
use super::shares::{ShareChanges, SharingError, acl};
use crate::domain::{Access, AccessRule, Permission, Principal, ShareId, ShareName, Subject, effective};

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

    /// The members of `group` changed from `before`. Where the group is
    /// denied, members who left lose their entry and new ones get one;
    /// where it has a grant, the group entry covers them already, and
    /// Samba follows the group itself. Where it may write, the share's
    /// folder is set again, which makes the new members' recycle bins:
    /// a writer whose bin is not made first could have one made by
    /// another writer, who would then see what lands in it.
    pub async fn group_members_changed(
        &self,
        group: i64,
        before: &[i64],
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<Job>, SharingError> {
        let _changes = self.changes.lock().await;
        let names = self.ports.directory.names().await?;
        let apps = self.ports.shares.app_access().await?;
        let shares = self.ports.shares.list().await?;
        // A denied group's members carry entries of their own: a walk with
        // the members as they were would undo them.
        let denying =
            |share: &&crate::domain::Share| share.rule_of(Subject::Group(group)) == Some(Permission::Deny);
        for share in shares.iter().filter(denying) {
            self.not_walking(share)?;
        }
        let mut jobs = Vec::new();
        for share in shares {
            let rule = share.rule_of(Subject::Group(group));
            if rule == Some(Permission::ReadWrite) {
                let now = acl(&share.access, &names, share.id, &apps);
                self.ports.host.set_access(share.volume, &share.name, &now, &[], false, request).await?;
                continue;
            }
            if rule != Some(Permission::Deny) {
                continue;
            }
            let now = acl(&share.access, &names, share.id, &apps);
            let removed: Vec<Principal> = before
                .iter()
                .filter_map(|id| names.accounts.get(id))
                .map(|name| Principal::User(name.clone()))
                .filter(|principal| !now.iter().any(|(entry, _)| entry == principal))
                .collect();
            self.ports.host.set_access(share.volume, &share.name, &now, &removed, false, request).await?;
            jobs.push(self.access_job(&share, removed, actor, request).await?);
        }
        Ok(jobs)
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
