use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header, request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use zendiloos_identity::domain::{AccountName, Role};
use zendiloos_identity::testing::{SETUP_CODE, TestIdentity};
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::MemoryJobs;
use zendiloos_kernel::{Outcome, PortError, RequestContext};
use zendiloos_network::application::NetworkUseCases;
use zendiloos_network::testing::FixedNetwork;
use zendiloos_notify::testing::MemoryNotifications;
use zendiloos_notify::{Level, NewNotification, Notifier};
use zendiloos_sharing::application::{FilesUseCases, SharingPorts, SharingUseCases};
use zendiloos_storage::application::StorageUseCases;
use zendiloos_storage::testing::FakeStorage;
use zendiloos_system::application::ports::{
    AuditReader, JournalReader, ListeningPorts, LiveMetrics, MetricsSubscription, PowerControl, SshControl,
    SystemInfoProvider, TimeControl, WebTlsControl,
};
use zendiloos_system::application::{SystemPorts, SystemUseCases};
use zendiloos_system::domain::{
    AuditQuery, AuditRecord, Hardware, JournalCursor, JournalEntry, JournalPage, JournalQuery, LogPriority,
    MemoryUsage, MetricsSample, PowerAction, SshSettings, SshStatus, SystemInfo, TimeSettings, TimeStatus,
    TlsMinimum,
};

use crate::{ApiState, CSRF_HEADER, UseCases, openapi, router};

pub(crate) const HOST: &str = "nas.local";
pub(crate) const PASSWORD: &str = "correct horse battery";

struct FakeProvider;

#[async_trait]
impl SystemInfoProvider for FakeProvider {
    async fn system_info(&self) -> Result<SystemInfo, PortError> {
        Ok(SystemInfo {
            hostname: "nas".to_owned(),
            os_name: Some("Debian GNU/Linux 13 (trixie)".to_owned()),
            kernel_release: "6.12.48+deb13-amd64".to_owned(),
            architecture: "x86_64".to_owned(),
            cpu_model: Some("Intel(R) N100".to_owned()),
            cpu_threads: 4,
            cpu_cores: Some(4),
            memory_total_bytes: 8 * 1024 * 1024 * 1024,
            swap_total_bytes: 1024 * 1024 * 1024,
            uptime_seconds: 3600,
            hardware: Hardware {
                vendor: Some("ASUSTeK COMPUTER INC.".to_owned()),
                product: Some("PN42".to_owned()),
                board: None,
                firmware: Some("American Megatrends 0304 (05/12/2024)".to_owned()),
                uefi: true,
            },
        })
    }
}

struct FailingProvider;

#[async_trait]
impl SystemInfoProvider for FailingProvider {
    async fn system_info(&self) -> Result<SystemInfo, PortError> {
        Err(PortError::new("system information", io::Error::other("secret internal detail")))
    }
}

/// Remembers power actions instead of carrying them out.
#[derive(Default)]
pub(crate) struct RecordingPower(Mutex<Vec<PowerAction>>);

#[async_trait]
impl PowerControl for RecordingPower {
    async fn request(&self, action: PowerAction, _: &RequestContext) -> Result<(), PortError> {
        self.0.lock().unwrap().push(action);
        Ok(())
    }
}

/// An SSH server on port 22, off; remembers what was set.
#[derive(Default)]
struct FakeSsh(Mutex<Option<SshSettings>>);

#[async_trait]
impl SshControl for FakeSsh {
    async fn status(&self, _: &RequestContext) -> Result<SshStatus, PortError> {
        let set = *self.0.lock().unwrap();
        Ok(SshStatus {
            installed: true,
            enabled: set.is_some_and(|s| s.enabled),
            running: set.is_some_and(|s| s.enabled),
            port: set.map_or(22, |s| s.port),
            root_login: set.and_then(|s| s.root_login),
            password_login: set.and_then(|s| s.password_login),
            off_at: set.and_then(|s| s.off_after_minutes).map(|minutes| {
                zendiloos_kernel::Timestamp::from_unix_seconds(1_800_000_000 + i64::from(minutes) * 60)
            }),
        })
    }

    async fn apply(&self, settings: &SshSettings, _: &RequestContext) -> Result<(), PortError> {
        *self.0.lock().unwrap() = Some(*settings);
        Ok(())
    }
}

/// nginx's TLS: 1.2 and newer until set.
#[derive(Default)]
struct FakeTls(Mutex<TlsMinimum>);

#[async_trait]
impl WebTlsControl for FakeTls {
    async fn minimum(&self, _: &RequestContext) -> Result<TlsMinimum, PortError> {
        Ok(*self.0.lock().unwrap())
    }

    async fn apply(&self, minimum: TlsMinimum, _: &RequestContext) -> Result<(), PortError> {
        *self.0.lock().unwrap() = minimum;
        Ok(())
    }
}

/// Something listens on 8080.
struct FakeListening;

#[async_trait]
impl ListeningPorts for FakeListening {
    async fn is_listening(&self, port: u16) -> Result<bool, PortError> {
        Ok(port == 8080)
    }
}

/// A clock in Etc/UTC kept by Debian's pool; remembers what was set.
#[derive(Default)]
struct FakeTime(Mutex<Option<TimeSettings>>);

#[async_trait]
impl TimeControl for FakeTime {
    async fn status(&self, _: &RequestContext) -> Result<TimeStatus, PortError> {
        let set = self.0.lock().unwrap().clone();
        Ok(TimeStatus {
            zone: set.as_ref().map_or("Etc/UTC".into(), |s| s.zone.clone()),
            network_time: set.as_ref().is_none_or(|s| s.network_time),
            can_network_time: true,
            synchronized: true,
            servers: set.as_ref().map(|s| s.servers.clone()).unwrap_or_default(),
            server_in_use: Some("2.debian.pool.ntp.org".into()),
            zones: vec!["Etc/UTC".into(), "Europe/Berlin".into()],
            now: zendiloos_kernel::Timestamp::from_unix_seconds(1_800_000_000),
        })
    }

    async fn apply(&self, settings: &TimeSettings, _: &RequestContext) -> Result<(), PortError> {
        *self.0.lock().unwrap() = Some(settings.clone());
        Ok(())
    }
}

fn sample(at: i64) -> MetricsSample {
    MetricsSample {
        at: zendiloos_kernel::Timestamp::from_unix_seconds(at),
        cpu_percent: 12.5,
        load_average: [0.5, 0.4, 0.3],
        memory: MemoryUsage { total_bytes: 8, used_bytes: 3, available_bytes: 5 },
        network: vec![],
        disks: vec![],
        temperatures: vec![],
        uptime_seconds: 60,
    }
}

/// Records queries and answers with one entry and a cursor to older ones.
#[derive(Default)]
pub(crate) struct FakeJournal(Mutex<Vec<JournalQuery>>);

#[async_trait]
impl JournalReader for FakeJournal {
    async fn read(&self, query: &JournalQuery) -> Result<JournalPage, PortError> {
        self.0.lock().unwrap().push(query.clone());
        let cursor = JournalCursor::parse("s=1;i=2").unwrap();
        let entry = JournalEntry {
            at_micros: 1_793_880_897_756_376,
            priority: LogPriority::Error,
            unit: Some("smbd.service".into()),
            identifier: Some("smbd".into()),
            pid: Some(42),
            message: "smbd failed".into(),
            cursor: cursor.clone(),
        };
        Ok(JournalPage { entries: vec![entry], older: Some(cursor) })
    }
}

/// The identity fakes' audit entries, newest first.
struct FakeAuditReader(Arc<zendiloos_identity::testing::MemoryAudit>);

#[async_trait]
impl AuditReader for FakeAuditReader {
    async fn read(&self, query: &AuditQuery) -> Result<Vec<AuditRecord>, PortError> {
        let entries = self.0.entries();
        Ok(entries
            .into_iter()
            .enumerate()
            .rev()
            .map(|(index, entry)| AuditRecord {
                id: i64::try_from(index).unwrap() + 1,
                at: entry.at,
                actor: entry.actor,
                client_ip: entry.client_ip.map(|ip| ip.to_string()),
                action: entry.action.to_owned(),
                target: entry.target,
                outcome: entry.outcome.as_str().to_owned(),
                reason: entry.reason.map(str::to_owned),
                request_id: entry.request_id,
            })
            .filter(|record| query.before.is_none_or(|before| record.id < before))
            .take(usize::from(query.limit))
            .collect())
    }
}

/// A latest sample at 100, then one new sample at 101.
struct FakeLive;

impl LiveMetrics for FakeLive {
    fn latest(&self) -> Option<MetricsSample> {
        Some(sample(100))
    }

    fn subscribe(&self) -> Box<dyn MetricsSubscription> {
        Box::new(FakeSubscription(Some(sample(101))))
    }

    fn recent(&self) -> Vec<MetricsSample> {
        vec![sample(98), sample(100)]
    }
}

struct FakeSubscription(Option<MetricsSample>);

#[async_trait]
impl MetricsSubscription for FakeSubscription {
    async fn next(&mut self) -> Option<MetricsSample> {
        match self.0.take() {
            Some(sample) => Some(sample),
            None => std::future::pending().await,
        }
    }
}

/// The API on in-memory ports.
pub(crate) struct Harness {
    pub(crate) app: Router,
    pub(crate) identity: TestIdentity,
    pub(crate) power: Arc<RecordingPower>,
    pub(crate) journal: Arc<FakeJournal>,
    pub(crate) jobs: Jobs,
    pub(crate) storage: Arc<FakeStorage>,
    pub(crate) notifier: Notifier,
    pub(crate) share_host: Arc<zendiloos_sharing::testing::Host>,
    pub(crate) file_host: Arc<zendiloos_sharing::testing::Files>,
    pub(crate) catalogs: Arc<zendiloos_apps::testing::FakeReader>,
    pub(crate) app_host: Arc<zendiloos_apps::testing::FakeAppHost>,
    /// Apps installed the earlier way.
    pub(crate) installed: Arc<zendiloos_apps::testing::MemoryInstalled>,
    pub(crate) engine: Arc<zendiloos_containers::testing::FakeEngine>,
    pub(crate) containers: zendiloos_containers::application::Containers,
    /// The jobs of the Docker app, kept apart from the others here.
    pub(crate) docker_jobs: Arc<zendiloos_jobs::testing::MemoryJobs>,
}

/// The App Center's projects, from the Docker fakes, as zendiloosd's
/// adapter has them.
struct TemplateProjects(zendiloos_containers::application::Containers);

#[async_trait]
impl zendiloos_apps::application::ports::AppProjects for TemplateProjects {
    async fn projects(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<zendiloos_apps::application::ports::TemplateProject>, PortError> {
        let projects = self.0.projects(context).await.map_err(|e| PortError::new("projects", e))?;
        Ok(projects
            .into_iter()
            .filter_map(|project| {
                let zendiloos_containers::domain::Origin::Own(own) = &project.origin else { return None };
                let template = own.template.clone()?;
                let running = u32::try_from(project.running()).unwrap_or(u32::MAX);
                let all = u32::try_from(project.containers.len()).unwrap_or(u32::MAX);
                let published = project
                    .containers
                    .iter()
                    .flat_map(|c| c.ports.iter().map(|p| (p.container_port, p.host_port)))
                    .collect();
                Some(zendiloos_apps::application::ports::TemplateProject {
                    name: project.name.clone(),
                    source: template.source,
                    app: template.app,
                    containers: zendiloos_apps::domain::Containers {
                        running,
                        restarting: 0,
                        other: all - running,
                    },
                    published,
                    created_at: own.created_at,
                })
            })
            .collect())
    }
}

/// Removable drives for the file manager from the Storage use cases, as
/// zendiloosd's adapter has them.
struct DrivesForFiles(Arc<zendiloos_storage::application::removable::RemovableDrives>);

#[async_trait]
impl zendiloos_sharing::application::ports::RemovableRoots for DrivesForFiles {
    async fn mounted(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Option<zendiloos_sharing::application::ports::RemovableRoot>, PortError> {
        let drive = self.0.mounted(id, context).await.map_err(|e| PortError::new("storage", e))?;
        Ok(drive.map(|drive| zendiloos_sharing::application::ports::RemovableRoot {
            id: drive.id,
            label: drive.label,
            read_only: drive.read_only,
        }))
    }
}

/// Account and group names from the identity fakes.
struct IdentityDirectory(
    Arc<zendiloos_identity::testing::InMemoryAccounts>,
    Arc<zendiloos_identity::testing::InMemoryGroups>,
);

#[async_trait]
impl zendiloos_sharing::application::ports::Directory for IdentityDirectory {
    async fn names(&self) -> Result<zendiloos_sharing::application::ports::Names, PortError> {
        Ok(zendiloos_sharing::application::ports::Names {
            accounts: self.0.all().into_iter().map(|a| (a.id.get(), a.name.to_string())).collect(),
            groups: self.1.all().into_iter().map(|g| (g.id.get(), g.name.to_string())).collect(),
            members: self
                .1
                .all()
                .into_iter()
                .map(|g| (g.id.get(), g.members.iter().map(|m| m.get()).collect()))
                .collect(),
        })
    }

    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        let groups = self.1.all().into_iter();
        Ok(groups.filter(|g| g.members.iter().any(|m| m.get() == account)).map(|g| g.id.get()).collect())
    }
}

pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn header(&self, name: impl header::AsHeaderName) -> &str {
        self.headers.get(name).expect("header present").to_str().unwrap()
    }
}

/// A signed-in browser.
pub(crate) struct Session {
    /// `name=value`, as sent back in the `Cookie` header.
    pub(crate) cookie: String,
    pub(crate) csrf: String,
}

impl Session {
    fn from(reply: &Reply) -> Self {
        let set_cookie = reply.header(header::SET_COOKIE);
        Self {
            cookie: set_cookie.split(';').next().unwrap().to_owned(),
            csrf: reply.body["csrf_token"].as_str().unwrap().to_owned(),
        }
    }
}

impl Harness {
    pub(crate) fn new() -> Self {
        Self::with(FakeProvider, true)
    }

    fn with(provider: impl SystemInfoProvider + 'static, secure_cookies: bool) -> Self {
        let identity = TestIdentity::new();
        let power = Arc::new(RecordingPower::default());
        let journal = Arc::new(FakeJournal::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), identity.clock.clone());
        let system = SystemUseCases::new(SystemPorts {
            info: Arc::new(provider),
            metrics: Arc::new(FakeLive),
            power: power.clone(),
            work: Arc::new(jobs.clone()),
            time: Arc::new(FakeTime::default()),
            ssh: Arc::new(FakeSsh::default()),
            listening: Arc::new(FakeListening),
            tls: Arc::new(FakeTls::default()),
            journal: journal.clone(),
            audit: identity.audit.clone(),
            audit_reader: Arc::new(FakeAuditReader(identity.audit.clone())),
            clock: identity.clock.clone(),
        });
        let (ports, storage_fake) =
            FakeStorage::ports(jobs.clone(), identity.audit.clone(), identity.clock.clone());
        let storage = StorageUseCases::new(ports);
        let notifier = Notifier::new(Arc::new(MemoryNotifications::default()), identity.clock.clone());
        let share_host = Arc::new(zendiloos_sharing::testing::Host::default());
        let file_host = Arc::new(zendiloos_sharing::testing::Files::default());
        let sharing_ports = SharingPorts {
            shares: Arc::new(zendiloos_sharing::testing::Shares::default()),
            directory: Arc::new(IdentityDirectory(identity.accounts.clone(), identity.groups.clone())),
            volumes: Arc::new(zendiloos_sharing::testing::Volumes),
            projects: Arc::new(zendiloos_sharing::testing::Projects::default()),
            host: share_host.clone(),
            files: file_host.clone(),
            removable: Arc::new(DrivesForFiles(Arc::clone(&storage.removable))),
            jobs: jobs.clone(),
            audit: identity.audit.clone(),
            clock: identity.clock.clone(),
            notifier: notifier.clone(),
        };
        let sharing = SharingUseCases::new(sharing_ports.clone());
        let files = FilesUseCases::new(sharing_ports);
        let catalogs = Arc::new(zendiloos_apps::testing::FakeReader::default());
        catalogs.serve(
            zendiloos_apps::domain::OFFICIAL_INDEX,
            vec![
                zendiloos_apps::testing::app("jellyfin", "Jellyfin"),
                zendiloos_apps::testing::app("immich", "Immich"),
            ],
        );
        let docker_host = Arc::new(zendiloos_apps::testing::FakeDocker::default());
        let catalog_use_cases = zendiloos_apps::application::Catalogs::new(
            Arc::new(zendiloos_apps::testing::MemorySources::default()),
            catalogs.clone(),
            identity.audit.clone(),
            identity.clock.clone(),
        );
        let app_host = Arc::new(zendiloos_apps::testing::FakeAppHost::default());
        let installed = Arc::new(zendiloos_apps::testing::MemoryInstalled::default());
        let installs =
            zendiloos_apps::application::Installs::new(zendiloos_apps::application::InstallPorts {
                installed: installed.clone(),
                host: app_host.clone(),
                shares: Arc::new(zendiloos_apps::testing::FakeShares::default()),
                jobs: jobs.clone(),
                audit: identity.audit.clone(),
                clock: identity.clock.clone(),
            });
        let docker_test = zendiloos_containers::testing::TestContainers::new(
            identity.audit.clone(),
            identity.clock.clone(),
        );
        let templates =
            zendiloos_apps::application::Templates::new(zendiloos_apps::application::TemplatePorts {
                catalogs: catalog_use_cases.clone(),
                installed: installed.clone(),
                shares: Arc::new(zendiloos_apps::testing::FakeUsableShares),
                listening: Arc::new(zendiloos_apps::testing::FakeListening),
                secrets: Arc::new(zendiloos_apps::testing::CountingSecrets::default()),
                host: Arc::new(zendiloos_apps::testing::FakeTemplateHost),
                filler: Arc::new(zendiloos_apps::testing::LineFiller),
                projects: Arc::new(TemplateProjects(docker_test.containers.clone())),
            });
        let engine = docker_test.engine.clone();
        let docker_jobs = docker_test.jobs.clone();
        let update_releases = Arc::new(zendiloos_updates::testing::FakeReleases::default());
        *update_releases.releases.lock().unwrap() =
            vec![zendiloos_updates::testing::release("v0.2.0", "0.2.0", false)];
        let containers = docker_test.containers.clone();
        let use_cases = UseCases {
            containers: docker_test.containers,
            system,
            identity: identity.use_cases.clone(),
            storage,
            sharing,
            files,
            network: NetworkUseCases::new(
                Arc::new(FixedNetwork),
                Arc::new(zendiloos_network::testing::FakeConfigurator::default()),
                identity.audit.clone(),
                identity.clock.clone(),
            ),
            jobs: jobs.clone(),
            notifier: notifier.clone(),
            docker: zendiloos_apps::application::Docker::new(
                docker_host,
                identity.audit.clone(),
                identity.clock.clone(),
                jobs.clone(),
            ),
            apps: catalog_use_cases,
            installs,
            templates,
            channels: zendiloos_notify::channels::Channels::new(
                Arc::new(zendiloos_notify::testing::MemoryChannels::default()),
                Arc::new(zendiloos_notify::testing::RecordingSender::default()),
                identity.audit.clone(),
                identity.clock.clone(),
                "nas".into(),
            ),
            updates: zendiloos_updates::application::Updates::new(
                zendiloos_updates::application::UpdatesPorts {
                    releases: update_releases.clone(),
                    host: Arc::new(zendiloos_updates::testing::FakeHost::default()),
                    store: Arc::new(zendiloos_updates::testing::MemoryStore::default()),
                    notifier: notifier.clone(),
                    work: Arc::new(jobs.clone()),
                    audit: identity.audit.clone(),
                    clock: identity.clock.clone(),
                },
                "0.1.0",
            ),
        };
        let state = ApiState::new(use_cases, secure_cookies);
        Self {
            app: router(state),
            identity,
            power,
            journal,
            jobs,
            storage: storage_fake,
            notifier,
            share_host,
            file_host,
            catalogs,
            engine,
            app_host,
            installed,
            containers,
            docker_jobs,
        }
    }

    pub(crate) async fn send(&self, request: Request<Body>) -> Reply {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        // Problems and data are JSON; a downloaded file is kept as text.
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        Reply { status, headers, body }
    }

    pub(crate) async fn sign_in(&self, name: &str, role: Role) -> Session {
        self.identity.add_account(name, PASSWORD, role);
        let reply =
            self.send(json(post("/api/v1/auth/login"), json!({ "name": name, "password": PASSWORD }))).await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
        Session::from(&reply)
    }
}

