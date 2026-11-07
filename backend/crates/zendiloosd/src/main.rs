mod config;
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
use zendiloos_agent_proto::{AgentTransport, ConsoleSetupCode, Request, Response, UnixTransport};
use zendiloos_api::{ApiState, UseCases};
use zendiloos_apps::application::{Catalogs, Docker, InstallPorts, Installs};
use zendiloos_containers::application::{Containers, ContainersPorts};
use zendiloos_identity::application::commands::PurgeExpiredSessions;
use zendiloos_identity::application::{IdentityConfig, IdentityPorts, IdentityUseCases};
use zendiloos_infra::SystemClock;
use zendiloos_infra::agent::{
    AgentApps, AgentDockerData, AgentDockerEngine, AgentFiles, AgentNetwork, AgentPowerControl, AgentSharing,
    AgentSsh, AgentStorage, AgentSystemAccounts, AgentTime, AgentUpdates, AgentWebTls, RandomPlanTokens,
    agent_version,
};
use zendiloos_infra::apps::{HttpsCatalogs, RandomSecrets, SharingForApps};
use zendiloos_infra::containers::{HttpsRegistries, SharesForProjects};
use zendiloos_infra::host::{HostSystemInfo, IpNetworkReader, JournalctlReader, MetricsHub, ProcListening};
use zendiloos_infra::identity::{
    Argon2PasswordHasher, FileSetupCodes, RandomSessionTokens, SharingFollowsGroups,
};
use zendiloos_infra::notify::NetworkChannels;
use zendiloos_infra::sqlite::{
    Database, Occupants, SqliteAccounts, SqliteAppSources, SqliteAuditLog, SqliteChannels, SqliteGroups,
    SqliteInstalledApps, SqliteJobs, SqliteNotifications, SqlitePreferences, SqliteProjects, SqliteSecurity,
    SqliteSelfTestPlans, SqliteSessions, SqliteShares, SqliteSystemUpdates, SqliteUpdateSettings,
};
use zendiloos_infra::updates::GitHubReleases;
use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AuditLog, RequestContext};
use zendiloos_network::application::NetworkUseCases;
use zendiloos_notify::Notifier;
use zendiloos_notify::channels::Channels;
use zendiloos_sharing::application::{FilesUseCases, SharingPorts, SharingUseCases};
use zendiloos_sim::SimulatedAgent;
use zendiloos_storage::application::monitor::StorageMonitor;
use zendiloos_storage::application::self_tests::SelfTestSchedule;
use zendiloos_storage::application::{StoragePorts, StorageUseCases};
use zendiloos_system::application::{SystemPorts, SystemUseCases};
use zendiloos_updates::application::{Updates, UpdatesPorts};

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
    let (agent, files_agent, patient_agent, moving_agent) = agent_transports(&config).await;
    let (metrics, sampler) = MetricsHub::spawn(Duration::from_secs(2), Arc::new(SystemClock));
    let journal = JournalctlReader::new(config.journal.journalctl.as_deref());
    match journal.path() {
        Some(path) => tracing::info!(path = %path.display(), "reading the journal with journalctl"),
        None => tracing::warn!("journalctl not found: the log viewer will not work"),
    }
    let jobs = Jobs::new(Arc::new(SqliteJobs::new(database.clone())), Arc::new(SystemClock));
    let interrupted = jobs.recover().await.context("cannot read the jobs")?;
    if interrupted > 0 {
        tracing::warn!(interrupted, "jobs of the previous run were interrupted");
    }
    let system = SystemUseCases::new(SystemPorts {
        info: Arc::new(HostSystemInfo),
        metrics: Arc::new(metrics),
        power: Arc::new(AgentPowerControl::new(agent.clone())),
        work: Arc::new(jobs.clone()),
        time: Arc::new(AgentTime::new(agent.clone())),
        ssh: Arc::new(AgentSsh::new(agent.clone())),
        listening: Arc::new(ProcListening::default()),
        tls: Arc::new(AgentWebTls::new(agent.clone())),
        journal: Arc::new(journal),
        audit: audit.clone(),
        audit_reader: Arc::new(SqliteAuditLog::new(database.clone())),
        clock: Arc::new(SystemClock),
    });
    let notifier = Notifier::new(Arc::new(SqliteNotifications::new(database.clone())), Arc::new(SystemClock));
    // Notifications also go out by email and to webhooks, named after the host.
    let host_name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|_| "zendiloOS".to_owned());
    let channels = Channels::new(
        Arc::new(SqliteChannels::new(database.clone())),
        Arc::new(NetworkChannels::new()),
        audit.clone(),
        Arc::new(SystemClock),
        host_name,
    );
    let channel_delivery = channels.clone().spawn(&notifier);
    let apps = Catalogs::new(
        Arc::new(SqliteAppSources::new(database.clone())),
        Arc::new(HttpsCatalogs::new()),
        audit.clone(),
        Arc::new(SystemClock),
    );
    let catalog_refresh = spawn_catalog_refresh(apps.clone());
    let agent_for_sharing = agent.clone();
    let console_agent = agent.clone();
    let network_agent = agent.clone();
    let agent_storage = Arc::new(AgentStorage::new(agent.clone()));
    let agent_storage_for_sharing = agent_storage.clone();
    let docker = Docker::new(agent_storage.clone(), audit.clone(), Arc::new(SystemClock), jobs.clone());
    let docker_host = agent_storage.clone();
    let storage = StorageUseCases::new(StoragePorts {
        devices: agent_storage.clone(),
        smart: agent_storage.clone(),
        arrays: agent_storage.clone(),
        zfs: agent_storage.clone(),
        occupants: Arc::new(Occupants::new(database.clone(), agent_storage.clone())),
        admin: agent_storage,
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
        tokens: Arc::new(RandomPlanTokens),
        notifier: notifier.clone(),
        self_test_plans: Arc::new(SqliteSelfTestPlans::new(database.clone())),
    });
    let monitor = spawn_storage_monitor(Arc::clone(&storage.monitor));
    let sharing_store = Arc::new(SqliteShares::new(database.clone()));
    let sharing_ports = SharingPorts {
        shares: sharing_store.clone(),
        directory: sharing_store,
        volumes: agent_storage_for_sharing,
        host: Arc::new(AgentSharing::new(agent_for_sharing, files_agent.clone())),
        files: Arc::new(AgentFiles::new(files_agent.clone())),
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
        notifier: notifier.clone(),
    };
    let sharing = SharingUseCases::new(sharing_ports.clone());
    // Shares whose data could not go before (busy, or zendiloosd stopped).
    let unfinished = sharing.clone();
    tokio::spawn(async move {
        match unfinished.finish_deletions(&RequestContext::default()).await {
            Ok(0) => {}
            Ok(left) => tracing::warn!(
                left,
                "the data of deleted shares is still there; tried again at the next start"
            ),
            Err(error) => tracing::error!(%error, "cannot finish deleting shares"),
        }
    });
    // After sharing: shares follow the members of groups they deny.
    let identity = identity(&config, &database, audit.clone(), agent.clone(), sharing.clone())?;
    let setup_code = announce_setup(&config, &identity).await?;
    let purge = spawn_session_purge(identity.purge_expired_sessions.clone());
    let files = FilesUseCases::new(sharing_ports);
    let installs = Installs::new(InstallPorts {
        catalogs: apps.clone(),
        installed: Arc::new(SqliteInstalledApps::new(database.clone())),
        host: Arc::new(AgentApps::new(patient_agent.clone(), files_agent.clone())),
        docker: docker_host,
        shares: Arc::new(SharingForApps::new(sharing.clone())),
        listening: Arc::new(ProcListening::default()),
        secrets: Arc::new(RandomSecrets),
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });
    match installs.recover().await {
        Ok(0) => {}
        Ok(failed) => {
            tracing::warn!(failed, "apps being installed or removed when zendiloosd stopped are failed")
        }
        Err(error) => tracing::warn!(error = %error_chain(&error), "cannot look at the installed apps"),
    }
    // The servers offer what the database says, whatever happened since.
    if let Err(error) = sharing.apply(&RequestContext::default()).await {
        tracing::warn!(error = %error_chain(&error), "cannot apply the sharing configuration");
    }

    let ip = IpNetworkReader::new(config.network.ip.as_deref());
    match ip.path() {
        Some(path) => tracing::info!(path = %path.display(), "reading the network with ip"),
        None => tracing::warn!("ip not found: the network will not be shown"),
    }
    let network = NetworkUseCases::new(
        Arc::new(ip),
        Arc::new(AgentNetwork::new(network_agent)),
        audit.clone(),
        Arc::new(SystemClock),
    );

    let containers = Containers::new(ContainersPorts {
        engine: Arc::new(AgentDockerEngine::new(patient_agent)),
        projects: Arc::new(SqliteProjects::new(database.clone())),
        storage: Arc::new(SharesForProjects::new(files.clone())),
        registries: Arc::new(HttpsRegistries::default()),
        updates: Arc::new(SqliteUpdateSettings::new(database.clone())),
        data: Arc::new(AgentDockerData::new(AgentStorage::new(agent.clone()), moving_agent)),
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });
    let container_sampler = spawn_container_sampler(containers.clone());
    let image_updates = spawn_image_updates(containers.clone());
    let updates = Updates::new(
        UpdatesPorts {
            releases: Arc::new(GitHubReleases::new(
                &config.updates.api,
                &config.updates.repository,
                config.update_dir(),
            )),
            host: Arc::new(AgentUpdates::new(agent.clone())),
            store: Arc::new(SqliteSystemUpdates::new(database.clone())),
            notifier: notifier.clone(),
            work: Arc::new(jobs.clone()),
            audit: audit.clone(),
            clock: Arc::new(SystemClock),
        },
        zendiloos_kernel::VERSION,
    );
    let update_checks = spawn_update_checks(updates.clone());

    let snapshots = spawn_snapshot_schedules(sharing.clone());
    let self_tests = spawn_self_test_schedule(storage.self_test_schedule.clone());
    let use_cases = UseCases {
        system,
        identity,
        storage,
        sharing,
        files,
        network,
        jobs,
        notifier,
        channels,
        apps,
        docker,
        installs,
        containers,
        updates,
    };
    let state = ApiState::new(use_cases, config.secure_cookies);
    let app = zendiloos_api::router(state);

    // The console shows where the interface is and, until the setup is done,
    // the setup code; the agent writes it, zendiloosd only names the code.
    let code = setup_code.as_deref().and_then(|code| ConsoleSetupCode::parse(code).ok());
    match console_agent.call(Request::ShowOnConsole { setup_code: code }, None).await {
        Ok(Response::Done) => {}
        other => tracing::warn!(answer = ?other, "cannot show the address on the console"),
    }

    // Under systemd the socket comes from zendiloosd.socket and only nginx
    // can reach it; run by hand, zendiloosd listens on TCP.
    if let Some(listener) = systemd::inherited_unix_listener()? {
        tracing::info!(version = zendiloos_kernel::VERSION, "zendiloosd started on the socket from systemd");
        systemd::notify_ready();
        axum::serve(listener, app).with_graceful_shutdown(shutdown_requested()).await?;
    } else {
        let listener = TcpListener::bind(config.listen)
            .await
            .with_context(|| format!("cannot listen on {}", config.listen))?;
        tracing::info!(address = %listener.local_addr()?, version = zendiloos_kernel::VERSION, "zendiloosd started");
        systemd::notify_ready();
        let app = app.into_make_service_with_connect_info::<SocketAddr>();
        axum::serve(listener, app).with_graceful_shutdown(shutdown_requested()).await?;
    }
    purge.abort();
    sampler.abort();
    container_sampler.abort();
    image_updates.abort();
    update_checks.abort();
    monitor.abort();
    snapshots.abort();
    self_tests.abort();
    channel_delivery.abort();
    catalog_refresh.abort();
    database.close().await;
    tracing::info!("zendiloosd stopped");
    Ok(())
}

