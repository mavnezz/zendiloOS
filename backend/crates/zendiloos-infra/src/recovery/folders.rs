//! The folder scheduled configuration backups go into: a folder of a share
//! or of a removable drive, through the files agent with the access of the
//! administrator who set the plan up, as the file manager writes there.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_recovery::application::BackupFolders;
use zendiloos_recovery::domain::Target;
use zendiloos_sharing::application::ports::{FileHost, FileHostError};
use zendiloos_sharing::domain::{EntryKind, FileLocation, FilePath, FileRoot, ShareName, is_drive_id};

use crate::sqlite::Database;

const WHAT: &str = "configuration backup's folder";

fn port(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> PortError {
    PortError::new(WHAT, error)
}

fn files(error: FileHostError) -> PortError {
    match error {
        FileHostError::Port(error) => error,
        other => port(other.to_string()),
    }
}

/// [`BackupFolders`] through a [`FileHost`] (the files agent's), the share
/// found by its record.
pub struct AgentBackupFolders {
    host: Arc<dyn FileHost>,
    db: Database,
}

impl AgentBackupFolders {
    pub fn new(host: Arc<dyn FileHost>, db: Database) -> Self {
        Self { host, db }
    }

    /// The target's folder, and `name` in it.
    async fn location(&self, target: &Target, name: Option<&str>) -> Result<FileLocation, PortError> {
        let (volume, root, folder) = match target {
            Target::Share { share_id, folder } => {
                let row: Option<(String, i64)> =
                    sqlx::query_as("SELECT name, volume FROM shares WHERE id = ?")
                        .bind(share_id)
                        .fetch_optional(self.db.pool())
                        .await
                        .map_err(port)?;
                let (share, volume) = row.ok_or_else(|| port("the share is gone"))?;
                let share = ShareName::parse(&share).map_err(|e| port(e.to_string()))?;
                (u16::try_from(volume).map_err(port)?, FileRoot::Share(share), folder)
            }
            Target::Drive { drive, folder } => {
                if !is_drive_id(drive) {
                    return Err(port("not a drive"));
                }
                (1, FileRoot::Removable(drive.clone()), folder)
            }
        };
        let mut path = FilePath::parse(folder).map_err(|e| port(e.to_string()))?;
        if let Some(name) = name {
            path = path.join(name).map_err(|e| port(e.to_string()))?;
        }
        Ok(FileLocation { volume, root, path, snapshot: None })
    }

    /// The folder and those above it, made where missing.
    async fn make_folder(
        &self,
        target: &Target,
        user: &str,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let folder = self.location(target, None).await?;
        let mut made = FilePath::root();
        for name in folder.path.as_str().split('/').filter(|name| !name.is_empty()) {
            made = made.join(name).map_err(|e| port(e.to_string()))?;
            let at = FileLocation { path: made.clone(), ..folder.clone() };
            match self.host.make_directory(user, &at, false, context).await {
                Ok(()) | Err(FileHostError::Exists) => {}
                Err(error) => return Err(files(error)),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl BackupFolders for AgentBackupFolders {
    async fn write(
        &self,
        target: &Target,
        user: &str,
        name: &str,
        bytes: Vec<u8>,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.make_folder(target, user, context).await?;
        let folder = self.location(target, None).await?;
        let (mut file, upload) = self.host.create_upload(user, &folder, context).await.map_err(files)?;
        let upload_at = self.location(target, Some(&upload)).await?;
        let written = async {
            file.write_all(&bytes).await?;
            file.sync_all().await
        }
        .await;
        drop(file);
        if let Err(error) = written {
            let _ = self.host.discard_upload(user, &upload_at, context).await;
            return Err(port(error));
        }
        let final_at = self.location(target, Some(name)).await?;
        if let Err(error) = self.host.move_entry(user, &upload_at, &final_at, false, context).await {
            let _ = self.host.discard_upload(user, &upload_at, context).await;
            return Err(files(error));
        }
        Ok(())
    }

    async fn names(
        &self,
        target: &Target,
        user: &str,
        context: &RequestContext,
    ) -> Result<Vec<String>, PortError> {
        let folder = self.location(target, None).await?;
        let listing = self.host.list(user, &folder, context).await.map_err(files)?;
        Ok(listing
            .entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::File && !entry.unreadable)
            .map(|entry| entry.name)
            .collect())
    }

    async fn remove(
        &self,
        target: &Target,
        user: &str,
        name: &str,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let at = self.location(target, Some(name)).await?;
        self.host.remove(user, &at, false, context).await.map_err(files)
    }
}
