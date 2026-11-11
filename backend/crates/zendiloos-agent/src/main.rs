use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use listenfd::ListenFd;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use zendiloos_agent::config::Config;
use zendiloos_agent::{
    HostPaths, Operations, Server, VolumeJob, lower_file_overrides, passwd, pin_project, pin_shares,
    power_off_drive, run_iso_job, run_packages, run_volume_job, serve, unpin_project, unpin_shares,
};
use zendiloos_agent_proto::{AppName, DiskName, ProjectName, Role};

#[derive(Parser)]
#[command(version, about = "The privileged zendiloos agent")]
struct Cli {
    /// Configuration file [default: /etc/zendiloos/agent.toml if it exists]
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Which requests to serve: `system` (disks, ZFS, accounts, server
    /// configuration) or `files` (the contents of volumes)
    #[arg(long, default_value = "system")]
    role: RoleArg,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Run once instead of serving, by an app's or a project's unit
/// (`zendiloos-app-<app>-shares.service`,
/// `zendiloos-project-<project>-shares.service`), outside the agent's
/// sandbox.
#[derive(clap::Subcommand)]
enum Command {
    /// Mount the folders of shares the app was given at their places
    PinShares { app: String },
    /// Unmount them again
    UnpinShares { app: String },
    /// Mount the folders and files in shares an administrator's project binds
    PinProject { project: String },
    /// Unmount them again
    UnpinProject { project: String },
    /// Run apt on a task (in zendiloos-packages.service), recording each step;
    /// an update installs the release given if the channel given takes it
    RunPackages { id: String, task: String, release: Option<String>, channel: Option<String> },
    /// Take an ejected removable drive's disk off the bus, so it can be pulled out
    PowerOffDrive { disk: String },
    /// Back up a Docker volume into a share (a job of the system agent's, in a unit of its own)
    BackupVolume { job: String },
    /// Extract an archive in a share beside a Docker volume's data, to restore it (likewise)
    RestoreVolume { job: String },
    /// Swap what a restore extracted in for the volume's data (likewise)
    SwapVolume { job: String },
    /// Remove a file a backup of a volume wrote (likewise)
    RemoveVolumeBackup { job: String },
    /// Remove what restores left beside the volumes' data (likewise, as the agent starts)
    SweepVolumes { job: String },
    /// Look for backups of Docker volumes in a folder of a share, to record them again (likewise)
    ScanVolumeBackups { job: String },
    /// Copy an ISO from a share for a virtual machine (likewise)
    CopyVmIso { job: String },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum RoleArg {
    System,
    Files,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // The files agent overrides file permissions only on removable drives,
    // on threads of their own: every other thread, this one and each the
    // runtime starts, works without.
    let files = matches!(cli.role, RoleArg::Files) && cli.command.is_none();
    let lower = move || {
        if files && let Err(error) = lower_file_overrides() {
            // Before logging is set up; stderr reaches the journal.
            eprintln!("cannot lower the file permission override: {error}");
            std::process::exit(1);
        }
    };
    lower();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().on_thread_start(lower).build()?;
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(Command::RunPackages { id, task, release, channel }) = &cli.command {
        return run_packages(&HostPaths::default(), id, task, release.as_deref(), channel.as_deref())
            .map_err(|error| anyhow::anyhow!("{error}"));
    }
    let volume_job = match &cli.command {
        Some(Command::BackupVolume { job }) => Some((VolumeJob::BackUp, job)),
        Some(Command::RestoreVolume { job }) => Some((VolumeJob::Restore, job)),
        Some(Command::SwapVolume { job }) => Some((VolumeJob::Swap, job)),
        Some(Command::RemoveVolumeBackup { job }) => Some((VolumeJob::RemoveBackup, job)),
        Some(Command::SweepVolumes { job }) => Some((VolumeJob::Sweep, job)),
        Some(Command::ScanVolumeBackups { job }) => Some((VolumeJob::Scan, job)),
        _ => None,
    };
    if let Some((kind, job)) = volume_job {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0));
        let job = job.clone();
        let done =
            tokio::task::spawn_blocking(move || run_volume_job(kind, &job, now, &mut std::io::stdout()))
                .await?;
        if !done {
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(Command::CopyVmIso { job }) = &cli.command {
        let job = job.clone();
        let done = tokio::task::spawn_blocking(move || run_iso_job(&job, &mut std::io::stdout())).await?;
        if !done {
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(Command::PowerOffDrive { disk }) = &cli.command {
        let name = DiskName::parse(disk).map_err(|_| anyhow::anyhow!("{disk:?} is no disk"))?;
        power_off_drive(&HostPaths::default(), &name).map_err(|error| anyhow::anyhow!("{error}"))?;
        println!("{disk}: powered off");
        return Ok(());
    }
    if let Some(command) = cli.command {
        let paths = HostPaths::default();
        let app = |name: &str| AppName::parse(name).map_err(|_| anyhow::anyhow!("{name:?} is no app"));
        let project =
            |name: &str| ProjectName::parse(name).map_err(|_| anyhow::anyhow!("{name:?} is no project"));
        let (name, done, outcome) = match &command {
            Command::PinShares { app: name } => (name, "pinned", pin_shares(&paths, &app(name)?)),
            Command::UnpinShares { app: name } => (name, "unpinned", unpin_shares(&paths, &app(name)?)),
            Command::PinProject { project: name } => (name, "pinned", pin_project(&paths, &project(name)?)),
            Command::UnpinProject { project: name } => {
                (name, "unpinned", unpin_project(&paths, &project(name)?))
            }
            Command::RunPackages { .. }
            | Command::PowerOffDrive { .. }
            | Command::BackupVolume { .. }
            | Command::RestoreVolume { .. }
            | Command::SwapVolume { .. }
            | Command::RemoveVolumeBackup { .. }
            | Command::SweepVolumes { .. }
            | Command::ScanVolumeBackups { .. }
            | Command::CopyVmIso { .. } => unreachable!("handled above"),
        };
        outcome.map_err(|error| anyhow::anyhow!("{name}: {error}"))?;
        println!("{name}: folders of shares {done}");
        return Ok(());
    }
    let config = Config::load(cli.config.as_deref())?;
    init_logging(&config.log_filter)?;

    let passwd = std::fs::read_to_string("/etc/passwd").context("cannot read /etc/passwd")?;
    let (client_uid, client_gid) = passwd::lookup(&passwd, &config.client_user)
        .with_context(|| format!("unknown client account {:?}", config.client_user))?;
    let role = match cli.role {
        RoleArg::System => Role::System,
        RoleArg::Files => Role::Files,
    };
    let operations = Operations::new(role, &config.tools, config.storage.erasable.clone())?;
    operations.recover().await;
    let listener = listener(&config.socket, client_gid)?;

    tracing::info!(
        client = config.client_user,
        client_uid,
        role = ?role,
        version = zendiloos_agent::VERSION,
        "agent started"
    );
    serve(listener, Server::new(operations, client_uid), shutdown_requested()).await;
    tracing::info!("agent stopped");
    Ok(())
}

/// The socket systemd passed (socket activation, the normal case), or one
/// created here for `root:<client group>` with mode 0660.
fn listener(socket: &Path, client_gid: u32) -> anyhow::Result<UnixListener> {
    if let Some(inherited) = ListenFd::from_env().take_unix_listener(0)? {
        inherited.set_nonblocking(true)?;
        tracing::debug!("using the socket passed by systemd");
        return Ok(UnixListener::from_std(inherited)?);
    }
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("cannot remove stale {}", socket.display()));
        }
        _ => {}
    }
    let listener = UnixListener::bind(socket).with_context(|| format!("cannot bind {}", socket.display()))?;
    std::os::unix::fs::chown(socket, None, Some(client_gid))
        .with_context(|| format!("cannot hand {} to the client group", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

fn init_logging(filter: &str) -> anyhow::Result<()> {
    zendiloos_logging::init(filter, zendiloos_logging::Format::Text)
}

/// Resolves on SIGTERM (systemd stop) or SIGINT.
async fn shutdown_requested() {
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
}