/// A same-origin browser request.
pub(crate) fn request(method: Method, uri: &str) -> request::Builder {
    Request::builder().method(method).uri(uri).header(header::HOST, HOST)
}

pub(crate) fn get(uri: &str) -> request::Builder {
    request(Method::GET, uri)
}

pub(crate) fn post(uri: &str) -> request::Builder {
    request(Method::POST, uri).header(header::ORIGIN, format!("https://{HOST}"))
}

/// A state-changing same-origin request.
pub(crate) fn changing(method: Method, uri: &str) -> request::Builder {
    request(method, uri).header(header::ORIGIN, format!("https://{HOST}"))
}

pub(crate) fn json(builder: request::Builder, body: Value) -> Request<Body> {
    builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
}

pub(crate) fn empty(builder: request::Builder) -> Request<Body> {
    builder.body(Body::empty()).unwrap()
}

pub(crate) fn with_cookie(builder: request::Builder, session: &Session) -> request::Builder {
    builder.header(header::COOKIE, &session.cookie)
}

pub(crate) fn with_csrf(builder: request::Builder, session: &Session) -> request::Builder {
    with_cookie(builder, session).header(CSRF_HEADER, &session.csrf)
}

pub(crate) fn assert_problem(reply: &Reply, status: StatusCode, code: &str) {
    assert_eq!(reply.status, status, "{:?}", reply.body);
    assert_eq!(reply.header(header::CONTENT_TYPE), "application/problem+json");
    assert_eq!(reply.body["code"], code, "{:?}", reply.body);
}

// Basics

#[tokio::test]
async fn health_is_public_and_reveals_nothing_else() {
    let reply = Harness::new().send(empty(get("/api/v1/health"))).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn unknown_paths_and_methods_are_problems() {
    let h = Harness::new();
    let reply = h.send(empty(get("/api/v1/nope"))).await;
    assert_problem(&reply, StatusCode::NOT_FOUND, "not_found");
    assert_eq!(
        reply.body,
        json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
    );
    let reply = h.send(empty(post("/api/v1/health"))).await;
    assert_problem(&reply, StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
}

#[tokio::test]
async fn reads_the_page_made_on_its_own_keep_no_session_alive() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;
    let background =
        || empty(with_cookie(get("/api/v1/auth/session"), &session).header("x-zendiloos-background", "1"));

    // Polled for longer than the idle timeout: the session ends.
    for _ in 0..4 {
        h.identity.clock.advance(std::time::Duration::from_secs(10 * 60));
        let reply = h.send(background()).await;
        if reply.status == StatusCode::UNAUTHORIZED {
            return;
        }
    }
    panic!("an open window alone kept the session alive");
}

#[tokio::test]
async fn what_the_person_does_keeps_the_session_alive() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;
    for _ in 0..4 {
        h.identity.clock.advance(std::time::Duration::from_secs(10 * 60));
        let reply = h.send(empty(with_cookie(get("/api/v1/auth/session"), &session))).await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    }
}

#[tokio::test]
async fn a_path_parameter_that_is_no_number_is_a_problem() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;
    let reply = h.send(empty(with_cookie(get("/api/v1/jobs/abc"), &session))).await;
    assert_problem(&reply, StatusCode::BAD_REQUEST, "bad_request");
    assert_eq!(reply.headers[header::CONTENT_TYPE], "application/problem+json");
}

#[tokio::test]
async fn responses_carry_a_request_id() {
    let reply = Harness::new().send(empty(get("/api/v1/health"))).await;
    assert_eq!(reply.header("x-request-id").len(), 36, "a UUID");
}

#[tokio::test]
async fn serves_its_openapi_document() {
    let reply = Harness::new().send(empty(get("/api/v1/openapi.json"))).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["info"]["title"], "Zendiloos API");
    assert_eq!(reply.body["components"]["securitySchemes"]["session"]["in"], "cookie");
}

// System information

#[tokio::test]
async fn system_info_needs_a_session() {
    let reply = Harness::new().send(empty(get("/api/v1/system/info"))).await;
    assert_problem(&reply, StatusCode::UNAUTHORIZED, "unauthenticated");
}

#[tokio::test]
async fn system_info_returns_the_host_facts() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;

    let reply = h.send(empty(with_cookie(get("/api/v1/system/info"), &session))).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body,
        json!({
            "hostname": "nas",
            "os_name": "Debian GNU/Linux 13 (trixie)",
            "kernel_release": "6.12.48+deb13-amd64",
            "architecture": "x86_64",
            "cpu_model": "Intel(R) N100",
            "cpu_threads": 4,
            "cpu_cores": 4,
            "memory_total_bytes": 8_u64 * 1024 * 1024 * 1024,
            "swap_total_bytes": 1024 * 1024 * 1024,
            "uptime_seconds": 3600,
            "version": env!("CARGO_PKG_VERSION"),
            "vendor": "ASUSTeK COMPUTER INC.",
            "product": "PN42",
            "board": null,
            "firmware": "American Megatrends 0304 (05/12/2024)",
            "uefi": true,
        })
    );
}

#[tokio::test]
async fn adapter_failures_are_generic_500s() {
    let h = Harness::with(FailingProvider, true);
    let session = h.sign_in("alice", Role::Admin).await;

    let reply = h.send(empty(with_cookie(get("/api/v1/system/info"), &session))).await;

    assert_problem(&reply, StatusCode::INTERNAL_SERVER_ERROR, "internal");
    assert!(!reply.body.to_string().contains("secret"), "internal details leaked: {:?}", reply.body);
}

#[tokio::test]
async fn admins_can_reboot_and_it_is_audited() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;

    let reply =
        h.send(json(with_csrf(post("/api/v1/system/power"), &session), json!({ "action": "reboot" }))).await;

    assert_eq!(reply.status, StatusCode::ACCEPTED, "{:?}", reply.body);
    assert_eq!(*h.power.0.lock().unwrap(), [PowerAction::Reboot]);
    let audit = h.identity.audit.entries();
    let entry = audit.last().unwrap();
    assert_eq!(
        (entry.action, entry.outcome, entry.actor.as_deref()),
        ("system.reboot", Outcome::Success, Some("alice"))
    );
}

#[tokio::test]
async fn only_admins_can_power_off() {
    let h = Harness::new();
    let session = h.sign_in("bob", Role::ReadOnly).await;

    let reply = h
        .send(json(with_csrf(post("/api/v1/system/power"), &session), json!({ "action": "power_off" })))
        .await;

    assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    assert!(h.power.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn power_actions_need_the_csrf_token() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;

    let reply = h
        .send(json(with_cookie(post("/api/v1/system/power"), &session), json!({ "action": "reboot" })))
        .await;

    assert_problem(&reply, StatusCode::FORBIDDEN, "csrf_rejected");
    assert!(h.power.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_reboot_nor_update_while_a_job_runs() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::Admin).await;
    let check = h.send(empty(with_csrf(post("/api/v1/system/updates/check"), &session))).await;
    assert_eq!(check.status, StatusCode::ACCEPTED);
    for _ in 0..100 {
        let found = h.send(empty(with_cookie(get("/api/v1/system/updates"), &session))).await;
        if found.body["phase"] == "idle" && found.body["checked_at"] != Value::Null {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let job = h
        .jobs
        .start("sharing.apply_access", Some("photos".into()), None, |_| async move {
            let _ = finished.await;
            Ok(())
        })
        .await
        .unwrap();

    let reboot =
        h.send(json(with_csrf(post("/api/v1/system/power"), &session), json!({ "action": "reboot" }))).await;
    assert_problem(&reboot, StatusCode::CONFLICT, "jobs_running");
    assert_eq!(reboot.body["detail"], "sharing.apply_access photos");
    assert!(h.power.0.lock().unwrap().is_empty());
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!((entry.action, entry.outcome), ("system.reboot", Outcome::Denied));
    let update = h.send(empty(with_csrf(post("/api/v1/system/updates/apply"), &session))).await;
    assert_problem(&update, StatusCode::CONFLICT, "jobs_running");
    assert_eq!(update.body["detail"], "sharing.apply_access photos");

    finish.send(()).unwrap();
    job_done(&h, job.id.get()).await;
    let reboot =
        h.send(json(with_csrf(post("/api/v1/system/power"), &session), json!({ "action": "reboot" }))).await;
    assert_eq!(reboot.status, StatusCode::ACCEPTED, "{:?}", reboot.body);
}

#[tokio::test]
async fn docker_is_for_those_who_manage_containers() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;

    assert_problem(
        &h.send(empty(with_cookie(get("/api/v1/docker/overview"), &viewer))).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let overview = h.send(empty(with_cookie(get("/api/v1/docker/overview"), &admin))).await;
    assert_eq!(overview.status, StatusCode::OK, "{:?}", overview.body);
    assert_eq!(
        (overview.body["containers_running"].clone(), overview.body["containers"].clone()),
        (json!(2), json!(4))
    );

    let started = h.send(empty(with_csrf(post("/api/v1/docker/containers/lonely-id/start"), &admin))).await;
    assert_eq!(started.status, StatusCode::NO_CONTENT);
    let running = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/containers/stack-web-id"), &admin)))
        .await;
    assert_problem(&running, StatusCode::BAD_GATEWAY, "host_failed");
    let images = h.send(empty(with_cookie(get("/api/v1/docker/images"), &admin))).await;
    let nginx = images.body.as_array().unwrap().iter().find(|i| i["id"] == "sha256:nginx").unwrap();
    assert_eq!(nginx["used_by"], json!(["stack-web"]));
    let bridge =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/networks/bridge"), &admin))).await;
    assert_problem(&bridge, StatusCode::CONFLICT, "refused");
    let created = h
        .send(json(
            with_csrf(post("/api/v1/docker/networks"), &admin),
            json!({ "name": "proxy_net", "subnet": "172.30.0.0/24", "gateway": "" }),
        ))
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert!(h.engine.calls().contains(&"network create proxy_net Some(\"172.30.0.0/24\") None".to_owned()));
}

#[tokio::test]
async fn newer_images_are_looked_for_and_taken_on_a_schedule() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;

    assert_eq!(h.send(empty(with_cookie(get("/api/v1/docker/updates"), &admin))).await.body, Value::Null);
    let checked = h.send(empty(with_csrf(post("/api/v1/docker/updates/check"), &admin))).await;
    let nginx = checked.body["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["reference"] == "nginx")
        .cloned()
        .unwrap();
    assert_eq!(
        (nginx["state"].clone(), nginx["containers"].clone()),
        (json!("available"), json!(["stack-web"]))
    );

    let put = |body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/docker/updates/settings"), &admin), body))
    };
    let weekly = json!({ "check": true, "schedule": { "days": [0, 6], "time": "05:40" } });
    assert_eq!(put(weekly.clone()).await.body, weekly);
    assert_problem(
        &put(json!({ "check": true, "schedule": { "days": [7], "time": "05:40" } })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    assert_problem(
        &put(json!({ "check": true, "schedule": { "days": [1], "time": "25:00" } })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    assert_problem(
        &put(json!({ "check": false, "schedule": { "days": [1], "time": "05:00" } })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );

    let applied = h.send(empty(with_csrf(post("/api/v1/docker/updates/apply"), &admin))).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED);
    assert_eq!(applied.body["job"]["kind"], "containers.update");
}

#[tokio::test]
async fn dockers_data_moves_to_another_pool() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;

    let location = h.send(empty(with_cookie(get("/api/v1/docker/data"), &admin))).await;
    assert_eq!(location.body["current"]["pool"], "tank");
    let targets: Vec<_> =
        location.body["targets"].as_array().unwrap().iter().map(|t| t["pool"].clone()).collect();
    assert_eq!(targets, [json!("backup"), json!("tiny")]);
    let tiny =
        h.send(json(with_csrf(post("/api/v1/docker/data/move"), &admin), json!({ "pool": "tiny" }))).await;
    assert_problem(&tiny, StatusCode::CONFLICT, "refused");
    let moved =
        h.send(json(with_csrf(post("/api/v1/docker/data/move"), &admin), json!({ "pool": "backup" }))).await;
    assert_eq!(
        (moved.status, moved.body["kind"].clone()),
        (StatusCode::ACCEPTED, json!("containers.data_move"))
    );
}

#[tokio::test]
async fn a_project_is_checked_shown_and_made_with_consent() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let compose = "services:\n  app:\n    image: nginx\n    privileged: true\n";

    let places = h.send(empty(with_cookie(get("/api/v1/docker/places"), &admin))).await;
    assert_eq!(places.body, json!([{ "share_id": 1, "share": "docker" }]));
    let check = h
        .send(json(
            with_csrf(post("/api/v1/docker/compose/check"), &admin),
            json!({ "name": "tesla", "share_id": 1, "folder": "docker/tesla", "compose": compose }),
        ))
        .await;
    assert_eq!(check.body, json!({ "risks": [{ "service": "app", "kind": "privileged", "detail": null }] }));

    let new = |accept: bool| json!({ "name": "tesla", "share_id": 1, "folder": "docker/tesla", "compose": compose, "start": true, "accept_risks": accept });
    let refused = h.send(json(with_csrf(post("/api/v1/docker/projects"), &admin), new(false))).await;
    assert_problem(&refused, StatusCode::CONFLICT, "compose_risks");
    let made = h.send(json(with_csrf(post("/api/v1/docker/projects"), &admin), new(true))).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["job"]["kind"], "containers.project_deploy");

    let projects = h.send(empty(with_cookie(get("/api/v1/docker/projects"), &admin))).await;
    let kinds: Vec<_> =
        projects.body.as_array().unwrap().iter().map(|p| (p["name"].clone(), p["kind"].clone())).collect();
    assert_eq!(
        kinds,
        [(json!("kuma"), json!("app")), (json!("stack"), json!("foreign")), (json!("tesla"), json!("own"))]
    );
    let app = h.send(empty(with_cookie(get("/api/v1/docker/projects/kuma/compose"), &admin))).await;
    assert_problem(&app, StatusCode::CONFLICT, "app_project");
    let foreign = h.send(empty(with_cookie(get("/api/v1/docker/projects/stack/compose"), &admin))).await;
    assert_eq!(
        (foreign.body["editable"].clone(), foreign.body["files"][0]["path"].clone()),
        (json!(false), json!("/opt/stack/compose.yml"))
    );
    let own = h.send(empty(with_cookie(get("/api/v1/docker/projects/tesla/compose"), &admin))).await;
    assert_eq!(
        (own.body["editable"].clone(), own.body["files"][0]["content"].clone()),
        (json!(true), json!(compose))
    );
    let taken = h.send(json(with_csrf(post("/api/v1/docker/projects"), &admin), new(true))).await;
    assert_problem(&taken, StatusCode::CONFLICT, "project_exists");
}

fn from_address(builder: request::Builder, address: &str) -> request::Builder {
    builder.header("x-real-ip", address)
}

#[tokio::test]
async fn admins_set_the_defences_of_the_web_interface() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/security/settings"), session), body))
    };

    let state = h.send(empty(with_cookie(get("/api/v1/security/settings"), &admin))).await;
    assert_eq!(state.status, StatusCode::OK);
    assert_eq!(state.body["idle_minutes"], 30);
    assert_eq!(
        state.body["auto_block"],
        json!({ "failures": 10, "window_minutes": 5, "release_after_days": 1 })
    );
    assert_problem(
        &h.send(empty(with_cookie(get("/api/v1/security/settings"), &viewer))).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );

    let settings = json!({
        "idle_minutes": 60,
        "account_lock": null,
        "auto_block": { "failures": 5, "window_minutes": 10, "release_after_days": null },
        "allowed": [" 192.168.1.7/24 "]
    });
    let reply = put(&admin, settings).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(
        (reply.body["idle_minutes"].clone(), reply.body["allowed"].clone()),
        (json!(60), json!(["192.168.1.0/24"]))
    );
    assert_eq!(reply.body["account_lock"], Value::Null);
    for bad in [
        json!({ "idle_minutes": 2, "account_lock": null, "auto_block": null }),
        json!({ "idle_minutes": 30, "account_lock": null, "auto_block": null, "allowed": ["nas.local"] }),
        json!({ "idle_minutes": 30, "account_lock": { "failures": 1, "window_minutes": 5 }, "auto_block": null }),
    ] {
        assert_problem(&put(&admin, bad).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!(entry.action, "identity.security_update");
}

#[tokio::test]
async fn blocked_addresses_are_refused_everywhere_but_the_health_check() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let block = |address: &str| {
        h.send(json(with_csrf(post("/api/v1/security/blocked"), &admin), json!({ "address": address })))
    };

    let reply = block("203.0.113.9/24").await;
    assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply.body);
    assert_eq!(
        (reply.body["address"].clone(), reply.body["automatic"].clone()),
        (json!("203.0.113.0/24"), json!(false))
    );
    assert_problem(&block("127.0.0.1").await, StatusCode::CONFLICT, "address_allowed");
    assert_problem(&block("10.0.0.300").await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let refused = h.send(empty(from_address(get("/api/v1/setup"), "203.0.113.5"))).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "address_blocked");
    let health = h.send(empty(from_address(get("/api/v1/health"), "203.0.113.5"))).await;
    assert_eq!(health.status, StatusCode::OK);
    let own = h
        .send(json(
            from_address(with_csrf(post("/api/v1/security/blocked"), &admin), "198.51.100.4"),
            json!({ "address": "198.51.100.0/24" }),
        ))
        .await;
    assert_problem(&own, StatusCode::CONFLICT, "own_address");
    let listed = h.send(empty(with_cookie(get("/api/v1/security/blocked"), &admin))).await;
    assert_eq!(listed.body.as_array().map(Vec::len), Some(1));

    let unblock = |address: &str| {
        h.send(json(with_csrf(post("/api/v1/security/unblock"), &admin), json!({ "address": address })))
    };
    assert_eq!(unblock("203.0.113.0/24").await.status, StatusCode::NO_CONTENT);
    assert_problem(&unblock("203.0.113.0/24").await, StatusCode::NOT_FOUND, "not_found");
    let open = h.send(empty(from_address(get("/api/v1/setup"), "203.0.113.5"))).await;
    assert_eq!(open.status, StatusCode::OK);
}

#[tokio::test]
async fn an_address_guessing_passwords_is_blocked() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    for _ in 0..10 {
        let guess = json(
            from_address(post("/api/v1/auth/login"), "198.51.100.9"),
            json!({ "name": "root", "password": "123456" }),
        );
        h.send(guess).await;
    }

    let next = h
        .send(json(
            from_address(post("/api/v1/auth/login"), "198.51.100.9"),
            json!({ "name": "alice", "password": PASSWORD }),
        ))
        .await;

    assert_problem(&next, StatusCode::FORBIDDEN, "address_blocked");
    let listed = h.send(empty(with_cookie(get("/api/v1/security/blocked"), &admin))).await;
    assert_eq!(listed.body[0]["address"], "198.51.100.9");
    assert_eq!(listed.body[0]["automatic"], true);
    assert!(listed.body[0]["until"].is_i64(), "lifted after a day");
}

