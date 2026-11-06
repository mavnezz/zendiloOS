use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use listenfd::ListenFd;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use zendiloos_agent::config::Config;
use zendiloos_agent::{HostPaths, Operations, Server, passwd, pin_shares, run_packages, serve, unpin_shares};
use zendiloos_agent_proto::{AppName, Role};

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

/// Run once instead of serving, by an app's unit
/// (`zendiloos-app-<app>-shares.service`), outside the agent's sandbox.
#[derive(clap::Subcommand)]
enum Command {
    /// Mount the folders of shares the app was given at their places
    PinShares { app: String },
    /// Unmount them again
    UnpinShares { app: String },
    /// Run apt on a task (in zendiloos-packages.service), recording each step
    RunPackages { id: String, task: String, release: Option<String> },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum RoleArg {
    System,
    Files,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(Command::RunPackages { id, task, release }) = &cli.command {
        return run_packages(&HostPaths::default(), id, task, release.as_deref())
            .map_err(|error| anyhow::anyhow!("{error}"));
    }
    if let Some(command) = cli.command {
        let paths = HostPaths::default();
        let (app, done) = match &command {
            Command::PinShares { app } => (app, "pinned"),
            Command::UnpinShares { app } => (app, "unpinned"),
            Command::RunPackages { .. } => unreachable!("handled above"),
        };
        let app = AppName::parse(app).map_err(|_| anyhow::anyhow!("{app:?} is no app"))?;
        match command {
            Command::PinShares { .. } => pin_shares(&paths, &app),
            Command::UnpinShares { .. } => unpin_shares(&paths, &app),
            Command::RunPackages { .. } => unreachable!("handled above"),
        }
        .map_err(|error| anyhow::anyhow!("{}: {error}", app.as_str()))?;
        println!("{}: folders of shares {done}", app.as_str());
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
        version = env!("CARGO_PKG_VERSION"),
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
