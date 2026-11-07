//! The file manager: share contents as the signed-in user.
//!
//! Every operation runs on the host with the user's own credentials, so
//! what is allowed is decided where it is decided for SMB and NFS: by the
//! file system's permissions. The checks here keep users out of shares that
//! are not theirs and refuse early what the share's rules forbid anyway.
//!
//! Removable drives are places too ([`PlaceId::Removable`]), for
//! administrators only: the caller lets only someone who manages storage
//! name one. A drive is worked on only while it is mounted, with root's
//! access to its files (a drive formatted elsewhere has owners nobody here
//! has); it has no recycle bin and no snapshots, so deleting is for good.

use std::sync::Arc;

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, PortError, RequestContext, Timestamp};

use super::SharingPorts;
use super::ports::FileHostError;
use crate::domain::{
    self, Access, Favourite, FileLocation, FilePath, FileRoot, HOME_SHARE, InvalidFilePath, Listing,
    MAX_FAVOURITES, PlaceId, Share, ShareId, ShareName, is_new_file_name,
};

/// Most entries one copy, move or delete takes.
pub const MAX_TRANSFER_ENTRIES: usize = 1000;
/// Prefix of the hidden files uploads are written to.
const UPLOAD_PREFIX: &str = ".zendiloos-upload-";

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// The share does not exist or is not shared with the user.
    #[error("there is no such share")]
    ShareNotFound,
    #[error("no such file or folder")]
    NotFound,
    #[error("permission denied")]
    Denied,
    #[error("the name is taken")]
    Exists,
    #[error("not a valid name: no control characters, no spaces at either end, no dot at the end")]
    InvalidName,
    #[error(transparent)]
    InvalidPath(#[from] InvalidFilePath),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<FileHostError> for FilesError {
    fn from(error: FileHostError) -> Self {
        match error {
            FileHostError::NotFound => FilesError::NotFound,
            FileHostError::Denied => FilesError::Denied,
            FileHostError::Exists => FilesError::Exists,
            FileHostError::Invalid(reason) => FilesError::Invalid(reason),
            FileHostError::Port(error) => FilesError::Port(error),
        }
    }
}

fn job_error(error: FileHostError) -> JobError {
    let code = match &error {
        FileHostError::NotFound => "not_found",
        FileHostError::Denied => "denied",
        FileHostError::Exists => "name_taken",
        FileHostError::Invalid(_) => "invalid_operation",
        FileHostError::Port(_) => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

/// A share the user may use, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsableShare {
    pub share: Share,
    pub access: Access,
}

/// A volume as the file manager shows it, with its folders the user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVolume {
    pub pool: String,
    pub number: u16,
    pub mounted: bool,
    /// By name.
    pub folders: Vec<VolumeFolder>,
}

/// A folder directly on a volume, and what the user may do in it: `None`
/// for one an administrator who manages shares sees but cannot open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeFolder {
    pub share: Share,
    pub access: Option<Access>,
}

/// What the rules give `account`, directly or by a group.
fn access_of(share: &Share, account: i64, groups: &[i64]) -> Option<Access> {
    domain::effective(&share.access, account, groups)
}

/// Where file operations happen: a share, the user's own personal folder
/// on the volume where those are, or a removable drive mounted now.
#[derive(Debug, Clone)]
enum Place {
    Share(Share),
    Home { volume: u16 },
    Removable { id: String, name: String },
}

impl Place {
    fn id(&self) -> PlaceId {
        match self {
            Place::Share(share) => PlaceId::Share(share.id),
            Place::Home { .. } => PlaceId::Share(HOME_SHARE),
            Place::Removable { id, .. } => PlaceId::Removable(id.clone()),
        }
    }

    /// Its name for people: the share's, `home`, or the drive's label.
    fn name(&self) -> &str {
        match self {
            Place::Share(share) => share.name.as_str(),
            Place::Home { .. } => "home",
            Place::Removable { name, .. } => name,
        }
    }
}

/// A place the user may use, and how.
struct Usable {
    place: Place,
    access: Access,
}