#[tokio::test]
async fn admins_see_every_session_and_end_one() {
    let h = Harness::new();
    let bob = h.sign_in("bob", Role::User).await;
    let admin = h.sign_in("alice", Role::Admin).await;

    let sessions = h.send(empty(with_cookie(get("/api/v1/security/sessions"), &admin))).await;

    assert_eq!(sessions.status, StatusCode::OK);
    let list = sessions.body.as_array().unwrap();
    assert_eq!(list.len(), 2);
    let current: Vec<_> = list.iter().map(|s| (s["account"].clone(), s["current"].clone())).collect();
    assert!(
        current.contains(&(json!("alice"), json!(true))) && current.contains(&(json!("bob"), json!(false)))
    );
    let id = list.iter().find(|s| s["account"] == "bob").unwrap()["id"].as_i64().unwrap();
    let ended = h
        .send(empty(with_csrf(changing(Method::DELETE, &format!("/api/v1/security/sessions/{id}")), &admin)))
        .await;
    assert_eq!(ended.status, StatusCode::NO_CONTENT);
    let signed_out = h.send(empty(with_cookie(get("/api/v1/system/info"), &bob))).await;
    assert_problem(&signed_out, StatusCode::UNAUTHORIZED, "unauthenticated");
}

#[tokio::test]
async fn admins_choose_the_oldest_tls_version() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/system/tls"), session), body))
    };

    let state = h.send(empty(with_cookie(get("/api/v1/system/tls"), &admin))).await;
    assert_eq!(state.body, json!({ "minimum": "tls1_2" }));
    assert_problem(&put(&viewer, json!({ "minimum": "tls1_3" })).await, StatusCode::FORBIDDEN, "forbidden");
    let reply = put(&admin, json!({ "minimum": "tls1_3" })).await;
    assert_eq!(reply.body, json!({ "minimum": "tls1_3" }));
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!((entry.action, entry.target.as_deref()), ("system.tls_update", Some("TLS 1.3")));
    assert_eq!(put(&admin, json!({ "minimum": "tls1_0" })).await.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn admins_turn_ssh_on_and_off() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/system/ssh"), session), body))
    };

    let state = h.send(empty(with_cookie(get("/api/v1/system/ssh"), &viewer))).await;
    assert_eq!(state.status, StatusCode::OK);
    assert_eq!((state.body["running"].clone(), state.body["port"].clone()), (json!(false), json!(22)));

    let on = json!({ "enabled": true, "port": 2222, "root_login": "key_only", "off_after_minutes": 30 });
    assert_problem(&put(&viewer, on.clone()).await, StatusCode::FORBIDDEN, "forbidden");
    let reply = put(&admin, on).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(
        (reply.body["running"].clone(), reply.body["root_login"].clone(), reply.body["off_at"].clone()),
        (json!(true), json!("key_only"), json!(1_800_001_800))
    );
    assert_eq!(reply.body["password_login"], Value::Null, "as the system has it");
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!((entry.action, entry.target.as_deref()), ("system.ssh_update", Some("on, port 2222")));

    for bad in [
        json!({ "enabled": true, "port": 23 }),
        json!({ "enabled": true, "port": 2049 }),
        json!({ "enabled": true, "port": 22, "off_after_minutes": 0 }),
    ] {
        assert_problem(&put(&admin, bad).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let taken = put(&admin, json!({ "enabled": true, "port": 8080 })).await;
    assert_problem(&taken, StatusCode::CONFLICT, "port_in_use");
    let unknown = put(&admin, json!({ "enabled": true, "port": 22, "banner": "hello" })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn admins_set_the_time_zone_and_servers() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/system/time"), session), body))
    };

    let state = h.send(empty(with_cookie(get("/api/v1/system/time"), &viewer))).await;
    assert_eq!(state.status, StatusCode::OK);
    assert_eq!(state.body["zone"], "Etc/UTC");
    assert_eq!(state.body["zones"], json!(["Etc/UTC", "Europe/Berlin"]));
    assert_eq!(state.body["now"], 1_800_000_000);

    let berlin = json!({ "zone": "Europe/Berlin", "network_time": true, "servers": [" PTB.de ", ""] });
    assert_problem(&put(&viewer, berlin.clone()).await, StatusCode::FORBIDDEN, "forbidden");
    let reply = put(&admin, berlin).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(
        (reply.body["zone"].clone(), reply.body["servers"].clone()),
        (json!("Europe/Berlin"), json!(["ptb.de"]))
    );
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!((entry.action, entry.target.as_deref()), ("system.time_update", Some("Europe/Berlin")));

    for bad in [
        json!({ "zone": "Mars/Olympus", "network_time": true }),
        json!({ "zone": "../etc/passwd", "network_time": true }),
        json!({ "zone": "Etc/UTC", "network_time": true, "servers": ["a b"] }),
        json!({ "zone": "Etc/UTC", "network_time": true, "clock": 1_800_000_000 }),
        json!({ "zone": "Etc/UTC", "network_time": false, "clock": 0 }),
    ] {
        assert_problem(&put(&admin, bad.clone()).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let by_hand =
        put(&admin, json!({ "zone": "Etc/UTC", "network_time": false, "clock": 1_800_000_000 })).await;
    assert_eq!(by_hand.status, StatusCode::OK, "{:?}", by_hand.body);
    assert_eq!(by_hand.body["network_time"], false);
}

// Live updates

#[tokio::test]
async fn the_live_socket_needs_a_session_and_our_origin() {
    let h = Harness::new();
    let unauthenticated = h.send(empty(get("/api/v1/ws"))).await;
    assert_problem(&unauthenticated, StatusCode::UNAUTHORIZED, "unauthenticated");

    let session = h.sign_in("alice", Role::User).await;
    let foreign = get("/api/v1/ws").header(header::ORIGIN, "https://evil.example");
    let hijack = h.send(empty(with_cookie(foreign, &session))).await;
    assert_problem(&hijack, StatusCode::FORBIDDEN, "csrf_rejected");
}

#[tokio::test]
async fn the_live_socket_streams_metrics() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let h = Harness::new();
    let session = h.sign_in("alice", Role::User).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());

    let mut request = format!("ws://{address}/api/v1/ws").into_client_request().unwrap();
    request.headers_mut().insert(header::COOKIE, session.cookie.parse().unwrap());
    request.headers_mut().insert(header::ORIGIN, format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    socket.send(Message::Text(r#"{"type":"subscribe","topic":"metrics"}"#.into())).await.unwrap();
    let mut next = async || {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next()).await;
        serde_json::from_str::<Value>(message.unwrap().unwrap().unwrap().to_text().unwrap()).unwrap()
    };
    let latest = next().await;
    let fresh = next().await;

    assert_eq!((latest["type"].as_str(), latest["data"]["at"].as_i64()), (Some("metrics"), Some(100)));
    assert_eq!(latest["data"]["cpu_percent"], 12.5);
    assert_eq!(fresh["data"]["at"], 101);
}

#[tokio::test]
async fn the_live_socket_streams_job_changes() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let h = Harness::new();
    // Users see only their own jobs; this one is the system's.
    let session = h.sign_in("alice", Role::ReadOnly).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let mut request = format!("ws://{address}/api/v1/ws").into_client_request().unwrap();
    request.headers_mut().insert(header::COOKIE, session.cookie.parse().unwrap());
    request.headers_mut().insert(header::ORIGIN, format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    socket.send(Message::Text(r#"{"type":"subscribe","topic":"jobs"}"#.into())).await.unwrap();
    // Requests are handled in order: once metrics arrive, jobs are subscribed.
    socket.send(Message::Text(r#"{"type":"subscribe","topic":"metrics"}"#.into())).await.unwrap();
    let mut next = async || {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next()).await;
        serde_json::from_str::<Value>(message.unwrap().unwrap().unwrap().to_text().unwrap()).unwrap()
    };
    assert_eq!(next().await["type"], "metrics");

    h.jobs.start("storage.scrub", Some("tank".into()), None, |_| async { Ok(()) }).await.unwrap();
    let mut states = Vec::new();
    while states.len() < 2 {
        let message = next().await;
        if message["type"] == "job" {
            assert_eq!(message["data"]["target"], "tank");
            states.push(message["data"]["state"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(states, ["running", "succeeded"]);
}

// Initial setup

#[tokio::test]
async fn setup_creates_the_admin_and_sets_a_strict_session_cookie() {
    let h = Harness::new();
    h.identity.use_cases.prepare_setup.execute().await.unwrap();
    assert_eq!(h.send(empty(get("/api/v1/setup"))).await.body, json!({ "required": true }));

    let reply = h
        .send(json(
            post("/api/v1/setup"),
            json!({ "setup_code": SETUP_CODE, "name": "alice", "password": PASSWORD }),
        ))
        .await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    let cookie = reply.header(header::SET_COOKIE);
    assert!(cookie.starts_with("__Host-zendiloos_session=token-1;"), "{cookie}");
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(attribute), "{attribute} missing: {cookie}");
    }
    assert!(!cookie.contains("Max-Age"), "a browser-session cookie: {cookie}");
    assert_eq!(reply.body["account"]["role"], "admin");
    assert!(reply.body["account"]["permissions"].as_array().unwrap().contains(&json!("users.manage")));
    assert_eq!(h.send(empty(get("/api/v1/setup"))).await.body, json!({ "required": false }));

    let session = Session::from(&reply);
    let me = h.send(empty(with_cookie(get("/api/v1/auth/session"), &session))).await;
    assert_eq!(me.body["account"]["name"], "alice");
    assert_eq!(me.body["csrf_token"], session.csrf.as_str());
}

#[tokio::test]
async fn setup_rejects_a_wrong_code_and_explains_invalid_input() {
    let h = Harness::new();
    h.identity.use_cases.prepare_setup.execute().await.unwrap();

    let wrong_code = h
        .send(json(
            post("/api/v1/setup"),
            json!({ "setup_code": "ZZZZ-ZZZZ-ZZZZ", "name": "alice", "password": PASSWORD }),
        ))
        .await;
    let bad_name = h
        .send(json(
            post("/api/v1/setup"),
            json!({ "setup_code": SETUP_CODE, "name": "Alice", "password": PASSWORD }),
        ))
        .await;

    assert_problem(&wrong_code, StatusCode::UNPROCESSABLE_ENTITY, "invalid_setup_code");
    assert_problem(&bad_name, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert_eq!(bad_name.body["detail"], "the name must start with a lowercase letter");
}

// Login

#[tokio::test]
async fn login_failures_are_throttled() {
    let h = Harness::new();
    h.identity.add_account("alice", PASSWORD, Role::User);
    let attempt =
        || json(post("/api/v1/auth/login"), json!({ "name": "alice", "password": "wrong password" }));

    for _ in 0..5 {
        assert_problem(&h.send(attempt()).await, StatusCode::UNAUTHORIZED, "invalid_credentials");
    }
    let reply = h.send(attempt()).await;

    assert_problem(&reply, StatusCode::TOO_MANY_REQUESTS, "too_many_attempts");
    assert_eq!(reply.header(header::RETRY_AFTER), "900");
}

#[tokio::test]
async fn logins_sent_at_once_are_throttled_as_if_one_after_another() {
    let h = Harness::new();
    h.identity.add_account("alice", PASSWORD, Role::User);
    let hasher = &h.identity.hasher;
    hasher.hold(12);
    let attempts = (0..12).map(|_| {
        h.send(json(post("/api/v1/auth/login"), json!({ "name": "alice", "password": "wrong password" })))
    });

    let (replies, held) = tokio::join!(futures_util::future::join_all(attempts), async {
        // Every attempt has reached its check of the password, or was
        // refused before it.
        for _ in 0..1_000 {
            tokio::task::yield_now().await;
        }
        let held = hasher.waiting();
        hasher.release();
        held
    });

    assert_eq!(held, 5);
    let statuses = |status| replies.iter().filter(|r| r.status == status).count();
    assert_eq!((statuses(StatusCode::UNAUTHORIZED), statuses(StatusCode::TOO_MANY_REQUESTS)), (5, 7));
}

#[tokio::test]
async fn logins_from_foreign_sites_are_rejected() {
    let h = Harness::new();
    h.identity.add_account("alice", PASSWORD, Role::User);
    let request = request(Method::POST, "/api/v1/auth/login").header(header::ORIGIN, "https://evil.example");

    let reply = h.send(json(request, json!({ "name": "alice", "password": PASSWORD }))).await;

    assert_problem(&reply, StatusCode::FORBIDDEN, "csrf_rejected");
    assert!(reply.headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn cookies_are_plain_where_secure_cookies_are_off() {
    let h = Harness::with(FakeProvider, false);
    h.identity.add_account("alice", PASSWORD, Role::User);

    let reply =
        h.send(json(post("/api/v1/auth/login"), json!({ "name": "alice", "password": PASSWORD }))).await;

    let cookie = reply.header(header::SET_COOKIE);
    assert!(cookie.starts_with("zendiloos_session="), "{cookie}");
    assert!(!cookie.contains("Secure"), "{cookie}");
    let session = Session::from(&reply);
    assert_eq!(
        h.send(empty(with_cookie(get("/api/v1/auth/session"), &session))).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn malformed_bodies_are_problems() {
    let h = Harness::new();
    let send = |body: &'static str| {
        h.send(
            post("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
    };

    assert_problem(&send("{").await, StatusCode::BAD_REQUEST, "invalid_body");
    assert_problem(&send(r#"{"name":"alice"}"#).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_body");
    let extra = send(r#"{"name":"alice","password":"x","admin":true}"#).await;
    assert_problem(&extra, StatusCode::UNPROCESSABLE_ENTITY, "invalid_body");
}

// Session protection

#[tokio::test]
async fn state_changing_requests_need_the_csrf_token_and_our_origin() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::User).await;

    let no_token = h.send(empty(with_cookie(post("/api/v1/auth/logout"), &session))).await;
    let wrong_token = h
        .send(empty(with_cookie(post("/api/v1/auth/logout"), &session).header(CSRF_HEADER, "csrf-999")))
        .await;
    let foreign = request(Method::POST, "/api/v1/auth/logout").header(header::ORIGIN, "https://evil.example");
    let foreign = h.send(empty(with_csrf(foreign, &session))).await;

    for reply in [no_token, wrong_token, foreign] {
        assert_problem(&reply, StatusCode::FORBIDDEN, "csrf_rejected");
    }
    let me = h.send(empty(with_cookie(get("/api/v1/auth/session"), &session))).await;
    assert_eq!(me.status, StatusCode::OK, "the session survived");
}

#[tokio::test]
async fn logout_ends_the_session_and_removes_the_cookie() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::User).await;

    let reply = h.send(empty(with_csrf(post("/api/v1/auth/logout"), &session))).await;

    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let cookie = reply.header(header::SET_COOKIE);
    assert!(cookie.starts_with("__Host-zendiloos_session=;") && cookie.contains("Max-Age=0"), "{cookie}");
    let me = h.send(empty(with_cookie(get("/api/v1/auth/session"), &session))).await;
    assert_problem(&me, StatusCode::UNAUTHORIZED, "unauthenticated");
}

#[tokio::test]
async fn a_pending_password_change_blocks_everything_else() {
    let h = Harness::new();
    let alice = h.identity.add_account("alice", PASSWORD, Role::Admin);
    h.identity.accounts.update(alice.id, |a| a.password_change_required = true);
    let reply =
        h.send(json(post("/api/v1/auth/login"), json!({ "name": "alice", "password": PASSWORD }))).await;
    let session = Session::from(&reply);
    assert_eq!(reply.body["account"]["password_change_required"], true);

    let blocked = h.send(empty(with_cookie(get("/api/v1/system/info"), &session))).await;
    assert_problem(&blocked, StatusCode::FORBIDDEN, "password_change_required");

    let change = json!({ "current_password": PASSWORD, "new_password": "a much better passphrase" });
    let changed = h.send(json(with_csrf(post("/api/v1/auth/password"), &session), change)).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{:?}", changed.body);

    let allowed = h.send(empty(with_cookie(get("/api/v1/system/info"), &session))).await;
    assert_eq!(allowed.status, StatusCode::OK);
    let account =
        h.identity.accounts.all().into_iter().find(|a| a.name == AccountName::parse("alice").unwrap());
    assert!(!account.unwrap().password_change_required);
}

#[tokio::test]
async fn a_wrong_current_password_is_not_mistaken_for_a_lost_session() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::User).await;
    let change = json!({ "current_password": "not it at all", "new_password": "a much better passphrase" });

    let reply = h.send(json(with_csrf(post("/api/v1/auth/password"), &session), change)).await;

    assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "wrong_password");
}

/// Routes anyone may call. Everything else must require a session; this
/// guards against a handler that forgets its `Auth` argument.
const PUBLIC: [(&str, &str); 4] = [
    ("GET", "/api/v1/health"),
    ("GET", "/api/v1/setup"),
    ("POST", "/api/v1/setup"),
    ("POST", "/api/v1/auth/login"),
];

#[tokio::test]
async fn every_route_except_the_public_ones_requires_a_session() {
    let h = Harness::new();
    let doc = openapi();
    let mut checked = 0;
    for (path, item) in &doc.paths.paths {
        let operations = [
            ("GET", &item.get),
            ("POST", &item.post),
            ("PUT", &item.put),
            ("PATCH", &item.patch),
            ("DELETE", &item.delete),
        ];
        for (method, operation) in operations {
            let Some(operation) = operation else { continue };
            let public = PUBLIC.contains(&(method, path.as_str()));
            assert_eq!(operation.security.is_none(), public, "{method} {path}: documented security");
            if !public {
                let reply = h.send(empty(request(method.parse().unwrap(), path))).await;
                assert_problem(&reply, StatusCode::UNAUTHORIZED, "unauthenticated");
                checked += 1;
            }
        }
    }
    assert!(checked >= 4, "only {checked} protected operations found");
}

#[test]
fn openapi_documents_every_route() {
    let doc = openapi();
    let paths: Vec<&str> = doc.paths.paths.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "/api/v1/accounts",
            "/api/v1/accounts/{id}",
            "/api/v1/accounts/{id}/password",
            "/api/v1/apps/catalog",
            "/api/v1/apps/catalog/{source}/{app}",
            "/api/v1/apps/catalog/{source}/{app}/compose",
            "/api/v1/apps/catalog/{source}/{app}/icon",
            "/api/v1/apps/docker",
            "/api/v1/apps/docker/install",
            "/api/v1/apps/installed",
            "/api/v1/apps/installed/{id}",
            "/api/v1/apps/installed/{id}/logs",
            "/api/v1/apps/installed/{id}/start",
            "/api/v1/apps/installed/{id}/stop",
            "/api/v1/apps/projects",
            "/api/v1/apps/sources",
            "/api/v1/apps/sources/refresh",
            "/api/v1/apps/sources/{id}",
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/password",
            "/api/v1/auth/session",
            "/api/v1/docker/compose/check",
            "/api/v1/docker/containers",
            "/api/v1/docker/containers/{id}",
            "/api/v1/docker/containers/{id}/logs",
            "/api/v1/docker/containers/{id}/processes",
            "/api/v1/docker/containers/{id}/update",
            "/api/v1/docker/containers/{id}/{action}",
            "/api/v1/docker/data",
            "/api/v1/docker/data/move",
            "/api/v1/docker/images",
            "/api/v1/docker/images/prune",
            "/api/v1/docker/images/pull",
            "/api/v1/docker/images/{id}",
            "/api/v1/docker/networks",
            "/api/v1/docker/networks/{name}",
            "/api/v1/docker/overview",
            "/api/v1/docker/places",
            "/api/v1/docker/projects",
            "/api/v1/docker/projects/{name}",
            "/api/v1/docker/projects/{name}/compose",
            "/api/v1/docker/projects/{name}/logs",
            "/api/v1/docker/projects/{name}/redeploy",
            "/api/v1/docker/projects/{name}/{action}",
            "/api/v1/docker/updates",
            "/api/v1/docker/updates/apply",
            "/api/v1/docker/updates/check",
            "/api/v1/docker/updates/settings",
            "/api/v1/docker/usage",
            "/api/v1/file-services",
            "/api/v1/files/favourites",
            "/api/v1/files/removable/{drive}/archive",
            "/api/v1/files/removable/{drive}/delete",
            "/api/v1/files/removable/{drive}/download",
            "/api/v1/files/removable/{drive}/folder",
            "/api/v1/files/removable/{drive}/folders",
            "/api/v1/files/removable/{drive}/rename",
            "/api/v1/files/removable/{drive}/search",
            "/api/v1/files/removable/{drive}/transfer",
            "/api/v1/files/removable/{drive}/upload",
            "/api/v1/files/shares",
            "/api/v1/files/shares/{id}/archive",
            "/api/v1/files/shares/{id}/delete",
            "/api/v1/files/shares/{id}/download",
            "/api/v1/files/shares/{id}/folder",
            "/api/v1/files/shares/{id}/folders",
            "/api/v1/files/shares/{id}/rename",
            "/api/v1/files/shares/{id}/restore",
            "/api/v1/files/shares/{id}/search",
            "/api/v1/files/shares/{id}/snapshots",
            "/api/v1/files/shares/{id}/snapshots/{label}/restore",
            "/api/v1/files/shares/{id}/transfer",
            "/api/v1/files/shares/{id}/upload",
            "/api/v1/files/volumes",
            "/api/v1/groups",
            "/api/v1/groups/{id}",
            "/api/v1/health",
            "/api/v1/homes",
            "/api/v1/homes/{account_id}",
            "/api/v1/jobs",
            "/api/v1/jobs/{id}",
            "/api/v1/logs/audit",
            "/api/v1/logs/journal",
            "/api/v1/network",
            "/api/v1/network/confirm",
            "/api/v1/network/interfaces/{name}",
            "/api/v1/network/setup",
            "/api/v1/notification-channels",
            "/api/v1/notification-channels/{id}",
            "/api/v1/notification-channels/{id}/test",
            "/api/v1/notifications",
            "/api/v1/notifications/clear",
            "/api/v1/notifications/read",
            "/api/v1/password-policy",
            "/api/v1/preferences",
            "/api/v1/security/blocked",
            "/api/v1/security/sessions",
            "/api/v1/security/sessions/{id}",
            "/api/v1/security/settings",
            "/api/v1/security/unblock",
            "/api/v1/setup",
            "/api/v1/shares",
            "/api/v1/shares/access/{kind}/{id}",
            "/api/v1/shares/{id}",
            "/api/v1/shares/{id}/delete",
            "/api/v1/shares/{id}/snapshots",
            "/api/v1/shares/{id}/snapshots/schedules",
            "/api/v1/shares/{id}/snapshots/{label}/delete",
            "/api/v1/shares/{id}/snapshots/{label}/rollback",
            "/api/v1/storage/arrays",
            "/api/v1/storage/disks",
            "/api/v1/storage/disks/{name}/self-test",
            "/api/v1/storage/disks/{name}/self-test/abort",
            "/api/v1/storage/disks/{name}/smart",
            "/api/v1/storage/overview",
            "/api/v1/storage/pools",
            "/api/v1/storage/pools/plan",
            "/api/v1/storage/pools/{name}/destroy",
            "/api/v1/storage/pools/{name}/replace",
            "/api/v1/storage/pools/{name}/replace/plan",
            "/api/v1/storage/pools/{name}/scrub",
            "/api/v1/storage/pools/{name}/scrub/stop",
            "/api/v1/storage/removable",
            "/api/v1/storage/removable/{id}/eject",
            "/api/v1/storage/removable/{id}/mount",
            "/api/v1/storage/self-test-plans",
            "/api/v1/storage/volumes",
            "/api/v1/system/info",
            "/api/v1/system/metrics/history",
            "/api/v1/system/power",
            "/api/v1/system/ssh",
            "/api/v1/system/time",
            "/api/v1/system/tls",
            "/api/v1/system/updates",
            "/api/v1/system/updates/apply",
            "/api/v1/system/updates/check",
            "/api/v1/system/updates/settings",
        ]
    );
}

#[tokio::test]
async fn storage_lists_disks_arrays_and_volumes() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;
    let read = |uri: &'static str| h.send(empty(with_cookie(get(uri), &session)));

    let disks = read("/api/v1/storage/disks").await;
    assert_eq!(disks.status, StatusCode::OK);
    let usage: Vec<_> =
        disks.body.as_array().unwrap().iter().map(|d| (d["name"].clone(), d["usage"].clone())).collect();
    assert_eq!(
        usage,
        [
            (json!("sda"), json!("array_member")),
            (json!("sdb"), json!("array_member")),
            (json!("sdc"), json!("pool_member")),
            (json!("sdd"), json!("unused")),
            (json!("sde"), json!("unused")),
            (json!("nvme0n1"), json!("system")),
        ]
    );
    assert_eq!(disks.body[0]["array"], "md0");
    assert_eq!(disks.body[0]["transport"], "sata");
    assert_eq!(disks.body[2]["pool"], "tank");
    assert_eq!(disks.body[2]["id_link"], "ata-TEST_HDD_T3");
    assert_eq!(disks.body[5]["media"], "solid_state");

    let arrays = read("/api/v1/storage/arrays").await;
    assert_eq!(arrays.body[0]["health"], "healthy");
    assert_eq!(arrays.body[0]["devices_working"], 2);
    assert_eq!(arrays.body[0]["members"][1], json!({ "device": "sdb", "index": 1, "state": "active" }));

    let volumes = read("/api/v1/storage/volumes").await;
    let labels: Vec<_> = volumes.body.as_array().unwrap().iter().map(|v| v["label"].clone()).collect();
    assert_eq!(labels, [json!("tank"), json!("data")]);
    assert_eq!(volumes.body[0]["mount_points"], json!(["/volume1"]));
    assert_eq!(volumes.body[0]["disks"], json!(["sdc"]));
    assert_eq!(volumes.body[1]["disks"], json!(["sda", "sdb"]));
    assert_eq!(volumes.body[1]["used_bytes"], 1_u64 << 40);

    let pools = read("/api/v1/storage/pools").await;
    assert_eq!(pools.status, StatusCode::OK, "{:?}", pools.body);
    assert_eq!(pools.body[0]["name"], "tank");
    assert_eq!(pools.body[0]["layout"], "single");
    assert_eq!(pools.body[0]["mount_point"], "/volume1");
    assert_eq!(pools.body[0]["guid"], "9846106214133009000");
    assert_eq!(pools.body[0]["devices"][0]["disk"], "sdc");

    // The simulated pool is a single disk: worth a notice, nothing worse.
    let overview = read("/api/v1/storage/overview").await;
    assert_eq!(overview.status, StatusCode::OK, "{:?}", overview.body);
    assert_eq!(overview.body["verdict"], "notice");
    assert_eq!(
        overview.body["concerns"],
        json!([{ "severity": "notice", "code": "no_redundancy", "params": { "pool": "tank" } }])
    );
    assert_eq!(overview.body["pools"], 1);
}

#[tokio::test]
async fn smart_reports_are_assessed_and_only_for_existing_disks() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;

    let report = h.send(empty(with_cookie(get("/api/v1/storage/disks/sda/smart"), &session))).await;
    assert_eq!(report.status, StatusCode::OK);
    assert_eq!(report.body["status"], "available");
    assert_eq!(report.body["health"], "good");
    assert_eq!(report.body["temperature_celsius"], 33);

    let sleeping = h.send(empty(with_cookie(get("/api/v1/storage/disks/sdb/smart"), &session))).await;
    assert_eq!(sleeping.body["status"], "standby");
    assert_eq!(sleeping.body["health"], Value::Null);

    for missing in ["sdz", "nvme0n1p1", "..%2Fsda"] {
        let uri = format!("/api/v1/storage/disks/{missing}/smart");
        let reply = h.send(empty(with_cookie(get(&uri), &session))).await;
        assert_problem(&reply, StatusCode::NOT_FOUND, "not_found");
    }
}

#[tokio::test]
async fn storage_needs_the_view_permission() {
    let h = Harness::new();
    let session = h.sign_in("bob", Role::User).await;
    for uri in [
        "/api/v1/storage/overview",
        "/api/v1/storage/disks",
        "/api/v1/storage/disks/sda/smart",
        "/api/v1/storage/arrays",
        "/api/v1/storage/pools",
        "/api/v1/storage/volumes",
    ] {
        let reply = h.send(empty(with_cookie(get(uri), &session))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
}

/// utoipa names a schema after its Rust type and silently lets a later
/// type of the same name replace an earlier one in the document.
#[test]
fn schema_type_names_are_unique() {
    let routes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![routes.join("error.rs")];
    let mut folders = vec![routes.join("routes")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { folders.push(path) } else { files.push(path) }
        }
    }
    let mut seen = std::collections::HashMap::new();
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        let mut schema = false;
        for line in source.lines().map(str::trim) {
            if line.starts_with("#[derive(") && line.contains("ToSchema") {
                schema = true;
            } else if schema && (line.starts_with("pub struct ") || line.starts_with("pub enum ")) {
                let name = line.split_whitespace().nth(2).unwrap().trim_end_matches(['{', '(', ';']);
                if let Some(other) = seen.insert(name.to_owned(), file.clone()) {
                    panic!("schema {name} is defined in {} and {}", other.display(), file.display());
                }
                schema = false;
            }
        }
    }
    assert!(seen.len() > 20, "found only {} schemas", seen.len());
}

#[tokio::test]
async fn admins_manage_accounts_that_must_change_their_password() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let body = json!({ "name": "bob", "full_name": "Bob", "role": "user", "password": "initial passphrase" });

    let created = h.send(json(with_csrf(post("/api/v1/accounts"), &admin), body.clone())).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["name"], "bob");
    assert_eq!(created.body["role"], "user");
    assert_eq!(created.body["password_change_required"], true);
    assert_eq!(h.identity.system.calls(), ["create_user bob", "set_password bob"]);

    let again = h.send(json(with_csrf(post("/api/v1/accounts"), &admin), body)).await;
    assert_problem(&again, StatusCode::CONFLICT, "name_taken");

    let listed = h.send(empty(with_cookie(get("/api/v1/accounts"), &admin))).await;
    let names: Vec<_> = listed.body.as_array().unwrap().iter().map(|a| a["name"].clone()).collect();
    assert_eq!(names, [json!("alice"), json!("bob")]);

    // Bob may sign in, but must change the password before anything else.
    let login = json!({ "name": "bob", "password": "initial passphrase" });
    let bob = Session::from(&h.send(json(post("/api/v1/auth/login"), login)).await);
    let blocked = h.send(empty(with_cookie(get("/api/v1/system/info"), &bob))).await;
    assert_problem(&blocked, StatusCode::FORBIDDEN, "password_change_required");
}

#[tokio::test]
async fn accounts_carry_details_and_follow_the_password_policy() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    let put_policy = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/password-policy"), session), body))
    };

    let policy = h.send(empty(with_cookie(get("/api/v1/password-policy"), &user))).await;
    assert_eq!(
        policy.body,
        json!({ "min_length": 10, "mixed_case": false, "digit": false, "symbol": false, "max_age_days": null })
    );
    let strict =
        json!({ "min_length": 12, "mixed_case": true, "digit": false, "symbol": false, "max_age_days": 90 });
    assert_problem(&put_policy(&user, strict.clone()).await, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(put_policy(&admin, strict.clone()).await.body, strict);
    let lax = json!({ "min_length": 6, "mixed_case": false, "digit": false, "symbol": false, "max_age_days": null });
    assert_problem(&put_policy(&admin, lax).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let create = |body: Value| h.send(json(with_csrf(post("/api/v1/accounts"), &admin), body));
    let common = create(json!({ "name": "carol", "role": "user", "password": "Qwertyuiop12" })).await;
    assert_problem(&common, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let kiosk = create(json!({
        "name": "kiosk",
        "role": "user",
        "password": "Kitchen tablet one",
        "description": "Tablet in the kitchen",
        "email": "family@example.org",
        "password_locked": true,
    }))
    .await;
    assert_eq!(kiosk.status, StatusCode::CREATED, "{:?}", kiosk.body);
    assert_eq!(
        (
            &kiosk.body["description"],
            &kiosk.body["email"],
            &kiosk.body["password_locked"],
            &kiosk.body["password_change_required"]
        ),
        (&json!("Tablet in the kitchen"), &json!("family@example.org"), &json!(true), &json!(false))
    );

    let login = json!({ "name": "kiosk", "password": "Kitchen tablet one" });
    let session = Session::from(&h.send(json(post("/api/v1/auth/login"), login)).await);
    let change =
        json!({ "current_password": "Kitchen tablet one", "new_password": "Another Kitchen Tablet" });
    let refused = h.send(json(with_csrf(post("/api/v1/auth/password"), &session), change)).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "password_locked");

    let id = kiosk.body["id"].as_i64().unwrap();
    let uri = format!("/api/v1/accounts/{id}");
    let bad =
        h.send(json(with_csrf(changing(Method::PATCH, &uri), &admin), json!({ "email": "kitchen" }))).await;
    assert_problem(&bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let cleared = h
        .send(json(
            with_csrf(changing(Method::PATCH, &uri), &admin),
            json!({ "email": "", "password_locked": false }),
        ))
        .await;
    assert_eq!((&cleared.body["email"], &cleared.body["password_locked"]), (&Value::Null, &json!(false)));
}

#[tokio::test]
async fn admins_update_reset_and_delete_other_accounts_only() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let alice = h.identity.accounts.all()[0].id.get();
    let bob = h.identity.add_account("bob", PASSWORD, Role::User).id.get();
    let send = |method: Method, uri: String, body: Option<Value>| {
        let builder = with_csrf(changing(method, &uri), &admin);
        h.send(match body {
            Some(body) => json(builder, body),
            None => empty(builder),
        })
    };

    let updated =
        send(Method::PATCH, format!("/api/v1/accounts/{bob}"), Some(json!({ "disabled": true }))).await;
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.body["disabled"], true);

    let own = send(Method::PATCH, format!("/api/v1/accounts/{alice}"), Some(json!({ "role": "user" }))).await;
    assert_problem(&own, StatusCode::CONFLICT, "own_account");

    let reset = json!({ "password": "a temporary passphrase" });
    let reply = send(Method::POST, format!("/api/v1/accounts/{bob}/password"), Some(reset)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);

    let deleted = send(Method::DELETE, format!("/api/v1/accounts/{bob}"), None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let gone = send(Method::DELETE, format!("/api/v1/accounts/{bob}"), None).await;
    assert_problem(&gone, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn admins_manage_groups() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.identity.add_account("bob", PASSWORD, Role::User).id.get();

    let body = json!({ "name": "family", "description": "At home", "members": [bob] });
    let created = h.send(json(with_csrf(post("/api/v1/groups"), &admin), body)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["members"], json!([bob]));
    let id = created.body["id"].as_i64().unwrap();

    let uri = format!("/api/v1/groups/{id}");
    let changes = json!({ "members": [], "description": "" });
    let updated = h.send(json(with_csrf(changing(Method::PATCH, &uri), &admin), changes)).await;
    assert_eq!(updated.body["members"], json!([]));
    assert_eq!(updated.body["description"], Value::Null);

    let listed = h.send(empty(with_cookie(get("/api/v1/groups"), &admin))).await;
    assert_eq!(listed.body.as_array().unwrap().len(), 1);
    let deleted = h.send(empty(with_csrf(changing(Method::DELETE, &uri), &admin))).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn only_admins_manage_accounts_and_groups() {
    let h = Harness::new();
    let session = h.sign_in("carol", Role::ReadOnly).await;
    for uri in ["/api/v1/accounts", "/api/v1/groups"] {
        let reply = h.send(empty(with_cookie(get(uri), &session))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
    let body = json!({ "name": "eve", "role": "admin", "password": "initial passphrase" });
    let reply = h.send(json(with_csrf(post("/api/v1/accounts"), &session), body)).await;
    assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    assert!(h.identity.system.calls().is_empty());
}

#[tokio::test]
async fn the_journal_is_read_through_fixed_views() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;
    let read = |uri: &'static str| h.send(empty(with_cookie(get(uri), &session)));

    let page = read("/api/v1/logs/journal?source=sharing&priority=warning&limit=1000&before=s%3D9").await;
    assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
    assert_eq!(page.body["entries"][0]["priority"], "error");
    assert_eq!(page.body["entries"][0]["unit"], "smbd.service");
    assert_eq!(page.body["older"], "s=1;i=2");
    let query = h.journal.0.lock().unwrap()[0].clone();
    assert_eq!(query.source, zendiloos_system::domain::LogSource::Sharing);
    assert_eq!(query.priority, LogPriority::Warning);
    assert_eq!(query.limit, 500, "capped");
    assert_eq!(query.before.unwrap().as_str(), "s=9");

    let defaults = read("/api/v1/logs/journal").await;
    assert_eq!(defaults.status, StatusCode::OK);
    assert_eq!(h.journal.0.lock().unwrap()[1].priority, LogPriority::Info);

    for bad in [
        "/api/v1/logs/journal?source=everything",
        "/api/v1/logs/journal?unit=ssh.service",
        "/api/v1/logs/journal?before=s%3D1%20--since%3Dtoday",
    ] {
        assert_problem(&read(bad).await, StatusCode::BAD_REQUEST, "bad_request");
    }
}

#[tokio::test]
async fn the_audit_log_is_paged_newest_first() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;
    let logout = h.sign_in("bob", Role::Admin).await;
    h.send(empty(with_csrf(post("/api/v1/auth/logout"), &logout))).await;

    let page = h.send(empty(with_cookie(get("/api/v1/logs/audit?limit=2"), &session))).await;
    assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
    let actions: Vec<_> =
        page.body["entries"].as_array().unwrap().iter().map(|e| e["action"].clone()).collect();
    assert_eq!(actions, [json!("identity.logout"), json!("identity.login")]);
    assert_eq!(page.body["entries"][0]["actor"], "bob");
    let older = page.body["older"].as_i64().unwrap();

    let uri = format!("/api/v1/logs/audit?before={older}");
    let rest = h.send(empty(with_cookie(get(&uri), &session))).await;
    assert_eq!(rest.body["entries"][0]["actor"], "alice");
    assert_eq!(rest.body["older"], Value::Null);
}

#[tokio::test]
async fn logs_need_the_view_permission() {
    let h = Harness::new();
    let session = h.sign_in("bob", Role::User).await;
    for uri in ["/api/v1/logs/journal", "/api/v1/logs/audit"] {
        let reply = h.send(empty(with_cookie(get(uri), &session))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
}

/// Types used only in parameters are not collected automatically; a
/// dangling reference breaks the TypeScript generator.
#[test]
fn every_schema_reference_resolves() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    let mut pending = vec![&doc];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    let name = reference.strip_prefix("#/components/schemas/").unwrap();
                    assert!(schemas.contains_key(name), "dangling reference {reference}");
                }
                pending.extend(map.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
}

#[tokio::test]
async fn jobs_are_listed_newest_first() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;
    for target in ["tank", "backup"] {
        h.jobs.start("storage.scrub", Some(target.into()), None, |_| async { Ok(()) }).await.unwrap();
    }

    let list = h.send(empty(with_cookie(get("/api/v1/jobs"), &session))).await;
    assert_eq!(list.status, StatusCode::OK);
    let targets: Vec<_> = list.body.as_array().unwrap().iter().map(|j| j["target"].clone()).collect();
    assert_eq!(targets, [json!("backup"), json!("tank")]);
    assert_eq!(list.body[0]["kind"], "storage.scrub");

    let id = list.body[1]["id"].as_i64().unwrap();
    let one = h.send(empty(with_cookie(get(&format!("/api/v1/jobs/{id}")), &session))).await;
    assert_eq!(one.body["target"], "tank");
    let missing = h.send(empty(with_cookie(get("/api/v1/jobs/999"), &session))).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");
}

async fn job_done(h: &Harness, id: i64) -> zendiloos_jobs::Job {
    for _ in 0..100 {
        let job = h.jobs.find(zendiloos_jobs::JobId::new(id)).await.unwrap().unwrap();
        if job.state != zendiloos_jobs::JobState::Running {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {id} did not finish");
}

#[tokio::test]
async fn pools_are_created_from_a_confirmed_plan() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let request = json!({ "name": "media", "layout": "mirror", "disks": ["sdd", "sde"] });

    let plan = h.send(json(with_csrf(post("/api/v1/storage/pools/plan"), &admin), request)).await;
    assert_eq!(plan.status, StatusCode::OK, "{:?}", plan.body);
    assert_eq!(plan.body["mount_point"], "/volume2");
    assert_eq!(plan.body["redundancy"], 1);
    assert_eq!(plan.body["disks"][0]["id_link"], "ata-TEST_HDD_T4");
    let token = plan.body["token"].as_str().unwrap().to_owned();

    let wrong = json!({ "token": token, "confirm": "Media" });
    let reply = h.send(json(with_csrf(post("/api/v1/storage/pools"), &admin), wrong)).await;
    assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");

    let confirm = json!({ "token": token, "confirm": "media" });
    let started = h.send(json(with_csrf(post("/api/v1/storage/pools"), &admin), confirm.clone())).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    assert_eq!(started.body["kind"], "storage.create_pool");
    let job = job_done(&h, started.body["id"].as_i64().unwrap()).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded);
    assert_eq!(h.storage.calls(), ["create media Mirror ata-TEST_HDD_T4,ata-TEST_HDD_T5 2"]);

    let reused = h.send(json(with_csrf(post("/api/v1/storage/pools"), &admin), confirm)).await;
    assert_problem(&reused, StatusCode::CONFLICT, "plan_expired");
}

#[tokio::test]
async fn plans_explain_why_a_disk_cannot_be_used() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let plan = |body| h.send(json(with_csrf(post("/api/v1/storage/pools/plan"), &admin), body));

    let system = plan(json!({ "name": "media", "layout": "mirror", "disks": ["nvme0n1", "sdd"] })).await;
    assert_problem(&system, StatusCode::UNPROCESSABLE_ENTITY, "disk_unusable");
    assert_eq!(system.body["detail"], "nvme0n1: system");
    let count = plan(json!({ "name": "media", "layout": "raidz1", "disks": ["sdd", "sde"] })).await;
    assert_problem(&count, StatusCode::UNPROCESSABLE_ENTITY, "disk_count");
    let taken = plan(json!({ "name": "tank", "layout": "single", "disks": ["sdd"] })).await;
    assert_problem(&taken, StatusCode::CONFLICT, "name_taken");
    let name = plan(json!({ "name": "Mirror", "layout": "single", "disks": ["sdd"] })).await;
    assert_problem(&name, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn pools_are_destroyed_replaced_and_scrubbed_by_admins_only() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let send = |uri: &str, body: Value| h.send(json(with_csrf(post(uri), &admin), body));

    let wrong = send("/api/v1/storage/pools/tank/destroy", json!({ "confirm": "tnak" })).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");

    let plan = send(
        "/api/v1/storage/pools/tank/replace/plan",
        json!({ "device": "4933705814018298948", "disk": "sde" }),
    )
    .await;
    assert_eq!(plan.status, StatusCode::OK, "{:?}", plan.body);
    assert_eq!(plan.body["disk"]["id_link"], "ata-TEST_HDD_T5");
    let started = send("/api/v1/storage/pools/tank/replace", json!({ "token": plan.body["token"] })).await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    job_done(&h, started.body["id"].as_i64().unwrap()).await;

    let scrub = h.send(empty(with_csrf(post("/api/v1/storage/pools/tank/scrub"), &admin))).await;
    assert_eq!(scrub.status, StatusCode::NO_CONTENT);

    let destroyed = send("/api/v1/storage/pools/tank/destroy", json!({ "confirm": "tank" })).await;
    assert_eq!(destroyed.status, StatusCode::ACCEPTED);
    job_done(&h, destroyed.body["id"].as_i64().unwrap()).await;
    assert_eq!(
        h.storage.calls(),
        ["replace tank 4933705814018298948 ata-TEST_HDD_T5", "scrub tank", "destroy tank 1"]
    );

    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let denied = h.send(empty(with_csrf(post("/api/v1/storage/pools/tank/scrub"), &viewer))).await;
    assert_problem(&denied, StatusCode::FORBIDDEN, "forbidden");
}

#[tokio::test]
async fn a_stick_plugged_in_is_mounted_when_listed_and_ejected_by_admins_only() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let list = || h.send(empty(with_cookie(get("/api/v1/storage/removable"), &admin)));
    let eject = |id: &str, body: Value| {
        h.send(json(with_csrf(post(&format!("/api/v1/storage/removable/{id}/eject")), &admin), body))
    };
    let mount =
        |id: &str| h.send(empty(with_csrf(post(&format!("/api/v1/storage/removable/{id}/mount")), &admin)));
    assert_eq!(list().await.body, json!([]));
    h.storage.plug_stick();

    let listed = list().await;

    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{
            "id": "1A2B-3C4D", "label": "STICK", "filesystem": "fat", "device": "sdf1", "disk": "sdf",
            "model": "TEST Stick", "size_bytes": 32_015_998_976_u64, "state": "mounted", "read_only": false,
            "path": "/run/zendiloos-removable/1A2B-3C4D", "used_bytes": null, "available_bytes": null,
            "problem": null,
        }])
    );
    // Mounted now: what it holds is known from the next look on.
    assert_eq!(list().await.body[0]["available_bytes"], 24_600_000_000_u64);

    // In use: refused, still mounted.
    *h.storage.stick_busy.lock().unwrap() = true;
    assert_problem(&eject("1A2B-3C4D", json!({})).await, StatusCode::CONFLICT, "drive_in_use");
    assert_eq!(list().await.body[0]["state"], "mounted");
    *h.storage.stick_busy.lock().unwrap() = false;

    // Ejected without powering it off: it stays, not mounted, until asked.
    assert_eq!(eject("1A2B-3C4D", json!({ "power_off": false })).await.status, StatusCode::NO_CONTENT);
    assert_eq!(list().await.body[0]["state"], "ejected");
    let again = mount("1A2B-3C4D").await;
    assert_eq!((again.status, again.body["state"].clone()), (StatusCode::OK, json!("mounted")));

    // Ejected for good (the default): gone from the bus.
    assert_eq!(eject("1A2B-3C4D", json!({})).await.status, StatusCode::NO_CONTENT);
    assert_eq!(list().await.body, json!([]));
    assert_problem(&eject("1A2B-3C4D", json!({})).await, StatusCode::NOT_FOUND, "not_found");
    assert_problem(&mount("..").await, StatusCode::NOT_FOUND, "not_found");
    assert_eq!(
        h.storage.calls(),
        [
            "mount 1A2B-3C4D",
            "eject 1A2B-3C4D true",
            "eject 1A2B-3C4D false",
            "mount 1A2B-3C4D",
            "eject 1A2B-3C4D true",
        ]
    );
    let actions: Vec<_> = h
        .identity
        .audit
        .entries()
        .into_iter()
        .filter(|e| e.action.contains("removable"))
        .map(|e| (e.action, e.outcome))
        .collect();
    assert_eq!(
        actions,
        [
            ("storage.eject_removable", Outcome::Failure),
            ("storage.eject_removable", Outcome::Success),
            ("storage.mount_removable", Outcome::Success),
            ("storage.eject_removable", Outcome::Success),
        ]
    );
    let unknown = eject("1A2B-3C4D", json!({ "power_off": true, "force": true })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", unknown.body);
}

#[tokio::test]
async fn removable_drives_are_for_administrators_only() {
    let h = Harness::new();
    h.storage.plug_stick();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let listed = h.send(empty(with_cookie(get("/api/v1/storage/removable"), &viewer))).await;
    assert_problem(&listed, StatusCode::FORBIDDEN, "forbidden");
    for action in ["mount", "eject"] {
        let uri = format!("/api/v1/storage/removable/1A2B-3C4D/{action}");
        let reply = h.send(json(with_csrf(post(&uri), &viewer), json!({}))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
    assert_eq!(h.storage.calls(), Vec::<String>::new(), "nothing was mounted for a viewer");
}

#[tokio::test]
async fn administrators_work_on_a_mounted_stick_and_copy_from_it_into_a_share() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::User).await;
    let alice_id =
        h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "alice").unwrap().id.get();
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank",
                    "access": [{ "kind": "account", "id": alice_id, "access": "read_write" }] }),
        ))
        .await;
    let share = created.body["id"].as_i64().unwrap();
    let stick = "/api/v1/files/removable/1A2B-3C4D";
    let folder = || h.send(empty(with_cookie(get(&format!("{stick}/folder?path=Photos")), &admin)));

    assert_problem(&folder().await, StatusCode::NOT_FOUND, "not_found");
    h.storage.plug_stick();
    assert_problem(&folder().await, StatusCode::NOT_FOUND, "not_found");
    h.send(empty(with_cookie(get("/api/v1/storage/removable"), &admin))).await;
    let listed = folder().await;
    assert_eq!(
        (listed.status, listed.body["path"].clone()),
        (StatusCode::OK, json!("Photos")),
        "{:?}",
        listed.body
    );

    let made =
        h.send(json(with_csrf(post(&format!("{stick}/folders")), &admin), json!({ "path": "new" }))).await;
    assert_eq!(made.status, StatusCode::CREATED);
    let renamed = h
        .send(json(
            with_csrf(post(&format!("{stick}/rename")), &admin),
            json!({ "path": "new", "name": "old" }),
        ))
        .await;
    assert_eq!(renamed.body["path"], "old");
    let download =
        h.send(empty(with_cookie(get(&format!("{stick}/download?path=Photos/a.jpg")), &admin))).await;
    assert_eq!((download.status, download.body.clone()), (StatusCode::OK, json!("abcdefghij")));
    let archive = h.send(empty(with_cookie(get(&format!("{stick}/archive?path=")), &admin))).await;
    assert_eq!(
        archive.header(header::CONTENT_DISPOSITION),
        "attachment; filename=\"STICK.tar\"; filename*=UTF-8''STICK.tar"
    );
    let request = with_csrf(post(&format!("{stick}/upload?path=&name=song.mp3")), &admin)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from("tune"))
        .unwrap();
    assert_eq!(h.send(request).await.status, StatusCode::CREATED);
    // From the stick into the share, and from the share onto the stick.
    let into =
        json!({ "mode": "copy", "paths": ["Photos/a.jpg"], "target_share": share, "target_path": "2024" });
    let copied = h.send(json(with_csrf(post(&format!("{stick}/transfer")), &admin), into)).await;
    assert_eq!(copied.status, StatusCode::ACCEPTED, "{:?}", copied.body);
    job_done(&h, copied.body["id"].as_i64().unwrap()).await;
    let onto =
        json!({ "mode": "move", "paths": ["2024/a.jpg"], "target_drive": "1A2B-3C4D", "target_path": "" });
    let shares_files = format!("/api/v1/files/shares/{share}");
    let moved =
        h.send(json(with_csrf(post(&format!("{shares_files}/transfer")), &admin), onto.clone())).await;
    assert_eq!(moved.status, StatusCode::ACCEPTED, "{:?}", moved.body);
    job_done(&h, moved.body["id"].as_i64().unwrap()).await;
    let both = json!({ "mode": "copy", "paths": ["a"], "target_share": share, "target_drive": "1A2B-3C4D" });
    let both = h.send(json(with_csrf(post(&format!("{stick}/transfer")), &admin), both)).await;
    assert_problem(&both, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let deleted =
        h.send(json(with_csrf(post(&format!("{stick}/delete")), &admin), json!({ "paths": ["old"] }))).await;
    job_done(&h, deleted.body["id"].as_i64().unwrap()).await;

    // Everyone else: not even a look, nor onto a drive from a share.
    let refused = h.send(empty(with_cookie(get(&format!("{stick}/folder")), &bob))).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "forbidden");
    let refused = h.send(json(with_csrf(post(&format!("{shares_files}/transfer")), &bob), onto)).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "forbidden");

    assert_eq!(
        h.file_host.calls(),
        [
            "list alice removable:1A2B-3C4D:Photos",
            "mkdir alice removable:1A2B-3C4D:new",
            "move alice removable:1A2B-3C4D:new -> removable:1A2B-3C4D:old",
            "open alice removable:1A2B-3C4D:Photos/a.jpg",
            "archive alice removable:1A2B-3C4D:",
            "create_upload alice removable:1A2B-3C4D:",
            "move alice removable:1A2B-3C4D:.zendiloos-upload-1 -> removable:1A2B-3C4D:song.mp3",
            "copy alice removable:1A2B-3C4D:Photos/a.jpg -> photos:2024/a.jpg",
            "move alice photos:2024/a.jpg -> removable:1A2B-3C4D:a.jpg",
            "remove alice removable:1A2B-3C4D:old",
        ],
        "for good: no bin on a drive"
    );
}

