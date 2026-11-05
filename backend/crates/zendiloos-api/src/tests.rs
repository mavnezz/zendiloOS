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
use zendiloos_notify::testing::MemoryNotifications;
use zendiloos_notify::{Level, NewNotification, Notifier};
use zendiloos_sharing::application::{FilesUseCases, SharingPorts, SharingUseCases};
use zendiloos_storage::application::StorageUseCases;
use zendiloos_storage::testing::FakeStorage;
use zendiloos_system::application::ports::{
    AuditReader, JournalReader, LiveMetrics, MetricsSubscription, PowerControl, SystemInfoProvider,
};
use zendiloos_system::application::{SystemPorts, SystemUseCases};
use zendiloos_system::domain::{
    AuditQuery, AuditRecord, JournalCursor, JournalEntry, JournalPage, JournalQuery, LogPriority,
    MemoryUsage, MetricsSample, PowerAction, SystemInfo,
};

use crate::{ApiState, CSRF_HEADER, UseCases, openapi, router};

const HOST: &str = "nas.local";
const PASSWORD: &str = "correct horse battery";

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
            memory_total_bytes: 8 * 1024 * 1024 * 1024,
            uptime_seconds: 3600,
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
struct RecordingPower(Mutex<Vec<PowerAction>>);

#[async_trait]
impl PowerControl for RecordingPower {
    async fn request(&self, action: PowerAction, _: &RequestContext) -> Result<(), PortError> {
        self.0.lock().unwrap().push(action);
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
struct FakeJournal(Mutex<Vec<JournalQuery>>);

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
struct Harness {
    app: Router,
    identity: TestIdentity,
    power: Arc<RecordingPower>,
    journal: Arc<FakeJournal>,
    jobs: Jobs,
    storage: Arc<FakeStorage>,
    notifier: Notifier,
    share_host: Arc<zendiloos_sharing::testing::Host>,
    file_host: Arc<zendiloos_sharing::testing::Files>,
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
        })
    }

    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        let groups = self.1.all().into_iter();
        Ok(groups.filter(|g| g.members.iter().any(|m| m.get() == account)).map(|g| g.id.get()).collect())
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    fn header(&self, name: impl header::AsHeaderName) -> &str {
        self.headers.get(name).expect("header present").to_str().unwrap()
    }
}

/// A signed-in browser.
struct Session {
    /// `name=value`, as sent back in the `Cookie` header.
    cookie: String,
    csrf: String,
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
    fn new() -> Self {
        Self::with(FakeProvider, true)
    }

    fn with(provider: impl SystemInfoProvider + 'static, secure_cookies: bool) -> Self {
        let identity = TestIdentity::new();
        let power = Arc::new(RecordingPower::default());
        let journal = Arc::new(FakeJournal::default());
        let system = SystemUseCases::new(SystemPorts {
            info: Arc::new(provider),
            metrics: Arc::new(FakeLive),
            power: power.clone(),
            journal: journal.clone(),
            audit: identity.audit.clone(),
            audit_reader: Arc::new(FakeAuditReader(identity.audit.clone())),
            clock: identity.clock.clone(),
        });
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), identity.clock.clone());
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
            host: share_host.clone(),
            files: file_host.clone(),
            jobs: jobs.clone(),
            audit: identity.audit.clone(),
            clock: identity.clock.clone(),
        };
        let sharing = SharingUseCases::new(sharing_ports.clone());
        let files = FilesUseCases::new(sharing_ports);
        let use_cases = UseCases {
            system,
            identity: identity.use_cases.clone(),
            storage,
            sharing,
            files,
            jobs: jobs.clone(),
            notifier: notifier.clone(),
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
        }
    }

    async fn send(&self, request: Request<Body>) -> Reply {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        Reply { status, headers, body }
    }

    async fn sign_in(&self, name: &str, role: Role) -> Session {
        self.identity.add_account(name, PASSWORD, role);
        let reply =
            self.send(json(post("/api/v1/auth/login"), json!({ "name": name, "password": PASSWORD }))).await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
        Session::from(&reply)
    }
}

