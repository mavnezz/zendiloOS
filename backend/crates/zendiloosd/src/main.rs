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
use zendiloos_agent_proto::{
    AgentTransport, ConsoleSetupCode, DOCKER_PATIENCE, Request, Response, UnixTransport,
};
use zendiloos_api::{ApiState, UseCases};
use zendiloos_apps::application::{Catalogs, Docker, InstallPorts, Installs, TemplatePorts, Templates};
use zendiloos_certificates::application::{Certificates, CertificatesError, CertificatesPorts, InStep};
use zendiloos_containers::application::{Containers, ContainersPorts};
use zendiloos_firewall::application::{Firewall, FirewallPorts};
use zendiloos_identity::application::commands::{PendingSetup, PurgeExpiredSessions};
use zendiloos_identity::application::{IdentityConfig, IdentityPorts, IdentityUseCases};
use zendiloos_infra::SystemClock;
use zendiloos_infra::agent::{
    AgentAcme, AgentApps, AgentDockerData, AgentDockerEngine, AgentFiles, AgentFirewall, AgentNetwork,
    AgentPowerControl, AgentSharing, AgentSsh, AgentStorage, AgentSystemAccounts, AgentTime, AgentUpdates,
    AgentVms, AgentVolumeArchiver, AgentWebTls, RandomPlanTokens, RandomVmIds, agent_version,
};
use zendiloos_infra::apps::{
    CatalogPolicy, ContainersForApps, FilesForApps, HttpsCatalogs, RandomSecrets, SharingForApps, YamlFiller,
};
use zendiloos_infra::containers::{HttpsRegistries, SharesForBackups, SharesForProjects};
use zendiloos_infra::enclosure::BuiltInCatalog;
use zendiloos_infra::firewall::ServicesForFirewall;
use zendiloos_infra::host::{HostSystemInfo, IpNetworkReader, JournalctlReader, MetricsHub, ProcListening};
use zendiloos_infra::identity::{
    Argon2PasswordHasher, FileSetupCodes, RandomSessionTokens, SharingFollowsAccounts, SharingFollowsGroups,
    TotpCodes,
};
use zendiloos_infra::notify::NetworkChannels;
use zendiloos_infra::sharing::DrivesForFiles;
use zendiloos_infra::sqlite::{
    Database, Occupants, SqliteAccountAddresses, SqliteAccounts, SqliteAppSources, SqliteAuditLog,
    SqliteCertificates, SqliteChannels, SqliteDiskInventory, SqliteEjectedDrives, SqliteEnclosure,
    SqliteFirewall, SqliteGroups, SqliteInstalledApps, SqliteJobs, SqliteNotifications, SqlitePreferences,
    SqliteProjects, SqliteSecurity, SqliteSelfTestPlans, SqliteSessions, SqliteShareProjects, SqliteShares,
    SqliteSystemUpdates, SqliteTwoFactor, SqliteUpdateSettings, SqliteVms, SqliteVolumeBackups,
};
use zendiloos_infra::updates::{GitHubReleases, installed_notes};
use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AllWork, AuditLog, Clock, RequestContext};
use zendiloos_network::application::NetworkUseCases;
use zendiloos_notify::Notifier;
use zendiloos_notify::channels::{ChannelPorts, Channels};
use zendiloos_sharing::application::{FilesUseCases, SharingPorts, SharingUseCases};
use zendiloos_sim::SimulatedAgent;
use zendiloos_storage::application::inventory::DiskInventory;
use zendiloos_storage::application::monitor::StorageMonitor;
use zendiloos_storage::application::removable::RemovableDrives;
use zendiloos_storage::application::self_tests::SelfTestSchedule;
use zendiloos_storage::application::{StoragePorts, StorageUseCases};
use zendiloos_system::application::{SystemPorts, SystemUseCases};
use zendiloos_updates::application::{Updates, UpdatesPorts};
use zendiloos_vms::application::{Vms, VmsPorts};

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
    // Each kind of notification is kept in zendiloOS or not as routed.
    let channel_store = SqliteChannels::new(database.clone());
    let notifier = Notifier::new(Arc::new(SqliteNotifications::new(database.clone())), Arc::new(SystemClock))
        .routed(Arc::new(channel_store.clone()));
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
    )
    .with_current_notes(installed_notes(&config.updates.notes));
    // An update starts only while no job runs, and no job starts while it
    // is started or installs (docs/architecture.md, 4.10).
    jobs.hold_while(Arc::new(updates.clone()));
    let system = SystemUseCases::new(SystemPorts {
        info: Arc::new(HostSystemInfo),
        metrics: Arc::new(metrics),
        power: Arc::new(AgentPowerControl::new(agent.clone())),
        // A reboot waits for jobs and for an update being installed.
        work: Arc::new(AllWork(vec![Arc::new(jobs.clone()), Arc::new(updates.clone())])),
        time: Arc::new(AgentTime::new(agent.clone())),
        ssh: Arc::new(AgentSsh::new(agent.clone())),
        listening: Arc::new(ProcListening::default()),
        tls: Arc::new(AgentWebTls::new(agent.clone())),
        journal: Arc::new(journal),
        audit: audit.clone(),
        audit_reader: Arc::new(SqliteAuditLog::new(database.clone())),
        clock: Arc::new(SystemClock),
    });
    // Notifications also go out by email and to webhooks, named after the host.
    let host_name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|_| "zendiloOS".to_owned());
    let channels = Channels::new(
        ChannelPorts {
            repository: Arc::new(channel_store.clone()),
            routes: Arc::new(channel_store),
            addresses: Arc::new(SqliteAccountAddresses::new(database.clone())),
            sender: Arc::new(NetworkChannels::new()),
            audit: audit.clone(),
            clock: Arc::new(SystemClock),
        },
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
    let storage = StorageUseCases::new(StoragePorts {
        devices: agent_storage.clone(),
        smart: agent_storage.clone(),
        arrays: agent_storage.clone(),
        zfs: agent_storage.clone(),
        occupants: Arc::new(Occupants::new(database.clone(), agent_storage.clone())),
        removable: agent_storage.clone(),
        machine: agent_storage.clone(),
        locator: agent_storage.clone(),
        catalog: Arc::new(BuiltInCatalog::load()?),
        enclosure: Arc::new(SqliteEnclosure::new(database.clone())),
        inventory: Arc::new(SqliteDiskInventory::new(database.clone())),
        ejected: Arc::new(SqliteEjectedDrives::new(database.clone())),
        admin: agent_storage,
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
        tokens: Arc::new(RandomPlanTokens),
        notifier: notifier.clone(),
        self_test_plans: Arc::new(SqliteSelfTestPlans::new(database.clone())),
    });
    let monitor = spawn_storage_monitor(
        Arc::clone(&storage.monitor),
        Arc::clone(&storage.removable),
        storage.inventory.clone(),
    );
    let sharing_store = Arc::new(SqliteShares::new(database.clone()));
    let sharing_ports = SharingPorts {
        shares: sharing_store.clone(),
        directory: sharing_store,
        volumes: agent_storage_for_sharing,
        projects: Arc::new(SqliteShareProjects::new(database.clone())),
        host: Arc::new(AgentSharing::new(agent_for_sharing, files_agent.clone())),
        files: Arc::new(AgentFiles::new(files_agent.clone())),
        removable: Arc::new(DrivesForFiles::new(Arc::clone(&storage.removable))),
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
    // After sharing: Samba's sections follow accounts and groups.
    let identity = identity(&config, &database, audit.clone(), agent.clone(), sharing.clone())?;
    let setup = announce_setup(&config, &identity).await?;
    let purge = spawn_session_purge(identity.purge_expired_sessions.clone());
    let files = FilesUseCases::new(sharing_ports);
    // Uploads that began before now were cut off when zendiloosd stopped:
    // their hidden files go.
    let leftover = (files.clone(), zendiloos_kernel::Clock::now(&SystemClock));
    tokio::spawn(async move {
        match leftover.0.discard_leftover_uploads(leftover.1, &RequestContext::default()).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "files: removed what interrupted uploads left"),
            Err(error) => tracing::warn!(%error, "files: cannot remove what interrupted uploads left"),
        }
    });
    let installed_apps = Arc::new(SqliteInstalledApps::new(database.clone()));
    let agent_apps = Arc::new(AgentApps::new(patient_agent.clone()));
    let installs = Installs::new(InstallPorts {
        installed: installed_apps.clone(),
        host: agent_apps.clone(),
        shares: Arc::new(SharingForApps::new(sharing.clone())),
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
        data: Arc::new(AgentDockerData::new(AgentStorage::new(agent.clone()), moving_agent.clone())),
        // Backups and restores of volumes may take hours, as moving does.
        archiver: Arc::new(AgentVolumeArchiver::new(moving_agent)),
        backups: Arc::new(SqliteVolumeBackups::new(database.clone())),
        backup_shares: Arc::new(SharesForBackups::new(sharing.clone())),
        jobs: jobs.clone(),
        notifier: notifier.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });
    // Backups cut off when zendiloosd stopped: failed, and what backups and
    // restores had stopped runs again once the agent said their work ended
    // (should Docker not be up yet, the minute's look tries again).
    let cut_off = containers.clone();
    tokio::spawn(async move {
        let interrupted = cut_off.recover_backups().await;
        if interrupted > 0 {
            tracing::warn!(interrupted, "backups of volumes were cut off when zendiloosd stopped");
        }
    });
    let templates = Templates::new(TemplatePorts {
        catalogs: apps.clone(),
        installed: installed_apps,
        shares: Arc::new(FilesForApps::new(files.clone())),
        listening: Arc::new(ProcListening::default()),
        secrets: Arc::new(RandomSecrets),
        host: agent_apps,
        filler: Arc::new(YamlFiller),
        policy: Arc::new(CatalogPolicy::new(containers.clone())),
        projects: Arc::new(ContainersForApps::new(containers.clone())),
    });
    let container_sampler = spawn_container_sampler(containers.clone());
    let image_updates = spawn_image_updates(containers.clone());
    let update_checks = spawn_update_checks(updates.clone());

    let certificates = Certificates::new(CertificatesPorts {
        repository: Arc::new(SqliteCertificates::new(database.clone())),
        host: Arc::new(AgentAcme::new(agent.clone())),
        listening: Arc::new(ProcListening::default()),
        jobs: jobs.clone(),
        notifier: notifier.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });
    let certificate_renewal = spawn_certificate_renewal(certificates.clone());

    let firewall = Firewall::new(FirewallPorts {
        repository: Arc::new(SqliteFirewall::new(database.clone())),
        facts: Arc::new(ServicesForFirewall::new(
            certificates.clone(),
            system.clone(),
            sharing.clone(),
            containers.clone(),
            network.clone(),
        )),
        host: Arc::new(AgentFirewall::new(agent.clone())),
        notifier: notifier.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });
    let firewall_upkeep = spawn_firewall_upkeep(firewall.clone(), jobs.clone());

    // Virtual machines: the agent's patience for the long requests (an
    // ISO copied, a VM's disks destroyed) is the system transport's own.
    let vms = Vms::new(VmsPorts {
        repository: Arc::new(SqliteVms::new(database.clone())),
        host: Arc::new(AgentVms::new(agent.clone())),
        isos: Arc::new(zendiloos_infra::vms::SharesForVms::new(files.clone())),
        ids: Arc::new(RandomVmIds),
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(SystemClock),
    });

    let snapshots = spawn_snapshot_schedules(sharing.clone());
    let migrations = spawn_permission_migrations(sharing.clone());
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
        templates,
        containers,
        updates,
        certificates,
        firewall,
        vms,
    };
    let state = ApiState::new(use_cases, config.secure_cookies);
    let app = zendiloos_api::router(state);

    // The console shows where the interface is and, until the setup is done,
    // the setup code or that it is open; the agent writes it, zendiloosd only
    // names the code.
    let (code, open_setup) = match &setup {
        Announced::Code(code) => (ConsoleSetupCode::parse(code).ok(), false),
        Announced::Open => (None, true),
        Announced::Done => (None, false),
    };
    match console_agent.call(Request::ShowOnConsole { setup_code: code, open_setup }, None).await {
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
    certificate_renewal.abort();
    firewall_upkeep.abort();
    monitor.abort();
    snapshots.abort();
    migrations.abort();
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
                // Docker's work: fetching images, making containers, a
                // project's deploy with its pull. The agent waits for a turn
                // half of this at most and works within the other half.
                Arc::new(UnixTransport::new(&config.agent.socket, DOCKER_PATIENCE)),
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
        group_observer: Arc::new(SharingFollowsGroups::new(sharing.clone())),
        account_observer: Arc::new(SharingFollowsAccounts::new(sharing)),
        sessions: Arc::new(SqliteSessions::new(database.clone())),
        security: Arc::new(SqliteSecurity::new(database.clone())),
        preferences: Arc::new(SqlitePreferences::new(database.clone())),
        system: Arc::new(AgentSystemAccounts::new(agent)),
        hasher: Arc::new(Argon2PasswordHasher::new()?),
        tokens: Arc::new(RandomSessionTokens),
        setup_codes: Arc::new(
            FileSetupCodes::new(config.setup_code_path()).with_open_marker(config.setup_open_path()),
        ),
        two_factor: Arc::new(SqliteTwoFactor::new(database.clone())),
        codes: Arc::new(TotpCodes),
        audit,
        clock: Arc::new(SystemClock),
    };
    Ok(IdentityUseCases::new(ports, IdentityConfig::default()))
}