#[tokio::test]
async fn notifications_are_listed_newest_first() {
    let h = Harness::new();
    let session = h.sign_in("alice", Role::ReadOnly).await;
    h.notifier.notify(NewNotification::new(Level::Warning, "pool_trouble").with("pool", "tank")).await;
    h.notifier.notify(NewNotification::new(Level::Success, "pool_recovered").with("pool", "tank")).await;

    let list = h.send(empty(with_cookie(get("/api/v1/notifications"), &session))).await;
    assert_eq!(list.status, StatusCode::OK);
    let listed = &list.body["notifications"];
    assert_eq!(listed[0]["code"], "pool_recovered");
    assert_eq!(listed[1]["level"], "warning");
    assert_eq!(listed[1]["params"], json!({ "pool": "tank" }));
    assert_eq!(list.body["read_up_to"], 0);
}

#[tokio::test]
async fn each_account_reads_and_clears_its_notifications_in_every_browser() {
    let h = Harness::new();
    let alice = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::ReadOnly).await;
    for code in ["pool_trouble", "pool_recovered", "scrub_finished"] {
        h.notifier.notify(NewNotification::new(Level::Info, code)).await;
    }
    let ids = |list: &Reply| -> Vec<i64> {
        list.body["notifications"].as_array().unwrap().iter().map(|n| n["id"].as_i64().unwrap()).collect()
    };

    let read =
        h.send(json(with_csrf(post("/api/v1/notifications/read"), &alice), json!({ "up_to": 2 }))).await;
    assert_eq!(read.status, StatusCode::NO_CONTENT, "{:?}", read.body);
    let cleared =
        h.send(json(with_csrf(post("/api/v1/notifications/clear"), &bob), json!({ "up_to": 1 }))).await;
    assert_eq!(cleared.status, StatusCode::NO_CONTENT, "{:?}", cleared.body);

    let list = h.send(empty(with_cookie(get("/api/v1/notifications"), &alice))).await;
    assert_eq!((ids(&list), list.body["read_up_to"].as_i64()), (vec![3, 2, 1], Some(2)));
    let list = h.send(empty(with_cookie(get("/api/v1/notifications"), &bob))).await;
    assert_eq!((ids(&list), list.body["read_up_to"].as_i64()), (vec![3, 2], Some(1)));

    let forged =
        h.send(json(with_cookie(post("/api/v1/notifications/read"), &alice), json!({ "up_to": 3 }))).await;
    assert_problem(&forged, StatusCode::FORBIDDEN, "csrf_rejected");
}