/// A same-origin browser request.
fn request(method: Method, uri: &str) -> request::Builder {
    Request::builder().method(method).uri(uri).header(header::HOST, HOST)
}

fn get(uri: &str) -> request::Builder {
    request(Method::GET, uri)
}

fn post(uri: &str) -> request::Builder {
    request(Method::POST, uri).header(header::ORIGIN, format!("https://{HOST}"))
}

/// A state-changing same-origin request.
fn changing(method: Method, uri: &str) -> request::Builder {
    request(method, uri).header(header::ORIGIN, format!("https://{HOST}"))
}

fn json(builder: request::Builder, body: Value) -> Request<Body> {
    builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
}

fn empty(builder: request::Builder) -> Request<Body> {
    builder.body(Body::empty()).unwrap()
}

fn with_cookie(builder: request::Builder, session: &Session) -> request::Builder {
    builder.header(header::COOKIE, &session.cookie)
}

fn with_csrf(builder: request::Builder, session: &Session) -> request::Builder {
    with_cookie(builder, session).header(CSRF_HEADER, &session.csrf)
}

fn assert_problem(reply: &Reply, status: StatusCode, code: &str) {
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
            "memory_total_bytes": 8_u64 * 1024 * 1024 * 1024,
            "uptime_seconds": 3600,
            "version": env!("CARGO_PKG_VERSION"),
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
    let session = h.sign_in("alice", Role::User).await;
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
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/password",
            "/api/v1/auth/session",
            "/api/v1/files/shares",
            "/api/v1/files/shares/{id}/delete",
            "/api/v1/files/shares/{id}/download",
            "/api/v1/files/shares/{id}/folder",
            "/api/v1/files/shares/{id}/folders",
            "/api/v1/files/shares/{id}/rename",
            "/api/v1/files/shares/{id}/transfer",
            "/api/v1/files/shares/{id}/upload",
            "/api/v1/groups",
            "/api/v1/groups/{id}",
            "/api/v1/health",
            "/api/v1/jobs",
            "/api/v1/jobs/{id}",
            "/api/v1/logs/audit",
            "/api/v1/logs/journal",
            "/api/v1/notifications",
            "/api/v1/notifications/clear",
            "/api/v1/notifications/read",
            "/api/v1/preferences",
            "/api/v1/setup",
            "/api/v1/shares",
            "/api/v1/shares/{id}",
            "/api/v1/shares/{id}/delete",
            "/api/v1/storage/arrays",
            "/api/v1/storage/disks",
            "/api/v1/storage/disks/{name}/smart",
            "/api/v1/storage/pools",
            "/api/v1/storage/pools/plan",
            "/api/v1/storage/pools/{name}/destroy",
            "/api/v1/storage/pools/{name}/replace",
            "/api/v1/storage/pools/{name}/replace/plan",
            "/api/v1/storage/pools/{name}/scrub",
            "/api/v1/storage/volumes",
            "/api/v1/system/info",
            "/api/v1/system/power",
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
    for entry in std::fs::read_dir(routes.join("routes")).unwrap() {
        files.push(entry.unwrap().path());
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
            "remove bob photos:2024/.zendiloos-upload-1",
            "open carol photos:2024/a.jpg",
            "mkdir bob photos:2025",
            "move bob photos:2024/b.jpg -> photos:2024/c.jpg",
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
        (StatusCode::OK, json!({ "language": null, "theme": null }))
    );

    let chosen = json!({ "language": "de", "theme": "dark" });
    let saved =
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/preferences"), &alice), chosen.clone())).await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT, "{:?}", saved.body);

    assert_eq!(h.send(empty(with_cookie(get("/api/v1/preferences"), &alice))).await.body, chosen);
    assert_eq!(h.send(empty(with_cookie(get("/api/v1/preferences"), &bob))).await.body["theme"], json!(null));

    let unknown = json!({ "language": "fr", "theme": "dark" });
    let refused =
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/preferences"), &alice), unknown)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
}
