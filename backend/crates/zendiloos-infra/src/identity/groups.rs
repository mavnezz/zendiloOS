use async_trait::async_trait;
use zendiloos_identity::application::ports::GroupObserver;
use zendiloos_identity::domain::{AccountId, GroupId};
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::SharingUseCases;

/// [`GroupObserver`] for the Sharing context: members of a group denied a
/// share carry ACL entries of their own, which follow the group.
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

#[async_trait]
impl GroupObserver for SharingFollowsGroups {
    async fn members_changed(
        &self,
        group: GroupId,
        before: &[AccountId],
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let before: Vec<i64> = before.iter().map(|id| id.get()).collect();
        self.sharing.group_members_changed(group.get(), &before, actor, context).await.map_err(failed)?;
        Ok(())
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
