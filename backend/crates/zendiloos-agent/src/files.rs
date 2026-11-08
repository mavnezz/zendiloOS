//! The files agent's work inside shares. Each request runs on a thread of
//! its own that first takes on the credentials of a managed user, so the
//! file system checks permissions and ACLs exactly as for that user's SMB
//! and NFS access. On Linux, credentials belong to a thread when set with
//! the raw system calls (as rustix does; glibc's wrappers would change the
//! whole process), and they end with the thread.
//!
//! On a removable drive the thread keeps root's access to files instead
//! (`CAP_DAC_OVERRIDE` and `CAP_DAC_READ_SEARCH`, nothing else): a drive
//! formatted elsewhere has owners nobody here has, and only an
//! administrator's request reaches one. That override is taken out of
//! every other thread of the files agent ([`lower_file_overrides`]). A copy
//! or move between a drive and a folder on a volume runs on two threads,
//! one with each side's credentials (see `zendiloos_fileops::Piece`): what
//! lands on a volume belongs to the person and takes the folder's ACL, as
//! an upload does, and only what the person may read leaves one.

use std::io;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustix::process::{Gid, Uid, getegid, geteuid, getgid, getuid};
use rustix::thread::{
    CapabilitySet, capabilities, set_capabilities, set_thread_groups, set_thread_res_gid, set_thread_res_uid,
};
use zendiloos_agent_proto::{EntryFacts, FileLocation, FileRoot, Request, Response, SharePath, UserName};
use zendiloos_fileops::{FileError, PIECES_IN_FLIGHT, SearchBounds, Sent, ShareRoot, Taken, settle};

use crate::accounts::{AccountFiles, ID_MAX, ID_MIN, USERS_GROUP};
use crate::operations::OperationError;
use crate::passwd;

#[derive(Debug)]
pub(crate) struct Files {
    accounts: AccountFiles,
    volumes: PathBuf,
    /// Where removable drives are mounted, a folder each.
    removable: PathBuf,
    /// Folders being written as archives; each holds a thread.
    archives: std::sync::Arc<tokio::sync::Semaphore>,
    /// Users with an archive being written: one each, so that one user's
    /// slow downloads cannot take every place.
    archiving: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
}

/// The most folders written as archives at once.
const CONCURRENT_ARCHIVES: usize = 4;
/// How long a search looks before it answers with what it found.
const SEARCH_TIME: std::time::Duration = std::time::Duration::from_secs(8);
/// How long counting a folder may take; what was counted by then is told.
const COUNT_TIME: std::time::Duration = std::time::Duration::from_secs(30);
/// An archive nobody reads for this long is given up.
const ARCHIVE_STALL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Least an archive's reader takes in every [`ARCHIVE_STALL`]: one that
/// reads a trickle would keep its place for ever.
const ARCHIVE_MIN_BYTES: u64 = 1024 * 1024;

/// The writing end of an archive's pipe (non-blocking), giving up when
/// the reader takes nothing for a while ([`ARCHIVE_STALL`]), or less than
/// [`ARCHIVE_MIN_BYTES`] in that time.
struct PatientPipe {
    fd: OwnedFd,
    stall: std::time::Duration,
    window: std::time::Instant,
    taken: u64,
    least: u64,
}

impl PatientPipe {
    fn new(fd: OwnedFd, stall: std::time::Duration, least: u64) -> Self {
        Self { fd, stall, window: std::time::Instant::now(), taken: 0, least }
    }

    fn took(&mut self, written: usize) -> io::Result<usize> {
        self.taken += written as u64;
        if self.window.elapsed() >= self.stall {
            if self.taken < self.least {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the archive is read too slowly"));
            }
            (self.window, self.taken) = (std::time::Instant::now(), 0);
        }
        Ok(written)
    }
}

impl io::Write for PatientPipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            match rustix::io::write(&self.fd, buffer) {
                Ok(written) => return self.took(written),
                Err(rustix::io::Errno::INTR) => {}
                Err(rustix::io::Errno::AGAIN) => {
                    let mut ready = [rustix::event::PollFd::new(&self.fd, rustix::event::PollFlags::OUT)];
                    let wait = rustix::event::Timespec {
                        tv_sec: self.stall.as_secs().try_into().unwrap_or(i64::MAX),
                        tv_nsec: self.stall.subsec_nanos().into(),
                    };
                    if rustix::event::poll(&mut ready, Some(&wait))? == 0 {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "nobody reads the archive"));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A user's place among those writing archives; freed when dropped.
struct ArchivingUser {
    uid: u32,
    users: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
}

impl Drop for ArchivingUser {
    fn drop(&mut self) {
        self.users.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.uid);
    }
}

