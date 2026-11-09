//! What this context needs from the outside world.

use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::domain::{
    Access, AppAccess, EntryFacts, Favourite, FileLocation, FilePath, FileServices, Found, Frequency, Home,
    HomesLocation, Listing, NewShare, Share, ShareId, ShareName, ShareSettings, ShareUsage, SharingConfig,
    Snapshot, SnapshotSchedule,
};

#[async_trait]
pub trait ShareRepository: Send + Sync {
    /// All shares, ordered by name.
    async fn list(&self) -> Result<Vec<Share>, PortError>;

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError>;

    /// `None` if the name is taken. A new share's files have share-level
    /// access from the start: no walk is owed (see
    /// [`Self::migrations_owed`]).
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

    /// What the accounts of apps installed the earlier way were given, on
    /// all shares.
    async fn app_access(&self) -> Result<Vec<AppAccess>, PortError>;

    /// Replaces what `account` (an app's) may do: exactly `access`.
    async fn set_app_access(&self, account: &str, access: &[(ShareId, Access)]) -> Result<(), PortError>;

    /// The account's favourite folders, in its order.
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

    /// The shares, not being deleted, whose files still wait for the walk
    /// that gives them share-level access (docs/architecture.md, 4.4): made
    /// before it came, or rolled back to a snapshot from before it, by name.
    async fn migrations_owed(&self) -> Result<Vec<ShareId>, PortError>;

    /// When the share's files were given share-level access; `None` while
    /// the walk is owed.
    async fn migrated_at(&self, share: ShareId) -> Result<Option<Timestamp>, PortError>;

    /// Records the share's files as given share-level access `at`, or as
    /// owing the walk again (`None`).
    async fn set_migrated(&self, share: ShareId, at: Option<Timestamp>) -> Result<(), PortError>;
}

/// Names of accounts and groups by ID, from the Identity context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub accounts: HashMap<i64, String>,
    pub groups: HashMap<i64, String>,
    /// The accounts in each group, by ID.
    pub members: HashMap<i64, Vec<i64>>,
    /// Disabled accounts: on the shares they may do nothing, as if denied.
    pub disabled: std::collections::HashSet<i64>,
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

/// Datasets, server configuration and the shares' folders; needs the
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

    /// Replaces what the SMB and NFS servers offer; then the open SMB
    /// connections of `close` end, so that clients connect again under the
    /// new rules.
    async fn apply(
        &self,
        config: &SharingConfig,
        close: &[ShareName],
        context: &RequestContext,
    ) -> Result<(), HostError>;

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

    /// The share's folder as every share has it: owner root, group
    /// `zendiloos-files`, mode 2770 and that group's one ACL, inherited by
    /// what is made in it; and the recycle bins' folder.
    async fn prepare_share(
        &self,
        volume: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError>;

    /// The walk to share-level access, once per share: the folder as
    /// [`Self::prepare_share`] makes it, then every entry given the group's
    /// ACL entries and rid of the named entries of people and their groups;
    /// apps' entries and owners stay. Tells `lines` how far it got.
    async fn migrate_share(
        &self,
        volume: u16,
        name: &ShareName,
        lines: &(dyn for<'line> Fn(&'line str) + Send + Sync),
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
    /// A move across file systems removed only part of its source: `kept`
    /// entries stayed where they were, such as `example`, because they
    /// changed or came since they were copied, were never copied, or could
    /// not be removed; what of them was copied is in the target too. With
    /// `too_many`, the copy is complete and the source stayed whole: more
    /// entries than a move checks one by one.
    #[error("moved in part: {kept} entries stayed where they were")]
    PartlyMoved { kept: u64, example: Option<FilePath>, too_many: bool },
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Contents of shares, personal folders and removable drives, handled on
/// the host for `user`, the Linux name of an account. In a personal folder
/// as that user, so the file system decides; on a share with root's access
/// to files, confined beneath the share's root, after the use cases
/// checked the share's rules; what is made in a share takes the folder's
/// owner and the group `zendiloos-files`. `user` names the recycle bin.
#[async_trait]
pub trait FileHost: Send + Sync {
    async fn list(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<Listing, FileHostError>;

    /// Names containing `text`, whatever the case, in the folder `at` and
    /// below it, as the user may see them; names starting with `.` only
    /// with `hidden`. Within a few seconds, a few hundred at most.
    async fn search(
        &self,
        user: &str,
        at: &FileLocation,
        text: &str,
        hidden: bool,
        context: &RequestContext,
    ) -> Result<Found, FileHostError>;

    /// What `at` is; for a folder, what is in it, counted as the user may
    /// see it, within half a minute.
    async fn facts(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<EntryFacts, FileHostError>;

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

    /// A new folder; with `own`, on a share it is `user`'s instead of its
    /// parent folder's owner's.
    async fn make_directory(
        &self,
        user: &str,
        at: &FileLocation,
        own: bool,
        context: &RequestContext,
    ) -> Result<(), FileHostError>;

    /// Renames or moves, also into another share. With `replace`, a file
    /// replaces a file. Into another place it may remove only part of its
    /// source ([`FileHostError::PartlyMoved`]).
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
