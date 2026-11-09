//! In-memory fakes of this context's ports, for tests here and in the API
//! crate (feature `testing`).

use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::{
    self, FileHost, FileHostError, HostError, ShareHost, ShareRepository, Volume,
};
use crate::domain::{
    Access, CountStop, Entry, EntryFacts, EntryKind, FileLocation, FilePath, Found, FoundEntry, Frequency,
    Listing, NewShare, SearchStop, Share, ShareId, ShareName, ShareSettings, ShareUsage, SharingConfig,
    Snapshot, SnapshotSchedule,
};
use zendiloos_kernel::Timestamp;

/// Volume 1 on pool `tank` (mounted) and volume 2 on `cold` (not mounted).
pub struct Volumes;

#[async_trait]
impl ports::Volumes for Volumes {
    async fn volumes(&self, _: &RequestContext) -> Result<Vec<Volume>, PortError> {
        Ok(vec![
            Volume { pool: "tank".into(), number: 1, mounted: true },
            Volume { pool: "cold".into(), number: 2, mounted: false },
        ])
    }
}

/// Removable drives mounted now: none until [`Removable::plug`].
#[derive(Default)]
pub struct Removable(pub Mutex<Vec<ports::RemovableRoot>>);

impl Removable {
    /// A drive mounted, writable unless `read_only`.
    pub fn plug(&self, id: &str, label: Option<&str>, read_only: bool) {
        let drive = ports::RemovableRoot { id: id.into(), label: label.map(Into::into), read_only };
        self.0.lock().unwrap().push(drive);
    }
}

#[async_trait]
impl ports::RemovableRoots for Removable {
    async fn mounted(&self, id: &str, _: &RequestContext) -> Result<Option<ports::RemovableRoot>, PortError> {
        Ok(self.0.lock().unwrap().iter().find(|drive| drive.id == id).cloned())
    }
}

/// Administrators' projects: those named here use every share.
#[derive(Default)]
pub struct Projects(pub Mutex<Vec<String>>);

#[async_trait]
impl ports::ShareProjects for Projects {
    async fn using(&self, _: ShareId, _: &str) -> Result<Vec<String>, PortError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

/// What every file of the fake host holds.
pub const FILE_CONTENT: &str = "abcdefghij";

#[derive(Default)]
pub struct Shares(
    Mutex<Vec<Share>>,
    Mutex<std::collections::HashMap<ShareId, Vec<SnapshotSchedule>>>,
    Mutex<std::collections::HashMap<i64, Vec<crate::domain::Favourite>>>,
    Mutex<Vec<crate::domain::AppAccess>>,
    Mutex<Option<crate::domain::FileServices>>,
    Mutex<(Option<crate::domain::HomesLocation>, Vec<crate::domain::Home>)>,
    /// Shares hidden while their data goes.
    Mutex<std::collections::HashSet<ShareId>>,
    /// Uploads under way, with when they began.
    pub Mutex<Vec<(ports::PendingUpload, Timestamp)>>,
    /// When each share's files were given share-level access; a share
    /// missing here owes its walk.
    pub Mutex<std::collections::HashMap<ShareId, Timestamp>>,
);

impl Shares {
    /// Whether the share's files still wait for their walk to share-level
    /// access.
    pub fn owes_walk(&self, share: ShareId) -> bool {
        !self.8.lock().unwrap().contains_key(&share)
    }
}

#[async_trait]
impl ShareRepository for Shares {
    async fn list(&self) -> Result<Vec<Share>, PortError> {
        let hidden = self.6.lock().unwrap();
        Ok(self.0.lock().unwrap().iter().filter(|s| !hidden.contains(&s.id)).cloned().collect())
    }

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError> {
        Ok(self.list().await?.into_iter().find(|s| s.id == id))
    }

    async fn set_deleting(&self, id: ShareId, deleting: bool) -> Result<(), PortError> {
        let mut hidden = self.6.lock().unwrap();
        if deleting {
            hidden.insert(id);
        } else {
            hidden.remove(&id);
        }
        Ok(())
    }

