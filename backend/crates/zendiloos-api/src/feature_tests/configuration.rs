//! The configuration backup through the API: downloaded with a passphrase,
//! planned with the passphrase never sent back, run now, restored with its
//! checks (passphrase, layout, a newer schema), also instead of the first
//! setup; for administrators only.

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use base64::Engine as _;
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;
use zendiloos_identity::testing::SETUP_CODE;

use crate::routes::configuration::{PASSPHRASE_HEADER, SETUP_CODE_HEADER};
use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, post, with_cookie, with_csrf,
};

const PASSPHRASE: &str = "correct horse battery";

async fn send(h: &Harness, session: &Session, method: Method, uri: &str, body: Value) -> Reply {
    let builder = with_csrf(changing(method, uri), session);
    h.send(if body.is_null() { empty(builder) } else { json(builder, body) }).await
}

fn encoded(passphrase: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(passphrase)
}

fn restore_request(
    session: Option<&Session>,
    uri: &str,
    passphrase: Option<&str>,
    body: &str,
) -> axum::http::Request<Body> {
    let mut builder = match session {
        Some(session) => with_csrf(changing(Method::POST, uri), session),
        None => post(uri),
    };
    builder = builder.header(header::CONTENT_TYPE, "application/octet-stream");
    if let Some(passphrase) = passphrase {
        builder = builder.header(PASSPHRASE_HEADER, encoded(passphrase));
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

fn account_id(h: &Harness, name: &str) -> i64 {
    h.identity.accounts.all().into_iter().find(|a| a.name.as_str() == name).unwrap().id.get()
}

#[tokio::test]
async fn administrators_download_a_backup_encrypted_with_their_passphrase() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let download = |session| {
        send(
            &h,
            session,
            Method::POST,
            "/api/v1/configuration/backup/download",
            json!({ "passphrase": PASSPHRASE }),
        )
    };

    assert_problem(&download(&viewer).await, StatusCode::FORBIDDEN, "forbidden");
    let reply = download(&admin).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(reply.header(header::CONTENT_TYPE), "application/octet-stream");
    let disposition = reply.header(header::CONTENT_DISPOSITION);
    assert!(disposition.starts_with("attachment; filename=\"zendiloos-config-nas-"), "{disposition}");
    assert_eq!(reply.header(header::CACHE_CONTROL), "no-store");
    assert_eq!(reply.body, json!(format!("backup:{PASSPHRASE}")));
    let short = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/configuration/backup/download",
        json!({ "passphrase": "short" }),
    )
    .await;
    assert_problem(&short, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert_eq!(h.recovery.audit.actions()[0].0, "configuration.download");
}

#[tokio::test]
async fn a_plan_is_set_its_passphrase_never_sent_back_and_run_now() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let id = account_id(&h, "alice");
    h.recovery.operators.0.lock().unwrap().insert(id, ("alice".into(), true));

    let state = h.send(empty(with_cookie(get("/api/v1/configuration/backup"), &admin))).await;
    assert_eq!(state.status, StatusCode::OK);
    assert_eq!(
        (state.body["enabled"].clone(), state.body["passphrase_set"].clone()),
        (json!(false), json!(false))
    );
    assert_eq!(
        (state.body["passphrase_min"].clone(), state.body["keep_max"].clone()),
        (json!(12), json!(60))
    );
    let run = send(&h, &admin, Method::POST, "/api/v1/configuration/backup/run", Value::Null).await;
    assert_problem(&run, StatusCode::CONFLICT, "not_planned");

    let plan = json!({
        "enabled": true, "frequency": "weekly", "weekday": 6, "hour": 4, "minute": 30,
        "target": { "kind": "share", "share_id": 3, "folder": "zendiloOS" }, "keep": 7, "passphrase": PASSPHRASE,
    });
    let reply = send(&h, &admin, Method::PUT, "/api/v1/configuration/backup", plan.clone()).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(reply.body["passphrase_set"], true);
    assert!(reply.body.get("passphrase").is_none(), "never sent back");
    assert_eq!(reply.body["account"], "alice");
    assert_eq!(reply.body["target"], json!({ "kind": "share", "share_id": 3, "folder": "zendiloOS" }));
    assert!(reply.body["next_at"].is_i64());
    let mut kept = plan.clone();
    kept["passphrase"] = Value::Null;
    kept["keep"] = json!(3);
    let reply = send(&h, &admin, Method::PUT, "/api/v1/configuration/backup", kept).await;
    assert_eq!((reply.body["passphrase_set"].clone(), reply.body["keep"].clone()), (json!(true), json!(3)));

    for bad in [
        json!({ "enabled": true, "frequency": "weekly", "hour": 4, "minute": 0, "keep": 7 }),
        json!({ "enabled": true, "frequency": "daily", "hour": 24, "minute": 0, "keep": 7 }),
        json!({ "enabled": false, "frequency": "daily", "hour": 3, "minute": 0, "keep": 61 }),
        json!({ "enabled": false, "frequency": "daily", "hour": 3, "minute": 0, "keep": 7,
                "target": { "kind": "drive", "drive": "1A2B-3C4D", "folder": "../x" } }),
    ] {
        let refused = send(&h, &admin, Method::PUT, "/api/v1/configuration/backup", bad).await;
        assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }

    let run = send(&h, &admin, Method::POST, "/api/v1/configuration/backup/run", Value::Null).await;
    assert_eq!(run.status, StatusCode::ACCEPTED, "{:?}", run.body);
    assert_eq!(run.body["kind"], "configuration.backup");
    for _ in 0..400 {
        if !h.recovery.folders.files.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let files = h.recovery.folders.files.lock().unwrap().keys().cloned().collect::<Vec<_>>();
    assert!(files[0].starts_with("share3:zendiloOS/zendiloos-config-nas-"), "{files:?}");
}

#[tokio::test]
async fn a_restore_checks_passphrase_layout_and_schema_then_restarts() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let admin = h.sign_in("alice", Role::Admin).await;
    let restore = |session, passphrase, body: &str| {
        h.send(restore_request(Some(session), "/api/v1/configuration/restore", passphrase, body))
    };

    assert_problem(&restore(&viewer, Some(PASSPHRASE), "backup:x").await, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(
        &restore(&admin, None, "backup:x").await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let wrong = restore(&admin, Some("another passphrase"), &format!("backup:{PASSPHRASE}")).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "wrong_passphrase");
    let newer = restore(&admin, Some(PASSPHRASE), &format!("too-new:{PASSPHRASE}")).await;
    assert_problem(&newer, StatusCode::UNPROCESSABLE_ENTITY, "backup_too_new");
    assert!(newer.body["detail"].as_str().unwrap().contains("update zendiloOS first"), "{:?}", newer.body);
    let other = restore(&admin, Some(PASSPHRASE), &format!("format:{PASSPHRASE}")).await;
    assert_problem(&other, StatusCode::UNPROCESSABLE_ENTITY, "unsupported_backup");
    assert_problem(
        &restore(&admin, Some(PASSPHRASE), "a zip file").await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "not_a_backup",
    );
    let taken = restore(&admin, Some(PASSPHRASE), &format!("taken:{PASSPHRASE}")).await;
    assert_problem(&taken, StatusCode::CONFLICT, "refused");
    assert!(taken.body["detail"].as_str().unwrap().contains("eve's"), "{:?}", taken.body);
    assert_eq!(h.recovery.restart.0.load(std::sync::atomic::Ordering::SeqCst), 0);

    let reply = restore(&admin, Some(PASSPHRASE), &format!("backup:{PASSPHRASE}")).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(reply.body["host_name"], "nas");
    assert_eq!(reply.body["https_port"], 443);
    assert_eq!(reply.body["network"], json!(["05-zendiloos-enp1s0.network"]));
    assert_eq!(h.recovery.restart.0.load(std::sync::atomic::Ordering::SeqCst), 1);

    h.recovery.work.0.lock().unwrap().push("storage.create_pool".into());
    let busy = restore(&admin, Some(PASSPHRASE), &format!("backup:{PASSPHRASE}")).await;
    assert_problem(&busy, StatusCode::CONFLICT, "jobs_running");
}

#[tokio::test]
async fn a_backup_is_restored_instead_of_the_first_setup_as_the_setup_allows() {
    let h = Harness::new();
    h.identity.use_cases.prepare_setup.execute().await.unwrap();
    let setup = |code: Option<&str>| {
        let mut request =
            restore_request(None, "/api/v1/setup/restore", Some(PASSPHRASE), &format!("backup:{PASSPHRASE}"));
        if let Some(code) = code {
            request.headers_mut().insert(SETUP_CODE_HEADER, code.parse().unwrap());
        }
        h.send(request)
    };

    assert_problem(
        &setup(Some("ZZZZ-ZZZZ-ZZZZ")).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_setup_code",
    );
    assert!(h.recovery.restorer.restored.lock().unwrap().is_empty());
    let reply = setup(Some(SETUP_CODE)).await;

    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(h.recovery.restorer.restored.lock().unwrap().len(), 1);
    let audit = h.recovery.restorer.audit.lock().unwrap()[0].clone();
    assert_eq!((audit.action, audit.actor), ("configuration.restore", None));
    assert_eq!(h.recovery.restart.0.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Once an administrator is there, the setup is done: no restore through it.
    h.sign_in("alice", Role::Admin).await;
    assert_problem(&setup(Some(SETUP_CODE)).await, StatusCode::CONFLICT, "setup_completed");
}
