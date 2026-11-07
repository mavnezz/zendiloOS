//! What this context needs from the outside world.

use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::domain::{
    Access, AppAccess, Favourite, FileLocation, FilePath, FileServices, Frequency, Home, HomesLocation,
    Listing, NewShare, Permission, Principal, Share, ShareId, ShareName, ShareSettings, ShareUsage,
    SharingConfig, Snapshot, SnapshotSchedule,
};

#[async_trait]
pub trait ShareRepository: Send + Sync {
    /// All shares, ordered by name.
    async fn list(&self) -> Result<Vec<Share>, PortError>;

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError>;

    /// `None` if the name is taken.
    async fn create(&self, share: NewShare) -> Result<Option<Share>, PortError>;

    /// Stores description, recycle bin, access, NFS clients and
    /// `updated_at`.
    async fn update(&self, share: &Share) -> Result<(), PortError>;

    async fn delete(&self, id: ShareId) -> Result<(), PortError>;

    /// Hides a share while its data goes (`true`), or shows it again. A
    /// hidden share is left out of `list` and `find`; its name stays taken.
    async fn set_deleting(&self, id: ShareId, deleting: bool) -> Result<(), PortError>;

    /// Shares hidden for deletion whose data has not gone yet.
    async fn deleting(&self) -> Result<Vec<Share>, PortError>;

    /// Remembers an upload under way, its hidden file at `at` written as
    /// `user`, so that what one left when zendiloosd stopped in the middle
    /// is removed when it starts again. Returns the record's id.
    async fn add_upload(&self, user: &str, at: &FileLocation, started: Timestamp) -> Result<i64, PortError>;

    async fn remove_upload(&self, id: i64) -> Result<(), PortError>;

    /// The uploads remembered that began before `before`.
    async fn uploads_before(&self, before: Timestamp) -> Result<Vec<PendingUpload>, PortError>;

    /// The share's snapshot schedules, ordered by frequency.
    async fn snapshot_schedules(&self, id: ShareId) -> Result<Vec<SnapshotSchedule>, PortError>;

    /// Replaces the share's snapshot schedules.
    async fn set_snapshot_schedules(
        &self,
        id: ShareId,
        schedules: &[SnapshotSchedule],
    ) -> Result<(), PortError>;

    /// The account's favourite folders, in its order.
    /// What the accounts of apps may do, on all shares.
    async fn app_access(&self) -> Result<Vec<AppAccess>, PortError>;

    /// Replaces what `account` (an app's) may do: exactly `access`.
    async fn set_app_access(&self, account: &str, access: &[(ShareId, Access)]) -> Result<(), PortError>;

    async fn favourites(&self, account: i64) -> Result<Vec<Favourite>, PortError>;

    /// The file services as set; the defaults until someone sets them.
    async fn file_services(&self) -> Result<FileServices, PortError>;

    async fn set_file_services(&self, services: &FileServices) -> Result<(), PortError>;

    /// Where the personal folders are; `None` while they are off.
    async fn homes_location(&self) -> Result<Option<HomesLocation>, PortError>;

    async fn set_homes_location(&self, location: Option<&HomesLocation>) -> Result<(), PortError>;

    /// The accounts with a personal folder.
    async fn homes(&self) -> Result<Vec<Home>, PortError>;

    /// Gives `home.account` a personal folder, or changes its quota.
    async fn set_home(&self, home: &Home) -> Result<(), PortError>;

    /// The account keeps its folder on disk but no longer reaches it.
    async fn remove_home(&self, account: i64) -> Result<(), PortError>;

    /// Replaces the account's favourites.
    async fn set_favourites(&self, account: i64, favourites: &[Favourite]) -> Result<(), PortError>;
}

/// Names of accounts and groups by ID, from the Identity context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub accounts: HashMap<i64, String>,
    pub groups: HashMap<i64, String>,
    /// The accounts in each group, by ID.
    pub members: HashMap<i64, Vec<i64>>,
}

/// An upload remembered by [`ShareRepository::add_upload`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpload {
    pub id: i64,
    pub user: String,
    pub at: FileLocation,
}

#[async_trait]
pub trait Directory: Send + Sync {
    async fn names(&self) -> Result<Names, PortError>;

    /// IDs of the groups `account` belongs to.
    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError>;
}

/// A volume shares can live on: a pool zendiloos created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub pool: String,
    pub number: u16,
    pub mounted: bool,
}

/// The volumes, from the Storage context.
#[async_trait]
pub trait Volumes: Send + Sync {
    async fn volumes(&self, context: &RequestContext) -> Result<Vec<Volume>, PortError>;
}

/// A removable drive mounted at its place (the Storage context's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovableRoot {
    pub id: String,
    pub label: Option<String>,
    /// Write-protected, or mounted read only.
    pub read_only: bool,
}