    async fn deleting(&self) -> Result<Vec<Share>, PortError> {
        let hidden = self.6.lock().unwrap();
        Ok(self.0.lock().unwrap().iter().filter(|s| hidden.contains(&s.id)).cloned().collect())
    }

    async fn add_upload(&self, user: &str, at: &FileLocation, started: Timestamp) -> Result<i64, PortError> {
        let mut uploads = self.7.lock().unwrap();
        let id = uploads.iter().map(|(upload, _)| upload.id).max().unwrap_or(0) + 1;
        uploads.push((ports::PendingUpload { id, user: user.to_owned(), at: at.clone() }, started));
        Ok(id)
    }

    async fn remove_upload(&self, id: i64) -> Result<(), PortError> {
        self.7.lock().unwrap().retain(|(upload, _)| upload.id != id);
        Ok(())
    }

    async fn uploads_before(&self, before: Timestamp) -> Result<Vec<ports::PendingUpload>, PortError> {
        let uploads = self.7.lock().unwrap();
        Ok(uploads
            .iter()
            .filter(|(_, started)| *started < before)
            .map(|(upload, _)| upload.clone())
            .collect())
    }

    async fn create(&self, new: NewShare) -> Result<Option<Share>, PortError> {
        let mut shares = self.0.lock().unwrap();
        let share = Share {
            id: ShareId::new(i64::try_from(shares.len()).unwrap() + 1),
            name: new.name,
            description: new.description,
            pool: new.pool,
            volume: new.volume,
            shared: new.shared,
            recycle_bin: new.recycle_bin,
            settings: new.settings,
            access: new.access,
            nfs: new.nfs,
            created_at: new.created_at,
            updated_at: new.created_at,
        };
        shares.push(share.clone());
        // A new share's files have share-level access from the start.
        self.8.lock().unwrap().insert(share.id, new.created_at);
        Ok(Some(share))
    }

    async fn update(&self, share: &Share) -> Result<(), PortError> {
        let mut shares = self.0.lock().unwrap();
        let stored = shares.iter_mut().find(|s| s.id == share.id).unwrap();
        *stored = share.clone();
        Ok(())
    }

    async fn delete(&self, id: ShareId) -> Result<(), PortError> {
        self.0.lock().unwrap().retain(|s| s.id != id);
        self.1.lock().unwrap().remove(&id);
        self.6.lock().unwrap().remove(&id);
        self.8.lock().unwrap().remove(&id);
        Ok(())
    }

    async fn migrations_owed(&self) -> Result<Vec<ShareId>, PortError> {
        let shares = self.list().await?;
        let migrated = self.8.lock().unwrap();
        let mut owed: Vec<Share> =
            shares.into_iter().filter(|share| !migrated.contains_key(&share.id)).collect();
        owed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(owed.into_iter().map(|share| share.id).collect())
    }

    async fn migrated_at(&self, share: ShareId) -> Result<Option<Timestamp>, PortError> {
        Ok(self.8.lock().unwrap().get(&share).copied())
    }

    async fn set_migrated(&self, share: ShareId, at: Option<Timestamp>) -> Result<(), PortError> {
        let mut migrated = self.8.lock().unwrap();
        match at {
            Some(at) => migrated.insert(share, at),
            None => migrated.remove(&share),
        };
        Ok(())
    }

    async fn snapshot_schedules(&self, id: ShareId) -> Result<Vec<SnapshotSchedule>, PortError> {
        Ok(self.1.lock().unwrap().get(&id).cloned().unwrap_or_default())
    }

    async fn set_snapshot_schedules(
        &self,
        id: ShareId,
        schedules: &[SnapshotSchedule],
    ) -> Result<(), PortError> {
        self.1.lock().unwrap().insert(id, schedules.to_vec());
        Ok(())
    }

    async fn app_access(&self) -> Result<Vec<crate::domain::AppAccess>, PortError> {
        Ok(self.3.lock().unwrap().clone())
    }

    async fn set_app_access(&self, account: &str, access: &[(ShareId, Access)]) -> Result<(), PortError> {
        let mut all = self.3.lock().unwrap();
        all.retain(|app| app.account != account);
        all.extend(access.iter().map(|(share, access)| crate::domain::AppAccess {
            share: *share,
            account: account.to_owned(),
            access: *access,
        }));
        Ok(())
    }

