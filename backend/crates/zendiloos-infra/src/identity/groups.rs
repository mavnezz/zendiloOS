use async_trait::async_trait;
use zendiloos_identity::application::ports::{AccessChange, AccountObserver, GroupObserver};
use zendiloos_identity::domain::{AccountId, GroupId};
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::{AccountChange, SharingUseCases};
use zendiloos_sharing::domain::ShareName;

/// [`GroupObserver`] for the Sharing context: members of a group denied a
/// share carry ACL entries of their own, which follow the group, and what
/// those who leave a group that writes own passes to root. What follows
/// is the shares, by name, whose files are walked in the background.
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

fn named(shares: Vec<ShareName>) -> Vec<String> {
    shares.into_iter().map(|share| share.to_string()).collect()
}

#[async_trait]
impl GroupObserver for SharingFollowsGroups {
    async fn members_changing(&self, group: GroupId, after: &[AccountId]) -> Result<Vec<String>, PortError> {
        Ok(named(self.sharing.group_members_changing(group.get(), &ids(after)).await.map_err(failed)?))
    }

    async fn members_changed(
        &self,
        group: GroupId,
        before: &[AccountId],
        following: &[String],
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<String>, PortError> {
        let owing = shares(following);
        let following = self
            .sharing
            .group_members_changed(group.get(), &ids(before), &owing, actor, context)
            .await
            .map_err(failed)?;
        Ok(named(following))
    }

    async fn deleting(
        &self,
        group: GroupId,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.sharing.group_deleting(group.get(), actor, context).await.map_err(failed)?;
        Ok(())
    }
}

/// [`AccountObserver`] for the Sharing context: a disabled account may do
/// nothing on any share, as if denied, and what it owns passes to root;
/// a deleted one's ID loses its entries and its files. Whatever follows
/// is named as for [`SharingFollowsGroups`].
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
        Ok(named(self.sharing.account_changing(account.get(), change(to)).await.map_err(failed)?))
    }

    async fn access_changed(
        &self,
        account: AccountId,
        to: AccessChange,
        following: &[String],
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<String>, PortError> {
        let owing = shares(following);
        let following = self
            .sharing
            .account_changed(account.get(), change(to), &owing, actor, context)
            .await
            .map_err(failed)?;
        Ok(named(following))
    }
}

#[cfg(test)]
mod tests;