/// The way to the privileged agent. An unreachable agent is not fatal:
/// systemd starts it on the first connection, and only privileged
/// operations need it.
/// The system and the files agent, and the system agent once more with
/// the patience apps need. The simulation serves all.
async fn agent_transports(
    config: &Config,
) -> (Arc<dyn AgentTransport>, Arc<dyn AgentTransport>, Arc<dyn AgentTransport>, Arc<dyn AgentTransport>) {
    type Transport = Arc<dyn AgentTransport>;
    let (system, files, patient, moving): (Transport, Transport, Transport, Transport) =
        match config.agent.transport {
            AgentTransportKind::Unix => (
                // Long enough for whatever the agent runs once it started
                // (each program it calls has a limit of two minutes at most,
                // a pool's operations several); a request still waiting for
                // its turn when the time is up is dropped by the agent.
                Arc::new(
                    UnixTransport::new(&config.agent.socket, Duration::from_secs(120))
                        .with_long(Duration::from_secs(2 * 3600)),
                ),
                // Copies, moves and permissions of a whole share may take
                // many hours; everything else little.
                Arc::new(
                    UnixTransport::new(&config.agent.files_socket, Duration::from_secs(600))
                        .with_long(Duration::from_secs(24 * 3600)),
                ),
                // Fetching an app's images too; the agent gives up after 30 minutes.
                Arc::new(UnixTransport::new(&config.agent.socket, Duration::from_secs(31 * 60))),
                // Copying Docker's data to another pool; the agent gives up after six hours.
                Arc::new(UnixTransport::new(&config.agent.socket, Duration::from_secs(6 * 3600 + 600))),
            ),
            AgentTransportKind::Simulated => {
                tracing::warn!("using the simulated agent: privileged operations are not carried out");
                // Share contents are real files, kept with the state.
                let simulated =
                    Arc::new(SimulatedAgent::new().with_files(config.state_dir.join("simulated-volumes")));
                (simulated.clone(), simulated.clone(), simulated.clone(), simulated)
            }
        };
    for (role, transport) in [("system", &system), ("files", &files)] {
        match agent_version(transport.as_ref()).await {
            Ok(version) => tracing::info!(role, version, "agent reachable"),
            Err(error) => tracing::warn!(role, error = %error_chain(&error), "agent not reachable"),
        }
    }
    (system, files, patient, moving)
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
    sharing: SharingUseCases,
) -> anyhow::Result<IdentityUseCases> {
    let ports = IdentityPorts {
        accounts: Arc::new(SqliteAccounts::new(database.clone())),
        groups: Arc::new(SqliteGroups::new(database.clone())),
        group_observer: Arc::new(SharingFollowsGroups::new(sharing)),
        sessions: Arc::new(SqliteSessions::new(database.clone())),
        security: Arc::new(SqliteSecurity::new(database.clone())),
        preferences: Arc::new(SqlitePreferences::new(database.clone())),
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

/// Looks at pools and disks every minute and notifies about changes.
fn spawn_storage_monitor(monitor: Arc<StorageMonitor>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_minute = tokio::time::interval(Duration::from_secs(60));
        loop {
            every_minute.tick().await;
            monitor.check().await;
        }
    })
}

