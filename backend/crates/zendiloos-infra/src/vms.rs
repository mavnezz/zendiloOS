//! Adapters of the Virtual machines context besides the agent's: the ISOs
//! administrators pick in shares, read as they may read them; and, for
//! the Network context, the VMs on a bridge it would remove.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_network::application::DeviceUsers;
use zendiloos_sharing::application::{FilesError, FilesUseCases};
use zendiloos_sharing::domain::{FilePath, ShareId};
use zendiloos_vms::application::{IsoError, IsoFiles, IsoSource, VmRepository};
use zendiloos_vms::domain::IsoChoice;

/// [`DeviceUsers`] from the VMs' records: a bridge is in use while a VM,
/// running or not, is on it, also one still being made.
pub struct VmsOnBridges {
    vms: Arc<dyn VmRepository>,
}

impl VmsOnBridges {
    pub fn new(vms: Arc<dyn VmRepository>) -> Self {
        Self { vms }
    }
}

#[async_trait]
impl DeviceUsers for VmsOnBridges {
    async fn users(&self, device: &str) -> Result<Vec<String>, PortError> {
        let vms = self.vms.list().await?;
        Ok(vms
            .into_iter()
            .filter(|vm| vm.settings.bridges().any(|bridge| bridge == device))
            .map(|vm| vm.settings.name)
            .collect())
    }
}

/// ISOs in shares, through the file manager's use cases: the file is
/// opened with the acting administrator's permissions, as SMB would, so a
/// share or a file they may not read is never copied for a VM.
#[derive(Clone)]
pub struct SharesForVms {
    files: FilesUseCases,
}

impl SharesForVms {
    pub fn new(files: FilesUseCases) -> Self {
        Self { files }
    }
}

fn iso_error(error: FilesError) -> IsoError {
    match error {
        FilesError::ShareNotFound | FilesError::Denied => IsoError::Denied,
        FilesError::NotFound => IsoError::NotFound,
        FilesError::Port(error) => IsoError::Port(error),
        other => IsoError::Invalid(other.to_string()),
    }
}

#[async_trait]
impl IsoFiles for SharesForVms {
    async fn locate(
        &self,
        actor: &Actor,
        iso: &IsoChoice,
        context: &RequestContext,
    ) -> Result<IsoSource, IsoError> {
        let shares = self.files.shares(actor).await.map_err(iso_error)?;
        let share = shares
            .into_iter()
            .find(|usable| usable.share.id.get() == iso.share_id)
            .ok_or(IsoError::Denied)?
            .share;
        let path =
            FilePath::parse(iso.path.trim_matches('/')).map_err(|e| IsoError::Invalid(e.to_string()))?;
        // Opened as the administrator, and closed again: the copy reads it
        // in a unit of its own.
        let (_file, _size) = self
            .files
            .open(actor, ShareId::new(iso.share_id), path.clone(), None, context)
            .await
            .map_err(iso_error)?;
        Ok(IsoSource { share: share.name.to_string(), volume: share.volume, path: path.as_str().to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_vms::domain::{Network, OsPreset, Vm, VmSettings};
    use zendiloos_vms::testing::MemoryVms;

    use super::*;

    #[tokio::test]
    async fn a_bridge_is_used_by_the_vms_on_it() {
        let vms = Arc::new(MemoryVms::default());
        let vm = |id: &str, name: &str, network: Network| Vm {
            id: id.into(),
            settings: {
                let mut settings = VmSettings::preset(name, OsPreset::Linux);
                settings.nics[0].network = network;
                settings
            },
            pool: "tank".into(),
            volume: 1,
            created_at: zendiloos_kernel::Timestamp::from_unix_seconds(0),
            ready: true,
        };
        vms.create(&vm("0a1b2c3d4e5f", "web", Network::Bridge("br0".into()))).await.unwrap();
        vms.create(&vm("0a1b2c3d4e60", "mail", Network::User)).await.unwrap();
        vms.create(&vm("0a1b2c3d4e61", "lab", Network::Bridge("br1".into()))).await.unwrap();
        let users = VmsOnBridges::new(vms);
        assert_eq!(users.users("br0").await.unwrap(), ["web"]);
        assert!(users.users("br9").await.unwrap().is_empty());
    }
}
