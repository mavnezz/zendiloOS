//! The ACME client plugin and the web interface's settings through the
//! API: installing acme.sh, accounts, challenges whose secrets never come
//! back, certificates issued in jobs, and ports changed with confirmation.

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

async fn admin(h: &Harness) -> Session {
    h.sign_in("alice", Role::Admin).await
}

async fn finished(h: &Harness, job: &Value) -> zendiloos_jobs::Job {
    let id = zendiloos_jobs::JobId::new(job["id"].as_i64().unwrap());
    for _ in 0..200 {
        let job = h.jobs.find(id).await.unwrap().unwrap();
        if job.finished_at.is_some() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the job did not finish");
}

async fn send(h: &Harness, session: &Session, method: Method, uri: &str, body: Value) -> Reply {
    let builder = with_csrf(changing(method, uri), session);
    h.send(if body.is_null() { empty(builder) } else { json(builder, body) }).await
}

/// acme.sh installed, an account, an IONOS challenge and a certificate.
async fn certificate(h: &Harness, admin: &Session, domains: Value) -> (i64, i64) {
    let installed = send(h, admin, Method::POST, "/api/v1/certificates/plugin/install", Value::Null).await;
    if installed.status == StatusCode::ACCEPTED {
        finished(h, &installed.body).await;
    }
    let account = send(
        h,
        admin,
        Method::POST,
        "/api/v1/certificates/accounts",
        json!({ "name": "Let's Encrypt", "server": "letsencrypt_staging", "email": "admin@example.org" }),
    )
    .await;
    assert_eq!(account.status, StatusCode::CREATED, "{:?}", account.body);
    let challenge = send(
        h,
        admin,
        Method::POST,
        "/api/v1/certificates/challenges",
        json!({ "name": "IONOS", "method": "dns", "provider": "dns_ionos",
                "values": { "IONOS_PREFIX": "prefix", "IONOS_SECRET": "s3cr3t" } }),
    )
    .await;
    assert_eq!(challenge.status, StatusCode::CREATED, "{:?}", challenge.body);
    let certificate = send(
        h,
        admin,
        Method::POST,
        "/api/v1/certificates",
        json!({ "domains": domains, "account_id": account.body["id"], "challenge_id": challenge.body["id"] }),
    )
    .await;
    assert_eq!(certificate.status, StatusCode::CREATED, "{:?}", certificate.body);
    assert_eq!(
        (certificate.body["key"].as_str(), certificate.body["auto_renew"].as_bool()),
        (Some("ec256"), Some(true))
    );
    (certificate.body["id"].as_i64().unwrap(), challenge.body["id"].as_i64().unwrap())
}

#[tokio::test]
async fn the_plugin_is_installed_and_offers_its_dns_providers() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let plugin = h.send(empty(with_cookie(get("/api/v1/certificates/plugin"), &admin))).await;
    assert_eq!(plugin.status, StatusCode::OK, "{:?}", plugin.body);
    assert_eq!(plugin.body["installed"], false);
    let ionos =
        plugin.body["providers"].as_array().unwrap().iter().find(|p| p["api"] == "dns_ionos").unwrap();
    assert_eq!(ionos["variables"][1], json!({ "name": "IONOS_SECRET", "secret": true, "required": true }));

    let job = send(&h, &admin, Method::POST, "/api/v1/certificates/plugin/install", Value::Null).await;
    assert_eq!(job.status, StatusCode::ACCEPTED, "{:?}", job.body);
    assert_eq!(finished(&h, &job.body).await.state, zendiloos_jobs::JobState::Succeeded);
    let plugin = h.send(empty(with_cookie(get("/api/v1/certificates/plugin"), &admin))).await;
    assert_eq!(plugin.body["installed"], true);
    let again = send(&h, &admin, Method::POST, "/api/v1/certificates/plugin/install", Value::Null).await;
    assert_problem(&again, StatusCode::CONFLICT, "acme_installed");

    // Users see whether it is there, but change nothing.
    let user = h.sign_in("bob", Role::User).await;
    let seen = h.send(empty(with_cookie(get("/api/v1/certificates/plugin"), &user))).await;
    assert_eq!(seen.status, StatusCode::OK);
    let accounts = h.send(empty(with_cookie(get("/api/v1/certificates/accounts"), &user))).await;
    assert_problem(&accounts, StatusCode::FORBIDDEN, "forbidden");
}

#[tokio::test]
async fn secrets_never_come_back_and_stay_when_left_empty() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let (_, challenge) = certificate(&h, &admin, json!(["*.example.org", "example.org"])).await;
    let listed = h.send(empty(with_cookie(get("/api/v1/certificates/challenges"), &admin))).await;
    let text = listed.body.to_string();
    assert!(!text.contains("s3cr3t"), "{text}");
    assert_eq!(
        listed.body[0]["values"],
        json!([
            { "name": "IONOS_PREFIX", "secret": false, "value": "prefix" },
            { "name": "IONOS_SECRET", "secret": true, "value": null },
        ])
    );
    let uri = format!("/api/v1/certificates/challenges/{challenge}");
    let kept = send(
        &h,
        &admin,
        Method::PUT,
        &uri,
        json!({ "name": "IONOS DNS", "method": "dns", "provider": "dns_ionos",
                "values": { "IONOS_PREFIX": "other", "IONOS_SECRET": "" } }),
    )
    .await;
    assert_eq!(kept.status, StatusCode::NO_CONTENT, "{:?}", kept.body);
    // The wildcard needs DNS.
    let http = send(&h, &admin, Method::PUT, &uri, json!({ "name": "HTTP", "method": "http" })).await;
    assert_problem(&http, StatusCode::CONFLICT, "in_use");
    let unknown = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/certificates/challenges",
        json!({ "name": "X", "method": "dns", "provider": "dns_nowhere" }),
    )
    .await;
    assert_problem(&unknown, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let no_provider = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/certificates/challenges",
        json!({ "name": "X", "method": "dns" }),
    )
    .await;
    assert_problem(&no_provider, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let used = send(&h, &admin, Method::DELETE, &uri, Value::Null).await;
    assert_problem(&used, StatusCode::CONFLICT, "in_use");
}

