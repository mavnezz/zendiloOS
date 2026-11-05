//! In-memory fakes of this context's ports, for tests here and in the API
//! crate (feature `testing`).

use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::{
    self, FileHost, FileHostError, HostError, ShareHost, ShareRepository, Volume,
};
use crate::domain::{
    Access, Entry, EntryKind, FileLocation, FilePath, Frequency, Listing, NewShare, Principal, Share,
    ShareId, ShareName, ShareSettings, ShareUsage, SharingConfig, Snapshot, SnapshotSchedule,
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

/// What every file of the fake host holds.
pub const FILE_CONTENT: &str = "abcdefghij";

#[derive(Default)]
pub struct Shares(Mutex<Vec<Share>>, Mutex<std::collections::HashMap<ShareId, Vec<SnapshotSchedule>>>);

#[async_trait]
impl ShareRepository for Shares {
    async fn list(&self) -> Result<Vec<Share>, PortError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError> {
        Ok(self.0.lock().unwrap().iter().find(|s| s.id == id).cloned())
    }

    async fn create(&self, new: NewShare) -> Result<Option<Share>, PortError> {
        let mut shares = self.0.lock().unwrap();
        let share = Share {
            id: ShareId::new(i64::try_from(shares.len()).unwrap() + 1),
            name: new.name,
            description: new.description,
            pool: new.pool,
            volume: new.volume,
            recycle_bin: new.recycle_bin,
            settings: new.settings,
            access: new.access,
            nfs: new.nfs,
            created_at: new.created_at,
            updated_at: new.created_at,
        };
        shares.push(share.clone());
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
}

#[derive(Default)]
pub struct Host {
    pub calls: Mutex<Vec<String>>,
    pub configs: Mutex<Vec<SharingConfig>>,
    pub refuse_access: std::sync::atomic::AtomicBool,
    /// Snapshots as the host has them; taking and destroying change them.
    pub snapshots: Mutex<Vec<Snapshot>>,
    /// Taking snapshots fails, e.g. because the pool is gone.
    pub refuse_snapshots: std::sync::atomic::AtomicBool,
    /// What every share dataset holds, in bytes.
    pub used_bytes: std::sync::atomic::AtomicU64,
}

impl Host {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn shown(principal: &Principal) -> String {
    match principal {
        Principal::User(user) => user.clone(),
        Principal::Group(group) => format!("@{group}"),
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
        Ok(())
    }

    async fn apply(&self, config: &SharingConfig, _: &RequestContext) -> Result<(), HostError> {
        self.calls.lock().unwrap().push(format!("apply {}", config.smb.len()));
        self.configs.lock().unwrap().push(config.clone());
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

    async fn set_access(
        &self,
        volume: u16,
        name: &ShareName,
        access: &[(Principal, Access)],
        removed: &[Principal],
        recursive: bool,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        if self.refuse_access.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(HostError::Refused("share is not mounted".into()));
        }
        let access: Vec<String> = access
            .iter()
            .map(|(p, a)| format!("{}:{}", shown(p), if *a == Access::ReadWrite { "rw" } else { "r" }))
            .collect();
        let removed: Vec<String> = removed.iter().map(shown).collect();
        self.calls.lock().unwrap().push(format!(
            "access {volume} {name} [{}] -[{}]{}",
            access.join(","),
            removed.join(","),
            if recursive { " recursive" } else { "" }
        ));
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

fn place(at: &FileLocation) -> String {
    format!("{}:{}", at.share.as_str(), at.path.as_str())
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
        };
        Ok(Listing { entries: vec![entry], truncated: false, writable: true })
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
        _: &RequestContext,
    ) -> Result<(), FileHostError> {
        self.record(format!("mkdir {user} {}", place(at)), at)
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
}
