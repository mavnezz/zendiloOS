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

mod compose;
mod docker;
mod host;
mod vms;
mod volumes;

use std::collections::HashSet;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, DiskName, ErrorCode, FileLocation, FileRoot, Layout, PROTOCOL_VERSION, PackageReport,
    PackageRun, PackageRunState, PackageTask, Report, Request, RequestId, Response, ShareName, TerminalSize,
    TransportError, UpgradablePackage,
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
    /// How the simulated host manages its network; nothing is changed.
    network: Mutex<SimNetwork>,
    /// Docker's data root, once put on a pool.
    docker: Mutex<Option<String>>,
    /// Whether Docker is installed, and the latest run of apt.
    packages: Mutex<SimPackages>,
    /// Installed apps: whether they run; absent until configured.
    apps: Mutex<std::collections::BTreeMap<String, bool>>,
    /// The Docker engine beside the apps.
    engine: Mutex<docker::SimDocker>,
    /// The clock's settings; the time itself stays the real one.
    time: Mutex<SimTime>,
    /// The oldest TLS version the web interface accepts.
    tls: Mutex<zendiloos_agent_proto::TlsMinimum>,
    /// The SSH server's settings and the time it turns itself off.
    ssh: Mutex<(zendiloos_agent_proto::SshConfig, Option<i64>)>,
    /// Certificates issued, and a change of the web server waiting to be
    /// confirmed; nothing is changed.
    acme: Mutex<SimAcme>,
    /// The firewall's rules in force (none: no table), and a change
    /// waiting to be confirmed with the rules that return without.
    firewall: Mutex<SimFirewall>,
    /// libvirt's domains and the VMs' storage; nothing runs.
    vms: Mutex<vms::SimVms>,
}

#[derive(Debug, Default)]
struct SimFirewall {
    rules: Option<zendiloos_agent_proto::FirewallConfig>,
    pending: Option<(zendiloos_agent_proto::PendingFirewall, Option<zendiloos_agent_proto::FirewallConfig>)>,
}

#[derive(Debug, Default)]
struct SimAcme {
    issued: std::collections::BTreeSet<u32>,
    pending: Option<zendiloos_agent_proto::PendingWebServer>,
}

/// What acme.sh writes while it issues a certificate, and whether it
/// succeeds: names under `.invalid` fail, as a name nobody can show is
/// theirs would.
fn acme_lines(order: &zendiloos_agent_proto::AcmeOrder) -> (Vec<String>, bool) {
    let main = order.domains.main().as_str();
    let mut lines = vec![
        format!(
            "Using CA: https://acme{}-v02.api.letsencrypt.org/directory",
            if order.ca == zendiloos_agent_proto::AcmeCa::LetsEncryptStaging { "-staging" } else { "" }
        ),
        "Creating domain key".to_owned(),
        format!("Single domain='{main}'"),
        format!("Getting webroot for domain='{main}'"),
    ];
    let dns = matches!(order.challenge, zendiloos_agent_proto::AcmeChallenge::Dns { .. });
    let fails = order.domains.as_slice().iter().any(|d| d.as_str().ends_with(".invalid"));
    if dns {
        lines.push(format!("Adding TXT value for domain: _acme-challenge.{}", main.trim_start_matches("*.")));
        lines.push("Sleeping for 20 seconds to wait for the TXT records to take effect".into());
    }
    lines.push(format!("Verifying: {main}"));
    if fails {
        lines.push(format!("{main}: Verify error: DNS problem: NXDOMAIN looking up A for {main}"));
        return (lines, false);
    }
    lines.extend([
        "Success".to_owned(),
        "Verification finished, beginning signing.".to_owned(),
        "Let's finalize the order.".to_owned(),
        "Downloading cert.".to_owned(),
        "Cert success.".to_owned(),
    ]);
    (lines, true)
}

/// The simulated host's network: managed by systemd-networkd, or by
/// ifupdown with these files until taken over.
#[derive(Debug, Default)]
struct SimNetwork {
    pending: Option<zendiloos_agent_proto::PendingNetwork>,
    /// `/etc/network/interfaces` while ifupdown manages the network.
    ifupdown: Option<String>,
    /// What ifupdown had, while a takeover waits for confirmation.
    taken_over: Option<String>,
}

impl SimNetwork {
    /// A change whose time ran out goes back: a takeover gives ifupdown
    /// its files again.
    fn lapse(&mut self, now: i64) {
        if self.pending.as_ref().is_some_and(|p| p.revert_at <= now) {
            self.pending = None;
            if let Some(files) = self.taken_over.take() {
                self.ifupdown = Some(files);
            }
        }
    }
}

/// A digest of the simulated ifupdown's file, as the agent hands one over.
fn ifupdown_digest(text: &str) -> zendiloos_agent_proto::FilesDigest {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    zendiloos_agent_proto::FilesDigest::parse(&format!("{:064x}", hasher.finish()))
        .unwrap_or_else(|_| unreachable!("64 hex digits"))
}

/// Docker's packages and the latest run of apt, simulated.
#[derive(Debug)]
struct SimPackages {
    docker: bool,
    /// acme.sh is installed: not at first, so its plugin can be installed.
    acme: bool,
    /// systemd-resolved is installed; only a host with ifupdown has none.
    resolved: bool,
    /// Kept while Docker is installed, with share contents.
    marker: Option<PathBuf>,
    run: Option<(PackageRun, std::time::Instant)>,
    /// The last update, kept apart as the runner keeps it.
    update: Option<(PackageRun, std::time::Instant)>,
    runs: u64,
    /// Debian has updates for the simulated system, until one is installed.
    pending: bool,
    /// An update brought a newer kernel; the simulated system never restarts.
    updated: bool,
}

impl SimPackages {
    /// The latest run as it stands now: a step a second or two.
    fn current(&self) -> Option<PackageRun> {
        self.run.as_ref().map(|run| self.as_now(run))
    }

    /// The last update as it stands now.
    fn last_update(&self) -> Option<PackageRun> {
        self.update.as_ref().map(|run| self.as_now(run))
    }

    fn as_now(&self, (run, started): &(PackageRun, std::time::Instant)) -> PackageRun {
        let mut run = run.clone();
        let steps: &[(u64, &str)] = match run.task {
            PackageTask::InstallDocker
            | PackageTask::InstallAcme
            | PackageTask::InstallVms
            | PackageTask::InstallNetworkd => &[(0, "lists"), (1, "install"), (3, "")],
            PackageTask::Check => &[(0, "lists"), (2, "")],
            PackageTask::Update if run.release.is_some() => {
                &[(0, "lists"), (1, "debian"), (3, "zendiloos"), (4, "modules"), (5, "")]
            }
            PackageTask::Update => &[(0, "lists"), (1, "debian"), (3, "modules"), (4, "")],
            PackageTask::Finish => &[(0, "configure"), (1, "repair"), (2, "modules"), (3, "")],
        };
        let elapsed = started.elapsed().as_secs();
        for (at, step) in steps {
            if elapsed < *at {
                break;
            }
            if step.is_empty() {
                run.state = PackageRunState::Succeeded;
                run.finished_at = Some(run.started_at + i64::try_from(*at).unwrap_or(0));
                if !matches!(
                    run.task,
                    PackageTask::InstallDocker
                        | PackageTask::InstallAcme
                        | PackageTask::InstallVms
                        | PackageTask::InstallNetworkd
                ) {
                    let done = run.task == PackageTask::Update;
                    run.report = Some(self.report(!done && self.pending, done || self.updated));
                }
            } else {
                run.step = (*step).to_owned();
            }
        }
        run
    }

    /// What apt would say: a security update, another one and a new kernel
    /// while updates are pending.
    fn report(&self, pending: bool, updated: bool) -> PackageReport {
        let package = |name: &str, from: Option<&str>, to: &str, security| UpgradablePackage {
            name: name.into(),
            from: from.map(Into::into),
            to: to.into(),
            security,
        };
        let upgradable = if pending {
            vec![
                package("openssl", Some("3.5.1-1"), "3.5.4-1~deb13u1", true),
                package("tzdata", Some("2025b-4"), "2025c-0+deb13u1", false),
                package("linux-image-6.12.57+deb13-amd64", None, "6.12.57-1", true),
            ]
        } else {
            Vec::new()
        };
        PackageReport {
            upgradable,
            running_kernel: "6.12.48+deb13-amd64".into(),
            newest_kernel: Some(if updated { "6.12.57+deb13-amd64" } else { "6.12.48+deb13-amd64" }.into()),
            zfs_ready: true,
            interrupted: false,
        }
    }
}