#[tokio::test]
async fn admins_switch_file_services_and_set_the_workgroup() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/file-services"), session), body))
    };

    let current = h.send(empty(with_cookie(get("/api/v1/file-services"), &viewer))).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(
        current.body,
        json!({
            "smb": true, "nfs": true, "workgroup": "WORKGROUP", "smb_min_protocol": "smb2_1",
            "smb_encryption": "default", "smb_signing": "default", "hide_unreadable": true, "multichannel": true,
            "oplocks": true, "leases": true, "durable_handles": false, "mac_compat": true, "follow_symlinks": true,
            "veto_files": null, "delete_veto_files": false, "wsdd": true, "bonjour": true, "time_machine": null,
        })
    );

    let wanted = json!({ "smb": true, "nfs": false, "workgroup": " home ", "smb_min_protocol": "smb3" });
    assert_problem(&put(&viewer, wanted.clone()).await, StatusCode::FORBIDDEN, "forbidden");
    let stored = put(&admin, wanted).await;
    assert_eq!(stored.status, StatusCode::OK, "{:?}", stored.body);
    assert_eq!((stored.body["workgroup"].clone(), stored.body["nfs"].clone()), (json!("HOME"), json!(false)));
    let applied = h.share_host.configs.lock().unwrap().last().unwrap().services.clone();
    assert_eq!((applied.workgroup.as_str(), applied.nfs), ("HOME", false));
    let entry = h.identity.audit.entries().last().cloned().unwrap();
    assert_eq!(entry.action, "sharing.services_update");

    for bad in [
        json!({ "smb": true, "nfs": true, "workgroup": "MY WORKGROUP", "smb_min_protocol": "smb3" }),
        json!({ "smb": true, "nfs": true, "workgroup": "", "smb_min_protocol": "smb3" }),
    ] {
        assert_problem(&put(&admin, bad).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let smb1 = json!({ "smb": true, "nfs": true, "workgroup": "HOME", "smb_min_protocol": "smb1" });
    assert_eq!(put(&admin, smb1).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    let base = json!({ "smb": true, "nfs": true, "workgroup": "HOME", "smb_min_protocol": "smb3" });
    let with = |extra: Value| {
        let mut body = base.clone();
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };
    let options =
        put(&admin, with(json!({ "smb_signing": "mandatory", "veto_files": " /*.tmp/ ", "oplocks": false })))
            .await;
    assert_eq!(options.status, StatusCode::OK, "{:?}", options.body);
    assert_eq!(
        (&options.body["smb_signing"], &options.body["veto_files"], &options.body["leases"]),
        (&json!("mandatory"), &json!("/*.tmp/"), &json!(false)),
        "tidied: no leases without oplocks"
    );
    let applied = h.share_host.configs.lock().unwrap().last().unwrap().services.clone();
    assert_eq!(applied.smb_signing, zendiloos_sharing::domain::SmbSigning::Mandatory);
    let bad_pattern = put(&admin, with(json!({ "veto_files": "*.tmp" }))).await;
    assert_problem(&bad_pattern, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let no_share = put(&admin, with(json!({ "time_machine": { "share_id": 99 } }))).await;
    assert_problem(&no_share, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn a_group_can_be_denied_a_share_and_its_members_see_it() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.identity.add_account("bob", PASSWORD, Role::User).id.get();
    let kids = json(with_csrf(post("/api/v1/groups"), &admin), json!({ "name": "kids", "members": [bob] }));
    let kids = h.send(kids).await.body["id"].as_i64().unwrap();
    let body = json!({
        "name": "photos",
        "pool": "tank",
        "access": [{ "kind": "account", "id": bob, "access": "read_write" }],
    });
    let photos =
        h.send(json(with_csrf(post("/api/v1/shares"), &admin), body)).await.body["id"].as_i64().unwrap();
    let put = |uri: String, body: Value| h.send(json(with_csrf(changing(Method::PUT, &uri), &admin), body));

    let denied = put(
        format!("/api/v1/shares/access/group/{kids}"),
        json!({ "rules": [{ "share_id": photos, "rule": "deny" }] }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::OK, "{:?}", denied.body);
    assert_eq!(denied.body["jobs"][0]["kind"], "sharing.apply_access");

    let seen = h.send(empty(with_cookie(get(&format!("/api/v1/shares/access/account/{bob}")), &admin))).await;
    assert_eq!(seen.status, StatusCode::OK);
    assert_eq!(
        seen.body,
        json!([{ "share_id": photos, "share": "photos", "rule": "read_write", "effective": null }]),
        "the group's denial wins over bob's own rule"
    );
    let calls = h.share_host.calls();
    assert!(calls.contains(&"access 1 photos [bob:-] -[]".to_owned()), "{calls:?}");

    let unknown = h.send(empty(with_cookie(get("/api/v1/shares/access/group/999"), &admin))).await;
    assert_problem(&unknown, StatusCode::UNPROCESSABLE_ENTITY, "unknown_subject");
    let twice = put(
        format!("/api/v1/shares/access/account/{bob}"),
        json!({ "rules": [{ "share_id": photos, "rule": "read" }, { "share_id": photos }] }),
    )
    .await;
    assert_problem(&twice, StatusCode::UNPROCESSABLE_ENTITY, "duplicate_rule");
    let cleared =
        put(format!("/api/v1/shares/access/group/{kids}"), json!({ "rules": [{ "share_id": photos }] }))
            .await;
    assert_eq!(cleared.status, StatusCode::OK);
    let seen = h.send(empty(with_cookie(get(&format!("/api/v1/shares/access/account/{bob}")), &admin))).await;
    assert_eq!(seen.body[0]["effective"], "read_write");
}

#[tokio::test]
async fn personal_folders_are_switched_on_and_given_to_accounts() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob_session = h.sign_in("bob", Role::User).await;
    let bob = h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "bob").unwrap().id.get();
    let put = |uri: String, body: Value| h.send(json(with_csrf(changing(Method::PUT, &uri), &admin), body));

    let off = put(format!("/api/v1/homes/{bob}"), json!({ "enabled": true })).await;
    assert_problem(&off, StatusCode::CONFLICT, "refused");
    let on = put("/api/v1/homes".into(), json!({ "pool": "tank" })).await;
    assert_eq!(on.status, StatusCode::OK, "{:?}", on.body);
    assert_eq!(on.body["location"], json!({ "pool": "tank", "volume": 1 }));
    let given =
        put(format!("/api/v1/homes/{bob}"), json!({ "enabled": true, "quota_bytes": 10_737_418_240_u64 }))
            .await;
    assert_eq!(given.body, json!({ "account_id": bob, "quota_bytes": 10_737_418_240_u64 }));
    let small = put(format!("/api/v1/homes/{bob}"), json!({ "enabled": true, "quota_bytes": 1024 })).await;
    assert_problem(&small, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let mine = h.send(empty(with_cookie(get("/api/v1/files/shares"), &bob_session))).await;
    assert_eq!(mine.body[0]["id"], 0);
    assert_eq!((&mine.body[0]["name"], &mine.body[0]["home"]), (&json!("home"), &json!(true)));
    let admins = h.send(empty(with_cookie(get("/api/v1/files/shares"), &admin))).await;
    assert!(admins.body.as_array().unwrap().iter().all(|s| s["home"] == false), "alice has none");
    let calls = h.share_host.calls();
    assert!(calls.contains(&"prepare_homes tank 1".to_owned()), "{calls:?}");
    assert!(calls.contains(&"set_home tank 1 bob Some(10737418240)".to_owned()), "{calls:?}");

    let listed = h.send(empty(with_cookie(get("/api/v1/homes"), &admin))).await;
    assert_eq!(listed.body["homes"], json!([{ "account_id": bob, "quota_bytes": 10_737_418_240_u64 }]));
    let refused = h.send(empty(with_cookie(get("/api/v1/homes"), &bob_session))).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "forbidden");
    let off = put("/api/v1/homes".into(), json!({ "pool": null })).await;
    assert_eq!(off.body["location"], Value::Null);
}

#[tokio::test]
async fn folders_on_a_volume_go_on_and_off_the_network_and_show_in_the_file_manager() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob_session = h.sign_in("bob", Role::User).await;
    let bob = h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "bob").unwrap().id.get();
    let create = |body| h.send(json(with_csrf(post("/api/v1/shares"), &admin), body));
    let docker = create(json!({
        "name": "docker",
        "description": null,
        "pool": "tank",
        "shared": false,
        "access": [{ "kind": "account", "id": bob, "access": "read" }],
    }))
    .await;
    assert_eq!(docker.status, StatusCode::CREATED, "{:?}", docker.body);
    assert_eq!(docker.body["shared"], false);
    let private = create(json!({ "name": "private", "description": null, "pool": "tank" })).await;
    assert_eq!(private.body["shared"], true, "on the network unless asked otherwise");

    let all = h.send(empty(with_cookie(get("/api/v1/files/volumes"), &admin))).await;
    assert_eq!(all.status, StatusCode::OK, "{:?}", all.body);
    let tank = all.body.as_array().unwrap().iter().find(|v| v["pool"] == "tank").unwrap();
    assert_eq!(tank["volume"], 1);
    let folders: Vec<_> = tank["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["shared"].as_bool().unwrap(), f["access"].clone()))
        .collect();
    assert_eq!(
        folders,
        [("docker", false, Value::Null), ("private", true, Value::Null)],
        "listed, not to open"
    );
    let bobs = h.send(empty(with_cookie(get("/api/v1/files/volumes"), &bob_session))).await;
    let bobs = bobs.body.as_array().unwrap();
    assert_eq!(bobs.len(), 1);
    assert_eq!(
        bobs[0]["folders"],
        json!([{
            "id": docker.body["id"], "name": "docker", "description": null, "shared": false,
            "access": "read", "recycle_bin": true
        }])
    );

    let uri = format!("/api/v1/shares/{}", docker.body["id"]);
    let shared =
        h.send(json(with_csrf(changing(Method::PATCH, &uri), &admin), json!({ "shared": true }))).await;
    assert_eq!(shared.status, StatusCode::OK, "{:?}", shared.body);
    assert_eq!((&shared.body["share"]["shared"], &shared.body["job"]), (&json!(true), &Value::Null));
}

#[tokio::test]
async fn admins_create_change_and_delete_shares() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.identity.add_account("bob", PASSWORD, Role::User).id.get();
    let body = json!({
        "name": "photos",
        "description": "Pictures",
        "pool": "tank",
        "access": [{ "kind": "account", "id": bob, "access": "read_write" }],
        "nfs": [{ "network": "192.168.1.0/24", "write": false }],
    });

    let created = h.send(json(with_csrf(post("/api/v1/shares"), &admin), body.clone())).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["path"], "/volume1/photos");
    assert_eq!(created.body["recycle_bin"], true);
    let again = h.send(json(with_csrf(post("/api/v1/shares"), &admin), body)).await;
    assert_problem(&again, StatusCode::CONFLICT, "name_taken");

    let id = created.body["id"].as_i64().unwrap();
    let uri = format!("/api/v1/shares/{id}");
    let changes = json!({ "access": [{ "kind": "account", "id": bob, "access": "read" }] });
    let updated = h.send(json(with_csrf(changing(Method::PATCH, &uri), &admin), changes)).await;
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.body["share"]["access"][0]["access"], "read");
    assert_eq!(updated.body["job"]["kind"], "sharing.apply_access");

    let listed = h.send(empty(with_cookie(get("/api/v1/shares"), &admin))).await;
    assert_eq!(listed.body.as_array().unwrap().len(), 1);

    let wrong =
        h.send(json(with_csrf(post(&format!("{uri}/delete")), &admin), json!({ "confirm": "x" }))).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");
    let deleted =
        h.send(json(with_csrf(post(&format!("{uri}/delete")), &admin), json!({ "confirm": "photos" }))).await;
    assert_eq!(deleted.status, StatusCode::ACCEPTED);
    job_done(&h, deleted.body["id"].as_i64().unwrap()).await;
    assert!(h.share_host.calls().contains(&"destroy tank 1 photos".to_owned()));
}

