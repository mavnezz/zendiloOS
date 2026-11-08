//! Commands of administrators on groups. Like accounts, each group is a
//! Linux group, whose members the agent keeps in step.

use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext};

use crate::application::context::Context;
use crate::application::ports::Creation;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{
    AccountId, AccountName, AuditAction, Group, GroupId, GroupName, NewGroup, Permission, text,
};

pub struct NewGroupInput {
    pub name: String,
    pub description: Option<String>,
    pub members: Vec<AccountId>,
}

/// Command: an administrator creates a group.
#[derive(Clone)]
pub struct CreateGroup {
    context: Arc<Context>,
}

impl CreateGroup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        input: NewGroupInput,
        request: &RequestContext,
    ) -> Result<Group, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        let name = GroupName::parse(input.name.trim())?;
        let description = text::description(input.description.as_deref())?;

        let _changes = c.changes.lock().await;
        let now = c.now();
        let (members, member_names) = c.members(input.members).await?;
        if c.ports.groups.list().await?.iter().any(|group| group.name == name) {
            return Err(IdentityError::NameTaken);
        }
        if c.ports.system.create_group(&name, request).await? == Creation::NameTaken {
            return Err(IdentityError::NameTaken);
        }
        let stored = async {
            if !member_names.is_empty() {
                c.ports.system.set_group_members(&name, &member_names, request).await?;
            }
            let new = NewGroup { name: name.clone(), description, members, created_at: now };
            c.ports.groups.create(new).await?.ok_or(IdentityError::NameTaken)
        }
        .await;
        let group = match stored {
            Ok(group) => group,
            Err(error) => {
                if let Err(cleanup) = c.ports.system.delete_group(&name, request).await {
                    tracing::error!(group = name.as_str(), error = %cleanup, "cannot remove the Linux group of a failed group creation");
                }
                return Err(error);
            }
        };

        let entry = AuditEntry::new(now, AuditAction::GroupCreate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(name.as_str())).await?;
        Ok(group)
    }
}

/// Changes to a group; `None` leaves a field as it is.
#[derive(Default)]
pub struct GroupChanges {
    /// `Some(None)` removes the description.
    pub description: Option<Option<String>>,
    /// Replaces all members.
    pub members: Option<Vec<AccountId>>,
}

/// Command: an administrator changes a group's description or members.
#[derive(Clone)]
pub struct UpdateGroup {
    context: Arc<Context>,
}

impl UpdateGroup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// Returns the group and what follows the change of its members in the
    /// background, named (the shares whose files are given their
    /// permissions anew); the audit names it too.
    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: GroupId,
        changes: GroupChanges,
        request: &RequestContext,
    ) -> Result<(Group, Vec<String>), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        let description = changes.description.map(|d| text::description(d.as_deref())).transpose()?;

        let _changes = c.changes.lock().await;
        let now = c.now();
        let mut group = c.ports.groups.find_by_id(id).await?.ok_or(IdentityError::GroupNotFound)?;
        let before = group.members.clone();
        let mut following = Vec::new();
        if let Some(members) = changes.members {
            let (members, names) = c.members(members).await?;
            if members != group.members {
                // Others note what has to follow first: whatever stops the
                // change halfway, they make it up.
                following = c.ports.group_observer.members_changing(group.id, &members).await?;
                c.ports.system.set_group_members(&group.name, &names, request).await?;
                group.members = members;
            }
        }
        if let Some(description) = description {
            group.description = description;
        }
        group.updated_at = now;
        c.ports.groups.update(&group).await?;
        if group.members != before {
            // The change stands; what others cannot follow now they make up
            // later, as noted.
            let actor = auth.actor();
            match c.ports.group_observer.members_changed(group.id, &before, &following, &actor, request).await
            {
                Ok(more) => join(&mut following, more),
                Err(error) => {
                    tracing::warn!(%error, group = group.name.as_str(), "others follow the group's members later");
                }
            }
        }

        let entry = AuditEntry::new(now, AuditAction::GroupUpdate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(pending(group.name.as_str(), &following))).await?;
        Ok((group, following))
    }
}

/// `more` added to `following`, each once.
pub(super) fn join(following: &mut Vec<String>, more: Vec<String>) {
    for item in more {
        if !following.contains(&item) {
            following.push(item);
        }
    }
}

/// An audit target with what follows in the background, if anything.
pub(super) fn pending(target: &str, following: &[String]) -> String {
    if following.is_empty() {
        target.to_owned()
    } else {
        format!("{target} (pending: {})", following.join(", "))
    }
}

/// Command: an administrator deletes a group with its Linux group.
#[derive(Clone)]
pub struct DeleteGroup {
    context: Arc<Context>,
}

impl DeleteGroup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: GroupId,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;

        let _changes = c.changes.lock().await;
        let now = c.now();
        let group = c.ports.groups.find_by_id(id).await?.ok_or(IdentityError::GroupNotFound)?;
        // Before it goes: shares still know its members.
        c.ports.group_observer.deleting(group.id, &auth.actor(), request).await?;
        c.ports.system.delete_group(&group.name, request).await?;
        c.ports.groups.delete(id).await?;

        let entry = AuditEntry::new(now, AuditAction::GroupDelete.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(group.name.as_str())).await?;
        Ok(())
    }
}

impl Context {
    /// Sorted, distinct member IDs and their names; every ID must exist.
    async fn members(
        &self,
        ids: Vec<AccountId>,
    ) -> Result<(Vec<AccountId>, Vec<AccountName>), IdentityError> {
        let mut ids = ids;
        ids.sort();
        ids.dedup();
        let mut names = Vec::with_capacity(ids.len());
        for &id in &ids {
            let account = self.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::AccountNotFound)?;
            names.push(account.name);
        }
        Ok((ids, names))
    }
}