/// Credentials of a managed user.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    uid: u32,
    gid: u32,
    /// Supplementary groups, the primary one included.
    groups: Vec<u32>,
}

fn read(path: &std::path::Path) -> Result<String, OperationError> {
    std::fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
}

/// Takes on `identity` for the rest of this thread's life.
fn become_user(identity: &Identity) -> io::Result<()> {
    let groups: Vec<Gid> = identity.groups.iter().map(|&gid| Gid::from_raw(gid)).collect();
    let (uid, gid) = (Uid::from_raw(identity.uid), Gid::from_raw(identity.gid));
    set_thread_groups(&groups)?;
    set_thread_res_gid(gid, gid, gid)?;
    // Last: without UID 0 the thread loses the right to change the others,
    // and all its capabilities.
    set_thread_res_uid(uid, uid, uid)?;
    let capabilities = capabilities(None)?;
    let switched = getuid() == uid && geteuid() == uid && getgid() == gid && getegid() == gid;
    if !switched || !capabilities.effective.is_empty() || !capabilities.permitted.is_empty() {
        return Err(io::Error::other("the thread kept privileges"));
    }
    Ok(())
}

/// What a thread working on a removable drive may do beyond its UID's
/// rights: read, search and write any file there, nothing else.
const DRIVE_ACCESS: CapabilitySet = CapabilitySet::DAC_OVERRIDE.union(CapabilitySet::DAC_READ_SEARCH);

/// Takes root's override of file permissions (`CAP_DAC_OVERRIDE`) out of
/// the calling thread's effective set, keeping it permitted: in the files
/// agent only a thread working on a removable drive takes it up again.
/// Called on every thread of the files agent's runtime as it starts.
pub fn lower_file_overrides() -> io::Result<()> {
    let mut sets = capabilities(None)?;
    sets.effective.remove(CapabilitySet::DAC_OVERRIDE);
    Ok(set_capabilities(None, sets)?)
}

/// Keeps only [`DRIVE_ACCESS`], of what this thread is permitted, effective.
fn take_drive_access() -> io::Result<()> {
    let mut sets = capabilities(None)?;
    sets.effective = sets.permitted & DRIVE_ACCESS;
    Ok(set_capabilities(None, sets)?)
}

/// Whose rights work on a root.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Acting {
    /// A managed user's, by their credentials.
    User(Identity),
    /// Root's access to files, on a removable drive.
    Drive,
}

impl Acting {
    fn at(location: &FileLocation, identity: &Identity) -> Self {
        match location.root {
            FileRoot::Removable(_) => Acting::Drive,
            FileRoot::Share(_) | FileRoot::Home => Acting::User(identity.clone()),
        }
    }
}

/// Runs `work` on a new thread acting as `acting`; fails only if the
/// thread cannot take on those rights.
async fn spawn_as<T: Send + 'static>(
    acting: Acting,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, OperationError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let name = match acting {
        Acting::User(_) => "files-as-user",
        Acting::Drive => "files-on-drive",
    };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let ready = match &acting {
                Acting::User(identity) => become_user(identity),
                Acting::Drive => take_drive_access(),
            };
            // Without its rights, `work` is dropped unrun.
            let _ = sender.send(ready.map(|()| work()));
        })
        .map_err(OperationError::Credentials)?;
    receiver
        .await
        .map_err(|_| OperationError::Credentials(io::Error::other("the thread ended early")))?
        .map_err(OperationError::Credentials)
}

/// Runs `work` on a new thread acting as `acting`.
async fn run<T: Send + 'static>(
    acting: Acting,
    work: impl FnOnce() -> Result<T, FileError> + Send + 'static,
) -> Result<T, OperationError> {
    spawn_as(acting, work).await?.map_err(OperationError::File)
}

/// Runs `work` on a new thread with `identity`'s credentials.
async fn as_user<T: Send + 'static>(
    identity: Identity,
    work: impl FnOnce() -> Result<T, FileError> + Send + 'static,
) -> Result<T, OperationError> {
    run(Acting::User(identity), work).await
}

/// One side of a copy across rights: its root and path, and whose rights.
struct Side {
    acting: Acting,
    root: Arc<ShareRoot>,
    path: SharePath,
}