#[tokio::test]
async fn shares_validate_their_rules() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let create = |body| h.send(json(with_csrf(post("/api/v1/shares"), &admin), body));
    let unknown = create(
        json!({ "name": "x", "pool": "tank", "access": [{ "kind": "group", "id": 99, "access": "read" }] }),
    )
    .await;
    assert_problem(&unknown, StatusCode::UNPROCESSABLE_ENTITY, "unknown_subject");
    let volume = create(json!({ "name": "x", "pool": "cold" })).await;
    assert_problem(&volume, StatusCode::UNPROCESSABLE_ENTITY, "volume_not_found");
    let network =
        create(json!({ "name": "x", "pool": "tank", "nfs": [{ "network": "*", "write": true }] })).await;
    assert_problem(&network, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let user = h.sign_in("bob", Role::User).await;
    let denied = h.send(empty(with_cookie(get("/api/v1/shares"), &user))).await;
    assert_problem(&denied, StatusCode::FORBIDDEN, "forbidden");
}

/// utoipa takes operation IDs from handler names; two handlers named alike
/// in different modules would share one, and the TypeScript types of one
/// route would describe the other.
#[test]
fn operation_ids_are_unique() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let mut seen = std::collections::HashMap::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            let Some(id) = operation.get("operationId").and_then(Value::as_str) else { continue };
            if let Some(other) = seen.insert(id.to_owned(), format!("{method} {path}")) {
                panic!("operation {id} is used by {other} and {method} {path}");
            }
        }
    }
    assert!(seen.len() > 30, "found only {} operations", seen.len());
}