/// How the initial setup stands, for the console.
enum Announced {
    Done,
    Code(String),
    Open,
}

/// While no administrator exists, shows the setup code to whoever can read
/// this machine's logs; nobody else can complete the setup. An open setup
/// (installed from the ISO) has no code: it is said so instead.
async fn announce_setup(config: &Config, identity: &IdentityUseCases) -> anyhow::Result<Announced> {
    Ok(match identity.prepare_setup.execute().await? {
        PendingSetup::Done => Announced::Done,
        PendingSetup::Code(code) => {
            let code = code.reveal();
            tracing::warn!(
                setup_code = %code,
                file = %config.setup_code_path().display(),
                "initial setup required: open the web interface and enter the setup code"
            );
            Announced::Code(code)
        }
        PendingSetup::Open => {
            tracing::warn!(
                marker = %config.setup_open_path().display(),
                "initial setup open: whoever first opens the web interface from this network becomes its administrator"
            );
            Announced::Open
        }
    })
}

/// Looks at pools and disks every minute and notifies about changes;
/// mounts removable drives plugged in meanwhile and lets go of those
/// pulled out (the file explorer has them looked at more often while it
/// is open, by listing them).
fn spawn_storage_monitor(
    monitor: Arc<StorageMonitor>,
    removable: Arc<RemovableDrives>,
    inventory: DiskInventory,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_minute = tokio::time::interval(Duration::from_secs(60));
        loop {
            every_minute.tick().await;
            monitor.check().await;
            removable.watch().await;
            // Every disk seen is remembered, also one never looked at.
            if let Err(error) = inventory.observe(&RequestContext::default()).await {
                tracing::warn!(%error, "cannot note the disks present");
            }
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

/// Walks the shares made before share-level access, once each, one after
/// another (docs/architecture.md, 4.4): at the start and every five
/// minutes, so a walk cut off or failed, or a share whose volume was not
/// mounted, is walked later.
fn spawn_permission_migrations(sharing: SharingUseCases) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut every_five_minutes = tokio::time::interval(Duration::from_secs(5 * 60));
        every_five_minutes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every_five_minutes.tick().await;
            match sharing.migrate_permissions(&RequestContext::default()).await {
                Ok(0) => {}
                Ok(walked) => tracing::info!(walked, "shares given share-level access"),
                Err(error) => tracing::warn!(%error, "cannot walk the shares to share-level access"),
            }
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

/// First brings the web server in step with the settings saved (see
/// [`Certificates::bring_web_server_in_step`]); then renews certificates a
/// month before they end, looked at daily (the first time five minutes
/// after the start); acme.sh's own renewal is never set up.
fn spawn_certificate_renewal(certificates: Certificates) -> JoinHandle<()> {
    tokio::spawn(async move {
        let started = tokio::time::Instant::now();
        bring_web_server_in_step(&certificates).await;
        tokio::time::sleep_until(started + Duration::from_secs(300)).await;
        let mut daily = tokio::time::interval(Duration::from_secs(24 * 3600));
        daily.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            daily.tick().await;
            match certificates.renew_due().await {
                Ok(jobs) if !jobs.is_empty() => tracing::info!(count = jobs.len(), "certificates: renewing"),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "certificates: cannot look for renewals"),
            }
        }
    })
}