/// Copies `from` to `to`, each side on a thread of its own with its own
/// rights; says where the copy went and what was copied.
async fn across(from: Side, to: Side, taken: Taken) -> Result<(SharePath, Sent), OperationError> {
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    let (source, from_path) = (from.root, from.path);
    let (target, to_path) = (to.root, to.path);
    let reading = spawn_as(from.acting, move || source.send_entry(&from_path, &out));
    let writing = spawn_as(to.acting, move || target.receive_entry(&to_path, &pieces, taken));
    let (read, written) = tokio::join!(reading, writing);
    settle(read?, written?).map_err(OperationError::File)
}

impl Files {
    pub(crate) fn new(accounts: AccountFiles, volumes: PathBuf, removable: PathBuf) -> Self {
        Self {
            accounts,
            volumes,
            removable,
            archives: std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENT_ARCHIVES)),
            archiving: std::sync::Arc::default(),
        }
    }

    /// Only users zendiloos manages: never root, a system account or the
    /// owner's own user.
    fn identity(&self, user: &UserName) -> Result<Identity, OperationError> {
        let not_managed = || OperationError::NotManaged(user.as_str().to_owned());
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let (uid, gid) = passwd::lookup(&passwd, user.as_str()).ok_or_else(not_managed)?;
        if !(ID_MIN..=ID_MAX).contains(&uid) || passwd::group_id(&group, USERS_GROUP) != Some(gid) {
            return Err(not_managed());
        }
        let mut groups = passwd::memberships(&group, user.as_str());
        groups.push(gid);
        groups.sort_unstable();
        groups.dedup();
        Ok(Identity { uid, gid, groups })
    }

    /// Opened as root, before the switch: refused unless the share's own
    /// dataset is mounted. A personal folder is always `user`'s own.
    fn share(&self, location: &FileLocation, user: &UserName) -> Result<ShareRoot, OperationError> {
        match (&location.root, &location.snapshot) {
            (FileRoot::Share(share), None) => ShareRoot::open(&self.volumes, location.volume, share, true),
            (FileRoot::Share(share), Some(label)) => {
                ShareRoot::open_snapshot(&self.volumes, location.volume, share, label)
            }
            (FileRoot::Home, None) => ShareRoot::open_home(&self.volumes, location.volume, user),
            (FileRoot::Home, Some(_)) => Err(FileError::Invalid("personal folders have no snapshots")),
            // Only the drive's own file system, never the tmpfs beneath.
            (FileRoot::Removable(id), None) => ShareRoot::open_removable(&self.removable, id, true),
            (FileRoot::Removable(_), Some(_)) => {
                Err(FileError::Invalid("removable drives have no snapshots"))
            }
        }
        .map_err(OperationError::File)
    }

    /// The live root at `location`: snapshots cannot be changed.
    fn writable(&self, location: &FileLocation, user: &UserName) -> Result<ShareRoot, OperationError> {
        if location.snapshot.is_some() {
            return Err(OperationError::File(FileError::Invalid("a snapshot cannot be changed")));
        }
        self.share(location, user)
    }

    pub(crate) async fn execute(
        &self,
        request: &Request,
    ) -> Result<(Response, Option<OwnedFd>), OperationError> {
        // Before anything else: only for a user zendiloOS manages, also on a
        // removable drive.
        let no_bin = || OperationError::File(FileError::Invalid("a removable drive has no recycle bin"));
        match request {
            Request::ListDirectory { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.share(location, user)?, location.path.clone());
                let listing = run(Acting::at(location, &identity), move || share.list(&path)).await?;
                let response = Response::Directory {
                    entries: listing.entries,
                    truncated: listing.truncated,
                    writable: listing.writable,
                };
                Ok((response, None))
            }
            Request::SearchFiles { user, location, text, hidden } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.share(location, user)?, location.path.clone());
                let (text, hidden) = (text.clone(), *hidden);
                let stop = Arc::new(AtomicBool::new(false));
                let _watching = crate::waiting::flag_when_given_up(Arc::clone(&stop));
                let until = std::time::Instant::now() + SEARCH_TIME;
                let found = run(Acting::at(location, &identity), move || {
                    share.search(&path, &text, hidden, SearchBounds { until, stop: &stop })
                })
                .await?;
                Ok((Response::Found { entries: found.entries, stopped: found.stopped }, None))
            }
            Request::EntryFacts { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.share(location, user)?, location.path.clone());
                let stop = Arc::new(AtomicBool::new(false));
                let _watching = crate::waiting::flag_when_given_up(Arc::clone(&stop));
                let until = std::time::Instant::now() + COUNT_TIME;
                let facts = run(Acting::at(location, &identity), move || {
                    share.facts(&path, SearchBounds { until, stop: &stop })
                })
                .await?;
                let passwd = read(&self.accounts.passwd)?;
                let owner = passwd::accounts(&passwd)
                    .find(|(_, uid, _)| *uid == facts.uid)
                    .map(|(name, _, _)| name.to_owned());
                let facts = EntryFacts {
                    folder: facts.folder,
                    bytes: facts.bytes,
                    files: facts.files,
                    folders: facts.folders,
                    hidden: facts.hidden,
                    stopped: facts.stopped,
                    created: facts.created,
                    modified: facts.modified,
                    owner_uid: facts.uid,
                    owner,
                };
                Ok((Response::Facts { facts }, None))
            }
            Request::OpenFile { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.share(location, user)?, location.path.clone());
                let (file, size) =
                    run(Acting::at(location, &identity), move || share.open_file(&path)).await?;
                Ok((Response::File { size }, Some(file)))
            }
            Request::CreateUpload { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.writable(location, user)?, location.path.clone());
                let acting = Acting::at(location, &identity);
                let (file, name) = run(acting, move || share.create_upload(&path)).await?;
                Ok((Response::Upload { name }, Some(file)))
            }
            Request::MakeDirectory { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.writable(location, user)?, location.path.clone());
                run(Acting::at(location, &identity), move || share.make_directory(&path)).await?;
                Ok((Response::Done, None))
            }
            Request::MoveEntry { user, from, to, replace } => {
                let identity = self.identity(user)?;
                let (reader, writer) = (Acting::at(from, &identity), Acting::at(to, &identity));
                let (source, target) = (self.writable(from, user)?, self.writable(to, user)?);
                let (from, to, replace) = (from.path.clone(), to.path.clone(), *replace);
                if reader == writer {
                    run(reader, move || source.move_entry(&from, &target, &to, replace)).await?;
                    return Ok((Response::Done, None));
                }
                if replace {
                    return Err(OperationError::File(FileError::Invalid(
                        "nothing is replaced between a drive and a volume",
                    )));
                }
                // A copy, then what was copied goes from where it was.
                let source = Arc::new(source);
                let from_side =
                    Side { acting: reader.clone(), root: Arc::clone(&source), path: from.clone() };
                let to_side = Side { acting: writer, root: Arc::new(target), path: to };
                let (_, sent) = across(from_side, to_side, Taken::Refuse).await?;
                run(reader, move || source.remove_sent(&from, sent)).await?;
                Ok((Response::Done, None))
            }
            Request::CopyEntry { user, from, to } => {
                let identity = self.identity(user)?;
                let (reader, writer) = (Acting::at(from, &identity), Acting::at(to, &identity));
                let (source, target) = (self.share(from, user)?, self.writable(to, user)?);
                let (from, to) = (from.path.clone(), to.path.clone());
                let path = if reader == writer {
                    run(reader, move || source.copy_entry(&from, &target, &to)).await?
                } else {
                    let from = Side { acting: reader, root: Arc::new(source), path: from };
                    let to = Side { acting: writer, root: Arc::new(target), path: to };
                    across(from, to, Taken::Beside).await?.0
                };
                Ok((Response::Copied { path }, None))
            }
            Request::RemoveEntry { user, location, recycle } => {
                let identity = self.identity(user)?;
                let acting = Acting::at(location, &identity);
                if *recycle && acting == Acting::Drive {
                    return Err(no_bin());
                }
                let (share, path) = (self.writable(location, user)?, location.path.clone());
                let bin_owner = recycle.then(|| user.as_str().to_owned());
                run(acting, move || share.remove(&path, bin_owner.as_deref())).await?;
                Ok((Response::Done, None))
            }
            Request::DiscardUpload { user, location } => {
                let identity = self.identity(user)?;
                let (share, path) = (self.writable(location, user)?, location.path.clone());
                run(Acting::at(location, &identity), move || share.discard_upload(&path)).await?;
                Ok((Response::Done, None))
            }
            Request::RestoreEntry { user, location } => {
                let identity = self.identity(user)?;
                if matches!(location.root, FileRoot::Removable(_)) {
                    return Err(no_bin());
                }
                let (share, path) = (self.writable(location, user)?, location.path.clone());
                let owner = user.as_str().to_owned();
                let path = as_user(identity, move || share.restore(&path, &owner)).await?;
                Ok((Response::Restored { path }, None))
            }
            Request::ArchiveFolder { user, location } => {
                let identity = self.identity(user)?;
                let busy = |what| OperationError::File(FileError::Invalid(what));
                let users = std::sync::Arc::clone(&self.archiving);
                if !users.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(identity.uid) {
                    return Err(busy("a folder of yours is being downloaded; wait until it is done"));
                }
                let place = ArchivingUser { uid: identity.uid, users };
                let permit = std::sync::Arc::clone(&self.archives)
                    .try_acquire_owned()
                    .map_err(|_| busy("too many folders are being downloaded"))?;
                let share = std::sync::Arc::new(self.share(location, user)?);
                let path = location.path.clone();
                let name = path
                    .file_name()
                    .unwrap_or(match &location.root {
                        FileRoot::Share(share) => share.as_str(),
                        FileRoot::Home => "home",
                        FileRoot::Removable(id) => id.as_str(),
                    })
                    .to_owned();
                let acting = Acting::at(location, &identity);
                let (planner, planned_path) = (std::sync::Arc::clone(&share), path.clone());
                let plan = run(acting.clone(), move || planner.plan_archive(&planned_path, &name)).await?;
                let (reader, writer) = std::io::pipe().map_err(|error| OperationError::File(error.into()))?;
                let writer = OwnedFd::from(writer);
                rustix::io::ioctl_fionbio(&writer, true)
                    .map_err(|error| OperationError::File(io::Error::from(error).into()))?;
                let size = plan.length();
                // Written while zendiloosd reads; it ends when done or when
                // zendiloosd stops reading (the pipe breaks).
                tokio::spawn(async move {
                    let written = run(acting, move || {
                        let mut out = std::io::BufWriter::with_capacity(
                            256 * 1024,
                            PatientPipe::new(writer, ARCHIVE_STALL, ARCHIVE_MIN_BYTES),
                        );
                        share.write_archive(&path, &plan, &mut out)
                    })
                    .await;
                    drop((permit, place));
                    if let Err(error) = written {
                        tracing::info!(%error, "archive: ended before its end");
                    }
                });
                Ok((Response::Archive { size }, Some(OwnedFd::from(reader))))
            }
            other => Err(OperationError::Refused(format!("{} is not a file operation", other.name()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{ShareName, SharePath, VolumeNumber};

    use super::*;

    fn files(directory: &std::path::Path) -> Files {
        std::fs::write(
            directory.join("passwd"),
            "root:x:0:0::/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n\
             anna:x:2000:990::/nonexistent:/usr/sbin/nologin\nsneaky:x:2001:0::/:/bin/sh\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("group"),
            "root:x:0:\nzendiloos-users:x:990:\nfamily:x:2000:anna,ben\nwork:x:2002:ben\n",
        )
        .unwrap();
        let accounts = AccountFiles {
            passwd: directory.join("passwd"),
            group: directory.join("group"),
            high_water: directory.join("high-water"),
        };
        Files::new(accounts, directory.to_owned(), directory.join("removable"))
    }

    fn drive(id: &str, path: &str) -> FileLocation {
        FileLocation {
            volume: VolumeNumber::new(1).unwrap(),
            root: FileRoot::Removable(zendiloos_agent_proto::RemovableId::parse(id).unwrap()),
            path: SharePath::parse(path).unwrap(),
            snapshot: None,
        }
    }

    #[tokio::test]
    async fn a_drive_is_worked_on_only_where_one_is_mounted() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        std::fs::create_dir_all(directory.path().join("removable/1A2B-3C4D")).unwrap();
        let anna = user("anna");
        // The empty folder of a drive not mounted: nothing lands on the tmpfs.
        for request in [
            Request::ListDirectory { user: anna.clone(), location: drive("1A2B-3C4D", "") },
            Request::MakeDirectory { user: anna.clone(), location: drive("1A2B-3C4D", "new") },
            Request::CreateUpload { user: anna.clone(), location: drive("1A2B-3C4D", "") },
        ] {
            let refused = files.execute(&request).await;
            assert!(
                matches!(&refused, Err(OperationError::File(FileError::Invalid(why))) if why.contains("not mounted")),
                "{}: {refused:?}",
                request.name()
            );
        }
        let unknown = Request::ListDirectory { user: anna.clone(), location: drive("FFFF", "") };
        assert!(matches!(files.execute(&unknown).await, Err(OperationError::File(FileError::NotFound))));
        let label = zendiloos_agent_proto::SnapshotLabel::parse("zdl-2026-11-05_19.40.01").unwrap();
        let frozen = FileLocation { snapshot: Some(label), ..drive("1A2B-3C4D", "a") };
        let refused = files.execute(&Request::OpenFile { user: anna.clone(), location: frozen }).await;
        assert!(
            matches!(refused, Err(OperationError::File(FileError::Invalid(why))) if why.contains("snapshots"))
        );
        for request in [
            Request::RemoveEntry { user: anna.clone(), location: drive("1A2B-3C4D", "a"), recycle: true },
            Request::RestoreEntry { user: anna.clone(), location: drive("1A2B-3C4D", ".recycle/anna/a") },
        ] {
            let refused = files.execute(&request).await;
            assert!(
                matches!(&refused, Err(OperationError::File(FileError::Invalid(why))) if why.contains("recycle bin")),
                "{}: {refused:?}",
                request.name()
            );
        }
        // Only for users zendiloOS manages, on a drive too.
        let root = Request::ListDirectory { user: user("root"), location: drive("1A2B-3C4D", "") };
        assert!(matches!(files.execute(&root).await, Err(OperationError::NotManaged(_))));
    }

    #[tokio::test]
    async fn on_a_drive_a_thread_keeps_root_access_to_files_and_nothing_else() {
        let effective = spawn_as(Acting::Drive, || capabilities(None).map(|sets| sets.effective)).await;
        let effective = effective.unwrap().unwrap();
        assert!(DRIVE_ACCESS.contains(effective), "{effective:?}");
        let lowered = std::thread::spawn(|| {
            lower_file_overrides()?;
            Ok::<_, io::Error>(capabilities(None)?.effective)
        });
        let lowered = lowered.join().unwrap().unwrap();
        assert!(!lowered.contains(CapabilitySet::DAC_OVERRIDE), "{lowered:?}");
    }

    #[tokio::test]
    async fn a_copy_across_runs_each_side_on_a_thread_of_its_own() {
        let directory = tempfile::tempdir().unwrap();
        for share in ["photos", "music"] {
            std::fs::create_dir_all(directory.path().join("volume1").join(share)).unwrap();
        }
        std::fs::create_dir_all(directory.path().join("volume1/photos/album")).unwrap();
        std::fs::write(directory.path().join("volume1/photos/album/a.jpg"), "a").unwrap();
        let root = |name: &str| {
            let share = ShareName::parse(name).unwrap();
            Arc::new(ShareRoot::open(directory.path(), VolumeNumber::new(1).unwrap(), &share, false).unwrap())
        };
        let side = |name: &str, path: &str| Side {
            acting: Acting::Drive,
            root: root(name),
            path: SharePath::parse(path).unwrap(),
        };

        let (copied, _) =
            across(side("photos", "album"), side("music", "album"), Taken::Beside).await.unwrap();
        assert_eq!(copied.as_str(), "album");
        assert_eq!(std::fs::read_to_string(directory.path().join("volume1/music/album/a.jpg")).unwrap(), "a");
        let (beside, _) =
            across(side("photos", "album"), side("music", "album"), Taken::Beside).await.unwrap();
        assert_eq!(beside.as_str(), "album (2)");
        let refused = across(side("photos", "album"), side("music", "album"), Taken::Refuse).await;
        assert!(matches!(refused, Err(OperationError::File(FileError::Exists))), "{refused:?}");
        let missing = across(side("photos", "gone"), side("music", "gone"), Taken::Beside).await;
        assert!(matches!(missing, Err(OperationError::File(FileError::NotFound))), "{missing:?}");
    }

    /// `/dev/shm` is a file system of its own below `/dev`, as a drive is
    /// below its place: a drive called `shm` there, for what needs one
    /// mounted. None where there is no such file system.
    fn mounted_drive() -> Option<(Files, tempfile::TempDir, tempfile::TempDir)> {
        let accounts = tempfile::tempdir().unwrap();
        let shm = tempfile::tempdir_in("/dev/shm").ok()?;
        let device = |path: &std::path::Path| rustix::fs::stat(path).ok().map(|stat| stat.st_dev);
        if device(std::path::Path::new("/dev")) == device(std::path::Path::new("/dev/shm")) {
            return None;
        }
        let mut files = files(accounts.path());
        files.removable = PathBuf::from("/dev");
        Some((files, accounts, shm))
    }

    #[tokio::test]
    async fn files_on_a_mounted_drive_are_listed_made_copied_and_removed() {
        let Some((files, _accounts, shm)) = mounted_drive() else { return };
        let top = shm.path().file_name().unwrap().to_str().unwrap().to_owned();
        std::fs::write(shm.path().join("song.mp3"), "tune").unwrap();
        let at =
            |path: &str| drive("shm", &if path.is_empty() { top.clone() } else { format!("{top}/{path}") });
        let anna = user("anna");

        let listed = files.execute(&Request::ListDirectory { user: anna.clone(), location: at("") }).await;
        let Ok((Response::Directory { entries, .. }, _)) = listed else { panic!("{listed:?}") };
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["song.mp3"]);
        let search = |text: &str| Request::SearchFiles {
            user: anna.clone(),
            location: at(""),
            text: zendiloos_agent_proto::SearchText::parse(text).unwrap(),
            hidden: false,
        };
        let found = files.execute(&search("SONG")).await;
        let Ok((Response::Found { entries, stopped: None }, None)) = found else { panic!("{found:?}") };
        assert_eq!(entries.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), [format!("{top}/song.mp3")]);
        let facts = files.execute(&Request::EntryFacts { user: anna.clone(), location: at("") }).await;
        let Ok((Response::Facts { facts }, None)) = facts else { panic!("{facts:?}") };
        assert!(facts.folder);
        assert_eq!(
            (facts.files, facts.folders, facts.hidden, facts.bytes, facts.stopped),
            (1, 0, 0, 4, None)
        );
        assert_eq!(facts.owner_uid, rustix::process::getuid().as_raw());
        let song = files.execute(&Request::EntryFacts { user: anna.clone(), location: at("song.mp3") }).await;
        let Ok((Response::Facts { facts: song }, None)) = song else { panic!("{song:?}") };
        assert_eq!((song.folder, song.bytes), (false, 4));
        files.execute(&Request::MakeDirectory { user: anna.clone(), location: at("new") }).await.unwrap();
        let copied = files
            .execute(&Request::CopyEntry { user: anna.clone(), from: at("song.mp3"), to: at("new/song.mp3") })
            .await;
        assert!(matches!(copied, Ok((Response::Copied { .. }, None))), "{copied:?}");
        files
            .execute(&Request::MoveEntry {
                user: anna.clone(),
                from: at("new/song.mp3"),
                to: at("moved.mp3"),
                replace: false,
            })
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(shm.path().join("moved.mp3")).unwrap(), "tune");
        let opened =
            files.execute(&Request::OpenFile { user: anna.clone(), location: at("moved.mp3") }).await;
        assert!(matches!(opened, Ok((Response::File { size: 4 }, Some(_)))), "{opened:?}");
        let archive =
            files.execute(&Request::ArchiveFolder { user: anna.clone(), location: at("new") }).await;
        assert!(matches!(archive, Ok((Response::Archive { .. }, Some(_)))), "{archive:?}");
        let removed = Request::RemoveEntry { user: anna.clone(), location: at("moved.mp3"), recycle: false };
        files.execute(&removed).await.unwrap();
        assert!(!shm.path().join("moved.mp3").exists(), "deleted for good, no bin on a drive");
        let upload = files.execute(&Request::CreateUpload { user: anna.clone(), location: at("") }).await;
        let Ok((Response::Upload { name }, Some(_))) = upload else { panic!("{upload:?}") };
        files.execute(&Request::DiscardUpload { user: anna, location: at(&name) }).await.unwrap();
        assert!(!shm.path().join(&name).exists());
    }

    fn user(name: &str) -> UserName {
        UserName::parse(name).unwrap()
    }

    #[test]
    fn acts_only_as_managed_users() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        assert_eq!(
            files.identity(&user("anna")).unwrap(),
            Identity { uid: 2000, gid: 990, groups: vec![990, 2000] }
        );
        for other in ["root", "debian", "sneaky", "nobody"] {
            assert!(
                matches!(files.identity(&user(other)), Err(OperationError::NotManaged(_))),
                "{other} must be refused"
            );
        }
    }

    #[test]
    fn an_archive_read_too_slowly_is_given_up() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        let writer = OwnedFd::from(writer);
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        let mut pipe = PatientPipe::new(writer, std::time::Duration::from_millis(100), 1024 * 1024);
        let reading = std::thread::spawn(move || io::copy(&mut reader, &mut io::sink()));
        let started = std::time::Instant::now();

        // A trickle: 1 KiB at a time, well below a mebibyte per 100 ms.
        let slow = (0..1000).try_for_each(|_| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            io::Write::write_all(&mut pipe, &[0u8; 1024])
        });

        assert_eq!(slow.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(pipe);
        reading.join().unwrap().unwrap();
    }

    #[test]
    fn an_archive_nobody_reads_is_given_up() {
        let (reader, writer) = std::io::pipe().unwrap();
        let writer = OwnedFd::from(writer);
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        let mut pipe = PatientPipe::new(writer, std::time::Duration::from_millis(50), 0);
        let chunk = [0u8; 64 * 1024];

        let stalled = (0..64).try_for_each(|_| io::Write::write_all(&mut pipe, &chunk));

        assert_eq!(stalled.unwrap_err().kind(), io::ErrorKind::TimedOut, "the pipe filled up");
        drop(reader);
    }

    #[tokio::test]
    async fn refuses_before_touching_anything() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        let location = FileLocation {
            volume: VolumeNumber::new(1).unwrap(),
            root: FileRoot::Share(ShareName::parse("photos").unwrap()),
            path: SharePath::root(),
            snapshot: None,
        };
        let request = Request::ListDirectory { user: user("root"), location: location.clone() };
        assert!(matches!(files.execute(&request).await, Err(OperationError::NotManaged(_))));
        // The share is missing: refused as root, before switching.
        let request = Request::ListDirectory { user: user("anna"), location: location.clone() };
        assert!(matches!(files.execute(&request).await, Err(OperationError::File(FileError::NotFound))));
        // Nothing is written into a snapshot.
        let snapshot = zendiloos_agent_proto::SnapshotLabel::parse("zdl-2026-11-05_19.40.01").unwrap();
        let frozen = FileLocation { snapshot: Some(snapshot), ..location };
        for request in [
            Request::MakeDirectory { user: user("anna"), location: frozen.clone() },
            Request::RemoveEntry { user: user("anna"), location: frozen.clone(), recycle: false },
            Request::CreateUpload { user: user("anna"), location: frozen.clone() },
        ] {
            let refused = files.execute(&request).await;
            assert!(matches!(refused, Err(OperationError::File(FileError::Invalid(_)))), "{refused:?}");
        }
    }

    #[tokio::test]
    async fn work_runs_on_a_thread_with_the_users_credentials_only() {
        let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
        let identity = Identity { uid, gid, groups: vec![gid] };
        let result = as_user(identity, || Ok(7)).await;
        if rustix::process::geteuid().is_root() {
            assert_eq!(result.unwrap(), 7);
        } else {
            // Without root a thread cannot even set its groups: nothing runs.
            assert!(matches!(result, Err(OperationError::Credentials(_))), "{result:?}");
        }
    }

    #[tokio::test]
    async fn every_file_operation_wants_its_share_first() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        let share = |path: &str| FileLocation {
            volume: VolumeNumber::new(1).unwrap(),
            root: FileRoot::Share(ShareName::parse("photos").unwrap()),
            path: SharePath::parse(path).unwrap(),
            snapshot: None,
        };
        let home = FileLocation { root: FileRoot::Home, ..share("a.txt") };
        let snapshot = zendiloos_agent_proto::SnapshotLabel::parse("zdl-2026-11-05_19.40.01").unwrap();
        let home_snapshot = FileLocation { snapshot: Some(snapshot.clone()), ..home.clone() };
        let share_snapshot = FileLocation { snapshot: Some(snapshot), ..share("a.txt") };
        let anna = user("anna");
        for request in [
            Request::OpenFile { user: anna.clone(), location: share("a.txt") },
            Request::OpenFile { user: anna.clone(), location: home.clone() },
            Request::SearchFiles {
                user: anna.clone(),
                location: share(""),
                text: zendiloos_agent_proto::SearchText::parse("a").unwrap(),
                hidden: true,
            },
            Request::OpenFile { user: anna.clone(), location: share_snapshot },
            Request::MoveEntry { user: anna.clone(), from: share("a"), to: share("b"), replace: false },
            Request::CopyEntry { user: anna.clone(), from: share("a"), to: share("b") },
            Request::DiscardUpload { user: anna.clone(), location: share(".zendiloos-upload-1") },
            Request::RestoreEntry { user: anna.clone(), location: share(".recycle/anna/a") },
            Request::ArchiveFolder { user: anna.clone(), location: share("") },
            Request::ArchiveFolder { user: anna.clone(), location: home },
        ] {
            let refused = files.execute(&request).await;
            assert!(matches!(refused, Err(OperationError::File(_))), "{}: {refused:?}", request.name());
        }
        let no_snapshots =
            files.execute(&Request::OpenFile { user: anna.clone(), location: home_snapshot }).await;
        assert!(
            matches!(no_snapshots, Err(OperationError::File(FileError::Invalid(why))) if why.contains("no snapshots"))
        );
        // A second folder download of anna's may start once the first ended.
        let again = files.execute(&Request::ArchiveFolder { user: anna, location: share("") }).await;
        assert!(matches!(again, Err(OperationError::File(_))), "{again:?}");
        let other = files.execute(&Request::Reboot {}).await;
        assert!(matches!(other, Err(OperationError::Refused(why)) if why.contains("not a file operation")));
    }
}