    async fn favourites(&self, account: i64) -> Result<Vec<crate::domain::Favourite>, PortError> {
        Ok(self.2.lock().unwrap().get(&account).cloned().unwrap_or_default())
    }

    async fn set_favourites(
        &self,
        account: i64,
        favourites: &[crate::domain::Favourite],
    ) -> Result<(), PortError> {
        self.2.lock().unwrap().insert(account, favourites.to_vec());
        Ok(())
    }

    async fn file_services(&self) -> Result<crate::domain::FileServices, PortError> {
        Ok(self.4.lock().unwrap().clone().unwrap_or_default())
    }

    async fn set_file_services(&self, services: &crate::domain::FileServices) -> Result<(), PortError> {
        *self.4.lock().unwrap() = Some(services.clone());
        Ok(())
    }

    async fn homes_location(&self) -> Result<Option<crate::domain::HomesLocation>, PortError> {
        Ok(self.5.lock().unwrap().0.clone())
    }

    async fn set_homes_location(
        &self,
        location: Option<&crate::domain::HomesLocation>,
    ) -> Result<(), PortError> {
        self.5.lock().unwrap().0 = location.cloned();
        Ok(())
    }

    async fn homes(&self) -> Result<Vec<crate::domain::Home>, PortError> {
        Ok(self.5.lock().unwrap().1.clone())
    }

    async fn set_home(&self, home: &crate::domain::Home) -> Result<(), PortError> {
        let mut homes = self.5.lock().unwrap();
        homes.1.retain(|h| h.account != home.account);
        homes.1.push(home.clone());
        Ok(())
    }

    async fn remove_home(&self, account: i64) -> Result<(), PortError> {
        self.5.lock().unwrap().1.retain(|h| h.account != account);
        Ok(())
    }
}

#[derive(Default)]
pub struct Host {
    pub calls: Mutex<Vec<String>>,
    pub configs: Mutex<Vec<SharingConfig>>,
    /// Preparing a share's folder is refused, as when it is not mounted.
    pub refuse_prepare: std::sync::atomic::AtomicBool,
    /// Snapshots as the host has them; taking and destroying change them.
    pub snapshots: Mutex<Vec<Snapshot>>,
    /// Taking snapshots fails, e.g. because the pool is gone.
    pub refuse_snapshots: std::sync::atomic::AtomicBool,
    /// What every share dataset holds, in bytes.
    pub used_bytes: std::sync::atomic::AtomicU64,
    /// The servers refuse a configuration where SMB and NFS are both off,
    /// standing for any configuration `testparm` rejects.
    pub refuse_all_off: std::sync::atomic::AtomicBool,
    /// Every apply is refused, as when smbd does not restart.
    pub refuse_apply: std::sync::atomic::AtomicBool,
    /// Destroying a dataset fails, as when it is busy.
    pub refuse_destroy: std::sync::atomic::AtomicBool,
    /// The agent refuses to destroy a dataset before doing anything, as
    /// when a project mounts something of it.
    pub deny_destroy: std::sync::atomic::AtomicBool,
    /// Walks to share-level access wait in the agent until `let_walks_on`
    /// lets one on, as a long walk would.
    pub hold_walks: std::sync::atomic::AtomicBool,
    /// How many walks are waiting or have waited.
    pub walks_held: std::sync::atomic::AtomicUsize,
    pub let_walks_on: tokio::sync::Notify,
    /// Walks fail, as when an entry could not be changed or the agent
    /// stopped; recorded as `failed migrate …`.
    pub fail_walks: std::sync::atomic::AtomicBool,
}

impl Host {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ShareHost for Host {
    async fn create_dataset(
        &self,
        pool: &str,
        volume: u16,
        name: &ShareName,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("create {pool} {volume} {name}"));
        Ok(())
    }