#[tokio::test]
async fn a_certificate_is_issued_in_a_job_with_what_acme_sh_wrote() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let (id, _) = certificate(&h, &admin, json!(["nas.example.org"])).await;
    let job = send(&h, &admin, Method::POST, &format!("/api/v1/certificates/{id}/issue"), Value::Null).await;
    assert_eq!(job.status, StatusCode::ACCEPTED, "{:?}", job.body);
    let job = finished(&h, &job.body).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded, "{job:?}");
    assert!(job.output.iter().any(|line| line == "Cert success."), "{:?}", job.output);

    let listed = h.send(empty(with_cookie(get("/api/v1/certificates"), &admin))).await;
    let issued = &listed.body[0];
    assert!(issued["not_after"].as_i64().unwrap() > issued["issued_at"].as_i64().unwrap());
    assert_eq!(issued["last_output"][1], "Cert success.");

    h.acme.fail_with(Some("Invalid TXT record"));
    let job = send(&h, &admin, Method::POST, &format!("/api/v1/certificates/{id}/issue"), Value::Null).await;
    assert_eq!(finished(&h, &job.body).await.state, zendiloos_jobs::JobState::Failed);
    let listed = h.send(empty(with_cookie(get("/api/v1/certificates"), &admin))).await;
    assert_eq!(listed.body[0]["last_error"], "Invalid TXT record");

    let renamed = send(
        &h,
        &admin,
        Method::PUT,
        &format!("/api/v1/certificates/{id}"),
        json!({ "domains": ["nas.example.org", "files.example.org"], "key": "rsa2048",
                "account_id": issued["account_id"], "challenge_id": issued["challenge_id"], "auto_renew": false }),
    )
    .await;
    assert_eq!(renamed.status, StatusCode::NO_CONTENT, "{:?}", renamed.body);
    let missing = send(&h, &admin, Method::POST, "/api/v1/certificates/99/issue", Value::Null).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");
    let bad = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/certificates",
        json!({ "domains": ["nas"], "account_id": issued["account_id"], "challenge_id": issued["challenge_id"] }),
    )
    .await;
    assert_problem(&bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn the_web_interface_moves_only_when_confirmed() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let setup = h.send(empty(with_cookie(get("/api/v1/system/web-server"), &admin))).await;
    assert_eq!(setup.status, StatusCode::OK, "{:?}", setup.body);
    assert_eq!(setup.body["current"], json!({ "http_port": 80, "https_port": 443, "certificate_id": null }));
    assert_eq!(setup.body["confirm_within"], 90);

    let (id, _) = certificate(&h, &admin, json!(["nas.example.org"])).await;
    let moved = json!({ "http_port": null, "https_port": 8443, "certificate_id": id });
    let unissued = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", moved.clone()).await;
    assert_problem(&unissued, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let job = send(&h, &admin, Method::POST, &format!("/api/v1/certificates/{id}/issue"), Value::Null).await;
    finished(&h, &job.body).await;

    let busy = json!({ "http_port": 8080, "https_port": 443, "certificate_id": null });
    let busy = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", busy).await;
    assert_problem(&busy, StatusCode::CONFLICT, "port_in_use");
    let reserved = json!({ "http_port": 80, "https_port": 445, "certificate_id": null });
    let reserved = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", reserved).await;
    assert_problem(&reserved, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let applied = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", moved.clone()).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert_eq!(applied.body["pending"]["settings"], moved);
    assert_eq!(applied.body["current"]["https_port"], 443);
    let again = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", moved.clone()).await;
    assert_problem(&again, StatusCode::CONFLICT, "change_pending");
    let shown = send(&h, &admin, Method::DELETE, &format!("/api/v1/certificates/{id}"), Value::Null).await;
    assert_problem(&shown, StatusCode::CONFLICT, "in_use");

    let confirmed = send(&h, &admin, Method::POST, "/api/v1/system/web-server/confirm", Value::Null).await;
    assert_eq!(confirmed.status, StatusCode::NO_CONTENT, "{:?}", confirmed.body);
    let setup = h.send(empty(with_cookie(get("/api/v1/system/web-server"), &admin))).await;
    assert_eq!((&setup.body["current"], &setup.body["pending"]), (&moved, &Value::Null));
    let nothing = send(&h, &admin, Method::POST, "/api/v1/system/web-server/confirm", Value::Null).await;
    assert_problem(&nothing, StatusCode::CONFLICT, "refused");

    // Not confirmed in time: back by itself. nginx listens where it was told.
    *h.listening.0.lock().unwrap() = [22, 8080, 8443].into();
    let back = json!({ "http_port": 80, "https_port": 443, "certificate_id": null });
    let applied = send(&h, &admin, Method::PUT, "/api/v1/system/web-server", back).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    h.acme.roll_back();
    let setup = h.send(empty(with_cookie(get("/api/v1/system/web-server"), &admin))).await;
    assert_eq!((&setup.body["current"], &setup.body["pending"]), (&moved, &Value::Null));
    assert!(h.listening.0.lock().unwrap().contains(&8080));

    let user = h.sign_in("bob", Role::User).await;
    let refused = send(&h, &user, Method::PUT, "/api/v1/system/web-server", moved).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "forbidden");
}
