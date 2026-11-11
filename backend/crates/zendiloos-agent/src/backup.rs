//! Backup tasks (docs/architecture.md, 4.14): the system agent checks the
//! places a request names (a share mounted, a drive mounted at its place),
//! then runs the work in a transient unit of its own (this binary as
//! `backup-unit`, see [`crate::backup_unit`]), outside its own sandbox,
//! which can neither write the volumes nor reach the network.
//!
//! Each unit has a sandbox of its own, as the volumes' backups do: the
//! system read-only, no new privileges, its own `/tmp`, devices, IPC and
//! process view; of the volumes and drives it sees only the roots it works
//! on (each volume and the drives' place a read-only tmpfs, the roots bound
//! back in: the one it writes writable, the others read-only); the
//! system's secrets hidden (`/etc/shadow`, SSH's host keys, zendiloOS's
//! database and state); the network only when a server is involved; and
//! only the capabilities its work needs: reading whatever the source holds,
//! and on the NAS writing owners, modes, ACLs and attributes as copied.
//! The NAS's SSH key reaches a unit as its credential (`LoadCredential=`),
//! a password on its standard input, neither on a command line.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zendiloos_agent_proto::{
    BACKUP_HOST_KEY_CHANGED, BACKUP_KEY, BACKUP_STATE, BackupEnd, BackupHostName, BackupMode, BackupPassword,
    BackupPruned, BackupRestore, BackupRoot, BackupRun, BackupTaskId, BackupTransport, Response,
};
use zendiloos_exec::{ExecError, Limits, Program, Surroundings};

use crate::backup_unit::{End, FailKind, Report, RestoreJob, RunJob, UnitJob, UnitSays, Work};
use crate::operations::OperationError;
use crate::pools::HostPaths;
use crate::volume_archive::encode;

/// A run or a restore may take up to a day: zendiloosd waits ten minutes
/// longer.
const RUN_FOR: Duration = Duration::from_secs(24 * 3600 - 600);
/// A listing or retention of a server's folder.
const LOOK_FOR: Duration = Duration::from_secs(5 * 60);
const PRUNE_FOR: Duration = Duration::from_secs(6 * 3600);
const KEYSCAN_FOR: Duration = Duration::from_secs(60);
const QUICK: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 256 * 1024 };
/// Reading whatever the source holds.
const READ_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH";
/// Writing on the NAS as copied: owners, modes with set-ID bits, ACLs and
/// attributes, also in folders nobody but their owner may write.
const WRITE_CAPABILITIES: &str =
    "CAP_CHOWN CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH CAP_FOWNER CAP_FSETID CAP_SETFCAP";
/// What a unit never sees.
const HIDDEN: [&str; 14] = [
    "-/etc/shadow",
    "-/etc/gshadow",
    "-/etc/ssh",
    "-/etc/samba",
    "-/etc/zendiloos",
    "-/etc/libvirt",
    "-/var/lib/zendiloos",
    "-/var/lib/zendiloos-acme",
    "-/var/lib/zendiloos-backup",
    "-/var/lib/zendiloos-packages",
    "-/var/lib/samba",
    "-/var/lib/docker",
    "-/var/lib/libvirt",
    "-/run/zendiloos-acme",
];
/// The sandbox every unit has.
const SANDBOX: [&str; 21] = [
    "--property=NoNewPrivileges=yes",
    "--property=ProtectSystem=strict",
    "--property=ProtectHome=yes",
    "--property=PrivateTmp=yes",
    "--property=PrivateDevices=yes",
    "--property=ProtectKernelTunables=yes",
    "--property=ProtectKernelModules=yes",
    "--property=ProtectKernelLogs=yes",
    "--property=ProtectControlGroups=yes",
    "--property=ProtectClock=yes",
    "--property=ProtectHostname=yes",
    "--property=RestrictNamespaces=yes",
    "--property=RestrictRealtime=yes",
    "--property=LockPersonality=yes",
    "--property=MemoryDenyWriteExecute=yes",
    "--property=SystemCallArchitectures=native",
    "--property=SystemCallFilter=@system-service",
    "--property=ProtectProc=invisible",
    "--property=PrivateIPC=yes",
    "--property=LimitNOFILE=65536",
    "--property=MemoryMax=2G",
];

