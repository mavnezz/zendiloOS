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
use zendiloos_kernel::{Outcome, PortError, RequestContext};
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
        let storage = StorageUseCases::new(FakeStorage::ports());
        let use_cases = UseCases { system, identity: identity.use_cases.clone(), storage };
        let state = ApiState::new(use_cases, secure_cookies);
        Self { app: router(state), identity, power, journal }
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
            "/api/v1/groups",
            "/api/v1/groups/{id}",
            "/api/v1/health",
            "/api/v1/logs/audit",
            "/api/v1/logs/journal",
            "/api/v1/setup",
            "/api/v1/storage/arrays",
            "/api/v1/storage/disks",
            "/api/v1/storage/disks/{name}/smart",
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
            (json!("nvme0n1"), json!("system")),
        ]
    );
    assert_eq!(disks.body[0]["array"], "md0");
    assert_eq!(disks.body[0]["transport"], "sata");
    assert_eq!(disks.body[2]["media"], "solid_state");

    let arrays = read("/api/v1/storage/arrays").await;
    assert_eq!(arrays.body[0]["health"], "healthy");
    assert_eq!(arrays.body[0]["devices_working"], 2);
    assert_eq!(arrays.body[0]["members"][1], json!({ "device": "sdb", "index": 1, "state": "active" }));

    let volumes = read("/api/v1/storage/volumes").await;
    assert_eq!(volumes.body.as_array().unwrap().len(), 1);
    assert_eq!(volumes.body[0]["label"], "data");
    assert_eq!(volumes.body[0]["disks"], json!(["sda", "sdb"]));
    assert_eq!(volumes.body[0]["used_bytes"], 1_u64 << 40);
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