/// Time zones the simulated system has: a few, enough to choose from.
const TIME_ZONES: &[&str] = &[
    "America/Los_Angeles",
    "America/New_York",
    "America/Sao_Paulo",
    "Asia/Kolkata",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Australia/Sydney",
    "Etc/UTC",
    "Europe/Amsterdam",
    "Europe/Berlin",
    "Europe/London",
    "Europe/Paris",
    "Europe/Vienna",
    "Europe/Zurich",
    "Pacific/Auckland",
];

#[derive(Debug)]
struct SimTime {
    zone: String,
    network_time: bool,
    servers: Vec<String>,
}

impl Default for SimTime {
    fn default() -> Self {
        Self { zone: "Europe/Berlin".to_owned(), network_time: true, servers: Vec::new() }
    }
}

impl SimTime {
    fn state(&self) -> Response {
        let ntp = if self.network_time { "yes" } else { "no" };
        let timedatectl =
            format!("Timezone={}\nLocalRTC=no\nCanNTP=yes\nNTP={ntp}\nNTPSynchronized={ntp}\n", self.zone);
        let timesync = if self.network_time {
            let server = self.servers.first().map_or("2.debian.pool.ntp.org", String::as_str);
            format!("ServerName={server}\nServerAddress=192.0.2.123\n")
        } else {
            String::new()
        };
        let servers_conf = if self.servers.is_empty() {
            String::new()
        } else {
            format!("[Time]\nNTP={}\n", self.servers.join(" "))
        };
        Response::TimeState { timedatectl, timesync, zones: TIME_ZONES.join("\n") + "\n", servers_conf }
    }
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
            network: Mutex::default(),
            docker: Mutex::default(),
            packages: Mutex::new(SimPackages {
                docker: true,
                acme: false,
                resolved: true,
                marker: None,
                run: None,
                update: None,
                runs: 0,
                pending: true,
                updated: false,
            }),
            apps: Mutex::default(),
            engine: Mutex::default(),
            time: Mutex::default(),
            tls: Mutex::new(zendiloos_agent_proto::TlsMinimum::Tls12),
            ssh: Mutex::new((
                zendiloos_agent_proto::SshConfig {
                    enabled: true,
                    port: zendiloos_agent_proto::SshPort::new(22).unwrap_or_else(|_| unreachable!()),
                    root_login: None,
                    password_login: None,
                    off_after: None,
                },
                None,
            )),
            acme: Mutex::default(),
            firewall: Mutex::default(),
            vms: Mutex::default(),
        }
    }

    /// Keeps share contents in `root`, as `root/volume<N>/<share>`, and the
    /// USB stick's as `root/removable/<id>`, with a file on it to copy;
    /// Docker counts as installed once it was installed there.
    pub fn with_files(mut self, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let stick = root.join("removable").join(host::STICK_ID);
        if !stick.exists() && std::fs::create_dir_all(stick.join("Photos")).is_ok() {
            let _ = std::fs::write(stick.join("Photos/holiday.txt"), "A file on the simulated USB stick.\n");
        }
        let marker = root.join(".docker-installed");
        {
            let mut packages = self.packages.lock().unwrap_or_else(PoisonError::into_inner);
            packages.docker = marker.exists();
            packages.marker = Some(marker);
        }
        self.files = Some(root);
        self
    }

    /// A host as the Debian installer leaves it: ifupdown manages its
    /// network with `interfaces`, systemd-resolved is not installed. Taken
    /// over, systemd-networkd manages it, unless the takeover lapses.
    pub fn with_ifupdown(self, interfaces: &str) -> Self {
        self.network.lock().unwrap_or_else(PoisonError::into_inner).ifupdown = Some(interfaces.to_owned());
        self.packages.lock().unwrap_or_else(PoisonError::into_inner).resolved = false;
        self
    }

    /// Has `app` installed the earlier way, stopped: apps are no longer
    /// installed so, only run and removed.
    pub fn with_app(self, app: &str) -> Self {
        self.apps().insert(app.to_owned(), false);
        self
    }

    fn share_directory(&self, volume: u16, share: &str) -> Option<PathBuf> {
        Some(self.files.as_ref()?.join(format!("volume{volume}")).join(share))
    }

    fn file_operation(&self, request: &Request) -> (Response, Option<OwnedFd>) {
        let Some(root) = &self.files else {
            return (error(ErrorCode::Failed, "the simulated agent keeps no files"), None);
        };
        // The user every file request is for: personal folders are theirs.
        let user = match request {
            Request::ListDirectory { user, .. }
            | Request::SearchFiles { user, .. }
            | Request::EntryFacts { user, .. }
            | Request::OpenFile { user, .. }
            | Request::CreateUpload { user, .. }
            | Request::MakeDirectory { user, .. }
            | Request::MoveEntry { user, .. }
            | Request::CopyEntry { user, .. }
            | Request::RemoveEntry { user, .. }
            | Request::DiscardUpload { user, .. }
            | Request::RestoreEntry { user, .. }
            | Request::ArchiveFolder { user, .. } => Some(user.as_str()),
            _ => None,
        };
        let share = |location: &FileLocation| {
            // The simulation keeps no snapshots.
            if location.snapshot.is_some() {
                return Err(FileError::NotFound);
            }
            // A personal folder is kept as the directory `home-<user>`.
            let name = match &location.root {
                FileRoot::Share(share) => share.clone(),
                FileRoot::Home => ShareName::parse(&format!("home-{}", user.unwrap_or("nobody")))
                    .map_err(|_| FileError::NotFound)?,
                // A removable drive as `removable/<id>`, whatever its state:
                // zendiloosd asks only for one it lists as mounted.
                FileRoot::Removable(id) => {
                    let places = root.join("removable");
                    std::fs::create_dir_all(places.join(id.as_str()))?;
                    return ShareRoot::open_removable(&places, id, false);
                }
            };
            // Shares from an earlier run have no directory yet.
            if let Some(directory) = self.share_directory(location.volume.get(), name.as_str()) {
                std::fs::create_dir_all(directory)?;
            }
            ShareRoot::open(root, location.volume, &name, false)
        };
        let result = (|| -> Result<(Response, Option<OwnedFd>), FileError> {
            Ok(match request {
                Request::ListDirectory { location, .. } => {
                    let listing = share(location)?.list(&location.path)?;
                    let (entries, truncated, writable) =
                        (listing.entries, listing.truncated, listing.writable);
                    (Response::Directory { entries, truncated, writable }, None)
                }
                Request::SearchFiles { location, text, hidden, .. } => {
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
                    let bounds = zendiloos_fileops::SearchBounds { until, stop: &stop };
                    let found = share(location)?.search(&location.path, text, *hidden, bounds)?;
                    (Response::Found { entries: found.entries, stopped: found.stopped }, None)
                }
                Request::EntryFacts { location, user } => {
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    let bounds = zendiloos_fileops::SearchBounds { until, stop: &stop };
                    let facts = share(location)?.facts(&location.path, bounds)?;
                    // Everything in the simulated volumes is the developer's;
                    // shown as the person asking.
                    let facts = zendiloos_agent_proto::EntryFacts {
                        folder: facts.folder,
                        bytes: facts.bytes,
                        files: facts.files,
                        folders: facts.folders,
                        hidden: facts.hidden,
                        stopped: facts.stopped,
                        created: facts.created,
                        modified: facts.modified,
                        owner_uid: facts.uid,
                        owner: Some(user.as_str().to_owned()),
                    };
                    (Response::Facts { facts }, None)
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
                    match share(from)?.move_entry(&from.path, &share(to)?, &to.path, *replace) {
                        Err(FileError::PartlyMoved(kept)) => {
                            let (kept, example, too_many) = (kept.count, kept.example, kept.too_many);
                            (Response::PartlyMoved { kept, example, too_many }, None)
                        }
                        other => {
                            other?;
                            (Response::Done, None)
                        }
                    }
                }
                Request::CopyEntry { from, to, .. } => {
                    let path = share(from)?.copy_entry(&from.path, &share(to)?, &to.path)?;
                    (Response::Copied { path }, None)
                }
                Request::RemoveEntry { user, location, recycle } => {
                    share(location)?.remove(&location.path, recycle.then_some(user.as_str()))?;
                    (Response::Done, None)
                }
                Request::DiscardUpload { location, .. } => {
                    share(location)?.discard_upload(&location.path)?;
                    (Response::Done, None)
                }
                Request::RestoreEntry { user, location } => {
                    let path = share(location)?.restore(&location.path, user.as_str())?;
                    (Response::Restored { path }, None)
                }
                Request::ArchiveFolder { location, .. } => {
                    let (share, path) = (share(location)?, location.path.clone());
                    let fallback = match &location.root {
                        FileRoot::Share(share) => share.as_str().to_owned(),
                        FileRoot::Home => "home".to_owned(),
                        FileRoot::Removable(id) => id.as_str().to_owned(),
                    };
                    let name = path.file_name().unwrap_or(&fallback).to_owned();
                    let plan = share.plan_archive(&path, &name)?;
                    let (reader, writer) = std::io::pipe()?;
                    let size = plan.length();
                    std::thread::spawn(move || {
                        let mut out = std::io::BufWriter::new(writer);
                        let _ = share.write_archive(&path, &plan, &mut out);
                    });
                    (Response::Archive { size }, Some(OwnedFd::from(reader)))
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
                FileError::Invalid(_) | FileError::CaseClash(..) => ErrorCode::InvalidRequest,
                FileError::PartlyMoved(_)
                | FileError::NotDurable(_)
                | FileError::ArchiveChanged(_)
                | FileError::Io(_) => ErrorCode::Failed,
            };
            (error(code, &failure.to_string()), None)
        })
    }

    fn apps(&self) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<String, bool>> {
        self.apps.lock().unwrap_or_else(PoisonError::into_inner)
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
        let now = (self.clock)();
        let docker = {
            let mut apps = self.apps();
            self.engine.lock().unwrap_or_else(PoisonError::into_inner).handle(
                &request,
                &mut apps,
                now,
                self.files.as_deref(),
            )
        };
        if let Some(response) = docker {
            tracing::debug!(request_id, request = request.name(), "simulated Docker");
            self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
            return Ok(response);
        }
        let response = match &request {
            Request::Hello {} => Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: concat!(env!("CARGO_PKG_VERSION"), "-simulated").to_owned(),
            },
            Request::Reboot {} | Request::PowerOff {} => {
                tracing::warn!(op = request.name(), request_id, "simulated agent: not executing");
                Response::Done
            }
            Request::ListBlockDevices {} => {
                let host = self.host();
                Response::BlockDevices { lsblk_json: host.lsblk(), udev_db: host.udev_db() }
            }
            // The simulated NAS is the catalog's first model.
            Request::Machine {} => {
                Response::Machine { sys_vendor: "UGREEN\n".into(), product_name: "DXP4800 Plus\n".into() }
            }
            // Reads nothing: answers at once, as if the time were over.
            Request::ReadBurst { device, .. } => {
                if self.host().has_disk(device.as_str()) {
                    Response::Done
                } else {
                    error(ErrorCode::Forbidden, &format!("refused: there is no disk {}", device.as_str()))
                }
            }
            Request::SmartData { device } => smart(device),
            // The simulated NAS has no eMMC: a card without files.
            Request::EmmcHealth { .. } => Response::Emmc {
                kind: String::new(),
                life_time: String::new(),
                pre_eol_info: String::new(),
                name: String::new(),
                serial: String::new(),
                fwrev: String::new(),
            },
            // Docker runs once installed and its data is on a pool.
            Request::DockerState {} => {
                let data_root = self.docker.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let installed = self.packages.lock().unwrap_or_else(PoisonError::into_inner).docker;
                let running = installed && data_root.is_some();
                let version = running.then(|| "26.1.5+dfsg1".to_owned());
                Response::DockerState { installed, running, data_root, version }
            }
            // apt takes three seconds: lists, then the packages.
            Request::StartPackages { task, release } => {
                let mut packages = self.packages.lock().unwrap_or_else(PoisonError::into_inner);
                if packages.current().is_some_and(|run| run.state == PackageRunState::Running) {
                    return Ok(error(ErrorCode::Forbidden, "refused: apt is busy with another run"));
                }
                if let Some(release) = release
                    && !release.channel.takes(release.package.kind())
                {
                    let refused =
                        format!("refused: the {} channel does not take it", release.channel.as_str());
                    return Ok(error(ErrorCode::Forbidden, &refused));
                }
                packages.runs += 1;
                let id = format!("{:016x}", packages.runs);
                let run = PackageRun {
                    id: id.clone(),
                    task: *task,
                    state: PackageRunState::Running,
                    step: if *task == PackageTask::Finish { "configure" } else { "lists" }.into(),
                    started_at: i64::try_from((self.clock)()).unwrap_or(i64::MAX),
                    finished_at: None,
                    error: None,
                    release: release.as_ref().map(|release| release.package.version().to_owned()),
                    report: None,
                };
                let now = std::time::Instant::now();
                if task.updates() {
                    packages.update = Some((run.clone(), now));
                }
                packages.run = Some((run, now));
                Response::PackagesStarted { id }
            }
            Request::PackagesState {} => {
                let mut packages = self.packages.lock().unwrap_or_else(PoisonError::into_inner);
                let run = packages.current();
                if run.as_ref().is_some_and(|run| {
                    run.state == PackageRunState::Succeeded && run.task == PackageTask::Update
                }) {
                    packages.pending = false;
                    packages.updated = true;
                }
                if run.as_ref().is_some_and(|run| {
                    run.state == PackageRunState::Succeeded && run.task == PackageTask::InstallDocker
                }) {
                    packages.docker = true;
                    if let Some(marker) = &packages.marker {
                        let _ = std::fs::create_dir_all(marker.parent().unwrap_or(marker));
                        let _ = std::fs::write(marker, "");
                    }
                }
                if run.as_ref().is_some_and(|run| {
                    run.state == PackageRunState::Succeeded && run.task == PackageTask::InstallAcme
                }) {
                    packages.acme = true;
                }
                if run.as_ref().is_some_and(|run| {
                    run.state == PackageRunState::Succeeded && run.task == PackageTask::InstallVms
                }) {
                    self.vms.lock().unwrap_or_else(PoisonError::into_inner).installed = true;
                }
                if run.as_ref().is_some_and(|run| {
                    run.state == PackageRunState::Succeeded && run.task == PackageTask::InstallNetworkd
                }) {
                    packages.resolved = true;
                }
                let update = packages.last_update().map(Box::new);
                Response::Packages { run, update }
            }
            Request::EnableDocker { .. }
                if !self.packages.lock().unwrap_or_else(PoisonError::into_inner).docker =>
            {
                error(ErrorCode::Forbidden, "refused: Docker is not installed")
            }
            Request::EnableDocker { volume, .. } => {
                *self.docker.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(format!("/volume{}/@docker", volume.get()));
                Response::Done
            }
            Request::MoveDocker { from_volume, to_volume, .. } => {
                let mut root = self.docker.lock().unwrap_or_else(PoisonError::into_inner);
                if root.as_deref() == Some(format!("/volume{}/@docker", from_volume.get()).as_str()) {
                    *root = Some(format!("/volume{}/@docker", to_volume.get()));
                    Response::Done
                } else {
                    error(ErrorCode::Forbidden, "refused: Docker's data is not on that pool")
                }
            }
            // The simulated host is managed by systemd-networkd (or ifupdown,
            // until taken over); a change waits for confirmation and lapses
            // at its time.
            // Apps installed the earlier way (`with_app`) run nowhere.
            Request::DeleteAppAccount { .. } => Response::Done,
            Request::StartApp { app } | Request::StopApp { app } => {
                let mut apps = self.apps();
                match apps.get_mut(app.as_str()) {
                    Some(running) => {
                        *running = matches!(request, Request::StartApp { .. });
                        Response::Done
                    }
                    None => {
                        error(ErrorCode::Forbidden, &format!("refused: {} is not installed", app.as_str()))
                    }
                }
            }
            Request::RemoveApp { app, .. } => {
                self.apps().remove(app.as_str());
                Response::Done
            }
            // Every account is UID 2000; the shares' group is 995; the machine
            // has a graphics card and nothing else.
            Request::HostFacts { devices, .. } => Response::HostFacts {
                uid: 2000,
                gid: 995,
                time_zone: "Europe/Berlin".into(),
                devices: devices.iter().filter(|device| device.as_str() == "/dev/dri").cloned().collect(),
            },
            // Answered by the simulated Docker engine above.
            Request::AppsStatus {}
            | Request::DockerContainers {}
            | Request::DockerStats {}
            | Request::DockerImages {}
            | Request::DockerNetworks {}
            | Request::DockerDiskUsage {}
            | Request::ControlContainer { .. }
            | Request::ContainerLogs { .. }
            | Request::InspectContainer { .. }
            | Request::ContainerProcesses { .. }
            | Request::RemoveImage { .. }
            | Request::PullImage { .. }
            | Request::PruneImages {}
            | Request::CreateNetwork { .. }
            | Request::RemoveNetwork { .. }
            | Request::CheckCompose { .. }
            | Request::ControlProject { .. }
            | Request::DeployProject { .. }
            | Request::ProjectLogs { .. }
            | Request::ProjectFiles { .. }
            | Request::DockerVolumes {}
            | Request::DockerVolumeSizes {}
            | Request::RemoveVolume { .. }
            | Request::PruneVolumes {}
            | Request::CreateVolume { .. }
            | Request::BackUpVolume { .. }
            | Request::RestoreVolume { .. }
            | Request::RemoveVolumeBackup { .. }
            | Request::EndVolumeWork { .. }
            | Request::ScanVolumeBackups { .. } => {
                error(ErrorCode::Failed, "simulated Docker did not answer")
            }
            // Its terminal comes with [`AgentTransport::call_with_file`].
            Request::OpenTerminal { .. } => {
                error(ErrorCode::InvalidRequest, "a terminal is opened with its descriptor")
            }
            Request::FinishVmImageUpload { image } => {
                let upload = self.vms.lock().unwrap_or_else(PoisonError::into_inner).uploads.remove(image);
                match upload {
                    None => error(
                        ErrorCode::Forbidden,
                        &format!("refused: no upload of image {} is under way", image.as_str()),
                    ),
                    Some((announced, thread)) => {
                        // The pipe is closed before the end is asked for: the
                        // thread has read it all, or is about to.
                        let received = thread.join().unwrap_or(0);
                        self.vms
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .finish_upload(image, announced, received)
                    }
                }
            }
            Request::VmHost {}
            | Request::EnableVms { .. }
            | Request::DisableVms { .. }
            | Request::VmStorageUsage {}
            | Request::CopyVmImage { .. }
            | Request::VmImages { .. }
            | Request::DeleteVmImage { .. }
            | Request::OpenVmImageUpload { .. }
            | Request::DefineVm { .. }
            | Request::UpdateVm { .. }
            | Request::ChangeVmMedia { .. }
            | Request::SetVmAutostart { .. }
            | Request::ControlVm { .. }
            | Request::DeleteVm { .. }
            | Request::VmStates {}
            | Request::VmStats {}
            | Request::OpenVmConsole { .. } => {
                self.vms.lock().unwrap_or_else(PoisonError::into_inner).handle(&request).unwrap_or_else(
                    || error(ErrorCode::InvalidRequest, "not a request about virtual machines"),
                )
            }
            Request::AppLogs { app, .. } => Response::AppLogs {
                text: format!(
                    "2026-11-06T12:00:00.000000000Z {}-app-1  | simulated: started\n",
                    app.as_str()
                ),
            },
            Request::NetworkState {} => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut network = self.network.lock().unwrap_or_else(PoisonError::into_inner);
                network.lapse(now);
                let resolved_installed =
                    self.packages.lock().unwrap_or_else(PoisonError::into_inner).resolved;
                let ifupdown = network.ifupdown.as_ref().map(|text| zendiloos_agent_proto::Ifupdown {
                    files: vec![zendiloos_agent_proto::IfupdownFile {
                        path: "interfaces".into(),
                        text: text.clone(),
                        skipped: None,
                    }],
                    digest: ifupdown_digest(text),
                    active: true,
                    resolved_installed,
                });
                Response::NetworkState {
                    managed: ifupdown.is_none(),
                    pending: network.pending.clone(),
                    network_manager: false,
                    ifupdown,
                    files: Vec::new(),
                }
            }
            // Nothing is changed: the change only waits, as one would.
            Request::ChangeNetwork { change, confirm_within } => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut network = self.network.lock().unwrap_or_else(PoisonError::into_inner);
                network.lapse(now);
                if network.ifupdown.is_some() {
                    error(ErrorCode::Forbidden, "refused: systemd-networkd does not manage the network here")
                } else if network.pending.is_some() {
                    error(ErrorCode::Conflict, "a change waits for confirmation")
                } else {
                    network.pending = Some(zendiloos_agent_proto::PendingNetwork {
                        change: change.change.clone(),
                        revert_at: now + i64::from(confirm_within.get()),
                    });
                    Response::Done
                }
            }
            Request::TakeOverNetwork { configs, digest, confirm_within } => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut network = self.network.lock().unwrap_or_else(PoisonError::into_inner);
                network.lapse(now);
                let resolved = self.packages.lock().unwrap_or_else(PoisonError::into_inner).resolved;
                match network.ifupdown.clone() {
                    _ if network.pending.is_some() => {
                        error(ErrorCode::Conflict, "a change waits for confirmation")
                    }
                    None => error(ErrorCode::Forbidden, "refused: ifupdown does not manage the network here"),
                    Some(text) if ifupdown_digest(&text) != *digest => {
                        error(ErrorCode::Forbidden, "refused: ifupdown's files changed since they were read")
                    }
                    Some(_) if !resolved => {
                        error(ErrorCode::Forbidden, "refused: systemd-resolved is not installed")
                    }
                    Some(text) => {
                        network.taken_over = Some(text);
                        network.ifupdown = None;
                        network.pending = Some(zendiloos_agent_proto::PendingNetwork {
                            change: zendiloos_agent_proto::NetworkChange::Takeover {
                                interfaces: configs.as_slice().iter().map(|c| c.interface.clone()).collect(),
                            },
                            revert_at: now + i64::from(confirm_within.get()),
                        });
                        Response::Done
                    }
                }
            }
            Request::ConfirmNetwork {} => {
                let mut network = self.network.lock().unwrap_or_else(PoisonError::into_inner);
                network.taken_over = None;
                match network.pending.take() {
                    Some(_) => Response::Done,
                    None => error(ErrorCode::Forbidden, "no change waits for confirmation"),
                }
            }
            Request::WebTlsState {} => {
                let minimum = *self.tls.lock().unwrap_or_else(PoisonError::into_inner);
                Response::WebTlsState { config: format!("ssl_protocols {};\n", minimum.nginx_protocols()) }
            }
            Request::SetWebTls { minimum } => {
                *self.tls.lock().unwrap_or_else(PoisonError::into_inner) = *minimum;
                Response::Done
            }
            Request::AcmeState {} => Response::AcmeState {
                installed: self.packages.lock().unwrap_or_else(PoisonError::into_inner).acme,
            },
            Request::IssueCertificate { order } => {
                if !self.packages.lock().unwrap_or_else(PoisonError::into_inner).acme {
                    error(ErrorCode::Failed, "the system is not set up for this: acme.sh is not installed")
                } else if acme_lines(order).1 {
                    self.acme.lock().unwrap_or_else(PoisonError::into_inner).issued.insert(order.certificate);
                    let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                    Response::CertificateIssued { not_after: now + 90 * 24 * 3600 }
                } else {
                    let main = order.domains.main().as_str();
                    error(
                        ErrorCode::Failed,
                        &format!(
                            "acme.sh: {main}: Verify error: DNS problem: NXDOMAIN looking up A for {main}"
                        ),
                    )
                }
            }
            Request::RemoveCertificate { certificate } => {
                self.acme.lock().unwrap_or_else(PoisonError::into_inner).issued.remove(certificate);
                Response::Done
            }
            Request::RemoveAcmeAccount { .. } => Response::Done,
            Request::WebServerState {} => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut acme = self.acme.lock().unwrap_or_else(PoisonError::into_inner);
                if acme.pending.is_some_and(|p| p.revert_at <= now) {
                    acme.pending = None;
                }
                Response::WebServerState { pending: acme.pending }
            }
            Request::ApplyWebServer { config, confirm_within } => {
                let mut acme = self.acme.lock().unwrap_or_else(PoisonError::into_inner);
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                if acme.pending.is_some_and(|p| p.revert_at > now) {
                    error(ErrorCode::Conflict, "a change waits for confirmation")
                } else if config.certificate.is_some_and(|n| !acme.issued.contains(&n)) {
                    error(ErrorCode::Forbidden, "refused: the certificate is not issued")
                } else {
                    // Without a time to confirm in, the settings stay at once.
                    acme.pending = confirm_within.map(|within| zendiloos_agent_proto::PendingWebServer {
                        revert_at: now + i64::from(within.get()),
                    });
                    Response::Done
                }
            }
            Request::ConfirmWebServer {} => {
                match self.acme.lock().unwrap_or_else(PoisonError::into_inner).pending.take() {
                    Some(_) => Response::Done,
                    None => error(ErrorCode::Forbidden, "no change waits for confirmation"),
                }
            }
            Request::ReloadWebCertificate {} => Response::Done,
            Request::FirewallState {} => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut firewall = self.firewall.lock().unwrap_or_else(PoisonError::into_inner);
                if firewall.pending.as_ref().is_some_and(|(p, _)| p.revert_at <= now) {
                    let (_, before) = firewall.pending.take().unwrap_or_else(|| unreachable!());
                    firewall.rules = before;
                }
                Response::FirewallState { pending: firewall.pending.as_ref().map(|(p, _)| *p) }
            }
            Request::ApplyFirewall { config, confirm_within, waiting } => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let mut firewall = self.firewall.lock().unwrap_or_else(PoisonError::into_inner);
                if firewall.pending.as_ref().is_some_and(|(p, _)| p.revert_at <= now) {
                    let (_, before) = firewall.pending.take().unwrap_or_else(|| unreachable!());
                    firewall.rules = before;
                }
                match (confirm_within, waiting) {
                    (Some(_), Some(_)) => {
                        error(ErrorCode::Forbidden, "a change that begins has no other rules")
                    }
                    (Some(_), None) if firewall.pending.is_some() => {
                        error(ErrorCode::Conflict, "a change waits for confirmation")
                    }
                    (Some(within), None) => {
                        let revert_at = now + i64::from(within.get());
                        let before = firewall.rules.take();
                        firewall.pending =
                            Some((zendiloos_agent_proto::PendingFirewall { revert_at }, before));
                        firewall.rules = config.clone();
                        Response::FirewallState { pending: firewall.pending.as_ref().map(|(p, _)| *p) }
                    }
                    (None, waiting) => {
                        match (firewall.pending.as_mut(), waiting) {
                            (Some((_, before)), Some(waiting)) => {
                                before.clone_from(config);
                                firewall.rules.clone_from(&waiting.config);
                            }
                            // Nobody can confirm it any more, or nothing waits.
                            _ => {
                                firewall.pending = None;
                                firewall.rules.clone_from(config);
                            }
                        }
                        Response::FirewallState { pending: firewall.pending.as_ref().map(|(p, _)| *p) }
                    }
                }
            }
            Request::ConfirmFirewall {} => {
                match self.firewall.lock().unwrap_or_else(PoisonError::into_inner).pending.take() {
                    Some(_) => Response::Done,
                    None => error(ErrorCode::Forbidden, "no change waits for confirmation"),
                }
            }
            Request::SshState {} => {
                let (config, off_at) = self.ssh.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let mut text = format!("# simulated\nPort {}\n", config.port.get());
                if let Some(root) = config.root_login {
                    text.push_str(&format!("PermitRootLogin {}\n", root.sshd_name()));
                }
                if let Some(passwords) = config.password_login {
                    text.push_str(&format!(
                        "PasswordAuthentication {}\n",
                        if passwords { "yes" } else { "no" }
                    ));
                }
                // Turned on for a while, it is enabled until the next boot only.
                let state = match (config.enabled, off_at) {
                    (true, Some(_)) => ("enabled-runtime", "active"),
                    (true, None) => ("enabled", "active"),
                    (false, _) => ("disabled", "inactive"),
                };
                Response::SshState {
                    installed: true,
                    enabled: state.0.to_owned(),
                    active: state.1.to_owned(),
                    config: text,
                    off_at: off_at.filter(|_| config.enabled),
                }
            }
            Request::SetSsh { config } => {
                let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
                let off_at = config.off_after.map(|off| now + i64::from(off.get()) * 60);
                *self.ssh.lock().unwrap_or_else(PoisonError::into_inner) = (config.clone(), off_at);
                Response::Done
            }
            Request::TimeState {} => self.time.lock().unwrap_or_else(PoisonError::into_inner).state(),
            Request::SetTime { zone, network_time, servers, clock } => {
                if *network_time && clock.is_some() {
                    error(ErrorCode::Forbidden, "the time servers set the clock; turn them off first")
                } else if !TIME_ZONES.contains(&zone.as_str()) {
                    error(ErrorCode::Failed, "Failed to set time zone: Invalid or not installed time zone")
                } else {
                    *self.time.lock().unwrap_or_else(PoisonError::into_inner) = SimTime {
                        zone: zone.as_str().to_owned(),
                        network_time: *network_time,
                        servers: servers.as_slice().iter().map(|s| s.as_str().to_owned()).collect(),
                    };
                    Response::Done
                }
            }
            // The simulated drives pass every test at once.
            Request::StartSelfTest { .. } | Request::AbortSelfTest { .. } => Response::Done,
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
            Request::MountRemovable { id } => match self.host().mount_removable(id.as_str()) {
                Ok(()) => Response::RemovableMounted { read_only: false },
                Err(refusal) => changed(Err(refusal)),
            },
            Request::EjectRemovable { id, power_off } => {
                changed(self.host().eject_removable(id.as_str(), *power_off))
            }
            // Nothing is ever left behind here.
            Request::CleanRemovable {} => Response::RemovableCleaned { ids: Vec::new() },
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
            Request::RollbackSnapshot { pool, volume, share, label } => changed(
                self.host().rollback_snapshot(pool.as_str(), volume.get(), share.as_str(), label.as_str()),
            ),
            Request::ListSnapshots {} => Response::Snapshots { zfs_list: self.host().snapshot_list() },
            // The console belongs to the machine the simulation pretends to be.
            Request::ShowOnConsole { .. } => Response::Done,
            // Properties change nothing the simulation shows.
            Request::SetShareProperties { volume, share, .. } => {
                changed(self.host().share_present(volume.get(), share.as_str()))
            }
            Request::ShareUsage {} => Response::ShareUsage { zfs_list: self.host().share_usage() },
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
            Request::PrepareHomes { .. } | Request::SetHomeQuota { .. } | Request::MakeHome { .. } => {
                Response::Done
            }
            Request::ApplySharing { smb, nfs, services, .. } => {
                tracing::info!(
                    smb = smb.len(),
                    nfs = nfs.len(),
                    smb_on = services.smb,
                    nfs_on = services.nfs,
                    workgroup = services.workgroup.as_str(),
                    "simulated agent: sharing configuration not written"
                );
                Response::Done
            }
            Request::PrepareShare { volume, name } | Request::MigrateShare { volume, name } => {
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
            | Request::SearchFiles { .. }
            | Request::EntryFacts { .. }
            | Request::OpenFile { .. }
            | Request::CreateUpload { .. }
            | Request::MakeDirectory { .. }
            | Request::MoveEntry { .. }
            | Request::CopyEntry { .. }
            | Request::RemoveEntry { .. }
            | Request::DiscardUpload { .. }
            | Request::RestoreEntry { .. }
            | Request::ArchiveFolder { .. } => self.file_operation(&request).0,
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
            Request::OpenFile { .. } | Request::CreateUpload { .. } | Request::ArchiveFolder { .. } => {
                let answer = self.file_operation(&request);
                self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                Ok(answer)
            }
            Request::OpenVmConsole { ref id } => {
                let opened = self.vms.lock().unwrap_or_else(PoisonError::into_inner).console(id);
                self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                Ok(opened)
            }
            Request::OpenVmImageUpload { ref image, volume, bytes } => {
                let opened =
                    self.vms.lock().unwrap_or_else(PoisonError::into_inner).open_upload(image, volume, bytes);
                self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                Ok(opened)
            }
            Request::OpenTerminal { ref container, ref command, size } => {
                let answer = match echo_terminal(container.as_str(), &command.line(), size) {
                    Ok(master) => (Response::Terminal {}, Some(master)),
                    Err(error) => (error_response(&error.to_string()), None),
                };
                self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                Ok(answer)
            }
            other => Ok((self.call(other, request_id).await?, None)),
        }
    }

    /// As the real agent: what Docker Compose writes while it deploys
    /// comes, line by line, before the answer.
    async fn call_reporting(
        &self,
        request: Request,
        request_id: Option<RequestId>,
        report: &dyn Report,
    ) -> Result<Response, TransportError> {
        for line in docker::compose_lines(&request) {
            report.line(&line);
        }
        if let Request::IssueCertificate { order } = &request
            && self.packages.lock().unwrap_or_else(PoisonError::into_inner).acme
        {
            for line in acme_lines(order).0 {
                report.line(&format!("[Sat Nov  7 12:00:00 UTC 2026] {line}"));
            }
        }
        self.call(request, request_id).await
    }
}