    async fn destroy_dataset(
        &self,
        pool: &str,
        volume: u16,
        name: &ShareName,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("destroy {pool} {volume} {name}"));
        if self.deny_destroy.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Refused("a project mounts something of it".into()));
        }
        if self.refuse_destroy.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Port(PortError::new("agent", "dataset is busy")));
        }
        Ok(())
    }

    async fn apply(
        &self,
        config: &SharingConfig,
        close: &[ShareName],
        _: &RequestContext,
    ) -> Result<(), HostError> {
        if self.refuse_apply.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Refused("smbd did not restart".into()));
        }
        if self.refuse_all_off.load(std::sync::atomic::Ordering::Relaxed)
            && !config.services.smb
            && !config.services.nfs
        {
            return Err(HostError::Refused("testparm: no".into()));
        }
        let mut calls = self.calls.lock().unwrap();
        calls.push(format!("apply {}", config.smb.len()));
        if !close.is_empty() {
            let close: Vec<&str> = close.iter().map(ShareName::as_str).collect();
            calls.push(format!("close {}", close.join(",")));
        }
        self.configs.lock().unwrap().push(config.clone());
        Ok(())
    }

    async fn prepare_homes(&self, pool: &str, volume: u16, _: &RequestContext) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("prepare_homes {pool} {volume}"));
        Ok(())
    }

    async fn set_home(
        &self,
        pool: &str,
        volume: u16,
        user: &str,
        quota_bytes: Option<u64>,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("set_home {pool} {volume} {user} {quota_bytes:?}"));
        Ok(())
    }

    async fn set_properties(
        &self,
        pool: &str,
        _volume: u16,
        name: &ShareName,
        settings: &ShareSettings,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        let quota = settings.quota_bytes.map_or("none".to_owned(), |q| q.to_string());
        self.calls.lock().unwrap().push(format!(
            "properties {pool}/{name} refquota={quota} compression={} recordsize={}",
            settings.compression.as_str(),
            settings.record_size.bytes()
        ));
        Ok(())
    }

    /// Every share created so far, holding [`Host::used_bytes`] each.
    async fn usage(&self, _: &RequestContext) -> Result<Vec<ShareUsage>, HostError> {
        let used = self.used_bytes.load(std::sync::atomic::Ordering::Relaxed);
        let calls = self.calls();
        Ok(calls
            .iter()
            .filter_map(|call| call.strip_prefix("create "))
            .filter_map(|rest| {
                let mut parts = rest.split(' ');
                let (pool, _volume, name) = (parts.next()?, parts.next()?, parts.next()?);
                Some(ShareUsage {
                    pool: pool.to_owned(),
                    name: ShareName::parse(name).ok()?,
                    used_bytes: used,
                    available_bytes: 1 << 40,
                })
            })
            .collect())
    }

    async fn snapshots(&self, _: &RequestContext) -> Result<Vec<Snapshot>, HostError> {
        Ok(self.snapshots.lock().unwrap().clone())
    }

    async fn create_snapshot(
        &self,
        pool: &str,
        _volume: u16,
        share: &ShareName,
        label: &str,
        schedules: &[Frequency],
        note: Option<&str>,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        if self.refuse_snapshots.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Refused("the pool is not imported".into()));
        }
        let names: Vec<&str> = schedules.iter().map(|f| f.as_str()).collect();
        self.calls.lock().unwrap().push(format!("snapshot {pool}/{share}@{label} [{}]", names.join(",")));
        let mut snapshots = self.snapshots.lock().unwrap();
        if snapshots.iter().any(|s| s.share == *share && s.label == label) {
            return Err(HostError::Conflict);
        }
        snapshots.push(Snapshot {
            pool: pool.to_owned(),
            share: share.clone(),
            label: label.to_owned(),
            taken_at: crate::domain::label_time(label)
                .unwrap_or(zendiloos_kernel::Timestamp::from_unix_seconds(0)),
            schedules: schedules.to_vec(),
            note: note.map(str::to_owned),
            used_bytes: 0,
            referenced_bytes: 0,
        });
        Ok(())
    }

    async fn destroy_snapshot(
        &self,
        pool: &str,
        _volume: u16,
        share: &ShareName,
        label: &str,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("destroy {pool}/{share}@{label}"));
        self.snapshots.lock().unwrap().retain(|s| !(s.share == *share && s.label == label));
        Ok(())
    }

    async fn rollback_snapshot(
        &self,
        pool: &str,
        _volume: u16,
        share: &ShareName,
        label: &str,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("rollback {pool}/{share}@{label}"));
        self.snapshots.lock().unwrap().retain(|s| s.share != *share || s.label.as_str() <= label);
        Ok(())
    }

    async fn prepare_share(
        &self,
        volume: u16,
        name: &ShareName,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        if self.refuse_prepare.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Refused("share is not mounted".into()));
        }
        self.calls.lock().unwrap().push(format!("prepare {volume} {name}"));
        Ok(())
    }

    async fn migrate_share(
        &self,
        volume: u16,
        name: &ShareName,
        lines: &(dyn for<'line> Fn(&'line str) + Send + Sync),
        _: &RequestContext,
    ) -> Result<(), HostError> {
        if self.hold_walks.load(std::sync::atomic::Ordering::Relaxed) {
            self.walks_held.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.let_walks_on.notified().await;
        }
        let failing = self.fail_walks.load(std::sync::atomic::Ordering::Relaxed);
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}migrate {volume} {name}", if failing { "failed " } else { "" }));
        if failing {
            return Err(HostError::Port(PortError::new("files agent", "the walk stopped")));
        }
        lines("3 entries");
        Ok(())
    }
}