/// Where the backup plugin's things are; other than the real places only
/// in tests.
#[derive(Debug, Clone)]
pub struct BackupPaths {
    /// The NAS's key pair: [`BACKUP_STATE`].
    pub state: PathBuf,
    /// Where systemd puts a unit's credentials, `/run/credentials`.
    pub credentials: PathBuf,
    /// Where systemd keeps transient units' files, `/run/systemd/transient`.
    pub transient: PathBuf,
}

impl Default for BackupPaths {
    fn default() -> Self {
        Self {
            state: PathBuf::from(BACKUP_STATE),
            credentials: PathBuf::from("/run/credentials"),
            transient: PathBuf::from("/run/systemd/transient"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BackupTools {
    pub systemd_run: Program,
    pub systemctl: Program,
    /// May be missing until the plugin is installed, as the SSH tools.
    pub rsync: PathBuf,
    pub ssh: PathBuf,
    pub ssh_keygen: PathBuf,
    pub ssh_keyscan: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Backups {
    tools: BackupTools,
    paths: BackupPaths,
    host: HostPaths,
}

/// What a unit works on: the roots it reads and the one it writes.
#[derive(Debug, Default)]
struct Places {
    read: Vec<PathBuf>,
    write: Option<PathBuf>,
    network: bool,
    ssh: bool,
}

impl Places {
    fn of(&mut self, end: &End, write: bool) {
        match end {
            End::Local { root, .. } if write => self.write = Some(root.clone()),
            End::Local { root, .. } => self.read.push(root.clone()),
            End::Daemon { .. } => self.network = true,
            End::Ssh { .. } => {
                self.network = true;
                self.ssh = true;
            }
        }
    }

    /// The capabilities the work needs.
    fn capabilities(&self) -> &'static str {
        if self.write.is_some() {
            WRITE_CAPABILITIES
        } else if self.read.is_empty() {
            ""
        } else {
            READ_CAPABILITIES
        }
    }
}

fn refused(message: impl Into<String>) -> OperationError {
    OperationError::Refused(message.into())
}

impl Backups {
    pub(crate) fn new(tools: BackupTools, paths: BackupPaths, host: HostPaths) -> Self {
        Self { tools, paths, host }
    }

    fn key_file(&self) -> PathBuf {
        self.paths.state.join(BACKUP_KEY)
    }

    pub(crate) fn facts(&self) -> Response {
        let public_key = std::fs::read_to_string(self.key_file().with_extension("pub"))
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| key.starts_with("ssh-ed25519 ") && !key.contains('\n'));
        Response::BackupFacts {
            rsync: self.tools.rsync.is_file(),
            ssh: self.tools.ssh.is_file() && self.tools.ssh_keygen.is_file(),
            public_key,
        }
    }

    fn require(&self, tool: &Path, what: &str) -> Result<Program, OperationError> {
        if !tool.is_file() {
            return Err(OperationError::Setup(format!("{what} is not installed")));
        }
        Program::new(tool).map_err(OperationError::from)
    }

    /// Makes the NAS's key pair, if it has none, in a unit that may write
    /// only the plugin's state (made root's, 0700, by systemd).
    pub(crate) async fn make_key(&self) -> Result<Response, OperationError> {
        let keygen = self.require(&self.tools.ssh_keygen, "ssh-keygen")?;
        if !self.key_file().exists() {
            let args: Vec<String> = [
                "--unit=zendiloos-backup-key.service".to_owned(),
                "--wait".into(),
                "--pipe".into(),
                "--collect".into(),
                "--quiet".into(),
                "--description=zendiloOS: make the backup's SSH key".into(),
                "--property=StateDirectory=zendiloos-backup".into(),
                "--property=StateDirectoryMode=0700".into(),
                "--property=PrivateNetwork=yes".into(),
                "--property=CapabilityBoundingSet=".into(),
                "--property=UMask=0077".into(),
                keygen.path().display().to_string(),
                "-q".into(),
                "-t".into(),
                "ed25519".into(),
                "-N".into(),
                String::new(),
                "-C".into(),
                "zendiloos-backup".into(),
                "-f".into(),
                self.key_file().display().to_string(),
            ]
            .into();
            run(&self.tools.systemd_run, &args, QUICK).await?;
        }
        match self.facts() {
            Response::BackupFacts { public_key: Some(public_key), .. } => {
                Ok(Response::BackupKey { public_key })
            }
            _ => Err(OperationError::Failed("the key pair was not made".into())),
        }
    }

    /// The host keys a server offers, by `ssh-keyscan` in a unit with the
    /// network, as nobody (a dynamic user) and no capability.
    pub(crate) async fn scan(&self, host: &BackupHostName, port: u16) -> Result<Response, OperationError> {
        let keyscan = self.require(&self.tools.ssh_keyscan, "ssh-keyscan")?;
        if port == 0 {
            return Err(refused("the port is from 1 to 65535"));
        }
        let mut args: Vec<String> = [
            "--wait".to_owned(),
            "--pipe".into(),
            "--collect".into(),
            "--quiet".into(),
            "--description=zendiloOS: look at a backup server's host keys".into(),
            "--property=DynamicUser=yes".into(),
            "--property=CapabilityBoundingSet=".into(),
            format!("--property=RuntimeMaxSec={}", KEYSCAN_FOR.as_secs()),
        ]
        .into();
        args.extend(SANDBOX.iter().map(|p| (*p).to_owned()));
        args.extend(network_properties(true));
        args.extend([
            keyscan.path().display().to_string(),
            "-T".into(),
            "10".into(),
            "-t".into(),
            "ed25519,ecdsa,rsa".into(),
            "-p".into(),
            port.to_string(),
            "--".into(),
            host.to_string(),
        ]);
        let output = zendiloos_exec::run(&self.tools.systemd_run, &args, QUICK).await?;
        let keyscan = String::from_utf8_lossy(&output.stdout).into_owned();
        if keyscan.trim().is_empty() {
            let tail = output.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
            return Err(OperationError::Failed(format!("{host} did not answer on port {port}: {tail}")));
        }
        Ok(Response::BackupHostKeys { keyscan })
    }

    /// An end as a unit reaches it: a share mounted, a drive mounted at its
    /// place; a server as given.
    fn end(&self, end: &BackupEnd) -> Result<End, OperationError> {
        match end {
            BackupEnd::Nas { root, path } => {
                let root = match root {
                    BackupRoot::Share { location } => {
                        if self.host.share_mounts {
                            zendiloos_fileops::ShareRoot::open(
                                &self.host.volumes,
                                location.volume,
                                &location.share,
                                true,
                            )
                            .map_err(|e| {
                                refused(format!("the share {} cannot be used: {e}", location.share.as_str()))
                            })?;
                        }
                        self.host
                            .volumes
                            .join(format!("volume{}", location.volume.get()))
                            .join(location.share.as_str())
                    }
                    BackupRoot::Drive { drive } => {
                        zendiloos_fileops::ShareRoot::open_removable(
                            &self.host.removable,
                            drive,
                            self.host.share_mounts,
                        )
                        .map_err(|e| refused(format!("the drive {drive} cannot be used: {e}")))?;
                        self.host.removable.join(drive.as_str())
                    }
                };
                Ok(End::Local { root, path: path.clone() })
            }
            BackupEnd::Remote { server, path } => Ok(match &server.transport {
                BackupTransport::Daemon { module } => End::Daemon {
                    host: server.host.clone(),
                    port: server.port,
                    user: server.user.clone(),
                    module: module.clone(),
                    path: path.clone(),
                },
                BackupTransport::Ssh { host_key } => {
                    self.require(&self.tools.ssh, "the SSH client")?;
                    if !self.key_file().exists() {
                        return Err(OperationError::Setup("the NAS has no SSH key yet".into()));
                    }
                    End::Ssh {
                        host: server.host.clone(),
                        port: server.port,
                        user: server.user.clone(),
                        path: path.clone(),
                        host_key: host_key.clone(),
                    }
                }
            }),
        }
    }

    pub(crate) async fn list(
        &self,
        at: &BackupEnd,
        password: Option<&BackupPassword>,
    ) -> Result<Response, OperationError> {
        let at = self.end(at)?;
        let mut places = Places::default();
        places.of(&at, false);
        let unit = format!("zendiloos-backup-look-{}.service", nonce());
        let report =
            self.unit(&unit, "look into a backup", Work::List { at }, &places, password, LOOK_FOR).await?;
        Ok(Response::BackupListing { listing: report.listing, missing: report.missing })
    }

    pub(crate) async fn run(
        &self,
        run: &BackupRun,
        password: Option<&BackupPassword>,
    ) -> Result<Response, OperationError> {
        if run.source.is_remote() && run.target.is_remote() {
            return Err(refused("a backup goes from or to the NAS"));
        }
        if matches!(run.source, BackupEnd::Nas { root: BackupRoot::Drive { .. }, .. }) {
            return Err(refused("a backup reads a folder of a share"));
        }
        let job = RunJob {
            source: self.end(&run.source)?,
            target: self.end(&run.target)?,
            folder: run.folder.clone(),
            mode: run.mode,
            stamp: run.stamp.clone(),
            excludes: run.excludes.clone(),
            hidden: run.hidden,
            bwlimit_kib: run.bwlimit_kib,
        };
        if job.excludes.len() > zendiloos_agent_proto::BACKUP_EXCLUDES_MAX {
            return Err(refused("too many exclude patterns"));
        }
        let mut places = Places::default();
        places.of(&job.source, false);
        places.of(&job.target, true);
        let unit = run_unit(run.task);
        let mode = match job.mode {
            BackupMode::Mirror { .. } => "mirror",
            BackupMode::Versioned => "version",
        };
        let what = format!("back up task {} ({mode})", run.task.get());
        let report = self.unit(&unit, &what, Work::Run(Box::new(job)), &places, password, RUN_FOR).await?;
        Ok(Response::BackupRan {
            files: report.files,
            bytes: report.bytes,
            partial: report.partial,
            transfers: report.transfers,
            transfers_total: report.transfers_total,
            listing: report.listing,
        })
    }

    /// Stops a task's run; none running is no failure.
    pub(crate) async fn stop(&self, task: BackupTaskId) -> Result<Response, OperationError> {
        let unit = run_unit(task);
        let output = zendiloos_exec::run(&self.tools.systemctl, ["stop", unit.as_str()], QUICK).await?;
        if !output.status.success() && !output.stderr.contains("not loaded") {
            return Err(OperationError::Failed(format!(
                "{unit} cannot be stopped: {}",
                output.stderr.trim()
            )));
        }
        Ok(Response::Done)
    }

    pub(crate) async fn prune(
        &self,
        task: BackupTaskId,
        at: &BackupEnd,
        names: &[BackupPruned],
        password: Option<&BackupPassword>,
    ) -> Result<Response, OperationError> {
        if names.len() > zendiloos_agent_proto::BACKUP_PRUNE_MAX {
            return Err(refused("too many versions at once"));
        }
        if names.is_empty() {
            return Ok(Response::Done);
        }
        let at = self.end(at)?;
        let mut places = Places::default();
        places.of(&at, true);
        let unit = format!("zendiloos-backup-prune-{}.service", task.get());
        let work = Work::Prune { at, names: names.to_vec() };
        self.unit(&unit, &format!("retention of task {}", task.get()), work, &places, password, PRUNE_FOR)
            .await?;
        Ok(Response::Done)
    }

    pub(crate) async fn restore(
        &self,
        restore: &BackupRestore,
        password: Option<&BackupPassword>,
    ) -> Result<Response, OperationError> {
        let BackupEnd::Nas { root, .. } = &restore.to else {
            return Err(refused("a restore goes into a folder of the NAS"));
        };
        if restore.paths.len() > zendiloos_agent_proto::BACKUP_RESTORE_PATHS_MAX {
            return Err(refused("too many paths at once"));
        }
        let job = RestoreJob {
            from: self.end(&restore.from)?,
            paths: restore.paths.clone(),
            to: self.end(&restore.to)?,
            existing: restore.existing,
            into_share: matches!(root, BackupRoot::Share { .. }),
        };
        let mut places = Places::default();
        places.of(&job.from, false);
        places.of(&job.to, true);
        let unit = format!("zendiloos-backup-restore-{}.service", restore.task.get());
        let what = format!("restore task {}'s backup", restore.task.get());
        let report =
            self.unit(&unit, &what, Work::Restore(Box::new(job)), &places, password, RUN_FOR).await?;
        Ok(Response::BackupRestored { files: report.files, bytes: report.bytes, partial: report.partial })
    }

    /// Stops what units of backups still run as the agent starts: their
    /// client is gone, and a task's next run would find its unit taken.
    pub(crate) async fn recover(&self) {
        let mut units: Vec<String> = std::fs::read_dir(&self.paths.transient)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| {
                        name.starts_with("zendiloos-backup-")
                            && name.ends_with(".service")
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if units.is_empty() {
            return;
        }
        units.sort();
        tracing::info!(?units, "stopping the backups' units left running");
        let mut args = vec!["stop".to_owned()];
        args.extend(units);
        if let Err(error) = zendiloos_exec::run(&self.tools.systemctl, &args, QUICK).await {
            tracing::warn!(%error, "cannot stop the backups' units left running");
        }
    }

    /// The unit's sandbox for `places`, and how it runs.
    fn unit_args(
        &self,
        unit: &str,
        description: &str,
        places: &Places,
        runtime: Duration,
        hex: &str,
    ) -> Vec<String> {
        let mut args: Vec<String> = [
            format!("--unit={unit}"),
            "--wait".into(),
            "--pipe".into(),
            "--collect".into(),
            "--quiet".into(),
            format!("--description=zendiloOS: {description}"),
        ]
        .into();
        args.extend(SANDBOX.iter().map(|p| (*p).to_owned()));
        args.extend(network_properties(places.network));
        // Of the volumes and drives, only the roots it works on.
        for hidden in self.volume_places() {
            args.push(format!("--property=TemporaryFileSystem={}:ro", hidden.display()));
        }
        for read in &places.read {
            if places.write.as_ref() != Some(read) {
                args.push(format!("--property=BindReadOnlyPaths={}", read.display()));
            }
        }
        if let Some(write) = &places.write {
            args.push(format!("--property=BindPaths={}", write.display()));
        }
        args.push(format!("--property=InaccessiblePaths={}", HIDDEN.join(" ")));
        if places.ssh {
            args.push(format!("--property=LoadCredential=backup-key:{}", self.key_file().display()));
        }
        args.extend([
            format!("--property=CapabilityBoundingSet={}", places.capabilities()),
            format!("--property=RuntimeMaxSec={}", runtime.as_secs()),
            "--property=TimeoutStopSec=60".into(),
            self.host.agent.display().to_string(),
            "backup-unit".into(),
            hex.into(),
        ]);
        args
    }

    /// Every volume's folder (`/volume<N>`) and the drives' place: each a
    /// read-only tmpfs in a unit, the roots it works on bound back in.
    fn volume_places(&self) -> Vec<PathBuf> {
        let mut places: Vec<PathBuf> = std::fs::read_dir(&self.host.volumes)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        name.strip_prefix("volume")
                            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    })
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default();
        places.sort();
        places.push(self.host.removable.clone());
        places
    }

    /// Runs `work` in the unit `unit`, its secrets on standard input; its
    /// progress becomes the request's, its last line the answer. Should the
    /// client hang up, the agent be asked to stop or the time run out, the
    /// unit is stopped first: no unit works on behind an answer.
    async fn unit(
        &self,
        unit: &str,
        description: &str,
        work: Work,
        places: &Places,
        password: Option<&BackupPassword>,
        runtime: Duration,
    ) -> Result<Report, OperationError> {
        let rsync = self.require(&self.tools.rsync, "rsync")?;
        let key = self.paths.credentials.join(unit).join("backup-key");
        let job = UnitJob { work, rsync: rsync.path().to_owned(), ssh: self.tools.ssh.clone(), key };
        let args = self.unit_args(unit, description, places, runtime, &encode(&job));
        let secrets = match password {
            Some(password) => serde_json::json!({ "password": password.expose() }).to_string(),
            None => String::new(),
        };
        let until = Instant::now() + runtime + Duration::from_secs(120);
        let mut said: Option<UnitSays> = None;
        let mut parts = String::new();
        let ran = {
            let mut heard = |line: &str| match serde_json::from_str::<UnitSays>(line) {
                Ok(UnitSays::Progress { step, percent }) => {
                    crate::progress::step(step_name(&step), Some(percent))
                }
                Ok(UnitSays::Part(part)) if parts.len() < 32 * 1024 * 1024 => parts.push_str(&part),
                Ok(UnitSays::Part(_)) => {}
                Ok(other) => said = Some(other),
                Err(_) => tracing::debug!(line, "a backup's unit wrote"),
            };
            let limits = Limits { timeout: until.saturating_duration_since(Instant::now()), max_stdout: 0 };
            let input = format!("{secrets}\n");
            let surroundings = Surroundings::default();
            let running = zendiloos_exec::run_lines_with_input(
                &self.tools.systemd_run,
                &args,
                input.as_bytes(),
                &surroundings,
                limits,
                &mut heard,
            );
            tokio::select! {
                ran = running => Some(ran),
                () = crate::waiting::given_up() => None,
            }
        };
        let output = match ran {
            Some(Ok(output)) => output,
            Some(Err(ExecError::Timeout { .. })) | None => {
                let _ = zendiloos_exec::run(&self.tools.systemctl, ["stop", unit], QUICK).await;
                return Err(OperationError::Failed(format!("{unit} was stopped before it ended")));
            }
            Some(Err(error)) => return Err(error.into()),
        };
        match said {
            Some(UnitSays::Done) if output.status.success() => serde_json::from_str::<Report>(&parts)
                .map_err(|e| OperationError::Failed(format!("{unit} answered what cannot be read: {e}"))),
            Some(UnitSays::Failed { message, kind: FailKind::HostKey }) => {
                Err(refused(format!("{BACKUP_HOST_KEY_CHANGED}: {message}")))
            }
            Some(UnitSays::Failed { message, kind: FailKind::Refused }) => Err(refused(message)),
            Some(UnitSays::Failed { message, .. }) => Err(OperationError::Failed(message)),
            _ => {
                let tail =
                    output.stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
                Err(OperationError::Failed(format!("{unit} ended unexpectedly: {tail}")))
            }
        }
    }
}

/// A task's run's unit: one at a time, as systemd refuses a second.
fn run_unit(task: BackupTaskId) -> String {
    format!("zendiloos-backup-run-{}.service", task.get())
}

/// The network a unit has: none, or the host's (IP, and Unix and netlink
/// sockets for name resolution) for a server.
fn network_properties(network: bool) -> [String; 2] {
    if network {
        [
            "--property=PrivateNetwork=no".into(),
            "--property=RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK".into(),
        ]
    } else {
        ["--property=PrivateNetwork=yes".into(), "--property=RestrictAddressFamilies=AF_UNIX".into()]
    }
}

/// The steps a unit tells, as the interface names them.
fn step_name(step: &str) -> &'static str {
    match step {
        "finishing" => "finishing",
        "restoring" => "restoring",
        _ => "transferring",
    }
}

/// A word no other unit's name has: for listings, which run side by side.
fn nonce() -> String {
    let mut bytes = [0u8; 6];
    let _ = rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn run(program: &Program, args: &[String], limits: Limits) -> Result<(), OperationError> {
    let output = zendiloos_exec::run(program, args, limits).await?;
    if output.status.success() {
        return Ok(());
    }
    Err(OperationError::Failed(format!("{} failed: {}", program.path().display(), output.stderr.trim())))
}