/// Waits out a change waiting on the host (it goes back by itself, as
/// nobody can confirm it now), and tries again for a few minutes while the
/// agent does not answer. What the agent refuses, or nginx, stays refused:
/// that is logged.
async fn bring_web_server_in_step(certificates: &Certificates) {
    let context = RequestContext::default();
    let mut failures = 0;
    loop {
        match certificates.bring_web_server_in_step(&context).await {
            Ok(InStep::Done) => return,
            Ok(InStep::WaitUntil(until)) => {
                let left = until.unix_seconds().saturating_sub(SystemClock.now().unix_seconds());
                let wait = u64::try_from(left).unwrap_or(0).clamp(1, 120) + 2;
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            Err(error @ (CertificatesError::Refused(_) | CertificatesError::HostFailed(_))) => {
                tracing::warn!(%error, "certificates: the host does not take the web server's settings saved");
                return;
            }
            Err(error) if failures < 20 => {
                failures += 1;
                tracing::debug!(%error, "certificates: cannot bring the web server in step yet");
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            Err(error) => {
                tracing::warn!(%error, "certificates: cannot bring the web server in step with its settings");
                return;
            }
        }
    }
}

/// Keeps the firewall's rules in force with the ports the services use:
/// at the start, when a job ends (a project deployed, stopped or removed),
/// when asked (SSH, the file services or the web interface's ports
/// changed, a change not confirmed in time), and every 30 seconds.
fn spawn_firewall_upkeep(firewall: Firewall, jobs: Jobs) -> JoinHandle<()> {
    tokio::spawn(async move {
        let context = RequestContext::default();
        let mut events = jobs.subscribe();
        let mut every = tokio::time::interval(Duration::from_secs(30));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                () = firewall.nudged() => {}
                event = events.recv() => match event {
                    Ok(job) if job.state == zendiloos_jobs::JobState::Running => continue,
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => events = jobs.subscribe(),
                },
            }
            if let Err(error) = firewall.refresh(&context).await {
                tracing::warn!(%error, "firewall: cannot bring the rules up to date");
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
            // Backups of volumes whose time came; a missed one (the machine
            // was off) on the first look.
            for job in containers.run_due_backups().await {
                tracing::info!(job = job.id.get(), "containers: backing up a volume on schedule");
            }
            // Containers a backup or restore stopped whose start failed.
            let started = containers.start_owed_containers().await;
            if started > 0 {
                tracing::info!(started, "containers: stopped for a backup or restore, running again");
            }
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
