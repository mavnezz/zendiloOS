//! Adapters of the Containers context besides the agent's: where projects'
//! Compose files live, in shares, and what registries publish.

mod registry;

pub use registry::HttpsRegistries;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use zendiloos_containers::application::ports::{
    BackupShares, ProjectPlace, ProjectStorage, SharePlace, StorageError,
};
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::{FilesError, FilesUseCases, SharingUseCases};
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
        self.make_folder(actor, share_id, folder, context).await?;
        let id = ShareId::new(share_id);
        let folder = path(folder)?;
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

    async fn make_folder(
        &self,
        actor: &Actor,
        share_id: i64,
        folder: &str,
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
        Ok(())
    }
}

/// Where the shares backups of volumes go to are, through the Sharing
/// context: backups run on a schedule, with nobody signed in.
#[derive(Clone)]
pub struct SharesForBackups {
    sharing: SharingUseCases,
}

impl SharesForBackups {
    pub fn new(sharing: SharingUseCases) -> Self {
        Self { sharing }
    }
}

#[async_trait]
impl BackupShares for SharesForBackups {
    async fn locate(&self, share_id: i64) -> Result<Option<SharePlace>, PortError> {
        let shares = self.sharing.list().await.map_err(|e| PortError::new("shares", e.to_string()))?;
        Ok(shares
            .into_iter()
            .find(|share| share.id.get() == share_id)
            .map(|share| SharePlace { volume: share.volume, share: share.name.to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zendiloos_jobs::Jobs;
    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_kernel::{AuditEntry, AuditLog, Timestamp};
    use zendiloos_notify::Notifier;
    use zendiloos_notify::testing::MemoryNotifications;
    use zendiloos_sharing::application::SharingPorts;
    use zendiloos_sharing::application::ports::{Directory, Names, ShareRepository};
    use zendiloos_sharing::domain::{AccessRule, NewShare, Permission, ShareName, ShareSettings, Subject};
    use zendiloos_sharing::testing::{Files, Host, Projects, Shares, Volumes};

    use super::*;

    struct Alice;

    #[async_trait]
    impl Directory for Alice {
        async fn names(&self) -> Result<Names, PortError> {
            let mut names = Names::default();
            names.accounts.insert(1, "alice".into());
            Ok(names)
        }

        async fn memberships(&self, _: i64) -> Result<Vec<i64>, PortError> {
            Ok(Vec::new())
        }
    }

    struct NoAudit;

    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    /// The shares `docker` (alice writes) and `media` (alice reads).
    async fn shares() -> (SharesForProjects, Arc<Files>, i64, i64) {
        let (projects, _, files, docker, media) = both().await;
        (projects, files, docker, media)
    }

    #[tokio::test]
    async fn backups_find_their_shares_by_id() {
        let (_, backups, _, docker, _) = both().await;
        let place = backups.locate(docker).await.unwrap().unwrap();
        assert_eq!((place.volume, place.share.as_str()), (1, "docker"));
        assert_eq!(backups.locate(99).await.unwrap(), None);
    }

    async fn both() -> (SharesForProjects, SharesForBackups, Arc<Files>, i64, i64) {
        let store = Arc::new(Shares::default());
        let mut ids = Vec::new();
        for (name, permission) in [("docker", Permission::ReadWrite), ("media", Permission::Read)] {
            let new = NewShare {
                name: ShareName::parse(name).unwrap(),
                description: None,
                pool: "tank".into(),
                volume: 1,
                shared: true,
                recycle_bin: false,
                settings: ShareSettings::default(),
                access: vec![AccessRule { subject: Subject::Account(1), permission }],
                nfs: Vec::new(),
                created_at: Timestamp::from_unix_seconds(0),
            };
            ids.push(store.create(new).await.unwrap().unwrap().id.get());
        }
        let files = Arc::new(Files::default());
        let ports = SharingPorts {
            shares: store,
            directory: Arc::new(Alice),
            volumes: Arc::new(Volumes),
            projects: Arc::new(Projects::default()),
            host: Arc::new(Host::default()),
            files: files.clone(),
            removable: Arc::new(zendiloos_sharing::testing::Removable::default()),
            jobs: Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock)),
            audit: Arc::new(NoAudit),
            clock: Arc::new(FixedClock),
            notifier: Notifier::new(Arc::new(MemoryNotifications::default()), Arc::new(FixedClock)),
        };
        let backups = SharesForBackups::new(SharingUseCases::new(ports.clone()));
        (SharesForProjects::new(FilesUseCases::new(ports)), backups, files, ids[0], ids[1])
    }

    #[tokio::test]
    async fn projects_go_where_the_administrator_may_write() {
        let (projects, _, docker, media) = shares().await;
        let places = projects.places(&alice()).await.unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!((places[0].share_id, places[0].share.as_str()), (docker, "docker"));
        assert_eq!(
            projects.directory(&alice(), docker, "projects/tesla").await.unwrap(),
            "/volume1/docker/projects/tesla"
        );
        let read_only = projects.directory(&alice(), media, "tesla").await;
        assert!(matches!(read_only, Err(StorageError::Denied)), "{read_only:?}");
        let unknown = projects.directory(&alice(), 99, "tesla").await;
        assert!(matches!(unknown, Err(StorageError::Denied)), "{unknown:?}");
        let odd = projects.directory(&alice(), docker, "../tesla").await;
        assert!(matches!(odd, Err(StorageError::Invalid(_))), "{odd:?}");
    }

    #[tokio::test]
    async fn the_compose_file_is_written_and_read_with_the_administrators_rights() {
        let (projects, files, docker, media) = shares().await;
        let context = RequestContext::default();
        projects
            .write(&alice(), docker, "projects/tesla", "compose.yaml", "services: {}\n", &context)
            .await
            .unwrap();
        let calls = files.calls();
        assert!(calls.iter().any(|c| c.contains("docker:projects/tesla")), "{calls:?}");
        assert!(calls.iter().any(|c| c.starts_with("create_upload alice")), "{calls:?}");
        let refused = projects.write(&alice(), media, "tesla", "compose.yaml", "x", &context).await;
        assert!(matches!(refused, Err(StorageError::Denied)), "{refused:?}");
        let missing = projects.write(&alice(), docker, "missing/tesla", "compose.yaml", "x", &context).await;
        assert!(matches!(missing, Err(StorageError::NotFound)), "{missing:?}");

        let text = projects.read(&alice(), docker, "projects/tesla/compose.yaml", &context).await.unwrap();
        assert_eq!(text, zendiloos_sharing::testing::FILE_CONTENT);
        let gone = projects.read(&alice(), docker, "missing/compose.yaml", &context).await;
        assert!(matches!(gone, Err(StorageError::NotFound)), "{gone:?}");
        let odd = projects.read(&alice(), docker, "../x", &context).await;
        assert!(matches!(odd, Err(StorageError::Invalid(_))), "{odd:?}");
    }
}
