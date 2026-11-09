use async_trait::async_trait;
use zendiloos_identity::application::ports::{AccessChange, AccountObserver, GroupObserver};
use zendiloos_identity::domain::{AccountId, GroupId};
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::{AccountChange, SharingUseCases};
use zendiloos_sharing::domain::ShareName;

/// [`GroupObserver`] for the Sharing context: Samba follows a group's
/// members itself; where someone may do less now, the share's open
/// connections end. Before a group goes, its rules go.
pub struct SharingFollowsGroups {
    sharing: SharingUseCases,
}

impl SharingFollowsGroups {
    pub fn new(sharing: SharingUseCases) -> Self {
        Self { sharing }
    }
}

fn failed(error: impl std::fmt::Display) -> PortError {
    PortError::new("shares", error.to_string())
}

fn ids(ids: &[AccountId]) -> Vec<i64> {
    ids.iter().map(|id| id.get()).collect()
}

/// Shares by name, as identity hands back what it was told; names that are
/// no share's (any more) are left out.
fn shares(names: &[String]) -> Vec<ShareName> {
    names.iter().filter_map(|name| ShareName::parse(name).ok()).collect()
}

#[async_trait]
impl GroupObserver for SharingFollowsGroups {
    async fn members_changed(
        &self,
        group: GroupId,
        before: &[AccountId],
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.sharing.group_members_changed(group.get(), &ids(before), context).await.map_err(failed)?;
        Ok(())
    }

    async fn deleting(
        &self,
        group: GroupId,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.sharing.group_deleting(group.get(), actor, context).await.map_err(failed)
    }
}

/// [`AccountObserver`] for the Sharing context: a disabled account may do
/// nothing on any share (Samba denies it by name), and the open
/// connections of the shares a disabled or deleted account could use end.
pub struct SharingFollowsAccounts {
    sharing: SharingUseCases,
}

impl SharingFollowsAccounts {
    pub fn new(sharing: SharingUseCases) -> Self {
        Self { sharing }
    }
}

fn change(change: AccessChange) -> AccountChange {
    match change {
        AccessChange::Disabled => AccountChange::Disabled,
        AccessChange::Enabled => AccountChange::Enabled,
        AccessChange::Deleted => AccountChange::Deleted,
    }
}

#[async_trait]
impl AccountObserver for SharingFollowsAccounts {
    async fn access_changing(&self, account: AccountId, to: AccessChange) -> Result<Vec<String>, PortError> {
        let reached = self.sharing.account_changing(account.get(), change(to)).await.map_err(failed)?;
        Ok(reached.into_iter().map(|share| share.to_string()).collect())
    }

    async fn access_changed(
        &self,
        account: AccountId,
        to: AccessChange,
        noted: &[String],
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.sharing.account_changed(account.get(), change(to), &shares(noted), context).await.map_err(failed)
    }
}

#[cfg(test)]
mod tests;
