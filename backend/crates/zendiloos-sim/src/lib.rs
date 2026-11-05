//! A simulated agent behind the same [`AgentTransport`] as the real one.
//!
//! zendiloosd uses it on a development machine: everything above the
//! transport, including the parsing of tool output, runs as in production,
//! but nothing is executed and nothing needs root.
//!
//! The simulated host is a small NAS ([`host`]): a system NVMe, a healthy
//! and a degraded ZFS mirror (one disk failing), a sleeping disk and an
//! unused NVMe. SMART data comes from `data/`. Its users and groups are
//! those of a Debian system plus whatever is created while it runs; any
//! other name counts as managed, so accounts from an earlier run keep
//! working.
//!
//! With [`SimulatedAgent::with_files`], share contents are real files in a
//! scratch directory, handled by the same code as in the files agent but
//! without taking on the user's credentials: nothing is refused for lack of
//! permissions.

mod host;

use std::collections::HashSet;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, DiskName, ErrorCode, FileLocation, Layout, PROTOCOL_VERSION, Request, RequestId,
    Response, TransportError,
};
use zendiloos_fileops::{FileError, ShareRoot};

/// Users and groups of the simulated Debian system, which the agent never
/// changes.
const SYSTEM_USERS: [&str; 7] = ["root", "daemon", "bin", "sys", "nobody", "debian", "zendiloos"];
const SYSTEM_GROUPS: [&str; 6] = ["root", "adm", "sudo", "users", "zendiloos", "zendiloos-users"];

const MDSTAT: &str = "Personalities : \nunused devices: <none>\n";
const SMART_HDD: &str = include_str!("../data/smartctl-hdd.json");
const SMART_HDD_FAILING: &str = include_str!("../data/smartctl-hdd-failing.json");
const SMART_NVME: &str = include_str!("../data/smartctl-nvme.json");
const SMART_STANDBY: &str = include_str!("../data/smartctl-standby.json");
const SMART_UNKNOWN: &str = r#"{"smartctl": {"messages": [{"string": "/dev/{{DEVICE}}: No such device",
    "severity": "error"}], "exit_status": 2}}"#;

/// Unix seconds; replaceable so tests control scrub and resilver progress.
pub type SimClock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct SimulatedAgent {
    received: Mutex<Vec<Request>>,
    /// Users and groups created while running.
    created: Mutex<HashSet<(Kind, String)>>,
    host: Mutex<host::Host>,
    clock: SimClock,
    /// Where `/volume<N>/<share>` directories are kept, if anywhere.
    files: Option<PathBuf>,
}

impl std::fmt::Debug for SimulatedAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedAgent").finish_non_exhaustive()
    }
}