/// Records file operations. A target named `taken.jpg` exists already, a
/// path containing `missing` does not exist. Uploads go to unlinked
/// temporary files, downloads come from `/dev/null`.
#[derive(Default)]
pub struct Files {
    pub calls: Mutex<Vec<String>>,
}

impl Files {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String, at: &FileLocation) -> Result<(), FileHostError> {
        self.calls.lock().unwrap().push(call);
        if at.path.as_str().contains("missing") {
            return Err(FileHostError::NotFound);
        }
        Ok(())
    }
}

/// `share:path`, or `share@label:path` in a snapshot.
fn place(at: &FileLocation) -> String {
    let snapshot = at.snapshot.as_ref().map(|label| format!("@{label}")).unwrap_or_default();
    format!("{}{snapshot}:{}", at.root, at.path.as_str())
}

#[async_trait]
impl FileHost for Files {
    async fn list(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<Listing, FileHostError> {
        self.record(format!("list {user} {}", place(at)), at)?;
        let entry = Entry {
            name: "a.jpg".into(),
            kind: EntryKind::File,
            size: 3,
            modified: Timestamp::from_unix_seconds(1),
            unreadable: false,
        };
        Ok(Listing { entries: vec![entry], truncated: false, writable: true })
    }

    /// Finds `beach.jpg` in `album` below where it looks; `many` stops at
    /// its limit.
    async fn search(
        &self,
        user: &str,
        at: &FileLocation,
        text: &str,
        hidden: bool,
        _: &RequestContext,
    ) -> Result<Found, FileHostError> {
        let hidden = if hidden { " hidden" } else { "" };
        self.record(format!("search {user} {} {text}{hidden}", place(at)), at)?;
        let entry = Entry {
            name: "beach.jpg".into(),
            kind: EntryKind::File,
            size: 4,
            modified: Timestamp::from_unix_seconds(2),
            unreadable: false,
        };
        let path = at.path.join("album").and_then(|album| album.join("beach.jpg")).expect("a short path");
        let stopped = (text == "many").then_some(SearchStop::Results);
        Ok(Found { entries: vec![FoundEntry { path, entry }], stopped })
    }

    /// A folder at the share's top or anywhere named `big` (counted only so
    /// far), a file elsewhere.
    async fn facts(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<EntryFacts, FileHostError> {
        self.record(format!("facts {user} {}", place(at)), at)?;
        let folder = at.path.as_str().is_empty() || at.path.as_str().ends_with("big");
        Ok(EntryFacts {
            folder,
            bytes: if folder { 1_000 } else { 4 },
            files: if folder { 12 } else { 0 },
            folders: if folder { 3 } else { 0 },
            hidden: if folder { 1 } else { 0 },
            stopped: at.path.as_str().ends_with("big").then_some(CountStop::Time),
            created: Some(Timestamp::from_unix_seconds(1_700_000_000)),
            modified: Some(Timestamp::from_unix_seconds(1_700_000_100)),
            owner: Some(user.to_owned()),
        })
    }

    async fn open(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError> {
        self.record(format!("open {user} {}", place(at)), at)?;
        // Every file holds the same ten letters.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("zendiloos-download-{}-{number}", std::process::id()));
        let error = |e| PortError::new("files", e);
        tokio::fs::write(&path, FILE_CONTENT).await.map_err(error)?;
        // The same time whenever it is opened, as a file kept on a disk has:
        // a download resumed with its date (`If-Range`) gets the part.
        let written = std::fs::File::options().write(true).open(&path).map_err(error)?;
        let modified = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        written.set_modified(modified).map_err(error)?;
        let file = tokio::fs::File::open(&path).await.map_err(error)?;
        let _ = tokio::fs::remove_file(&path).await;
        Ok((file, FILE_CONTENT.len() as u64))
    }

    async fn create_upload(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<(tokio::fs::File, String), FileHostError> {
        self.record(format!("create_upload {user} {}", place(at)), at)?;
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("zendiloos-upload-{}-{number}", std::process::id()));
        let file = tokio::fs::File::create(&path).await.map_err(|e| PortError::new("files", e))?;
        let _ = tokio::fs::remove_file(&path).await;
        Ok((file, ".zendiloos-upload-1".into()))
    }

    async fn make_directory(
        &self,
        user: &str,
        at: &FileLocation,
        own: bool,
        _: &RequestContext,
    ) -> Result<(), FileHostError> {
        let mkdir = if own { "mkdir-own" } else { "mkdir" };
        self.record(format!("{mkdir} {user} {}", place(at)), at)
    }

    async fn move_entry(
        &self,
        user: &str,
        from: &FileLocation,
        to: &FileLocation,
        replace: bool,
        _: &RequestContext,
    ) -> Result<(), FileHostError> {
        let replace = if replace { " replace" } else { "" };
        self.record(format!("move {user} {} -> {}{replace}", place(from), place(to)), from)?;
        if to.path.name() == Some("taken.jpg") && replace.is_empty() {
            return Err(FileHostError::Exists);
        }
        // Written to while it was moved: what changed stays.
        if from.path.name() == Some("changing") {
            let example = from.path.join("new.txt").ok();
            return Err(FileHostError::PartlyMoved { kept: 2, example, too_many: false });
        }
        if from.path.name() == Some("huge") {
            return Err(FileHostError::PartlyMoved { kept: 1_000_001, example: None, too_many: true });
        }
        Ok(())
    }

    async fn copy_entry(
        &self,
        user: &str,
        from: &FileLocation,
        to: &FileLocation,
        _: &RequestContext,
    ) -> Result<FilePath, FileHostError> {
        self.record(format!("copy {user} {} -> {}", place(from), place(to)), from)?;
        Ok(to.path.clone())
    }

    async fn remove(
        &self,
        user: &str,
        at: &FileLocation,
        recycle: bool,
        _: &RequestContext,
    ) -> Result<(), FileHostError> {
        let recycle = if recycle { " recycle" } else { "" };
        self.record(format!("remove {user} {}{recycle}", place(at)), at)
    }

    async fn discard_upload(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<(), FileHostError> {
        self.record(format!("discard {user} {}", place(at)), at)
    }

    /// The ten letters every file holds, as the archive.
    async fn archive(
        &self,
        user: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError> {
        let file = self.open(user, at, context).await?;
        let mut calls = self.calls.lock().unwrap();
        calls.pop();
        calls.push(format!("archive {user} {}", place(at)));
        Ok(file)
    }

    /// Back to the path without `.recycle/<user>`.
    async fn restore(
        &self,
        user: &str,
        at: &FileLocation,
        _: &RequestContext,
    ) -> Result<FilePath, FileHostError> {
        self.record(format!("restore {user} {}", place(at)), at)?;
        let original = at.path.as_str().splitn(3, '/').nth(2).unwrap_or_default();
        FilePath::parse(original).map_err(|_| FileHostError::Invalid("not in a recycle bin".into()))
    }
}
