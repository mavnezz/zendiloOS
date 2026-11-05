mod config;
mod console;
mod logging;
mod systemd;

use std::net::SocketAddr;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{AgentTransport, UnixTransport};
use zendiloos_api::{ApiState, UseCases};
use zendiloos_identity::application::commands::PurgeExpiredSessions;
use zendiloos_identity::application::{IdentityConfig, IdentityPorts, IdentityUseCases};
use zendiloos_infra::SystemClock;
use zendiloos_infra::agent::{AgentPowerControl, AgentStorage, AgentSystemAccounts, agent_version};
use zendiloos_infra::host::{HostSystemInfo, JournalctlReader, MetricsHub};
use zendiloos_infra::identity::{Argon2PasswordHasher, FileSetupCodes, RandomSessionTokens};
use zendiloos_infra::sqlite::{Database, SqliteAccounts, SqliteAuditLog, SqliteGroups, SqliteSessions};
use zendiloos_kernel::AuditLog;
use zendiloos_sim::SimulatedAgent;
use zendiloos_storage::application::{StoragePorts, StorageUseCases};
use zendiloos_system::application::{SystemPorts, SystemUseCases};

use crate::config::{AgentTransportKind, Config, Source};

#[derive(Parser)]
#[command(version, about = "The Zendiloos API server")]
struct Cli {
    /// Configuration file [default: /etc/zendiloos/zendiloosd.toml if it exists]
    #[arg(short, long, value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the API server (default)
    Serve,
    /// Print the OpenAPI document as JSON and exit
    Openapi,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Openapi => {
            println!("{}", zendiloos_api::openapi().to_pretty_json()?);
            Ok(())
        }
        Command::Serve => {
            let (config, source) = Config::load(cli.config.as_deref())?;
            logging::init(&config.log)?;
            match source {
                Source::File(path) => tracing::info!(path = %path.display(), "configuration loaded"),
                Source::Defaults => {
                    tracing::info!(path = config::DEFAULT_PATH, "no configuration file, using defaults")
                }
            }
            serve(config).await
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let database = open_database(&config).await?;
    let audit: Arc<dyn AuditLog> = Arc::new(SqliteAuditLog::new(database.clone()));
    let agent = agent_transport(&config).await;
    let identity = identity(&config, &database, audit.clone(), agent.clone())?;
    let setup_code = announce_setup(&config, &identity).await?;
    let purge = spawn_session_purge(identity.purge_expired_sessions.clone());
    let (metrics, sampler) = MetricsHub::spawn(Duration::from_secs(2), Arc::new(SystemClock));
    let journal = JournalctlReader::new(config.journal.journalctl.as_deref());
    match journal.path() {
        Some(path) => tracing::info!(path = %path.display(), "reading the journal with journalctl"),
        None => tracing::warn!("journalctl not found: the log viewer will not work"),
    }
    let system = SystemUseCases::new(SystemPorts {
        info: Arc::new(HostSystemInfo),
        metrics: Arc::new(metrics),
        power: Arc::new(AgentPowerControl::new(agent.clone())),
        journal: Arc::new(journal),
        audit,
        audit_reader: Arc::new(SqliteAuditLog::new(database.clone())),
        clock: Arc::new(SystemClock),
    });
    let agent_storage = Arc::new(AgentStorage::new(agent));
    let storage = StorageUseCases::new(StoragePorts {
        devices: agent_storage.clone(),
        smart: agent_storage.clone(),
        arrays: agent_storage,
    });

    let state = ApiState::new(UseCases { system, identity, storage }, config.secure_cookies);
    let app = zendiloos_api::router(state);

    let issue = config.state_dir.join("console.issue");
    console::write_issue(&issue, &console::issue_text(setup_code.as_deref()))
        .with_context(|| format!("cannot write {}", issue.display()))?;

    // Under systemd the socket comes from zendiloosd.socket and only nginx
    // can reach it; run by hand, zendiloosd listens on TCP.
    if let Some(listener) = systemd::inherited_unix_listener()? {
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "zendiloosd started on the socket from systemd");
        systemd::notify_ready();
        axum::serve(listener, app).with_graceful_shutdown(shutdown_requested()).await?;
    } else {
        let listener = TcpListener::bind(config.listen)
            .await
            .with_context(|| format!("cannot listen on {}", config.listen))?;
        tracing::info!(address = %listener.local_addr()?, version = env!("CARGO_PKG_VERSION"), "zendiloosd started");
        systemd::notify_ready();
        let app = app.into_make_service_with_connect_info::<SocketAddr>();
        axum::serve(listener, app).with_graceful_shutdown(shutdown_requested()).await?;
    }
    purge.abort();
    sampler.abort();
    database.close().await;
    tracing::info!("zendiloosd stopped");
    Ok(())
}