/// Takes scheduled snapshots of shares and lets go of old ones. Every five
/// minutes, so a schedule runs at most that late; a missed one (the machine
/// was off) is taken on the first look.
fn spawn_snapshot_schedules(sharing: SharingUseCases) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_five_minutes = tokio::time::interval(Duration::from_secs(5 * 60));
        loop {
            every_five_minutes.tick().await;
            sharing.take_due_snapshots().await;
        }
    })
}

/// Starts the disks' scheduled self-tests. Every five minutes, so a test
/// starts at most that late into its window.
fn spawn_self_test_schedule(schedule: SelfTestSchedule) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_five_minutes = tokio::time::interval(Duration::from_secs(5 * 60));
        loop {
            every_five_minutes.tick().await;
            schedule.run_due().await;
        }
    })
}

/// Reads the app catalogs at startup and every six hours; a catalog that
/// cannot be reached keeps the apps read before.
fn spawn_catalog_refresh(apps: Catalogs) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_six_hours = tokio::time::interval(Duration::from_secs(6 * 3600));
        loop {
            every_six_hours.tick().await;
            if let Err(error) = apps.refresh().await {
                tracing::warn!(%error, "apps: cannot read the catalogs");
            }
        }
    })
}

/// Takes newer images when the schedule says so, checked every minute,
/// and asks the registries for them every six hours, the first time three
/// minutes after the start.
/// Says how a finished update went and looks for updates daily, as the
/// settings ask; the first time a few minutes after the start.
fn spawn_update_checks(updates: Updates) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut every = tokio::time::interval(zendiloos_updates::application::TICK);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            if let Err(error) = updates.tick().await {
                tracing::info!(%error, "updates: cannot look for updates");
            }
        }
    })
}

fn spawn_image_updates(containers: Containers) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_minute = tokio::time::interval(Duration::from_secs(60));
        every_minute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut minutes: u32 = 0;
        loop {
            every_minute.tick().await;
            match containers.run_schedule().await {
                Ok(Some(job)) => {
                    tracing::info!(job = job.id.get(), "containers: updating images on schedule")
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "containers: cannot update images on schedule"),
            }
            if minutes % 360 == 3
                && let Err(error) = containers.check_if_wanted().await
            {
                tracing::info!(%error, "containers: cannot look for newer images");
            }
            minutes = minutes.wrapping_add(1);
        }
    })
}

/// Samples what the containers use, for the history. Says once when Docker
/// stops answering, and once when it answers again.
fn spawn_container_sampler(containers: Containers) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(zendiloos_containers::application::SAMPLE_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut answering = true;
        loop {
            every.tick().await;
            match containers.sample().await {
                Ok(()) if !answering => {
                    tracing::info!("containers: Docker answers again");
                    answering = true;
                }
                Ok(()) => {}
                Err(error) if answering => {
                    tracing::info!(%error, "containers: no samples while Docker does not answer");
                    answering = false;
                }
                Err(_) => {}
            }
        }
    })
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
