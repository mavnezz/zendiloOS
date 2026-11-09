//! The engine's volumes: listed with the containers that use them and how
//! they are backed up, their sizes asked for apart (counting takes a
//! while), removed when unused.

use zendiloos_kernel::{Actor, RequestContext};

use super::backups::PlanView;
use super::{Containers, ContainersError};
use crate::domain::{Backup, Volume, VolumeSize, VolumeUser, users};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeView {
    pub volume: Volume,
    /// The containers that mount it, running or not, by name.
    pub users: Vec<VolumeUser>,
    pub plan: Option<PlanView>,
    /// Its newest backup's run, whatever came of it.
    pub last_backup: Option<Backup>,
}

impl Containers {
    /// Every volume, by name.
    pub async fn volumes(&self, context: &RequestContext) -> Result<Vec<VolumeView>, ContainersError> {
        let mut volumes = self.ports.engine.volumes(context).await?;
        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        let containers = self.ports.engine.containers(context).await?;
        let plans = self.ports.backups.plans().await?;
        let backups = self.ports.backups.backups(None).await?;
        Ok(volumes
            .into_iter()
            .map(|volume| VolumeView {
                users: users(&volume.name, &containers),
                plan: plans.iter().find(|p| p.volume == volume.name).map(|plan| self.plan_view(plan.clone())),
                last_backup: backups.iter().find(|b| b.volume == volume.name).cloned(),
                volume,
            })
            .collect())
    }

    /// What the local volumes' data takes up.
    pub async fn volume_sizes(&self, context: &RequestContext) -> Result<Vec<VolumeSize>, ContainersError> {
        Ok(self.ports.engine.volume_sizes(context).await?)
    }

    /// The volume `name`, as Docker lists it.
    pub(super) async fn find_volume(
        &self,
        name: &str,
        context: &RequestContext,
    ) -> Result<Volume, ContainersError> {
        let volumes = self.ports.engine.volumes(context).await?;
        volumes.into_iter().find(|v| v.name == name).ok_or(ContainersError::NotFound)
    }

    /// Removes a volume no container uses, with its data; its backup plan
    /// goes with it, its backups stay.
    pub async fn remove_volume(
        &self,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let volume = self.find_volume(name, context).await?;
        let containers = self.ports.engine.containers(context).await?;
        let users: Vec<String> = users(&volume.name, &containers).into_iter().map(|u| u.name).collect();
        if !users.is_empty() {
            return Err(ContainersError::Refused(format!("{} is used by {}", volume.name, users.join(", "))));
        }
        let done = async {
            self.claim(&volume.name, false, false, context).await?;
            let removed =
                self.ports.engine.remove_volume(&volume.name, context).await.map_err(ContainersError::from);
            self.unclaim(&volume.name);
            removed?;
            self.ports.backups.delete_plan(&volume.name).await?;
            Ok(())
        }
        .await;
        self.audited("containers.volume_remove", &volume.name, actor, context, done).await
    }

    /// Removes every volume no container uses, named ones too; refused
    /// while a volume is backed up or restored (or waits for its turn).
    pub async fn prune_volumes(
        &self,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<String, ContainersError> {
        self.no_volume_work()?;
        let done = self.ports.engine.prune_volumes(context).await.map_err(Into::into);
        self.audited("containers.volume_prune", "unused volumes", actor, context, done).await
    }
}
