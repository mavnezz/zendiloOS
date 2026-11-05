use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use listenfd::ListenFd;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;
use zendiloos_agent::config::Config;
use zendiloos_agent::{Operations, Server, passwd, serve};

#[derive(Parser)]
#[command(version, about = "The privileged zendiloos agent")]
struct Cli {
    /// Configuration file [default: /etc/zendiloos/agent.toml if it exists]
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    init_logging(&config.log_filter)?;

    let passwd = std::fs::read_to_string("/etc/passwd").context("cannot read /etc/passwd")?;
    let (client_uid, client_gid) = passwd::lookup(&passwd, &config.client_user)
        .with_context(|| format!("unknown client account {:?}", config.client_user))?;
    let operations = Operations::new(&config.tools)?;
    let listener = listener(&config.socket, client_gid)?;

    tracing::info!(
        client = config.client_user,
        client_uid,
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
    let filter = match std::env::var("RUST_LOG") {
        Ok(directive) if !directive.is_empty() => EnvFilter::try_new(directive)?,
        _ => EnvFilter::try_new(filter)?,
    };
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr);
    // Under systemd stderr goes to the journal, which adds its own timestamps.
    let result = if std::env::var_os("JOURNAL_STREAM").is_some() {
        builder.without_time().with_ansi(false).try_init()
    } else {
        builder.with_ansi(std::io::stderr().is_terminal()).try_init()
    };
    result.map_err(|error| anyhow::anyhow!(error))
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
