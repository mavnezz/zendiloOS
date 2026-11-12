//! The Sharing context's view of other contexts, through their use cases.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_sharing::application::SharingUseCases;
use zendiloos_sharing::application::ports::{RemovableRoot, RemovableRoots};
use zendiloos_storage::application::ports::VolumeObserver;
use zendiloos_storage::application::removable::RemovableDrives;
use zendiloos_storage::domain::VolumeNumber;

/// Storage's [`VolumeObserver`] through the Sharing context: once a
/// volume's mounts came back, Samba's and NFS's configuration is written
/// again, so NFS exports the datasets mounted now rather than the folders
/// beneath them. Sharing is made after Storage, so it is handed over once
/// it is there ([`SharingFollowsVolumes::follow`]); before, nothing follows.
#[derive(Clone, Default)]
pub struct SharingFollowsVolumes {
    sharing: Arc<OnceLock<SharingUseCases>>,
}

impl SharingFollowsVolumes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn follow(&self, sharing: SharingUseCases) {
        let _ = self.sharing.set(sharing);
    }
}

#[async_trait]
impl VolumeObserver for SharingFollowsVolumes {
    async fn volume_back(&self, volume: VolumeNumber, context: &RequestContext) {
        let Some(sharing) = self.sharing.get() else { return };
        if let Err(error) = sharing.apply(context).await {
            tracing::warn!(volume = volume.get(), %error, "sharing: cannot follow a volume mounted again");
        }
    }
}

/// [`RemovableRoots`] through the Storage context: a drive is there for
/// the file manager while the disks say it is mounted at its place.
#[derive(Clone)]
pub struct DrivesForFiles {
    drives: Arc<RemovableDrives>,
}

impl DrivesForFiles {
    pub fn new(drives: Arc<RemovableDrives>) -> Self {
        Self { drives }
    }
}

#[async_trait]
impl RemovableRoots for DrivesForFiles {
    async fn mounted(&self, id: &str, context: &RequestContext) -> Result<Option<RemovableRoot>, PortError> {
        let drive =
            self.drives.mounted(id, context).await.map_err(|error| PortError::new("storage", error))?;
        Ok(drive.map(|drive| RemovableRoot { id: drive.id, label: drive.label, read_only: drive.read_only }))
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
    async fn a_drive_is_there_for_files_while_mounted() {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let (ports, fake) = FakeStorage::ports(jobs, Arc::new(NoAudit), Arc::new(FixedClock));
        let storage = StorageUseCases::new(ports);
        let drives = DrivesForFiles::new(Arc::clone(&storage.removable));
        let context = RequestContext::default();
        assert_eq!(drives.mounted(STICK_ID, &context).await.unwrap(), None);
        fake.plug_stick();
        assert_eq!(drives.mounted(STICK_ID, &context).await.unwrap(), None, "plugged in, not mounted yet");
        storage.removable.list(&context).await.unwrap();

        let drive = drives.mounted(STICK_ID, &context).await.unwrap().unwrap();

        assert_eq!(
            drive,
            RemovableRoot { id: STICK_ID.into(), label: Some("STICK".into()), read_only: false }
        );
    }
}