#[tokio::test]
async fn a_search_looks_below_a_folder_the_user_may_see() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let carol = h.sign_in("carol", Role::User).await;
    let carol_id =
        h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "carol").unwrap().id.get();
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank",
                    "access": [{ "kind": "account", "id": carol_id, "access": "read" }] }),
        ))
        .await;
    let files = format!("/api/v1/files/shares/{}", created.body["id"]);
    let search = |session: &Session, query: &str| {
        h.send(empty(with_cookie(get(&format!("{files}/search?{query}")), session)))
    };

    let found = search(&carol, "path=2024&text=Beach").await;
    assert_eq!(found.status, StatusCode::OK, "{:?}", found.body);
    assert_eq!(
        found.body,
        json!({
            "path": "2024",
            "entries": [{ "path": "2024/album/beach.jpg", "name": "beach.jpg", "kind": "file",
                          "size": 4, "modified": 2, "unreadable": false }],
            "stopped": null,
        })
    );
    let many = search(&carol, "text=many&hidden=true").await;
    assert_eq!(many.body["stopped"], "results");
    assert_problem(&search(&carol, "text=").await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_operation");
    assert_problem(&search(&carol, "path=2024").await, StatusCode::BAD_REQUEST, "bad_request");
    assert_problem(
        &search(&carol, "text=x&path=../x").await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    assert_problem(&search(&admin, "text=beach").await, StatusCode::NOT_FOUND, "not_found");

    // A drive, for someone who manages storage, while it is mounted.
    let stick = "/api/v1/files/removable/1A2B-3C4D/search?text=song";
    let on_stick = |session: &Session| h.send(empty(with_cookie(get(stick), session)));
    assert_problem(&on_stick(&carol).await, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(&on_stick(&admin).await, StatusCode::NOT_FOUND, "not_found");
    h.storage.plug_stick();
    h.send(empty(with_cookie(get("/api/v1/storage/removable"), &admin))).await;
    let songs = on_stick(&admin).await;
    assert_eq!((songs.status, songs.body["path"].clone()), (StatusCode::OK, json!("")), "{:?}", songs.body);
    assert_eq!(
        h.file_host.calls(),
        [
            "search carol photos:2024 Beach",
            "search carol photos: many hidden",
            "search alice removable:1A2B-3C4D: song",
        ]
    );
}

#[tokio::test]
async fn users_work_with_files_in_their_shares() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::User).await;
    let carol = h.sign_in("carol", Role::User).await;
    let id_of = |name: &str| {
        h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == name).unwrap().id.get()
    };
    let rules = json!([
        { "kind": "account", "id": id_of("bob"), "access": "read_write" },
        { "kind": "account", "id": id_of("carol"), "access": "read" },
    ]);
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank", "access": rules }),
        ))
        .await;
    let share = created.body["id"].as_i64().unwrap();
    let files = format!("/api/v1/files/shares/{share}");

    let mine = h.send(empty(with_cookie(get("/api/v1/files/shares"), &carol))).await;
    assert_eq!(
        (mine.body[0]["name"].clone(), mine.body[0]["access"].clone()),
        (json!("photos"), json!("read"))
    );
    let admins = h.send(empty(with_cookie(get("/api/v1/files/shares"), &admin))).await;
    assert_eq!(admins.body, json!([]), "administrators need a rule too");

    let folder = h.send(empty(with_cookie(get(&format!("{files}/folder?path=2024")), &carol))).await;
    assert_eq!(folder.status, StatusCode::OK, "{:?}", folder.body);
    assert_eq!(
        (folder.body["path"].clone(), folder.body["entries"][0]["name"].clone()),
        (json!("2024"), json!("a.jpg"))
    );
    let foreign = h.send(empty(with_cookie(get(&format!("{files}/folder")), &admin))).await;
    assert_problem(&foreign, StatusCode::NOT_FOUND, "not_found");
    let escape = h.send(empty(with_cookie(get(&format!("{files}/folder?path=../x")), &carol))).await;
    assert_problem(&escape, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let upload = |session: &Session, name: &str| {
        let request = with_csrf(post(&format!("{files}/upload?path=2024&name={name}")), session)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from("bytes"))
            .unwrap();
        h.send(request)
    };
    assert_problem(&upload(&carol, "b.jpg").await, StatusCode::FORBIDDEN, "permission_denied");
    let stored = upload(&bob, "b.jpg").await;
    assert_eq!((stored.status, stored.body["path"].clone()), (StatusCode::CREATED, json!("2024/b.jpg")));
    assert_problem(&upload(&bob, "taken.jpg").await, StatusCode::CONFLICT, "name_taken");

    let download =
        h.send(empty(with_cookie(get(&format!("{files}/download?path=2024/a.jpg")), &carol))).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(
        download.header(header::CONTENT_DISPOSITION),
        "attachment; filename=\"a.jpg\"; filename*=UTF-8''a.jpg"
    );
    assert_eq!(download.header(header::CONTENT_TYPE), "application/octet-stream");
    assert_eq!(download.header(header::ACCEPT_RANGES), "bytes");
    assert_eq!(download.body, "abcdefghij");

    let created =
        h.send(json(with_csrf(post(&format!("{files}/folders")), &bob), json!({ "path": "2025" }))).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let renamed = h
        .send(json(
            with_csrf(post(&format!("{files}/rename")), &bob),
            json!({ "path": "2024/b.jpg", "name": "c.jpg" }),
        ))
        .await;
    assert_eq!(renamed.body["path"], "2024/c.jpg");
    let restored = h
        .send(json(
            with_csrf(post(&format!("{files}/restore")), &bob),
            json!({ "paths": [".recycle/bob/2024/old.jpg"] }),
        ))
        .await;
    assert_eq!(
        (restored.status, restored.body.clone()),
        (StatusCode::OK, json!({ "paths": ["2024/old.jpg"] }))
    );
    let not_yours = h
        .send(json(
            with_csrf(post(&format!("{files}/restore")), &carol),
            json!({ "paths": [".recycle/carol/x"] }),
        ))
        .await;
    assert_problem(&not_yours, StatusCode::FORBIDDEN, "permission_denied");
    let copy =
        json!({ "mode": "copy", "paths": ["2024/c.jpg"], "target_share": share, "target_path": "2025" });
    let copied = h.send(json(with_csrf(post(&format!("{files}/transfer")), &bob), copy)).await;
    assert_eq!((copied.status, copied.body["kind"].clone()), (StatusCode::ACCEPTED, json!("files.copy")));
    job_done(&h, copied.body["id"].as_i64().unwrap()).await;
    let deleted = h
        .send(json(with_csrf(post(&format!("{files}/delete")), &bob), json!({ "paths": ["2024/c.jpg"] })))
        .await;
    assert_eq!(deleted.status, StatusCode::ACCEPTED);
    job_done(&h, deleted.body["id"].as_i64().unwrap()).await;

    assert_eq!(
        h.file_host.calls(),
        [
            "list carol photos:2024",
            "create_upload bob photos:2024",
            "move bob photos:2024/.zendiloos-upload-1 -> photos:2024/b.jpg",
            "create_upload bob photos:2024",
            "move bob photos:2024/.zendiloos-upload-1 -> photos:2024/taken.jpg",
            "discard bob photos:2024/.zendiloos-upload-1",
            "open carol photos:2024/a.jpg",
            "mkdir bob photos:2025",
            "move bob photos:2024/b.jpg -> photos:2024/c.jpg",
            "restore bob photos:.recycle/bob/2024/old.jpg",
            "copy bob photos:2024/c.jpg -> photos:2025/c.jpg",
            "remove bob photos:2024/c.jpg recycle",
        ]
    );
}

#[tokio::test]
async fn each_account_keeps_its_interface_preferences() {
    let h = Harness::new();
    let alice = h.sign_in("alice", Role::ReadOnly).await;
    let bob = h.sign_in("bob", Role::User).await;

    let none = h.send(empty(with_cookie(get("/api/v1/preferences"), &alice))).await;
    assert_eq!(
        (none.status, none.body.clone()),
        (
            StatusCode::OK,
            json!({ "language": null, "theme": null, "wallpaper": null, "widgets": null, "desktop_icons": null })
        )
    );

    let chosen = json!({
        "language": "de",
        "theme": "dark",
        "wallpaper": "nebula",
        "widgets": ["status", "storage"],
        "desktop_icons": ["files", "storage"]
    });
    let saved =
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/preferences"), &alice), chosen.clone())).await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT, "{:?}", saved.body);

    assert_eq!(h.send(empty(with_cookie(get("/api/v1/preferences"), &alice))).await.body, chosen);
    assert_eq!(h.send(empty(with_cookie(get("/api/v1/preferences"), &bob))).await.body["theme"], json!(null));

    let unknown = json!({ "language": "fr", "theme": "dark" });
    let refused =
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/preferences"), &alice), unknown)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    for bad in [json!({ "wallpaper": "../etc" }), json!({ "widgets": ["status", "status"] })] {
        let refused =
            h.send(json(with_csrf(changing(Method::PUT, "/api/v1/preferences"), &alice), bad)).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn every_account_sees_how_the_machine_is_connected() {
    let h = Harness::new();
    let session = h.sign_in("bob", Role::ReadOnly).await;

    let reply = h.send(empty(with_cookie(get("/api/v1/network"), &session))).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(reply.body["interfaces"][0]["name"], "enp1s0");
    assert_eq!(reply.body["interfaces"][0]["kind"], "ethernet");
    assert_eq!(
        reply.body["interfaces"][0]["addresses"][0],
        json!({ "address": "192.168.1.20", "prefix": 24, "scope": "global", "dynamic": true })
    );
    assert_eq!(
        reply.body["default_routes"][0],
        json!({ "gateway": "192.168.1.1", "interface": "enp1s0", "metric": 100 })
    );
    assert_eq!(reply.body["name_servers"], json!(["192.168.1.1"]));
}

#[tokio::test]
async fn no_path_leads_out_of_a_share_and_new_names_are_safe() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::User).await;
    let bob_id = h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "bob").unwrap().id.get();
    let rules = json!([{ "kind": "account", "id": bob_id, "access": "read_write" }]);
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank", "access": rules }),
        ))
        .await;
    let files = format!("/api/v1/files/shares/{}", created.body["id"]);

    // Cases from storix's tests, percent-encoded ones included: the query
    // is decoded first, so they are refused as the plain ones are.
    for path in [
        "..",
        "../outside",
        "../../etc",
        "2024/../../outside",
        "%2e%2e%2foutside",
        "%2E%2E/outside",
        "..%2Foutside",
        "2024/%2e%2e/%2e%2e",
        "a%00b",
    ] {
        for operation in ["folder", "download"] {
            let reply =
                h.send(empty(with_cookie(get(&format!("{files}/{operation}?path={path}")), &bob))).await;
            assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
        }
    }
    for body in [json!({ "path": ".." }), json!({ "path": "../outside" }), json!({ "path": "" })] {
        let reply = h.send(json(with_csrf(post(&format!("{files}/folders")), &bob), body.clone())).await;
        assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }

    // New names: nothing Windows could not reach, nothing invisible. A
    // folder path may have several names; a new name is one.
    for name in ["trail ", " lead", "dot.", "line\nbreak", "../x", "a/b"] {
        let folder =
            h.send(json(with_csrf(post(&format!("{files}/folders")), &bob), json!({ "path": name })));
        if name != "a/b" {
            assert_problem(&folder.await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
        }
        let body = json!({ "path": "2024/a.jpg", "name": name });
        let rename = h.send(json(with_csrf(post(&format!("{files}/rename")), &bob), body)).await;
        assert_problem(&rename, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let upload = with_csrf(post(&format!("{files}/upload?path=2024&name=dot.")), &bob)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from("bytes"))
        .unwrap();
    assert_problem(&h.send(upload).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    // Nothing of it reached the files agent.
    assert_eq!(h.file_host.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn administrators_stop_a_running_scrub() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let stop =
        |session: &Session| h.send(empty(with_csrf(post("/api/v1/storage/pools/tank/scrub/stop"), session)));

    assert_problem(&stop(&admin).await, StatusCode::CONFLICT, "not_scrubbing");
    let started = h.send(empty(with_csrf(post("/api/v1/storage/pools/tank/scrub"), &admin))).await;
    assert_eq!(started.status, StatusCode::NO_CONTENT);
    let pools = h.send(empty(with_cookie(get("/api/v1/storage/pools"), &viewer))).await;
    assert_eq!(
        (pools.body[0]["scan"]["kind"].clone(), pools.body[0]["scan"]["state"].clone()),
        (json!("scrub"), json!("running"))
    );
    assert_eq!(pools.body[0]["scan"]["paused"], false);

    assert_problem(&stop(&viewer).await, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(stop(&admin).await.status, StatusCode::NO_CONTENT);
    let pools = h.send(empty(with_cookie(get("/api/v1/storage/pools"), &viewer))).await;
    assert_eq!(pools.body[0]["scan"]["state"], "canceled");
}

#[tokio::test]
async fn charts_start_with_the_last_minutes() {
    let h = Harness::new();
    let session = h.sign_in("bob", Role::ReadOnly).await;

    let reply = h.send(empty(with_cookie(get("/api/v1/system/metrics/history"), &session))).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(reply.body["at"], json!([98, 100]));
    assert_eq!(reply.body["cpu_percent"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn administrators_take_schedule_and_delete_snapshots_of_a_share() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::User).await;
    let bob_id = h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "bob").unwrap().id.get();
    let rules = json!([{ "kind": "account", "id": bob_id, "access": "read_write" }]);
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank", "access": rules }),
        ))
        .await;
    let base = format!("/api/v1/shares/{}/snapshots", created.body["id"]);

    let taken = h.send(json(with_csrf(post(&base), &admin), json!({ "note": "before the move" }))).await;
    assert_eq!(taken.status, StatusCode::CREATED, "{:?}", taken.body);
    let label = taken.body["label"].as_str().unwrap().to_owned();
    assert!(label.starts_with("zdl-"), "{label}");
    assert_eq!(taken.body["schedules"], json!([]));

    let schedules = json!([{ "frequency": "daily", "keep": 7 }, { "frequency": "hourly", "keep": 24 }]);
    let saved =
        h.send(json(with_csrf(changing(Method::PUT, &format!("{base}/schedules")), &admin), schedules)).await;
    assert_eq!(
        saved.body,
        json!([{ "frequency": "hourly", "keep": 24 }, { "frequency": "daily", "keep": 7 }])
    );
    let refused = h
        .send(json(
            with_csrf(changing(Method::PUT, &format!("{base}/schedules")), &admin),
            json!([{ "frequency": "daily", "keep": 0 }]),
        ))
        .await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let listed = h.send(empty(with_cookie(get(&base), &admin))).await;
    assert_eq!(
        (listed.body["snapshots"][0]["note"].clone(), listed.body["schedules"].as_array().unwrap().len()),
        (json!("before the move"), 2)
    );
    assert_problem(&h.send(empty(with_cookie(get(&base), &bob))).await, StatusCode::FORBIDDEN, "forbidden");

    let missing =
        h.send(empty(with_csrf(post(&format!("{base}/zdl-2020-01-01_00.00.00/delete")), &admin))).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");
    let deleted = h.send(empty(with_csrf(post(&format!("{base}/{label}/delete")), &admin))).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(h.send(empty(with_cookie(get(&base), &admin))).await.body["snapshots"], json!([]));
}

#[tokio::test]
async fn users_see_their_own_jobs_and_no_storage_notifications() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let bob = h.sign_in("bob", Role::User).await;
    let carol = h.sign_in("carol", Role::User).await;
    h.notifier.notify(NewNotification::new(Level::Warning, "pool_trouble").with("pool", "tank")).await;
    let id_of = |name: &str| {
        h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == name).unwrap().id.get()
    };
    let rules = json!([
        { "kind": "account", "id": id_of("bob"), "access": "read_write" },
        { "kind": "account", "id": id_of("carol"), "access": "read_write" },
    ]);
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank", "access": rules }),
        ))
        .await;
    let files = format!("/api/v1/files/shares/{}", created.body["id"]);
    let deleted =
        h.send(json(with_csrf(post(&format!("{files}/delete")), &bob), json!({ "paths": ["2024"] }))).await;
    let job = deleted.body["id"].as_i64().unwrap();
    job_done(&h, job).await;

    let mine = h.send(empty(with_cookie(get("/api/v1/jobs"), &bob))).await;
    assert_eq!(
        mine.body.as_array().unwrap().iter().map(|j| j["id"].as_i64().unwrap()).collect::<Vec<_>>(),
        [job]
    );
    assert_eq!(h.send(empty(with_cookie(get("/api/v1/jobs"), &carol))).await.body, json!([]));
    let foreign = h.send(empty(with_cookie(get(&format!("/api/v1/jobs/{job}")), &carol))).await;
    assert_problem(&foreign, StatusCode::NOT_FOUND, "not_found");
    assert!(
        !h.send(empty(with_cookie(get("/api/v1/jobs"), &admin))).await.body.as_array().unwrap().is_empty()
    );

    let notices = h.send(empty(with_cookie(get("/api/v1/notifications"), &bob))).await;
    assert_problem(&notices, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(h.send(empty(with_cookie(get("/api/v1/notifications"), &admin))).await.status, StatusCode::OK);
}

#[tokio::test]
async fn shares_have_a_quota_compression_and_record_size_and_show_their_usage() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let gib: u64 = 1 << 30;
    let settings = json!({ "quota_bytes": 5 * gib, "compression": "zstd", "record_size_bytes": 1_048_576 });
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "media", "pool": "tank", "settings": settings }),
        ))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["settings"], settings);

    let listed = h.send(empty(with_cookie(get("/api/v1/shares"), &admin))).await;
    assert_eq!(listed.body[0]["settings"]["compression"], "zstd");
    assert_eq!(listed.body[0]["used_bytes"], 0);

    let path = format!("/api/v1/shares/{}", created.body["id"]);
    let odd = json!({ "settings": { "quota_bytes": null, "compression": "lz4", "record_size_bytes": 4096 } });
    let refused = h.send(json(with_csrf(changing(Method::PATCH, &path), &admin), odd)).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let tiny =
        json!({ "settings": { "quota_bytes": 1024, "compression": "lz4", "record_size_bytes": 131_072 } });
    let refused = h.send(json(with_csrf(changing(Method::PATCH, &path), &admin), tiny)).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let none =
        json!({ "settings": { "quota_bytes": null, "compression": "lz4", "record_size_bytes": 131_072 } });
    let changed = h.send(json(with_csrf(changing(Method::PATCH, &path), &admin), none.clone())).await;
    assert_eq!(changed.body["share"]["settings"], none["settings"], "{:?}", changed.body);
}

#[tokio::test]
async fn administrators_have_disks_test_themselves() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let start = |session: &Session, disk: &str, kind: &str| {
        h.send(json(
            with_csrf(post(&format!("/api/v1/storage/disks/{disk}/self-test")), session),
            json!({ "kind": kind }),
        ))
    };

    assert_eq!(start(&admin, "sda", "long").await.status, StatusCode::NO_CONTENT);
    assert_problem(&start(&viewer, "sda", "short").await, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(&start(&admin, "sdb", "short").await, StatusCode::CONFLICT, "refused");
    assert_problem(&start(&admin, "sdz", "short").await, StatusCode::NOT_FOUND, "not_found");
    let stop = h.send(empty(with_csrf(post("/api/v1/storage/disks/sda/self-test/abort"), &admin))).await;
    assert_eq!(stop.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn administrators_plan_self_tests_everyone_with_storage_view_sees_them() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let plans = json!({ "plans": [
        { "kind": "long", "frequency": "monthly", "hour": 3, "disks": ["T1"] },
        { "kind": "short", "frequency": "daily", "hour": 2 },
    ] });
    let put = |session: &Session, body: serde_json::Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/storage/self-test-plans"), session), body))
    };

    let saved = put(&admin, plans.clone()).await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.body["plans"][0]["kind"], "short");
    assert_eq!(saved.body["plans"][0]["disks"], json!([]));
    assert_eq!(saved.body["plans"][1]["disks"], json!(["T1"]));
    assert_eq!(saved.body["plans"][1]["last_run_at"], serde_json::Value::Null);
    assert!(saved.body["utc_offset_seconds"].is_i64());

    let seen = h.send(empty(with_cookie(get("/api/v1/storage/self-test-plans"), &viewer))).await;
    assert_eq!(seen.status, StatusCode::OK);
    assert_eq!(seen.body["plans"], saved.body["plans"]);
    assert_problem(&put(&viewer, plans).await, StatusCode::FORBIDDEN, "forbidden");
    let nightly_long = json!({ "plans": [{ "kind": "long", "frequency": "daily", "hour": 2 }] });
    assert_problem(&put(&admin, nightly_long).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn downloads_resume_and_harmless_kinds_show_in_the_browser() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let carol = h.sign_in("carol", Role::User).await;
    let carol_id =
        h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == "carol").unwrap().id.get();
    let created = h
        .send(json(
            with_csrf(post("/api/v1/shares"), &admin),
            json!({ "name": "photos", "pool": "tank", "access": [{ "kind": "account", "id": carol_id, "access": "read" }] }),
        ))
        .await;
    let files = format!("/api/v1/files/shares/{}", created.body["id"]);
    let fetch = |query: &str, headers: &[(header::HeaderName, &str)]| {
        let mut request = with_cookie(get(&format!("{files}/download?{query}")), &carol);
        for (name, value) in headers {
            request = request.header(name, *value);
        }
        h.send(empty(request))
    };

    let whole = fetch("path=a.jpg", &[]).await;
    let modified = whole.header(header::LAST_MODIFIED).to_owned();
    let part = fetch("path=a.jpg", &[(header::RANGE, "bytes=4-")]).await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.header(header::CONTENT_RANGE), "bytes 4-9/10");
    assert_eq!(part.header(header::CONTENT_LENGTH), "6");
    assert_eq!(part.body, "efghij");
    let resumed = fetch("path=a.jpg", &[(header::RANGE, "bytes=-3"), (header::IF_RANGE, &modified)]).await;
    assert_eq!((resumed.status, resumed.body.clone()), (StatusCode::PARTIAL_CONTENT, json!("hij")));
    let changed = fetch(
        "path=a.jpg",
        &[(header::RANGE, "bytes=4-"), (header::IF_RANGE, "Thu, 01 Jan 1970 00:00:00 GMT")],
    )
    .await;
    assert_eq!(
        (changed.status, changed.body.clone()),
        (StatusCode::OK, json!("abcdefghij")),
        "changed since"
    );
    let beyond = fetch("path=a.jpg", &[(header::RANGE, "bytes=10-")]).await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(beyond.header(header::CONTENT_RANGE), "bytes */10");

    let folder = h.send(empty(with_cookie(get(&format!("{files}/archive?path=2024")), &carol))).await;
    assert_eq!(folder.status, StatusCode::OK);
    assert_eq!(folder.header(header::CONTENT_TYPE), "application/x-tar");
    assert_eq!(
        folder.header(header::CONTENT_DISPOSITION),
        "attachment; filename=\"2024.tar\"; filename*=UTF-8''2024.tar"
    );
    assert_eq!(folder.header(header::CONTENT_LENGTH), "10");

    let share_id = created.body["id"].as_i64().unwrap();
    let put_favourites = |body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/files/favourites"), &carol), body))
    };
    let saved = put_favourites(json!({ "favourites": [{ "share_id": share_id, "path": "2024" }] })).await;
    assert_eq!(saved.body, json!([{ "share_id": share_id, "share_name": "photos", "path": "2024" }]));
    let listed = h.send(empty(with_cookie(get("/api/v1/files/favourites"), &carol))).await;
    assert_eq!(listed.body, saved.body);
    let foreign = put_favourites(json!({ "favourites": [{ "share_id": share_id + 1 }] })).await;
    assert_problem(&foreign, StatusCode::NOT_FOUND, "not_found");

    let picture = fetch("path=a.jpg&inline=true", &[]).await;
    assert_eq!(picture.header(header::CONTENT_TYPE), "image/jpeg");
    assert!(picture.header(header::CONTENT_DISPOSITION).starts_with("inline;"));
    assert!(picture.header(header::CONTENT_SECURITY_POLICY).starts_with("sandbox"));
    let page = fetch("path=page.html&inline=true", &[]).await;
    assert_eq!(page.header(header::CONTENT_TYPE), "application/octet-stream");
    assert!(page.header(header::CONTENT_DISPOSITION).starts_with("attachment;"), "never shown");
}

