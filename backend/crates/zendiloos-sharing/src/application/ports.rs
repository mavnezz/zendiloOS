//! What this context needs from the outside world.

use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{
    Access, Favourite, FileLocation, FilePath, Frequency, Listing, NewShare, Principal, Share, ShareId,
    ShareName, ShareSettings, ShareUsage, SharingConfig, Snapshot, SnapshotSchedule,
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

    /// The share's snapshot schedules, ordered by frequency.
    async fn snapshot_schedules(&self, id: ShareId) -> Result<Vec<SnapshotSchedule>, PortError>;

    /// Replaces the share's snapshot schedules.
    async fn set_snapshot_schedules(
        &self,
        id: ShareId,
        schedules: &[SnapshotSchedule],
    ) -> Result<(), PortError>;

    /// The account's favourite folders, in its order.
    async fn favourites(&self, account: i64) -> Result<Vec<Favourite>, PortError>;

    /// Replaces the account's favourites.
    async fn set_favourites(&self, account: i64, favourites: &[Favourite]) -> Result<(), PortError>;
}

/// Names of accounts and groups by ID, from the Identity context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub accounts: HashMap<i64, String>,
    pub groups: HashMap<i64, String>,
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
        access: &[(Principal, Access)],
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
