//! Adapters of the Virtual machines context besides the agent's: the ISOs
//! administrators pick in shares, read as they may read them.

use async_trait::async_trait;
use zendiloos_kernel::{Actor, RequestContext};
use zendiloos_sharing::application::{FilesError, FilesUseCases};
use zendiloos_sharing::domain::{FilePath, ShareId};
use zendiloos_vms::application::{IsoError, IsoFiles, IsoSource};
use zendiloos_vms::domain::IsoChoice;

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