#[tokio::test]
async fn administrators_set_up_channels_and_never_see_a_password_again() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let mail = json!({
        "name": "Admins",
        "target": { "kind": "email", "server": "mail.example.org", "port": 587, "security": "starttls",
                    "username": "nas", "from": "nas@example.org", "to": ["me@example.org"] },
        "password": "secret",
        "min_level": "warning",
        "language": "de",
    });
    let send = |method: Method, uri: &str, session: &Session, body: Option<Value>| {
        let request = with_csrf(changing(method, uri), session);
        h.send(match body {
            Some(body) => json(request, body),
            None => empty(request),
        })
    };

    let created = send(Method::POST, "/api/v1/notification-channels", &admin, Some(mail.clone())).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["password_set"], true);
    assert!(created.body.get("password").is_none() && created.body["target"].get("password").is_none());
    let id = created.body["id"].as_i64().unwrap();
    let channel = format!("/api/v1/notification-channels/{id}");

    let mut renamed = mail.clone();
    renamed["name"] = json!("Family");
    renamed.as_object_mut().unwrap().remove("password");
    let updated = send(Method::PUT, &channel, &admin, Some(renamed)).await;
    assert_eq!(
        (updated.body["name"].clone(), updated.body["password_set"].clone()),
        (json!("Family"), json!(true))
    );

    let tested = send(Method::POST, &format!("{channel}/test"), &admin, None).await;
    assert_eq!(tested.status, StatusCode::NO_CONTENT);
    let broken = json!({
        "name": "Chat", "target": { "kind": "webhook", "url": "https://broken.example/hook", "format": "json" },
        "min_level": "error", "language": "en",
    });
    let chat = send(Method::POST, "/api/v1/notification-channels", &admin, Some(broken)).await;
    let failed =
        send(Method::POST, &format!("/api/v1/notification-channels/{}/test", chat.body["id"]), &admin, None)
            .await;
    assert_problem(&failed, StatusCode::BAD_GATEWAY, "delivery_failed");
    assert_eq!(failed.body["detail"], "connection refused");

    let listed = h.send(empty(with_cookie(get("/api/v1/notification-channels"), &admin))).await;
    let names: Vec<&str> =
        listed.body.as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Chat", "Family"]);
    assert_eq!(listed.body[0]["last_error"], "connection refused");

    let plain = json!({
        "name": "Relay",
        "target": { "kind": "email", "server": "relay", "port": 25, "security": "none",
                    "from": "nas@example.org", "to": ["me@example.org"] },
        "password": "secret", "min_level": "info", "language": "en",
    });
    let refused = send(Method::POST, "/api/v1/notification-channels", &admin, Some(plain)).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let forbidden = h.send(empty(with_cookie(get("/api/v1/notification-channels"), &viewer))).await;
    assert_problem(&forbidden, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(send(Method::DELETE, &channel, &admin, None).await.status, StatusCode::NO_CONTENT);
    assert_problem(&send(Method::DELETE, &channel, &admin, None).await, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn administrators_change_an_interface_and_confirm_it() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let fixed = json!({
        "ipv4": { "mode": "static", "address": "192.168.1.10", "prefix": 24, "gateway": "192.168.1.1" },
        "ipv6": "automatic",
        "dns": ["192.168.1.1"],
    });
    let put = |session: &Session, body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/network/interfaces/enp1s0"), session), body))
    };

    assert_problem(&put(&viewer, fixed.clone()).await, StatusCode::FORBIDDEN, "forbidden");
    let mut wrong = fixed.clone();
    wrong["ipv4"]["gateway"] = json!("10.0.0.1");
    assert_problem(&put(&admin, wrong).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let applied = put(&admin, fixed.clone()).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert_eq!(applied.body["pending"]["interface"], "enp1s0");
    let seen = h.send(empty(with_cookie(get("/api/v1/network/setup"), &viewer))).await;
    assert_eq!(
        (seen.body["managed"].clone(), seen.body["pending"]["interface"].clone()),
        (json!(true), json!("enp1s0"))
    );
    assert_problem(&put(&admin, fixed).await, StatusCode::CONFLICT, "change_pending");

    let confirm = |session: &Session| h.send(empty(with_csrf(post("/api/v1/network/confirm"), session)));
    assert_problem(&confirm(&viewer).await, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(confirm(&admin).await.status, StatusCode::NO_CONTENT);
    assert_problem(&confirm(&admin).await, StatusCode::CONFLICT, "refused");
}

#[tokio::test]
async fn administrators_browse_catalogs_and_add_their_own() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    let send = |method: Method, uri: &str, body: Option<Value>| {
        let request = with_csrf(changing(method, uri), &admin);
        h.send(match body {
            Some(body) => json(request, body),
            None => empty(request),
        })
    };

    let refreshed = send(Method::POST, "/api/v1/apps/sources/refresh", None).await;
    assert_eq!(refreshed.body[0]["official"], true);
    let listed = h.send(empty(with_cookie(get("/api/v1/apps/catalog"), &admin))).await;
    let names: Vec<&str> =
        listed.body.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Immich", "Jellyfin"]);
    let source = listed.body[0]["source_id"].as_i64().unwrap();
    let forbidden = h.send(empty(with_cookie(get("/api/v1/apps/catalog"), &user))).await;
    assert_problem(&forbidden, StatusCode::FORBIDDEN, "forbidden");

    let one =
        h.send(empty(with_cookie(get(&format!("/api/v1/apps/catalog/{source}/jellyfin")), &admin))).await;
    assert_eq!(
        (one.body["id"].clone(), one.body["ports"][0]["default"].clone()),
        (json!("jellyfin"), json!(8096))
    );
    let icon = h
        .send(empty(with_cookie(get(&format!("/api/v1/apps/catalog/{source}/jellyfin/icon")), &admin)))
        .await;
    assert_eq!(icon.header(header::CONTENT_TYPE), "image/png");
    let missing =
        h.send(empty(with_cookie(get(&format!("/api/v1/apps/catalog/{source}/nothing")), &admin))).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");

    h.catalogs.serve(
        "https://friend.github.io/apps/v1/index.json",
        vec![zendiloos_apps::testing::app("paperless", "Paperless")],
    );
    let added =
        send(Method::POST, "/api/v1/apps/sources", Some(json!({ "address": "github.com/friend/apps" })))
            .await;
    assert_eq!(added.status, StatusCode::CREATED, "{:?}", added.body);
    assert_eq!(
        (added.body["official"].clone(), added.body["address"].clone()),
        (json!(false), json!("https://friend.github.io/apps/v1/index.json"))
    );
    let twice =
        send(Method::POST, "/api/v1/apps/sources", Some(json!({ "address": "github.com/friend/apps" })))
            .await;
    assert_problem(&twice, StatusCode::CONFLICT, "source_exists");
    let unreachable = send(
        Method::POST,
        "/api/v1/apps/sources",
        Some(json!({ "address": "https://nowhere.example/index.json" })),
    )
    .await;
    assert_problem(&unreachable, StatusCode::BAD_GATEWAY, "catalog_unreachable");
    let plain = send(
        Method::POST,
        "/api/v1/apps/sources",
        Some(json!({ "address": "http://nowhere.example/index.json" })),
    )
    .await;
    assert_problem(&plain, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let official = send(Method::DELETE, &format!("/api/v1/apps/sources/{source}"), None).await;
    assert_problem(&official, StatusCode::CONFLICT, "official_source");
    let removed = send(Method::DELETE, &format!("/api/v1/apps/sources/{}", added.body["id"]), None).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn administrators_put_dockers_data_on_a_pool() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let before = h.send(empty(with_cookie(get("/api/v1/apps/docker"), &admin))).await;
    assert_eq!((before.body["installed"].clone(), before.body["volume"].clone()), (json!(true), Value::Null));
    assert_eq!(before.body["volumes"][0]["pool"], "tank");

    let enable =
        |pool: &str| h.send(json(with_csrf(post("/api/v1/apps/docker"), &admin), json!({ "pool": pool })));
    assert_problem(&enable("nowhere").await, StatusCode::NOT_FOUND, "not_found");
    let after = enable("tank").await;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(
        (after.body["running"].clone(), after.body["volume"]["number"].clone()),
        (json!(true), json!(1))
    );
}

#[tokio::test]
async fn administrators_make_projects_from_apps_of_a_catalog() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    let send = |method: Method, uri: &str, body: Option<Value>| {
        let request = with_csrf(Request::builder().method(method).uri(uri), &admin);
        h.send(match body {
            Some(body) => json(request, body),
            None => empty(request),
        })
    };
    send(Method::POST, "/api/v1/apps/sources/refresh", None).await;
    let installed = send(Method::POST, "/api/v1/apps/docker/install", Some(json!({ "pool": "tank" }))).await;
    assert_problem(&installed, StatusCode::CONFLICT, "docker_installed");
    let fill = |body: Value| send(Method::POST, "/api/v1/apps/catalog/1/jellyfin/compose", Some(body));

    let reserved = fill(json!({ "ports": { "web": 445 } })).await;
    assert_problem(&reserved, StatusCode::UNPROCESSABLE_ENTITY, "invalid_app_value");
    assert_eq!(reserved.body["detail"], "ports.web: port 445 is used by zendiloOS");
    let unknown = send(Method::POST, "/api/v1/apps/catalog/1/plex/compose", Some(json!({}))).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let filled = fill(json!({ "ports": { "web": 8200 } })).await;
    assert_eq!(filled.status, StatusCode::OK, "{:?}", filled.body);
    let compose = filled.body["compose"].as_str().unwrap().to_owned();
    assert!(compose.starts_with("# Jellyfin, filled in by the zendiloOS App Center"), "{compose}");
    assert!(compose.contains("image: example/jellyfin:1@sha256:00"), "{compose}");

    // The file becomes a project like any other, naming the app.
    let made = send(
        Method::POST,
        "/api/v1/docker/projects",
        Some(json!({
            "name": "media", "share_id": 1, "folder": "docker/media", "compose": compose,
            "start": true, "template": { "source_id": 1, "app": "jellyfin" }
        })),
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["job"]["kind"], "containers.project_deploy");
    let project = h.send(empty(with_cookie(get("/api/v1/docker/projects/media"), &admin))).await;
    assert_eq!(project.body["kind"], "own");
    assert_eq!(project.body["template"], json!({ "source_id": 1, "app": "jellyfin" }));

    let projects = h.send(empty(with_cookie(get("/api/v1/apps/projects"), &admin))).await;
    assert_eq!(projects.status, StatusCode::OK, "{:?}", projects.body);
    let listed = &projects.body[0];
    assert_eq!((listed["project"].clone(), listed["id"].clone()), (json!("media"), json!("jellyfin")));
    assert_eq!(
        (listed["name"].clone(), listed["digest"].clone()),
        (json!("Jellyfin"), json!("sha256:jellyfin"))
    );
    let forbidden = h.send(empty(with_cookie(get("/api/v1/apps/projects"), &user))).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let refused =
        h.send(json(with_csrf(post("/api/v1/apps/catalog/1/jellyfin/compose"), &user), json!({}))).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn apps_installed_the_earlier_way_are_run_and_removed() {
    use zendiloos_apps::application::ports::InstalledRepository;

    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    let send = |method: Method, uri: &str, body: Option<Value>| {
        let request = with_csrf(Request::builder().method(method).uri(uri), &admin);
        h.send(match body {
            Some(body) => json(request, body),
            None => empty(request),
        })
    };
    let mut values = zendiloos_apps::domain::InstallValues::default();
    values.ports.insert("web".into(), 8200);
    let at = zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000);
    let jellyfin = zendiloos_apps::domain::InstalledApp {
        id: "jellyfin".into(),
        source_id: 1,
        app: zendiloos_apps::testing::app("jellyfin", "Jellyfin"),
        pool: "tank".into(),
        volume: 1,
        values,
        state: zendiloos_apps::domain::InstallState::Installed,
        installed_at: at,
        updated_at: at,
    };
    assert!(h.installed.create(&jellyfin).await.unwrap());

    assert_eq!(
        send(Method::POST, "/api/v1/apps/installed/jellyfin/start", None).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.app_host.calls().last().unwrap(), "start jellyfin");
    let listed = h.send(empty(with_cookie(get("/api/v1/apps/installed"), &admin))).await;
    assert_eq!(listed.body[0]["id"], "jellyfin");
    assert_eq!(listed.body[0]["state"], "installed");
    assert_eq!(listed.body[0]["run_state"], "running");
    assert_eq!(listed.body[0]["ports"]["web"], 8200);
    let forbidden = h.send(empty(with_cookie(get("/api/v1/apps/installed"), &user))).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    assert_eq!(
        send(Method::POST, "/api/v1/apps/installed/jellyfin/stop", None).await.status,
        StatusCode::NO_CONTENT
    );
    let stopped = h.send(empty(with_cookie(get("/api/v1/apps/installed/jellyfin"), &admin))).await;
    assert_eq!(stopped.body["run_state"], "stopped");
    let logs = h.send(empty(with_cookie(get("/api/v1/apps/installed/jellyfin/logs?lines=50"), &admin))).await;
    assert_eq!(logs.body["text"], "jellyfin: 50 lines\n");
    let gone =
        send(Method::POST, "/api/v1/apps/installed", Some(json!({ "source": 1, "app": "jellyfin" }))).await;
    assert_eq!(gone.status, StatusCode::METHOD_NOT_ALLOWED, "no longer installed so");

    let wrong =
        send(Method::DELETE, "/api/v1/apps/installed/jellyfin", Some(json!({ "confirm": "Jellyfin" }))).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");
    let removing =
        send(Method::DELETE, "/api/v1/apps/installed/jellyfin", Some(json!({ "confirm": "jellyfin" }))).await;
    assert_eq!(removing.status, StatusCode::ACCEPTED);
    job_done(&h, removing.body["id"].as_i64().unwrap()).await;
    let gone = h.send(empty(with_cookie(get("/api/v1/apps/installed/jellyfin"), &admin))).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn administrators_look_for_updates_and_install_them() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    let send = |method: Method, uri: &str, body: Option<Value>, session: &Session| {
        let request = with_csrf(Request::builder().method(method).uri(uri), session);
        h.send(match body {
            Some(body) => json(request, body),
            None => empty(request),
        })
    };
    let denied = send(Method::GET, "/api/v1/system/updates", None, &user).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let before = send(Method::GET, "/api/v1/system/updates", None, &admin).await;
    assert_eq!(before.status, StatusCode::OK, "{:?}", before.body);
    assert_eq!(before.body["current"], "0.1.0");
    assert_eq!(before.body["settings"]["channel"], "stable");
    assert_eq!(before.body["checked_at"], Value::Null);
    let early = send(Method::POST, "/api/v1/system/updates/apply", None, &admin).await;
    assert_problem(&early, StatusCode::CONFLICT, "not_checked");

    let check = send(Method::POST, "/api/v1/system/updates/check", None, &admin).await;
    assert_eq!(check.status, StatusCode::ACCEPTED);
    let mut found = before;
    for _ in 0..100 {
        found = send(Method::GET, "/api/v1/system/updates", None, &admin).await;
        if found.body["phase"] == "idle" && found.body["checked_at"] != Value::Null {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(found.body["release"]["version"], "0.2.0");
    assert_eq!(found.body["packages"].as_array().unwrap().len(), 2);
    assert_eq!(found.body["packages"][0]["security"], true);

    let apply = send(Method::POST, "/api/v1/system/updates/apply", None, &admin).await;
    assert_eq!(apply.status, StatusCode::ACCEPTED, "{:?}", apply.body);
    let mut after = found;
    for _ in 0..100 {
        after = send(Method::GET, "/api/v1/system/updates", None, &admin).await;
        if after.body["run"] != Value::Null {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(after.body["run"]["state"], "succeeded");
    assert_eq!(after.body["run"]["zendiloos"], "0.2.0");
    assert_eq!(after.body["run"]["restart_needed"], true);

    let settings = send(
        Method::PUT,
        "/api/v1/system/updates/settings",
        Some(json!({ "channel": "development", "check_daily": false })),
        &admin,
    )
    .await;
    assert_eq!(settings.status, StatusCode::OK, "{:?}", settings.body);
    let saved = send(Method::GET, "/api/v1/system/updates", None, &admin).await;
    assert_eq!(saved.body["settings"]["channel"], "development");
    assert_eq!(saved.body["release"], Value::Null, "what was found was for the other channel");
}