/// The removable drives the file manager may work on: those mounted now.
#[async_trait]
pub trait RemovableRoots: Send + Sync {
    /// The drive `id` if it is mounted now.
    async fn mounted(&self, id: &str, context: &RequestContext) -> Result<Option<RemovableRoot>, PortError>;
}

/// Administrators' Compose projects (the Containers context's) that live in
/// a share or mount something of it.
#[async_trait]
pub trait ShareProjects: Send + Sync {
    /// Their names, for the share `share` at `path` (`/volume<N>/<name>`).
    async fn using(&self, share: ShareId, path: &str) -> Result<Vec<String>, PortError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// A safety check on the host refused.
    #[error("refused: {0}")]
    Refused(String),
    #[error("it exists already")]
    Conflict,
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Datasets, server configuration and directory permissions; needs the
/// privileged agents.
#[async_trait]
pub trait ShareHost: Send + Sync {
    async fn create_dataset(
        &self,
        pool: &str,
        volume: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    async fn destroy_dataset(
        &self,
        pool: &str,
        volume: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// Replaces what the SMB and NFS servers offer.
    async fn apply(&self, config: &SharingConfig, context: &RequestContext) -> Result<(), HostError>;

    /// The personal folders' dataset on `pool`, mounted at
    /// `/volume<volume>/@homes`.
    async fn prepare_homes(&self, pool: &str, volume: u16, context: &RequestContext)
    -> Result<(), HostError>;

    /// `user`'s personal folder, made if missing, and how much it may hold.
    async fn set_home(
        &self,
        pool: &str,
        volume: u16,
        user: &str,
        quota_bytes: Option<u64>,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// Quota, compression and record size of the share's dataset.
    async fn set_properties(
        &self,
        pool: &str,
        volume: u16,
        name: &ShareName,
        settings: &ShareSettings,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// What the share datasets hold and may still take, on every pool.
    async fn usage(&self, context: &RequestContext) -> Result<Vec<ShareUsage>, HostError>;

    /// The snapshots of share datasets, on every pool.
    async fn snapshots(&self, context: &RequestContext) -> Result<Vec<Snapshot>, HostError>;

    /// Takes snapshot `<pool>/<share>@<label>` for `schedules` (none: by
    /// hand), with an optional note.
    #[allow(clippy::too_many_arguments)]
    async fn create_snapshot(
        &self,
        pool: &str,
        volume: u16,
        share: &ShareName,
        label: &str,
        schedules: &[Frequency],
        note: Option<&str>,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    async fn destroy_snapshot(
        &self,
        pool: &str,
        volume: u16,
        share: &ShareName,
        label: &str,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// Returns the share to the snapshot; later snapshots are destroyed.
    async fn rollback_snapshot(
        &self,
        pool: &str,
        volume: u16,
        share: &ShareName,
        label: &str,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// Permissions of the share's directory; with `recursive` also of its
    /// contents.
    async fn set_access(
        &self,
        volume: u16,
        name: &ShareName,
        access: &[(Principal, Permission)],
        removed: &[Principal],
        recursive: bool,
        context: &RequestContext,
    ) -> Result<(), HostError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FileHostError {
    #[error("no such file or folder")]
    NotFound,
    /// The file system refused the user.
    #[error("permission denied")]
    Denied,
    #[error("the name is taken")]
    Exists,
    /// Not possible, e.g. moving a folder into itself.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Share contents, handled on the host with the credentials of `user`, the
/// Linux name of an account: the file system decides what is allowed.
#[async_trait]
pub trait FileHost: Send + Sync {
    async fn list(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<Listing, FileHostError>;

    /// A regular file, opened for reading, and its size.
    async fn open(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError>;

    /// A new hidden file in the folder `at`, opened for writing, and its name.
    async fn create_upload(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, String), FileHostError>;

    async fn make_directory(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(), FileHostError>;

    /// Renames or moves, also into another share. With `replace`, a file
    /// replaces a file.
    async fn move_entry(
        &self,
        user: &str,
        from: &FileLocation,
        to: &FileLocation,
        replace: bool,
        context: &RequestContext,
    ) -> Result<(), FileHostError>;

    /// Copies; next to `to` under a free name if `to` exists. Returns where
    /// the copy went.
    async fn copy_entry(
        &self,
        user: &str,
        from: &FileLocation,
        to: &FileLocation,
        context: &RequestContext,
    ) -> Result<FilePath, FileHostError>;

    /// Into the user's recycle bin, or for good if not `recycle`.
    async fn remove(
        &self,
        user: &str,
        at: &FileLocation,
        recycle: bool,
        context: &RequestContext,
    ) -> Result<(), FileHostError>;

    /// Removes an upload's hidden file that was given up; never a folder.
    async fn discard_upload(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(), FileHostError>;

    /// The folder `at` as a tar archive being written, and the archive's
    /// exact length; it ends short only if writing fails midway.
    async fn archive(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError>;

    /// Out of the user's recycle bin, back to where it was deleted from or
    /// beside it. Returns where it went.
    async fn restore(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<FilePath, FileHostError>;
}
