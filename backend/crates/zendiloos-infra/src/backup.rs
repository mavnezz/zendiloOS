//! The Backup context's view of other contexts, through their use cases:
//! shares (where they are, who may read them) from Sharing, removable
//! drives from Storage.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_backup::application::{Drive, Drives, Readers, ShareSpot, Shares};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_sharing::application::SharingUseCases;
use zendiloos_storage::application::removable::RemovableDrives;
use zendiloos_storage::domain::RemovableFilesystem;

/// [`Shares`] through the Sharing context.
#[derive(Clone)]
pub struct SharesForBackupTasks {
    sharing: SharingUseCases,
}

impl SharesForBackupTasks {
    pub fn new(sharing: SharingUseCases) -> Self {
        Self { sharing }
    }
}

#[async_trait]
impl Shares for SharesForBackupTasks {
    async fn locate(&self, share_id: i64) -> Result<Option<ShareSpot>, PortError> {
        let shares = self.sharing.list().await.map_err(|e| PortError::new("shares", e.to_string()))?;
        Ok(shares.into_iter().find(|share| share.id.get() == share_id).map(|share| ShareSpot {
            id: share_id,
            name: share.name.to_string(),
            volume: share.volume,
        }))
    }

    async fn readers(&self, share_id: i64) -> Result<Option<Readers>, PortError> {
        let readers =
            self.sharing.readers(share_id).await.map_err(|e| PortError::new("shares", e.to_string()))?;
        Ok(readers.map(|readers| Readers {
            accounts: readers.accounts,
            groups: readers.groups,
            nfs: readers.nfs,
        }))
    }
}

/// [`Drives`] through the Storage context: the drives plugged in, those
/// that can be mounted mounted (as the explorer has them).
#[derive(Clone)]
pub struct DrivesForBackup {
    drives: Arc<RemovableDrives>,
}

impl DrivesForBackup {
    pub fn new(drives: Arc<RemovableDrives>) -> Self {
        Self { drives }
    }
}

#[async_trait]
impl Drives for DrivesForBackup {
    async fn drives(&self, context: &RequestContext) -> Result<Vec<Drive>, PortError> {
        let listed = self.drives.list(context).await.map_err(|error| PortError::new("storage", error))?;
        Ok(listed
            .into_iter()
            .map(|listed| {
                let drive = listed.drive;
                Drive {
                    mounted: drive.is_mounted(),
                    id: drive.id,
                    label: drive.label,
                    filesystem: match drive.filesystem {
                        RemovableFilesystem::Fat => "vfat",
                        RemovableFilesystem::Exfat => "exfat",
                        RemovableFilesystem::Ntfs => "ntfs",
                        RemovableFilesystem::Ext4 => "ext4",
                    }
                    .to_owned(),
                    read_only: drive.read_only,
                    plugged_in: drive.plugged_in,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_jobs::Jobs;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_storage::application::StorageUseCases;
    use zendiloos_storage::testing::{FakeStorage, STICK_ID};

    use super::*;

    struct NoAudit;

    #[async_trait]
    impl zendiloos_kernel::AuditLog for NoAudit {
        async fn record(&self, _: zendiloos_kernel::AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn drives_are_named_with_their_file_system_and_plugging_in() {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let (ports, fake) = FakeStorage::ports(jobs, Arc::new(NoAudit), Arc::new(FixedClock));
        let storage = StorageUseCases::new(ports);
        let drives = DrivesForBackup::new(Arc::clone(&storage.removable));
        let context = RequestContext::default();
        assert!(drives.drives(&context).await.unwrap().is_empty());
        fake.plug_stick();
        let listed = drives.drives(&context).await.unwrap();
        assert_eq!(listed.len(), 1);
        let stick = &listed[0];
        assert_eq!((stick.id.as_str(), stick.filesystem.as_str()), (STICK_ID, "vfat"));
        assert!(stick.mounted, "listing mounts it");
        assert!(!stick.links(), "FAT has no hard links");
    }
}
