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
use zendiloos_system::application::ports::{PowerControl, SystemInfoProvider};
use zendiloos_system::application::{SystemPorts, SystemUseCases};
use zendiloos_system::domain::{PowerAction, SystemInfo};

use crate::{ApiState, CSRF_HEADER, openapi, router};

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

/// The API on in-memory ports.
struct Harness {
    app: Router,
    identity: TestIdentity,
    power: Arc<RecordingPower>,
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
        let system = SystemUseCases::new(SystemPorts {
            info: Arc::new(provider),
            power: power.clone(),
            audit: identity.audit.clone(),
            clock: identity.clock.clone(),
        });
        let state = ApiState::new(system, identity.use_cases.clone(), secure_cookies);
        Self { app: router(state), identity, power }
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
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/password",
            "/api/v1/auth/session",
            "/api/v1/health",
            "/api/v1/setup",
            "/api/v1/system/info",
            "/api/v1/system/power",
        ]
    );
}