fn error_response(message: &str) -> Response {
    error(ErrorCode::Failed, message)
}

/// A terminal whose other side answers each line typed, instead of a
/// program in a container: the simulation runs nothing. Its master is
/// handed out; closing it ends the answering thread.
fn echo_terminal(container: &str, command: &str, size: TerminalSize) -> std::io::Result<OwnedFd> {
    use std::io::{BufRead, BufReader, Write};

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;

    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    let path = rustix::pty::ptsname(&master, Vec::new())?;
    let slave =
        rustix::fs::open(path.as_c_str(), OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC, Mode::empty())?;
    let winsize =
        rustix::termios::Winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 };
    rustix::termios::tcsetwinsize(&master, winsize)?;
    let mut writer = std::fs::File::from(slave);
    let reader = BufReader::new(writer.try_clone()?);
    let (container, command) = (container.to_owned(), command.to_owned());
    std::thread::spawn(move || {
        let _ = write!(writer, "simulated terminal in {container}, running {command}\n$ ");
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim() == "exit" {
                break;
            }
            let _ = write!(writer, "simulated: {line}\n$ ");
        }
    });
    Ok(master)
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

    /// Reads from a terminal's master until `wanted` came, or it ended.
    fn read_until(master: &OwnedFd, wanted: &str) -> String {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 256];
        while !String::from_utf8_lossy(&seen).contains(wanted) {
            match rustix::io::read(master, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buffer[..n]),
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    #[tokio::test]
    async fn one_container_is_shown_its_processes_listed_and_a_terminal_answers() {
        use zendiloos_agent_proto::{ContainerId, ExecCommand};

        let agent = SimulatedAgent::new();
        let web = || ContainerId::parse("stack-web-1").unwrap();
        let Response::DockerOutput { text } =
            agent.call(Request::InspectContainer { container: web() }, None).await.unwrap()
        else {
            panic!("no output")
        };
        let inspected: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(inspected[0]["Name"], "/stack-web-1");
        assert_eq!(inspected[0]["HostConfig"]["CapDrop"][0], "ALL");
        let Response::DockerOutput { text } =
            agent.call(Request::ContainerProcesses { container: web() }, None).await.unwrap()
        else {
            panic!("no output")
        };
        assert!(text.contains("nginx: worker process"), "{text}");
        let stopped = Request::ContainerProcesses { container: ContainerId::parse("lonely").unwrap() };
        assert!(matches!(agent.call(stopped, None).await.unwrap(), Response::Error { .. }));
        let unknown = Request::InspectContainer { container: ContainerId::parse("ghost").unwrap() };
        assert!(matches!(agent.call(unknown, None).await.unwrap(), Response::Error { .. }));

        let open = Request::OpenTerminal {
            container: web(),
            command: ExecCommand::from_line("/bin/sh").unwrap(),
            size: TerminalSize::default(),
        };
        let (answer, master) = agent.call_with_file(open.clone(), None).await.unwrap();
        assert_eq!(answer, Response::Terminal {});
        let master = master.unwrap();
        assert!(read_until(&master, "$ ").contains("simulated terminal in stack-web-1, running /bin/sh"));
        rustix::io::write(&master, b"ls\n").unwrap();
        assert!(read_until(&master, "simulated: ls").contains("simulated: ls"));
        assert!(matches!(agent.call(open, None).await.unwrap(), Response::Error { .. }));
    }

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
        let at = |path: &str| FileLocation {
            volume,
            root: FileRoot::Share(name.clone()),
            path: SharePath::parse(path).unwrap(),
            snapshot: None,
        };
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

    fn done(response: Response) {
        assert_eq!(response, Response::Done);
    }

    fn refused(response: &Response, code: ErrorCode) {
        assert!(matches!(response, Response::Error { code: c, .. } if *c == code), "{response:?}");
    }

    #[tokio::test]
    async fn shares_and_their_snapshots_come_and_go_like_datasets() {
        use zendiloos_agent_proto::{PoolName, ShareName, SnapshotLabel, SnapshotNote, VolumeNumber};
        let agent = at(1_793_907_601);
        let (tank, one, photos) = (
            || PoolName::parse("tank").unwrap(),
            || VolumeNumber::new(1).unwrap(),
            || ShareName::parse("photos").unwrap(),
        );
        let label = |text: &str| SnapshotLabel::parse(text).unwrap();
        done(
            agent
                .call(Request::CreateShare { pool: tank(), volume: one(), name: photos() }, None)
                .await
                .unwrap(),
        );
        let again =
            agent.call(Request::CreateShare { pool: tank(), volume: one(), name: photos() }, None).await;
        refused(&again.unwrap(), ErrorCode::Conflict);
        let unmanaged = agent
            .call(
                Request::CreateShare { pool: tank(), volume: VolumeNumber::new(7).unwrap(), name: photos() },
                None,
            )
            .await;
        refused(&unmanaged.unwrap(), ErrorCode::Forbidden);
        let Response::ShareUsage { zfs_list } = agent.call(Request::ShareUsage {}, None).await.unwrap()
        else {
            panic!()
        };
        assert!(zfs_list.contains("tank/photos\t"), "{zfs_list}");

        let snapshot = |text: &str| Request::CreateSnapshot {
            pool: tank(),
            volume: one(),
            share: photos(),
            label: label(text),
            schedules: vec![zendiloos_agent_proto::SnapshotFrequency::Daily],
            note: Some(SnapshotNote::parse("before").unwrap()),
        };
        done(agent.call(snapshot("zdl-2026-11-05_19.40.01"), None).await.unwrap());
        done(agent.call(snapshot("zdl-2026-11-05_20.40.01"), None).await.unwrap());
        refused(&agent.call(snapshot("zdl-2026-11-05_20.40.01"), None).await.unwrap(), ErrorCode::Conflict);
        let Response::Snapshots { zfs_list } = agent.call(Request::ListSnapshots {}, None).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(zfs_list.lines().count(), 2);
        assert!(zfs_list.contains("daily\tbefore"), "{zfs_list}");

        let rollback = |text: &str| Request::RollbackSnapshot {
            pool: tank(),
            volume: one(),
            share: photos(),
            label: label(text),
        };
        done(agent.call(rollback("zdl-2026-11-05_19.40.01"), None).await.unwrap());
        let Response::Snapshots { zfs_list } = agent.call(Request::ListSnapshots {}, None).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(zfs_list.lines().count(), 1, "the later one went");
        refused(&agent.call(rollback("zdl-2026-11-05_21.00.00"), None).await.unwrap(), ErrorCode::Forbidden);
        let destroy = |text: &str| Request::DestroySnapshot {
            pool: tank(),
            volume: one(),
            share: photos(),
            label: label(text),
        };
        done(agent.call(destroy("zdl-2026-11-05_19.40.01"), None).await.unwrap());
        refused(&agent.call(destroy("zdl-2026-11-05_19.40.01"), None).await.unwrap(), ErrorCode::Forbidden);
        let of_nothing = Request::CreateSnapshot {
            pool: tank(),
            volume: one(),
            share: ShareName::parse("nothing").unwrap(),
            label: label("zdl-2026-11-05_19.40.01"),
            schedules: vec![],
            note: None,
        };
        refused(&agent.call(of_nothing, None).await.unwrap(), ErrorCode::Forbidden);

        let access = |name: ShareName| Request::MigrateShare { volume: one(), name };
        done(agent.call(access(photos()), None).await.unwrap());
        refused(
            &agent.call(access(ShareName::parse("nothing").unwrap()), None).await.unwrap(),
            ErrorCode::Forbidden,
        );
        done(
            agent
                .call(Request::DestroyShare { pool: tank(), volume: one(), name: photos() }, None)
                .await
                .unwrap(),
        );
        refused(&agent.call(access(photos()), None).await.unwrap(), ErrorCode::Forbidden);
    }

    #[tokio::test]
    async fn files_are_copied_deleted_restored_and_packed_in_a_directory() {
        use std::io::Read;

        use zendiloos_agent_proto::{PoolName, ShareName, SharePath, UserName, VolumeNumber};
        let root = tempfile::tempdir().unwrap();
        let agent = SimulatedAgent::new().with_files(root.path());
        let (volume, docs) = (VolumeNumber::new(1).unwrap(), ShareName::parse("docs").unwrap());
        let pool = PoolName::parse("tank").unwrap();
        done(agent.call(Request::CreateShare { pool, volume, name: docs.clone() }, None).await.unwrap());
        let at = |path: &str| FileLocation {
            volume,
            root: FileRoot::Share(docs.clone()),
            path: SharePath::parse(path).unwrap(),
            snapshot: None,
        };
        let anna = || UserName::parse("anna").unwrap();
        done(
            agent
                .call(Request::MakeDirectory { user: anna(), location: at("album"), own: false }, None)
                .await
                .unwrap(),
        );
        std::fs::write(root.path().join("volume1/docs/album/a.txt"), "hello").unwrap();
        let copied =
            agent.call(Request::CopyEntry { user: anna(), from: at("album"), to: at("copy") }, None).await;
        assert!(matches!(copied.unwrap(), Response::Copied { .. }));
        done(
            agent
                .call(Request::RemoveEntry { user: anna(), location: at("copy"), recycle: true }, None)
                .await
                .unwrap(),
        );
        let restored = agent
            .call(Request::RestoreEntry { user: anna(), location: at(".recycle/anna/copy") }, None)
            .await;
        assert!(matches!(restored.unwrap(), Response::Restored { .. }));
        done(
            agent
                .call(Request::RemoveEntry { user: anna(), location: at("copy"), recycle: false }, None)
                .await
                .unwrap(),
        );

        let (answer, file) = agent
            .call_with_file(Request::ArchiveFolder { user: anna(), location: at("album") }, None)
            .await
            .unwrap();
        let Response::Archive { size } = answer else { panic!("{answer:?}") };
        let mut archive = Vec::new();
        std::fs::File::from(file.unwrap()).read_to_end(&mut archive).unwrap();
        assert_eq!(archive.len() as u64, size);

        let (answer, _) = agent
            .call_with_file(Request::CreateUpload { user: anna(), location: at("") }, None)
            .await
            .unwrap();
        let Response::Upload { name } = answer else { panic!("{answer:?}") };
        done(agent.call(Request::DiscardUpload { user: anna(), location: at(&name) }, None).await.unwrap());

        let home = FileLocation {
            volume,
            root: FileRoot::Home,
            path: SharePath::parse("").unwrap(),
            snapshot: None,
        };
        let listed = agent.call(Request::ListDirectory { user: anna(), location: home }, None).await.unwrap();
        assert!(matches!(listed, Response::Directory { .. }), "{listed:?}");
        let in_a_snapshot = FileLocation {
            snapshot: Some(zendiloos_agent_proto::SnapshotLabel::parse("zdl-2026-11-05_19.40.01").unwrap()),
            ..at("")
        };
        let none =
            agent.call(Request::ListDirectory { user: anna(), location: in_a_snapshot }, None).await.unwrap();
        refused(&none, ErrorCode::NotFound);
        let taken = agent
            .call(Request::MakeDirectory { user: anna(), location: at("album"), own: false }, None)
            .await
            .unwrap();
        refused(&taken, ErrorCode::Conflict);

        // Without a directory for files, nothing is kept.
        let bare = SimulatedAgent::new();
        let nothing =
            bare.call(Request::ListDirectory { user: anna(), location: at("") }, None).await.unwrap();
        refused(&nothing, ErrorCode::Failed);
    }

    #[tokio::test]
    async fn accounts_are_made_changed_and_refused_like_the_agent() {
        use zendiloos_agent_proto::{GroupName, Password, UserName};
        let agent = SimulatedAgent::new();
        let (anna, family) = (|| UserName::parse("anna").unwrap(), || GroupName::parse("family").unwrap());
        done(agent.call(Request::CreateUser { name: anna() }, None).await.unwrap());
        refused(&agent.call(Request::CreateUser { name: anna() }, None).await.unwrap(), ErrorCode::Conflict);
        let password = Password::new("correct horse battery").unwrap();
        done(agent.call(Request::SetSmbPassword { name: anna(), password }, None).await.unwrap());
        done(agent.call(Request::SetUserEnabled { name: anna(), enabled: false }, None).await.unwrap());
        done(agent.call(Request::CreateGroup { name: family() }, None).await.unwrap());
        done(
            agent
                .call(Request::SetGroupMembers { group: family(), members: vec![anna()] }, None)
                .await
                .unwrap(),
        );
        let root =
            Request::SetGroupMembers { group: family(), members: vec![UserName::parse("root").unwrap()] };
        refused(&agent.call(root, None).await.unwrap(), ErrorCode::Forbidden);
        done(agent.call(Request::DeleteGroup { name: family() }, None).await.unwrap());
        done(agent.call(Request::DeleteUser { name: anna() }, None).await.unwrap());
        let system = agent.call(Request::DeleteUser { name: UserName::parse("root").unwrap() }, None).await;
        refused(&system.unwrap(), ErrorCode::Forbidden);
    }

    #[tokio::test]
    async fn an_update_runs_its_steps_and_a_second_waits() {
        use zendiloos_agent_proto::PackageTask;
        let agent = SimulatedAgent::new();
        let started =
            agent.call(Request::StartPackages { task: PackageTask::Check, release: None }, None).await;
        assert!(matches!(started.unwrap(), Response::PackagesStarted { .. }));
        let busy =
            agent.call(Request::StartPackages { task: PackageTask::Update, release: None }, None).await;
        refused(&busy.unwrap(), ErrorCode::Forbidden);
        let Response::Packages { run: Some(run), .. } =
            agent.call(Request::PackagesState {}, None).await.unwrap()
        else {
            panic!()
        };
        assert_eq!((run.state, run.step.as_str()), (PackageRunState::Running, "lists"));
        let refused_docker = agent
            .call(
                Request::MoveDocker {
                    from_pool: zendiloos_agent_proto::PoolName::parse("tank").unwrap(),
                    from_volume: zendiloos_agent_proto::VolumeNumber::new(9).unwrap(),
                    to_pool: zendiloos_agent_proto::PoolName::parse("backup").unwrap(),
                    to_volume: zendiloos_agent_proto::VolumeNumber::new(2).unwrap(),
                },
                None,
            )
            .await
            .unwrap();
        refused(&refused_docker, ErrorCode::Forbidden);
    }

    #[tokio::test]
    async fn the_usb_stick_is_mounted_and_ejected_like_a_real_one() {
        use zendiloos_agent_proto::RemovableId;
        let agent = SimulatedAgent::new();
        let id = || RemovableId::parse(host::STICK_ID).unwrap();
        let lsblk = || async {
            match agent.call(Request::ListBlockDevices {}, None).await.unwrap() {
                Response::BlockDevices { lsblk_json, .. } => lsblk_json,
                other => panic!("{other:?}"),
            }
        };
        assert!(lsblk().await.contains("\"tran\":\"usb\""));

        let mounted = agent.call(Request::MountRemovable { id: id() }, None).await.unwrap();
        assert_eq!(mounted, Response::RemovableMounted { read_only: false });
        assert!(lsblk().await.contains("/run/zendiloos-removable/1A2B-3C4D"));
        let kept = agent.call(Request::EjectRemovable { id: id(), power_off: false }, None).await.unwrap();
        assert_eq!(kept, Response::Done);
        assert!(lsblk().await.contains("SIMUSB0001") && !lsblk().await.contains("/run/zendiloos-removable"));
        let off = agent.call(Request::EjectRemovable { id: id(), power_off: true }, None).await.unwrap();
        assert_eq!(off, Response::Done);
        assert!(!lsblk().await.contains("SIMUSB0001"), "gone from the bus");
        let gone = agent.call(Request::MountRemovable { id: id() }, None).await.unwrap();
        assert!(matches!(gone, Response::Error { code: ErrorCode::Forbidden, .. }), "{gone:?}");
        let cleaned = agent.call(Request::CleanRemovable {}, None).await.unwrap();
        assert_eq!(cleaned, Response::RemovableCleaned { ids: Vec::new() });
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

    #[tokio::test]
    async fn certificates_are_issued_and_the_web_server_waits_for_confirmation() {
        use zendiloos_agent_proto::{
            AcmeCa, AcmeChallenge, AcmeDomain, AcmeDomains, AcmeEmail, AcmeKey, AcmeOrder, ConfirmWithin,
            WebPort, WebServerConfig,
        };
        let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = now.clone();
        let agent =
            SimulatedAgent::with_clock(Arc::new(move || clock.load(std::sync::atomic::Ordering::Relaxed)));
        let order = |name: &str| AcmeOrder {
            certificate: 3,
            account: 1,
            ca: AcmeCa::LetsEncryptStaging,
            email: AcmeEmail::parse("admin@example.org").unwrap(),
            domains: AcmeDomains::new(vec![AcmeDomain::parse(name).unwrap()]).unwrap(),
            key: AcmeKey::Ec256,
            challenge: AcmeChallenge::Http,
        };
        let issue = |name: &str| Request::IssueCertificate { order: order(name) };
        assert_eq!(
            agent.call(Request::AcmeState {}, None).await.unwrap(),
            Response::AcmeState { installed: false }
        );
        refused(&agent.call(issue("nas.example.org"), None).await.unwrap(), ErrorCode::Failed);
        agent.packages.lock().unwrap().acme = true;

        struct Lines(Mutex<Vec<String>>);
        impl Report for Lines {
            fn step(&self, _: &str, _: Option<u8>) {}
            fn line(&self, line: &str) {
                self.0.lock().unwrap().push(line.to_owned());
            }
        }
        let lines = Lines(Mutex::default());
        let issued = agent.call_reporting(issue("nas.example.org"), None, &lines).await.unwrap();
        assert_eq!(issued, Response::CertificateIssued { not_after: 1_000 + 90 * 24 * 3600 });
        assert!(lines.0.lock().unwrap().last().unwrap().ends_with("Cert success."));
        refused(&agent.call(issue("nas.example.invalid"), None).await.unwrap(), ErrorCode::Failed);

        let config =
            |certificate| WebServerConfig { http: None, https: WebPort::new(8443).unwrap(), certificate };
        let within = ConfirmWithin::seconds(90).ok();
        let apply =
            |certificate| Request::ApplyWebServer { config: config(certificate), confirm_within: within };
        refused(&agent.call(apply(Some(4)), None).await.unwrap(), ErrorCode::Forbidden);
        assert_eq!(agent.call(apply(Some(3)), None).await.unwrap(), Response::Done);
        refused(&agent.call(apply(None), None).await.unwrap(), ErrorCode::Conflict);
        let state = agent.call(Request::WebServerState {}, None).await.unwrap();
        assert!(
            matches!(state, Response::WebServerState { pending: Some(p) } if p.revert_at == 1_090),
            "{state:?}"
        );
        assert_eq!(agent.call(Request::ConfirmWebServer {}, None).await.unwrap(), Response::Done);
        refused(&agent.call(Request::ConfirmWebServer {}, None).await.unwrap(), ErrorCode::Forbidden);

        // Not confirmed in time: back by itself.
        assert_eq!(agent.call(apply(None), None).await.unwrap(), Response::Done);
        now.store(1_091, std::sync::atomic::Ordering::Relaxed);
        let state = agent.call(Request::WebServerState {}, None).await.unwrap();
        assert_eq!(state, Response::WebServerState { pending: None });
        assert_eq!(
            agent.call(Request::RemoveCertificate { certificate: 3 }, None).await.unwrap(),
            Response::Done
        );
        refused(&agent.call(apply(Some(3)), None).await.unwrap(), ErrorCode::Forbidden);
        // Settings confirmed before stay at once.
        let keep = Request::ApplyWebServer { config: config(None), confirm_within: None };
        assert_eq!(agent.call(keep, None).await.unwrap(), Response::Done);
        let state = agent.call(Request::WebServerState {}, None).await.unwrap();
        assert_eq!(state, Response::WebServerState { pending: None });
    }

    #[tokio::test]
    async fn the_firewall_waits_for_confirmation_and_goes_back_without() {
        use zendiloos_agent_proto::{
            ConfirmWithin, FirewallConfig, FirewallRules, FirewallVerdict, PendingFirewall, WaitingFirewall,
        };
        let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = now.clone();
        let agent =
            SimulatedAgent::with_clock(Arc::new(move || clock.load(std::sync::atomic::Ordering::Relaxed)));
        let closed = FirewallConfig {
            rules: FirewallRules::new(vec![]).unwrap(),
            fallback: FirewallVerdict::Drop,
            bridges: vec![],
            own_bridges: vec![],
        };
        let within = ConfirmWithin::seconds(90).ok();
        let apply = |config: Option<FirewallConfig>, confirm_within, waiting| Request::ApplyFirewall {
            config,
            confirm_within,
            waiting,
        };
        let nothing = Response::FirewallState { pending: None };
        let until = Response::FirewallState { pending: Some(PendingFirewall { revert_at: 1_090 }) };
        let state = || agent.call(Request::FirewallState {}, None);
        assert_eq!(state().await.unwrap(), nothing);
        assert_eq!(agent.call(apply(Some(closed.clone()), within, None), None).await.unwrap(), until);
        refused(&agent.call(apply(None, within, None), None).await.unwrap(), ErrorCode::Conflict);
        let both = Some(WaitingFirewall { config: None });
        refused(&agent.call(apply(None, within, both), None).await.unwrap(), ErrorCode::Forbidden);
        // Brought up to date while it waits: what returns is the rules kept.
        let waiting = Some(WaitingFirewall { config: Some(closed.clone()) });
        assert_eq!(agent.call(apply(None, None, waiting.clone()), None).await.unwrap(), until);
        assert_eq!(state().await.unwrap(), until);
        assert_eq!(agent.call(Request::ConfirmFirewall {}, None).await.unwrap(), Response::Done);
        refused(&agent.call(Request::ConfirmFirewall {}, None).await.unwrap(), ErrorCode::Forbidden);
        assert_eq!(agent.firewall.lock().unwrap().rules, Some(closed.clone()));

        // Not confirmed in time: the rules from before are back.
        agent.call(apply(None, within, None), None).await.unwrap();
        now.store(1_091, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(state().await.unwrap(), nothing);
        assert_eq!(agent.firewall.lock().unwrap().rules, Some(closed.clone()));
        // A change nobody names any more ends with the rules kept.
        agent.call(apply(None, ConfirmWithin::seconds(90).ok(), None), None).await.unwrap();
        assert_eq!(agent.call(apply(Some(closed.clone()), None, None), None).await.unwrap(), nothing);
        assert_eq!(agent.firewall.lock().unwrap().rules, Some(closed.clone()));
        // Rules for a change that went back: the rules kept.
        assert_eq!(agent.call(apply(None, None, waiting), None).await.unwrap(), nothing);
        assert_eq!(agent.firewall.lock().unwrap().rules, None);
    }
}