fn at(place: &Place, path: FilePath) -> FileLocation {
    match place {
        Place::Share(share) => FileLocation {
            volume: share.volume,
            root: FileRoot::Share(share.name.clone()),
            path,
            snapshot: None,
        },
        Place::Home { volume } => {
            FileLocation { volume: *volume, root: FileRoot::Home, path, snapshot: None }
        }
        Place::Removable { id, .. } => {
            FileLocation { volume: 0, root: FileRoot::Removable(id.clone()), path, snapshot: None }
        }
    }
}

/// `path` in the place, or in a share's snapshot `label`.
fn at_snapshot(place: &Place, path: FilePath, label: Option<&str>) -> Result<FileLocation, FilesError> {
    let snapshot = match (label, place) {
        (None, _) => None,
        (Some(_), Place::Home { .. }) => {
            return Err(FilesError::Invalid("personal folders have no snapshots".into()));
        }
        (Some(_), Place::Removable { .. }) => {
            return Err(FilesError::Invalid("removable drives have no snapshots".into()));
        }
        (Some(label), Place::Share(_)) if domain::is_label(label) => Some(label.to_owned()),
        (Some(_), Place::Share(_)) => return Err(FilesError::Invalid("not a snapshot of this share".into())),
    };
    Ok(FileLocation { snapshot, ..at(place, path) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

/// An upload in progress: written to a hidden file in the target folder,
/// renamed when complete.
#[derive(Debug)]
pub struct Upload {
    pub file: tokio::fs::File,
    pub ticket: UploadTicket,
}

#[derive(Debug)]
pub struct UploadTicket {
    temporary: FileLocation,
    target: FileLocation,
    replace: bool,
    /// Its record, until it is finished or removed.
    record: i64,
}

/// All use cases of the file manager. Cheap to clone.
#[derive(Clone)]
pub struct FilesUseCases {
    ports: SharingPorts,
}

impl FilesUseCases {
    pub fn new(ports: SharingPorts) -> Self {
        Self { ports }
    }

    /// Shares whose rules name the user or one of their groups, by name.
    pub async fn shares(&self, actor: &Actor) -> Result<Vec<UsableShare>, FilesError> {
        let groups = self.ports.directory.memberships(actor.account_id).await?;
        let shares = self.ports.shares.list().await?;
        Ok(shares
            .into_iter()
            .filter_map(|share| {
                let access = access_of(&share, actor.account_id, &groups)?;
                Some(UsableShare { share, access })
            })
            .collect())
    }

    /// The volumes and their folders: with `everything` (someone who
    /// manages shares) every volume and every folder on it, also those the
    /// user cannot open; otherwise only the folders the user may use, and
    /// the volumes that have one. zendiloOS's own datasets are no folders.
    pub async fn volumes(
        &self,
        actor: &Actor,
        everything: bool,
        context: &RequestContext,
    ) -> Result<Vec<FileVolume>, FilesError> {
        let groups = self.ports.directory.memberships(actor.account_id).await?;
        let shares = self.ports.shares.list().await?;
        // Without the agent, the volumes the shares are on still show.
        let mut volumes: Vec<(String, u16, bool)> = match self.ports.volumes.volumes(context).await {
            Ok(volumes) => volumes.into_iter().map(|v| (v.pool, v.number, v.mounted)).collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the volumes; showing those of the shares");
                Vec::new()
            }
        };
        for share in &shares {
            if !volumes.iter().any(|(pool, ..)| *pool == share.pool) {
                volumes.push((share.pool.clone(), share.volume, true));
            }
        }
        volumes.sort_by_key(|(_, number, _)| *number);
        Ok(volumes
            .into_iter()
            .filter_map(|(pool, number, mounted)| {
                let folders: Vec<VolumeFolder> = shares
                    .iter()
                    .filter(|share| share.pool == pool)
                    .map(|share| VolumeFolder {
                        share: share.clone(),
                        access: access_of(share, actor.account_id, &groups),
                    })
                    .filter(|folder| everything || folder.access.is_some())
                    .collect();
                (everything || !folders.is_empty()).then_some(FileVolume { pool, number, mounted, folders })
            })
            .collect())
    }

    /// The volume of the user's personal folder, if they have one.
    pub async fn home(&self, actor: &Actor) -> Result<Option<u16>, FilesError> {
        let Some(location) = self.ports.shares.homes_location().await? else { return Ok(None) };
        let homes = self.ports.shares.homes().await?;
        Ok(homes.iter().any(|home| home.account == actor.account_id).then_some(location.volume))
    }

    /// The place `id` and what the user may do there. A removable drive
    /// only while mounted; the caller names one only for an administrator.
    async fn share(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        context: &RequestContext,
    ) -> Result<Usable, FilesError> {
        let id = match id.into() {
            PlaceId::Share(id) => id,
            PlaceId::Removable(id) => {
                if !domain::is_drive_id(&id) {
                    return Err(FilesError::ShareNotFound);
                }
                let drive =
                    self.ports.removable.mounted(&id, context).await?.ok_or(FilesError::ShareNotFound)?;
                let access = if drive.read_only { Access::Read } else { Access::ReadWrite };
                let name = drive.label.unwrap_or_else(|| drive.id.clone());
                return Ok(Usable { place: Place::Removable { id: drive.id, name }, access });
            }
        };
        if id == HOME_SHARE {
            let volume = self.home(actor).await?.ok_or(FilesError::ShareNotFound)?;
            return Ok(Usable { place: Place::Home { volume }, access: Access::ReadWrite });
        }
        let share = self.ports.shares.find(id).await?.ok_or(FilesError::ShareNotFound)?;
        let groups = self.ports.directory.memberships(actor.account_id).await?;
        let access = access_of(&share, actor.account_id, &groups).ok_or(FilesError::ShareNotFound)?;
        Ok(Usable { place: Place::Share(share), access })
    }

    /// The place, if the user may change things in it.
    async fn writable_share(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        context: &RequestContext,
    ) -> Result<Place, FilesError> {
        let usable = self.share(actor, id, context).await?;
        if usable.access != Access::ReadWrite {
            return Err(FilesError::Denied);
        }
        Ok(usable.place)
    }

    /// A folder of the share, or of its snapshot `snapshot`.
    pub async fn list(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        path: FilePath,
        snapshot: Option<&str>,
        context: &RequestContext,
    ) -> Result<Listing, FilesError> {
        let share = self.share(actor, id, context).await?.place;
        Ok(self.ports.files.list(&actor.name, &at_snapshot(&share, path, snapshot)?, context).await?)
    }

    /// A file for download, and its size; from a snapshot if given.
    pub async fn open(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        path: FilePath,
        snapshot: Option<&str>,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FilesError> {
        let share = self.share(actor, id, context).await?.place;
        Ok(self.ports.files.open(&actor.name, &at_snapshot(&share, path, snapshot)?, context).await?)
    }

    /// The share's snapshots the user can look into, newest first.
    pub async fn snapshots(
        &self,
        actor: &Actor,
        id: ShareId,
        context: &RequestContext,
    ) -> Result<Vec<domain::Snapshot>, FilesError> {
        let Place::Share(share) = self.share(actor, id, context).await?.place else { return Ok(Vec::new()) };
        let mut mine: Vec<domain::Snapshot> = self
            .ports
            .host
            .snapshots(context)
            .await
            .map_err(|error| match error {
                super::ports::HostError::Port(error) => FilesError::Port(error),
                other => FilesError::Invalid(other.to_string()),
            })?
            .into_iter()
            .filter(|s| s.pool == share.pool && s.share == share.name)
            .collect();
        mine.sort_by_key(|s| std::cmp::Reverse(s.taken_at));
        Ok(mine)
    }

    /// Copies `paths` out of the snapshot `label` back into the share, each
    /// into the folder it was in; beside a name that is taken, as
    /// `name (2).ext`. Nothing in the share is replaced. As a job.
    pub async fn restore_from_snapshot(
        &self,
        actor: &Actor,
        id: ShareId,
        label: &str,
        paths: Vec<FilePath>,
        context: &RequestContext,
    ) -> Result<Job, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES || paths.iter().any(FilePath::is_root) {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let share = self.writable_share(actor, id, context).await?;
        let copies = paths
            .into_iter()
            .map(|path| Ok((at_snapshot(&share, path.clone(), Some(label))?, at(&share, path))))
            .collect::<Result<Vec<_>, FilesError>>()?;
        let files = Arc::clone(&self.ports.files);
        let (user, context) = (actor.name.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start(
                "files.restore_snapshot",
                Some(share.name().to_owned()),
                Some(actor),
                move |progress| async move {
                    let total = copies.len();
                    for (done, (from, to)) in copies.iter().enumerate() {
                        #[allow(clippy::cast_precision_loss)]
                        progress.step("restoring", Some(done as f32 / total as f32)).await;
                        files.copy_entry(&user, from, to, &context).await.map_err(job_error)?;
                    }
                    tracing::info!(user, entries = total, "files: restored from a snapshot");
                    Ok(())
                },
            )
            .await?;
        Ok(job)
    }

    /// The account's favourite folders in shares it may still use, with
    /// each share's name.
    pub async fn favourites(&self, actor: &Actor) -> Result<Vec<(Favourite, ShareName)>, FilesError> {
        let usable = self.shares(actor).await?;
        let favourites = self.ports.shares.favourites(actor.account_id).await?;
        Ok(favourites
            .into_iter()
            .filter_map(|favourite| {
                let share = usable.iter().find(|u| u.share.id == favourite.share)?;
                Some((favourite, share.share.name.clone()))
            })
            .collect())
    }

    /// Replaces the account's favourites; each must be in a share it may
    /// use. Favourites in shares it lost access to are kept, unshown, in
    /// case access comes back.
    pub async fn set_favourites(
        &self,
        actor: &Actor,
        list: Vec<Favourite>,
    ) -> Result<Vec<(Favourite, ShareName)>, FilesError> {
        let usable = self.shares(actor).await?;
        if list.iter().any(|favourite| !usable.iter().any(|u| u.share.id == favourite.share)) {
            return Err(FilesError::ShareNotFound);
        }
        let mut list = domain::favourites(list).map_err(|error| FilesError::Invalid(error.to_string()))?;
        // Those in shares the account cannot use now: never in `list`.
        let hidden: Vec<Favourite> = self
            .ports
            .shares
            .favourites(actor.account_id)
            .await?
            .into_iter()
            .filter(|favourite| !usable.iter().any(|u| u.share.id == favourite.share))
            .collect();
        list.extend(hidden);
        list.truncate(MAX_FAVOURITES);
        self.ports.shares.set_favourites(actor.account_id, &list).await?;
        self.favourites(actor).await
    }

    /// The folder `path` as a tar archive being written, its exact length,
    /// and its name: the folder's, or the share's for its top.
    pub async fn archive(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        path: FilePath,
        snapshot: Option<&str>,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64, String), FilesError> {
        let share = self.share(actor, id, context).await?.place;
        let name = path.name().unwrap_or(share.name()).to_owned();
        let location = at_snapshot(&share, path, snapshot)?;
        let (file, size) = self.ports.files.archive(&actor.name, &location, context).await?;
        Ok((file, size, name))
    }

    /// Starts an upload of `name` into `directory`. The caller writes the
    /// file, then calls [`FilesUseCases::finish_upload`], or
    /// [`FilesUseCases::abort_upload`] if anything failed.
    pub async fn begin_upload(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        directory: FilePath,
        name: &str,
        replace: bool,
        context: &RequestContext,
    ) -> Result<Upload, FilesError> {
        if !is_new_file_name(name) || name.starts_with(UPLOAD_PREFIX) {
            return Err(FilesError::InvalidName);
        }
        let share = self.writable_share(actor, id, context).await?;
        let target = at(&share, directory.join(name)?);
        let folder = at(&share, directory.clone());
        let (file, temporary) = self.ports.files.create_upload(&actor.name, &folder, context).await?;
        let temporary = at(&share, directory.join(&temporary)?);
        let started = self.ports.clock.now();
        let record = match self.ports.shares.add_upload(&actor.name, &temporary, started).await {
            Ok(record) => record,
            Err(error) => {
                if let Err(left) = self.ports.files.discard_upload(&actor.name, &temporary, context).await {
                    tracing::warn!(error = %left, share = %temporary.root, "cannot remove an upload not begun");
                }
                return Err(error.into());
            }
        };
        Ok(Upload { file, ticket: UploadTicket { temporary, target, replace, record } })
    }

    /// Gives the uploaded file its name. On failure, the upload is removed.
    pub async fn finish_upload(
        &self,
        actor: &Actor,
        ticket: UploadTicket,
        context: &RequestContext,
    ) -> Result<FilePath, FilesError> {
        let files = &self.ports.files;
        match files.move_entry(&actor.name, &ticket.temporary, &ticket.target, ticket.replace, context).await
        {
            Ok(()) => {
                tracing::info!(user = actor.name, share = %ticket.target.root, "files: uploaded");
                self.forget_upload(ticket.record).await;
                Ok(ticket.target.path)
            }
            Err(error) => {
                self.abort_upload(actor, ticket, context).await;
                Err(error.into())
            }
        }
    }

    /// Removes the upload's hidden file, as a file only: whoever may write
    /// the folder could have put one of the user's folders in its place.
    pub async fn abort_upload(&self, actor: &Actor, ticket: UploadTicket, context: &RequestContext) {
        if let Err(error) = self.ports.files.discard_upload(&actor.name, &ticket.temporary, context).await {
            tracing::warn!(%error, share = %ticket.temporary.root, "cannot remove an aborted upload");
        }
        self.forget_upload(ticket.record).await;
    }

    async fn forget_upload(&self, record: i64) {
        if let Err(error) = self.ports.shares.remove_upload(record).await {
            tracing::warn!(%error, record, "cannot forget an upload");
        }
    }

    /// Removes the hidden files of uploads that began before `before`, when
    /// zendiloosd started, and were neither finished nor removed: it stopped
    /// in the middle of them. Each goes as the user who uploaded it, and only
    /// as a file. Says how many were removed.
    pub async fn discard_leftover_uploads(
        &self,
        before: Timestamp,
        context: &RequestContext,
    ) -> Result<usize, FilesError> {
        let mut removed = 0;
        for upload in self.ports.shares.uploads_before(before).await? {
            match self.ports.files.discard_upload(&upload.user, &upload.at, context).await {
                Ok(()) => removed += 1,
                Err(FileHostError::NotFound) => {}
                Err(error) => {
                    tracing::warn!(%error, share = %upload.at.root, "cannot remove a leftover upload")
                }
            }
            self.forget_upload(upload.id).await;
        }
        Ok(removed)
    }

    pub async fn make_directory(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        path: FilePath,
        context: &RequestContext,
    ) -> Result<(), FilesError> {
        if !path.name().is_some_and(is_new_file_name) {
            return Err(FilesError::InvalidName);
        }
        let share = self.writable_share(actor, id, context).await?;
        Ok(self.ports.files.make_directory(&actor.name, &at(&share, path), context).await?)
    }

    /// Gives `path` the name `name` in the same folder.
    pub async fn rename(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        path: FilePath,
        name: &str,
        context: &RequestContext,
    ) -> Result<FilePath, FilesError> {
        let parent = path.parent().ok_or(FilesError::Invalid("the share itself cannot be renamed".into()))?;
        if !is_new_file_name(name) {
            return Err(FilesError::InvalidName);
        }
        let target = parent.join(name).map_err(|_| FilesError::InvalidName)?;
        let share = self.writable_share(actor, id, context).await?;
        let (from, to) = (at(&share, path), at(&share, target.clone()));
        self.ports.files.move_entry(&actor.name, &from, &to, false, context).await?;
        Ok(target)
    }

    /// Copies or moves `paths` of place `id` into the folder `directory` of
    /// place `target`, one after the other, as a job. Between a removable
    /// drive and a share, the files agent reads one side and writes the
    /// other each with its own rights: what lands in a share is the user's.
    #[allow(clippy::too_many_arguments)]
    pub async fn transfer(
        &self,
        actor: &Actor,
        mode: Transfer,
        id: impl Into<PlaceId>,
        paths: Vec<FilePath>,
        target: impl Into<PlaceId>,
        directory: FilePath,
        context: &RequestContext,
    ) -> Result<Job, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES || paths.iter().any(FilePath::is_root) {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let source = match mode {
            Transfer::Copy => self.share(actor, id, context).await?.place,
            Transfer::Move => self.writable_share(actor, id, context).await?,
        };
        let destination = self.writable_share(actor, target, context).await?;
        let mut moves = Vec::with_capacity(paths.len());
        for path in paths {
            if source.id() == destination.id() && directory.starts_with(&path) {
                return Err(FilesError::Invalid("a folder cannot go into itself".into()));
            }
            let name = path.name().unwrap_or_default().to_owned();
            moves.push((at(&source, path), at(&destination, directory.join(&name)?)));
        }

        let (kind, step) = match mode {
            Transfer::Copy => ("files.copy", "copying"),
            Transfer::Move => ("files.move", "moving"),
        };
        let files = Arc::clone(&self.ports.files);
        let (user, context) = (actor.name.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start(kind, Some(destination.name().to_owned()), Some(actor), move |progress| async move {
                let total = moves.len();
                for (done, (from, to)) in moves.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    progress.step(step, Some(done as f32 / total as f32)).await;
                    match mode {
                        Transfer::Copy => files.copy_entry(&user, from, to, &context).await.map(drop),
                        Transfer::Move => files.move_entry(&user, from, to, false, &context).await,
                    }
                    .map_err(job_error)?;
                }
                tracing::info!(user, entries = total, kind = step, "files: transferred");
                Ok(())
            })
            .await?;
        Ok(job)
    }

    /// Moves `paths` out of the user's recycle bin in share `id` back to
    /// where they were deleted from, or beside it if the name is taken.
    /// Returns where each went; stops at the first that fails.
    pub async fn restore(
        &self,
        actor: &Actor,
        id: ShareId,
        paths: Vec<FilePath>,
        context: &RequestContext,
    ) -> Result<Vec<FilePath>, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        // A share's or the personal folder's bin: drives have none.
        let share = self.writable_share(actor, id, context).await?;
        let mut restored = Vec::with_capacity(paths.len());
        for path in paths {
            restored.push(self.ports.files.restore(&actor.name, &at(&share, path), context).await?);
        }
        tracing::info!(user = actor.name, entries = restored.len(), "files: restored");
        Ok(restored)
    }

    /// Moves `paths` into the user's recycle bin if the share has one,
    /// otherwise (and always on a removable drive) deletes them for good;
    /// as a job.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: impl Into<PlaceId>,
        paths: Vec<FilePath>,
        context: &RequestContext,
    ) -> Result<Job, FilesError> {
        if paths.is_empty() || paths.len() > MAX_TRANSFER_ENTRIES || paths.iter().any(FilePath::is_root) {
            return Err(FilesError::Invalid(format!("between 1 and {MAX_TRANSFER_ENTRIES} entries")));
        }
        let share = self.writable_share(actor, id, context).await?;
        // Personal folders always have a bin, as over SMB; drives none.
        let recycle = match &share {
            Place::Share(share) => share.recycle_bin,
            Place::Home { .. } => true,
            Place::Removable { .. } => false,
        };
        let locations: Vec<FileLocation> = paths.into_iter().map(|path| at(&share, path)).collect();
        let files = Arc::clone(&self.ports.files);
        let (user, context) = (actor.name.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("files.delete", Some(share.name().to_owned()), Some(actor), move |progress| async move {
                let total = locations.len();
                for (done, location) in locations.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    progress.step("deleting", Some(done as f32 / total as f32)).await;
                    files.remove(&user, location, recycle, &context).await.map_err(job_error)?;
                }
                tracing::info!(user, entries = total, recycle, "files: deleted");
                Ok(())
            })
            .await?;
        Ok(job)
    }
}
