//! Adapters of the Containers context besides the agent's: where projects'
//! Compose files live, in shares, and what registries publish.

mod registry;

pub use registry::HttpsRegistries;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use zendiloos_containers::application::ports::{ProjectPlace, ProjectStorage, StorageError};
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::{FilesError, FilesUseCases};
use zendiloos_sharing::domain::{Access, FilePath, ShareId};

/// Projects' folders and files in shares, through the file manager's use
/// cases: with the acting administrator's permissions, as the file manager
/// and SMB have them.
#[derive(Clone)]
pub struct SharesForProjects {
    files: FilesUseCases,
}

impl SharesForProjects {
    pub fn new(files: FilesUseCases) -> Self {
        Self { files }
    }

    async fn writable(&self, actor: &Actor, share_id: i64) -> Result<(u16, String), StorageError> {
        let shares = self.files.shares(actor).await.map_err(storage_error)?;
        shares
            .into_iter()
            .find(|usable| usable.share.id.get() == share_id && usable.access == Access::ReadWrite)
            .map(|usable| (usable.share.volume, usable.share.name.to_string()))
            .ok_or(StorageError::Denied)
    }
}

fn storage_error(error: FilesError) -> StorageError {
    match error {
        FilesError::ShareNotFound | FilesError::Denied => StorageError::Denied,
        FilesError::NotFound => StorageError::NotFound,
        FilesError::Port(error) => StorageError::Port(error),
        other => StorageError::Invalid(other.to_string()),
    }
}

fn path(text: &str) -> Result<FilePath, StorageError> {
    FilePath::parse(text).map_err(|e| StorageError::Invalid(e.to_string()))
}

#[async_trait]
impl ProjectStorage for SharesForProjects {
    async fn places(&self, actor: &Actor) -> Result<Vec<ProjectPlace>, StorageError> {
        let shares = self.files.shares(actor).await.map_err(storage_error)?;
        Ok(shares
            .into_iter()
            .filter(|usable| usable.access == Access::ReadWrite)
            .map(|usable| ProjectPlace {
                share_id: usable.share.id.get(),
                share: usable.share.name.to_string(),
            })
            .collect())
    }

    async fn directory(&self, actor: &Actor, share_id: i64, folder: &str) -> Result<String, StorageError> {
        let (volume, share) = self.writable(actor, share_id).await?;
        let folder = path(folder)?;
        Ok(format!("/volume{volume}/{share}/{}", folder.as_str()))
    }

    async fn write(
        &self,
        actor: &Actor,
        share_id: i64,
        folder: &str,
        name: &str,
        content: &str,
        context: &RequestContext,
    ) -> Result<(), StorageError> {
        self.writable(actor, share_id).await?;
        let id = ShareId::new(share_id);
        let folder = path(folder)?;
        // Each folder on the way, as far as it is missing.
        let mut made = FilePath::root();
        for part in folder.as_str().split('/') {
            made = made.join(part).map_err(|e| StorageError::Invalid(e.to_string()))?;
            match self.files.make_directory(actor, id, made.clone(), context).await {
                Ok(()) | Err(FilesError::Exists) => {}
                Err(error) => return Err(storage_error(error)),
            }
        }
        let upload =
            self.files.begin_upload(actor, id, folder, name, true, context).await.map_err(storage_error)?;
        let mut file = upload.file;
        let written = async {
            file.write_all(content.as_bytes()).await?;
            file.sync_all().await
        }
        .await;
        if let Err(error) = written {
            self.files.abort_upload(actor, upload.ticket, context).await;
            return Err(StorageError::Port(PortError::new("project file", error)));
        }
        self.files.finish_upload(actor, upload.ticket, context).await.map_err(storage_error)?;
        Ok(())
    }

    async fn read(
        &self,
        actor: &Actor,
        share_id: i64,
        file: &str,
        context: &RequestContext,
    ) -> Result<String, StorageError> {
        use tokio::io::AsyncReadExt;
        let (mut opened, size) = self
            .files
            .open(actor, ShareId::new(share_id), path(file)?, None, context)
            .await
            .map_err(storage_error)?;
        if size > 256 * 1024 {
            return Err(StorageError::Invalid("the Compose file is larger than 256 KiB".into()));
        }
        let mut text = String::new();
        opened
            .read_to_string(&mut text)
            .await
            .map_err(|e| StorageError::Port(PortError::new("project file", e)))?;
        Ok(text)
    }
}