impl Default for SimulatedAgent {
    fn default() -> Self {
        Self::with_clock(Arc::new(|| {
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    User,
    Group,
}

fn system(kind: Kind, name: &str) -> bool {
    match kind {
        Kind::User => SYSTEM_USERS.contains(&name),
        Kind::Group => SYSTEM_GROUPS.contains(&name),
    }
}

fn changed(result: Result<(), host::Refusal>) -> Response {
    match result {
        Ok(()) => Response::Done,
        Err(host::Refusal::Refused(reason)) => error(ErrorCode::Forbidden, &reason),
        Err(host::Refusal::Conflict) => error(ErrorCode::Conflict, "the name or volume is taken"),
    }
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

impl SimulatedAgent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock(clock: SimClock) -> Self {
        Self {
            received: Mutex::default(),
            created: Mutex::default(),
            host: Mutex::new(host::Host::nas()),
            clock,
            files: None,
        }
    }

    /// Keeps share contents in `root`, as `root/volume<N>/<share>`.
    pub fn with_files(mut self, root: impl Into<PathBuf>) -> Self {
        self.files = Some(root.into());
        self
    }

    fn share_directory(&self, volume: u16, share: &str) -> Option<PathBuf> {
        Some(self.files.as_ref()?.join(format!("volume{volume}")).join(share))
    }

    fn file_operation(&self, request: &Request) -> (Response, Option<OwnedFd>) {
        let Some(root) = &self.files else {
            return (error(ErrorCode::Failed, "the simulated agent keeps no files"), None);
        };
        let share = |location: &FileLocation| {
            // Shares from an earlier run have no directory yet.
            if let Some(directory) = self.share_directory(location.volume.get(), location.share.as_str()) {
                std::fs::create_dir_all(directory)?;
            }
            ShareRoot::open(root, location.volume, &location.share, false)
        };
        let result = (|| -> Result<(Response, Option<OwnedFd>), FileError> {
            Ok(match request {
                Request::ListDirectory { location, .. } => {
                    let listing = share(location)?.list(&location.path)?;
                    let (entries, truncated, writable) =
                        (listing.entries, listing.truncated, listing.writable);
                    (Response::Directory { entries, truncated, writable }, None)
                }
                Request::OpenFile { location, .. } => {
                    let (file, size) = share(location)?.open_file(&location.path)?;
                    (Response::File { size }, Some(file))
                }
                Request::CreateUpload { location, .. } => {
                    let (file, name) = share(location)?.create_upload(&location.path)?;
                    (Response::Upload { name }, Some(file))
                }
                Request::MakeDirectory { location, .. } => {
                    share(location)?.make_directory(&location.path)?;
                    (Response::Done, None)
                }
                Request::MoveEntry { from, to, replace, .. } => {
                    share(from)?.move_entry(&from.path, &share(to)?, &to.path, *replace)?;
                    (Response::Done, None)
                }
                Request::CopyEntry { from, to, .. } => {
                    let path = share(from)?.copy_entry(&from.path, &share(to)?, &to.path)?;
                    (Response::Copied { path }, None)
                }
                Request::RemoveEntry { user, location, recycle } => {
                    share(location)?.remove(&location.path, recycle.then_some(user.as_str()))?;
                    (Response::Done, None)
                }
                other => (
                    error(ErrorCode::InvalidRequest, &format!("{} is not a file operation", other.name())),
                    None,
                ),
            })
        })();
        result.unwrap_or_else(|failure| {
            let code = match failure {
                FileError::NotFound => ErrorCode::NotFound,
                FileError::Denied => ErrorCode::Denied,
                FileError::Exists => ErrorCode::Conflict,
                FileError::Invalid(_) => ErrorCode::InvalidRequest,
                FileError::Io(_) => ErrorCode::Failed,
            };
            (error(code, &failure.to_string()), None)
        })
    }

    fn host(&self) -> std::sync::MutexGuard<'_, host::Host> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every request received so far, oldest first.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn create(&self, kind: Kind, name: &str) -> Response {
        let mut created = self.created.lock().unwrap_or_else(PoisonError::into_inner);
        if system(kind, name) || !created.insert((kind, name.to_owned())) {
            return error(ErrorCode::Conflict, "the name is already taken");
        }
        Response::Done
    }

    fn change(&self, kind: Kind, name: &str, delete: bool) -> Response {
        if system(kind, name) {
            return error(ErrorCode::Forbidden, "not managed by zendiloos");
        }
        if delete {
            self.created.lock().unwrap_or_else(PoisonError::into_inner).remove(&(kind, name.to_owned()));
        }
        Response::Done
    }
}

#[async_trait]
impl AgentTransport for SimulatedAgent {
    async fn call(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<Response, TransportError> {
        let request_id = request_id.as_ref().map(RequestId::as_str).unwrap_or("-");
        let response = match &request {
            Request::Hello {} => Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: concat!(env!("CARGO_PKG_VERSION"), "-simulated").to_owned(),
            },
            Request::Reboot {} | Request::PowerOff {} => {
                tracing::warn!(op = request.name(), request_id, "simulated agent: not executing");
                Response::Done
            }
            Request::ListBlockDevices {} => Response::BlockDevices { lsblk_json: self.host().lsblk() },
            Request::SmartData { device } => smart(device),
            Request::ArrayStatus {} => Response::ArrayStatus { mdstat: MDSTAT.to_owned() },
            Request::ZfsStatus {} => {
                let now = (self.clock)();
                let mut host = self.host();
                Response::Zfs { zpool_status: host.zpool_status(now), zfs_list: host.zfs_list() }
            }
            Request::CreatePool { name, layout, disks, volume } => {
                let layout = match layout {
                    Layout::Single => host::Layout::Single,
                    Layout::Mirror => host::Layout::Mirror,
                    Layout::Raidz1 => host::Layout::Raidz(1),
                    Layout::Raidz2 => host::Layout::Raidz(2),
                    Layout::Raidz3 => host::Layout::Raidz(3),
                };
                let ids: Vec<&str> = disks.iter().map(|id| id.as_str()).collect();
                changed(self.host().create(name.as_str(), layout, &ids, volume.get()))
            }
            Request::DestroyPool { name, volume } => {
                changed(self.host().destroy(name.as_str(), volume.get()))
            }
            Request::ReplaceDisk { pool, device, disk } => {
                let now = (self.clock)();
                changed(self.host().replace(pool.as_str(), *device, disk.as_str(), now))
            }
            Request::ScrubPool { pool } => {
                let now = (self.clock)();
                changed(self.host().scrub(pool.as_str(), now))
            }
            Request::StopScrub { pool } => {
                let now = (self.clock)();
                changed(self.host().stop_scrub(pool.as_str(), now))
            }
            Request::CreateSnapshot { pool, volume, share, label, schedules, note } => {
                let names: Vec<&str> = schedules.iter().map(|f| f.as_str()).collect();
                let snapshot = host::SimSnapshot {
                    share: share.as_str().to_owned(),
                    label: label.as_str().to_owned(),
                    created: (self.clock)(),
                    schedules: if names.is_empty() { "-".to_owned() } else { names.join(",") },
                    note: note.as_ref().map_or("-".to_owned(), |n| n.as_str().to_owned()),
                };
                changed(self.host().create_snapshot(pool.as_str(), volume.get(), share.as_str(), snapshot))
            }
            Request::DestroySnapshot { pool, volume, share, label } => changed(self.host().destroy_snapshot(
                pool.as_str(),
                volume.get(),
                share.as_str(),
                label.as_str(),
            )),
            Request::ListSnapshots {} => Response::Snapshots { zfs_list: self.host().snapshot_list() },
            // The console belongs to the machine the simulation pretends to be.
            Request::ShowOnConsole { .. } => Response::Done,
            Request::CreateShare { pool, volume, name } => {
                let created = self.host().create_share(pool.as_str(), volume.get(), name.as_str());
                if let (Ok(()), Some(directory)) =
                    (&created, self.share_directory(volume.get(), name.as_str()))
                    && let Err(error) = std::fs::create_dir_all(&directory)
                {
                    tracing::warn!(%error, directory = %directory.display(), "simulated agent: no share directory");
                }
                changed(created)
            }
            Request::DestroyShare { pool, volume, name } => {
                let destroyed = self.host().destroy_share(pool.as_str(), volume.get(), name.as_str());
                if let (Ok(()), Some(directory)) =
                    (&destroyed, self.share_directory(volume.get(), name.as_str()))
                {
                    let _ = std::fs::remove_dir_all(directory);
                }
                changed(destroyed)
            }
            Request::ApplySharing { smb, nfs } => {
                tracing::info!(
                    smb = smb.len(),
                    nfs = nfs.len(),
                    "simulated agent: sharing configuration not written"
                );
                Response::Done
            }
            Request::SetShareAccess { volume, name, .. } => {
                changed(self.host().share_present(volume.get(), name.as_str()))
            }
            Request::CreateUser { name } => self.create(Kind::User, name.as_str()),
            Request::DeleteUser { name } => self.change(Kind::User, name.as_str(), true),
            Request::SetSmbPassword { name, .. } | Request::SetUserEnabled { name, .. } => {
                self.change(Kind::User, name.as_str(), false)
            }
            Request::CreateGroup { name } => self.create(Kind::Group, name.as_str()),
            Request::DeleteGroup { name } => self.change(Kind::Group, name.as_str(), true),
            Request::SetGroupMembers { group, members } => {
                match members.iter().find(|member| system(Kind::User, member.as_str())) {
                    Some(_) => error(ErrorCode::Forbidden, "not managed by zendiloos"),
                    None => self.change(Kind::Group, group.as_str(), false),
                }
            }
            // A descriptor that comes along is closed here.
            Request::ListDirectory { .. }
            | Request::OpenFile { .. }
            | Request::CreateUpload { .. }
            | Request::MakeDirectory { .. }
            | Request::MoveEntry { .. }
            | Request::CopyEntry { .. }
            | Request::RemoveEntry { .. } => self.file_operation(&request).0,
        };
        self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        Ok(response)
    }

    async fn call_with_file(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<(Response, Option<OwnedFd>), TransportError> {
        match request {
            Request::OpenFile { .. } | Request::CreateUpload { .. } => {
                let answer = self.file_operation(&request);
                self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                Ok(answer)
            }
            other => Ok((self.call(other, request_id).await?, None)),
        }
    }
}

/// smartctl's answer for each simulated disk, with its exit status.
fn smart(device: &DiskName) -> Response {
    let (template, exit_status, model, serial) = match device.as_str() {
        "sda" => (SMART_HDD, 0, "", "SIM4T0001"),
        "sdb" => (SMART_HDD, 0, "", "SIM4T0002"),
        "sdc" => (SMART_HDD, 0, "", "SIM4T0003"),
        "sdd" => (SMART_HDD_FAILING, 8 | 16 | 64 | 128, "", "SIM4T0004"),
        "sde" => (SMART_STANDBY, 2, "", ""),
        "nvme0n1" => (SMART_NVME, 0, "SIM NVMe SSD 512GB", "SIMNV0001"),
        "nvme1n1" => (SMART_NVME, 0, "SIM NVMe SSD 1TB", "SIMNV0002"),
        _ => (SMART_UNKNOWN, 2, "", ""),
    };
    let smartctl_json = template
        .replace("{{DEVICE}}", device.as_str())
        .replace("{{MODEL}}", model)
        .replace("{{SERIAL}}", serial);
    Response::Smart { exit_status, smartctl_json }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_like_the_agent_without_doing_anything() {
        let agent = SimulatedAgent::new();
        let hello = agent.call(Request::Hello {}, None).await.unwrap();
        assert!(matches!(hello, Response::Hello { protocol_version: PROTOCOL_VERSION, .. }));
        assert_eq!(agent.call(Request::Reboot {}, None).await.unwrap(), Response::Done);
        assert_eq!(agent.received(), [Request::Hello {}, Request::Reboot {}]);
    }

    fn at(seconds: u64) -> SimulatedAgent {
        SimulatedAgent::with_clock(Arc::new(move || seconds))
    }

    async fn zfs(agent: &SimulatedAgent) -> serde_json::Value {
        match agent.call(Request::ZfsStatus {}, None).await.unwrap() {
            Response::Zfs { zpool_status, .. } => serde_json::from_str(&zpool_status).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn keeps_share_contents_in_a_directory() {
        use std::io::{Read, Write};

        use zendiloos_agent_proto::{PoolName, ShareName, SharePath, UserName, VolumeNumber};
        let root = tempfile::tempdir().unwrap();
        let agent = SimulatedAgent::new().with_files(root.path());
        let (pool, volume, name) = (
            PoolName::parse("tank").unwrap(),
            VolumeNumber::new(1).unwrap(),
            ShareName::parse("docs").unwrap(),
        );
        let created =
            agent.call(Request::CreateShare { pool, volume, name: name.clone() }, None).await.unwrap();
        assert_eq!(created, Response::Done);
        let at =
            |path: &str| FileLocation { volume, share: name.clone(), path: SharePath::parse(path).unwrap() };
        let user = UserName::parse("anna").unwrap();

        let (answer, file) = agent
            .call_with_file(Request::CreateUpload { user: user.clone(), location: at("") }, None)
            .await
            .unwrap();
        let Response::Upload { name: upload } = answer else { panic!("{answer:?}") };
        std::fs::File::from(file.unwrap()).write_all(b"hello").unwrap();
        let commit =
            Request::MoveEntry { user: user.clone(), from: at(&upload), to: at("a.txt"), replace: false };
        assert_eq!(agent.call(commit, None).await.unwrap(), Response::Done);

        let (answer, file) = agent
            .call_with_file(Request::OpenFile { user: user.clone(), location: at("a.txt") }, None)
            .await
            .unwrap();
        assert_eq!(answer, Response::File { size: 5 });
        let mut text = String::new();
        std::fs::File::from(file.unwrap()).read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");

        let listing = agent.call(Request::ListDirectory { user, location: at("") }, None).await.unwrap();
        let Response::Directory { entries, .. } = listing else { panic!("{listing:?}") };
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a.txt"]);
        let missing =
            agent.call(Request::OpenFile { user: UserName::parse("x").unwrap(), location: at("b") }, None);
        assert!(matches!(missing.await.unwrap(), Response::Error { code: ErrorCode::NotFound, .. }));
    }

    #[tokio::test]
    async fn creates_destroys_and_refuses_like_the_agent() {
        use zendiloos_agent_proto::{DiskId, PoolName, VolumeNumber};
        let agent = at(1_000);
        let create = |name: &str, disks: &[&str], volume| Request::CreatePool {
            name: PoolName::parse(name).unwrap(),
            layout: Layout::Mirror,
            disks: disks.iter().map(|d| DiskId::parse(d).unwrap()).collect(),
            volume: VolumeNumber::new(volume).unwrap(),
        };
        let free = ["ata-SIM_HDD_4TB_SIM4T0005", "nvme-SIM_NVMe_SSD_1TB_SIMNV0002"];
        let busy =
            agent.call(create("media", &["ata-SIM_HDD_4TB_SIM4T0001", free[0]], 3), None).await.unwrap();
        assert!(matches!(busy, Response::Error { code: ErrorCode::Forbidden, .. }), "{busy:?}");
        let taken = agent.call(create("tank", &free, 3), None).await.unwrap();
        assert!(matches!(taken, Response::Error { code: ErrorCode::Conflict, .. }), "{taken:?}");
        assert_eq!(agent.call(create("media", &free, 3), None).await.unwrap(), Response::Done);
        assert_eq!(zfs(&agent).await["pools"]["media"]["state"], "ONLINE");

        let destroy = Request::DestroyPool {
            name: PoolName::parse("media").unwrap(),
            volume: VolumeNumber::new(3).unwrap(),
        };
        assert_eq!(agent.call(destroy, None).await.unwrap(), Response::Done);
        assert!(zfs(&agent).await["pools"].get("media").is_none());
    }

    #[tokio::test]
    async fn a_replacement_resilvers_and_then_leaves_the_old_device() {
        use zendiloos_agent_proto::{DiskId, PoolName};
        let start = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = start.clone();
        let agent =
            SimulatedAgent::with_clock(Arc::new(move || clock.load(std::sync::atomic::Ordering::Relaxed)));
        let status = zfs(&agent).await;
        let mirror = &status["pools"]["backup"]["vdevs"]["backup"]["vdevs"]["mirror-0"]["vdevs"];
        let failing = mirror.as_object().unwrap().values().find(|d| d["state"] == "FAULTED").unwrap();
        let device = failing["guid"].as_u64().unwrap();

        let replace = Request::ReplaceDisk {
            pool: PoolName::parse("backup").unwrap(),
            device,
            disk: DiskId::parse("ata-SIM_HDD_4TB_SIM4T0005").unwrap(),
        };
        assert_eq!(agent.call(replace, None).await.unwrap(), Response::Done);
        let resilvering = zfs(&agent).await;
        assert_eq!(resilvering["pools"]["backup"]["scan_stats"]["function"], "RESILVER");
        assert_eq!(resilvering["pools"]["backup"]["scan_stats"]["state"], "SCANNING");

        start.store(1_000 + host::SCAN_SECONDS, std::sync::atomic::Ordering::Relaxed);
        let done = zfs(&agent).await;
        assert_eq!(done["pools"]["backup"]["state"], "ONLINE");
        assert_eq!(done["pools"]["backup"]["scan_stats"]["state"], "FINISHED");
    }

    #[tokio::test]
    async fn a_scrub_can_be_stopped_while_it_runs() {
        use zendiloos_agent_proto::PoolName;
        let start = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = start.clone();
        let agent =
            SimulatedAgent::with_clock(Arc::new(move || clock.load(std::sync::atomic::Ordering::Relaxed)));
        let tank = || PoolName::parse("tank").unwrap();

        let idle = agent.call(Request::StopScrub { pool: tank() }, None).await.unwrap();
        assert!(matches!(idle, Response::Error { .. }), "{idle:?}");
        assert_eq!(agent.call(Request::ScrubPool { pool: tank() }, None).await.unwrap(), Response::Done);
        start.store(1_002, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(agent.call(Request::StopScrub { pool: tank() }, None).await.unwrap(), Response::Done);

        start.store(1_000 + host::SCAN_SECONDS, std::sync::atomic::Ordering::Relaxed);
        let stats = &zfs(&agent).await["pools"]["tank"]["scan_stats"];
        assert_eq!((stats["state"].as_str(), stats["end_time"].as_u64()), (Some("CANCELED"), Some(1_002)));
        assert_eq!(stats["pass_start"], 1_000);
    }
}