/// The way to the privileged agent. An unreachable agent is not fatal:
/// systemd starts it on the first connection, and only privileged
/// operations need it.
async fn agent_transport(config: &Config) -> Arc<dyn AgentTransport> {
    let transport: Arc<dyn AgentTransport> = match config.agent.transport {
        AgentTransportKind::Unix => {
            Arc::new(UnixTransport::new(&config.agent.socket, Duration::from_secs(30)))
        }
        AgentTransportKind::Simulated => {
            tracing::warn!("using the simulated agent: privileged operations are not carried out");
            Arc::new(SimulatedAgent::new())
        }
    };
    match agent_version(transport.as_ref()).await {
        Ok(version) => tracing::info!(version, "agent reachable"),
        Err(error) => tracing::warn!(error = %error_chain(&error), "agent not reachable"),
    }
    transport
}

/// `outer: inner: innermost`, so the log shows the root cause.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain = format!("{chain}: {cause}");
        source = cause.source();
    }
    chain
}

/// Plugs the adapters into the ports of the Identity context.
fn identity(
    config: &Config,
    database: &Database,
    audit: Arc<dyn AuditLog>,
    agent: Arc<dyn AgentTransport>,
) -> anyhow::Result<IdentityUseCases> {
    let ports = IdentityPorts {
        accounts: Arc::new(SqliteAccounts::new(database.clone())),
        groups: Arc::new(SqliteGroups::new(database.clone())),
        sessions: Arc::new(SqliteSessions::new(database.clone())),
        system: Arc::new(AgentSystemAccounts::new(agent)),
        hasher: Arc::new(Argon2PasswordHasher::new()?),
        tokens: Arc::new(RandomSessionTokens),
        setup_codes: Arc::new(FileSetupCodes::new(config.setup_code_path())),
        audit,
        clock: Arc::new(SystemClock),
    };
    Ok(IdentityUseCases::new(ports, IdentityConfig::default()))
}

/// While no administrator exists, shows the setup code to whoever can read
/// this machine's logs. Nobody else can complete the setup.
/// Returns the setup code while it is needed, for the console.
async fn announce_setup(config: &Config, identity: &IdentityUseCases) -> anyhow::Result<Option<String>> {
    let Some(code) = identity.prepare_setup.execute().await? else { return Ok(None) };
    let code = code.reveal();
    tracing::warn!(
        setup_code = %code,
        file = %config.setup_code_path().display(),
        "initial setup required: open the web interface and enter the setup code"
    );
    Ok(Some(code))
}

fn spawn_session_purge(purge: PurgeExpiredSessions) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut hourly = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            hourly.tick().await;
            match purge.execute().await {
                Ok(0) => {}
                Ok(removed) => tracing::debug!(removed, "removed ended sessions"),
                Err(error) => tracing::warn!(%error, "cannot remove ended sessions"),
            }
        }
    })
}

async fn open_database(config: &Config) -> anyhow::Result<Database> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config.state_dir)
        .with_context(|| format!("cannot create state directory {}", config.state_dir.display()))?;

    let path = config.database_path();
    let (database, report) = Database::open(&path, &config.backup_dir()).await?;
    if let Some(backup) = &report.backup {
        tracing::info!(path = %backup.display(), "database backed up before migrating");
    }
    tracing::info!(
        path = %path.display(),
        created = report.created,
        schema_before = report.schema_before,
        schema = report.schema_after,
        applied = ?report.applied,
        "database ready"
    );
    Ok(database)
}

/// Resolves on SIGTERM (systemd stop) or SIGINT (Ctrl+C).
async fn shutdown_requested() {
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM, only Ctrl+C stops the server");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}
