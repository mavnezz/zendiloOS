//! Backup tasks through the API: the plugin installed, the NAS's key and a
//! server's host keys, tasks made, changed and removed with a daemon's
//! password never sent back, runs started and logged, versions looked into
//! and restored, settings; for administrators only.

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

async fn finished(h: &Harness, job: &Value) -> zendiloos_jobs::Job {
    let id = zendiloos_jobs::JobId::new(job["id"].as_i64().unwrap());
    for _ in 0..400 {
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

async fn read(h: &Harness, session: &Session, uri: &str) -> Reply {
    h.send(empty(with_cookie(get(uri), session))).await
}

async fn installed(h: &Harness, admin: &Session) {
    let install = send(h, admin, Method::POST, "/api/v1/backup/plugin/install", Value::Null).await;
    assert_eq!(install.status, StatusCode::ACCEPTED, "{:?}", install.body);
    assert_eq!(finished(h, &install.body).await.state, zendiloos_jobs::JobState::Succeeded);
}

fn local(name: &str) -> Value {
    json!({
        "name": name,
        "source": { "kind": "share", "share_id": 1, "path": "albums" },
        "target": { "kind": "share", "share_id": 2, "path": "" },
        "mode": { "kind": "versioned", "keep_last": 10, "keep_days": null },
        "schedule": { "frequency": "daily", "hour": 3, "minute": 0 },
        "excludes": ["*.tmp", "@eaDir"],
        "hidden": true
    })
}

fn push(name: &str) -> Value {
    json!({
        "name": name,
        "source": { "kind": "share", "share_id": 1, "path": "" },
        "target": { "kind": "server", "path": "nas" },
        "server": { "transport": "daemon", "host": "nas2.lan", "port": 873, "user": "zendi", "module": "backup" },
        "password": "s3cret-password",
        "mode": { "kind": "mirror", "keep_changed_days": 7 },
        "excludes": [],
        "hidden": false,
        "bwlimit_kib": 1000
    })
}

#[tokio::test]
async fn only_administrators_back_up() {
    let h = Harness::new();
    let user = h.sign_in("bob", Role::User).await;
    for uri in
        ["/api/v1/backup/plugin", "/api/v1/backup/tasks", "/api/v1/backup/runs", "/api/v1/backup/settings"]
    {
        assert_eq!(read(&h, &user, uri).await.status, StatusCode::FORBIDDEN, "{uri}");
    }
    let made = send(&h, &user, Method::POST, "/api/v1/backup/tasks", local("x")).await;
    assert_eq!(made.status, StatusCode::FORBIDDEN);
    let readonly = h.sign_in("carol", Role::ReadOnly).await;
    assert_eq!(read(&h, &readonly, "/api/v1/backup/tasks").await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_plugin_installs_and_hands_out_the_nas_key() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let plugin = read(&h, &admin, "/api/v1/backup/plugin").await;
    assert_eq!(plugin.body, json!({ "installed": false, "ssh": false, "rclone": false, "public_key": null }));
    let refused = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", local("Albums")).await;
    assert_problem(&refused, StatusCode::CONFLICT, "backup_not_installed");
    installed(&h, &admin).await;
    assert!(h.backup_host.calls().contains(&zendiloos_backup::testing::HostCall::Install));
    let again = send(&h, &admin, Method::POST, "/api/v1/backup/plugin/install", Value::Null).await;
    assert_problem(&again, StatusCode::CONFLICT, "backup_installed");

    let key = send(&h, &admin, Method::POST, "/api/v1/backup/key", Value::Null).await;
    assert_eq!(key.status, StatusCode::OK);
    assert!(key.body["public_key"].as_str().unwrap().starts_with("ssh-ed25519 "));
    assert_eq!(read(&h, &admin, "/api/v1/backup/plugin").await.body["public_key"], key.body["public_key"]);

    let keys = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/backup/servers/host-keys",
        json!({ "host": "nas2.lan", "port": 22 }),
    )
    .await;
    assert_eq!(keys.status, StatusCode::OK, "{:?}", keys.body);
    assert!(keys.body[0]["fingerprint"].as_str().unwrap().starts_with("SHA256:"));
    let bad = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/backup/servers/host-keys",
        json!({ "host": "-oProxyCommand=sh", "port": 22 }),
    )
    .await;
    assert_problem(&bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn tasks_are_made_changed_and_removed_without_showing_a_password() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    installed(&h, &admin).await;
    let made = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", local("Albums")).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["kind"], "local");
    assert_eq!(made.body["folder"], "Albums");
    assert!(made.body["next_at"].is_i64());
    let offsite = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", push("Offsite")).await;
    assert_eq!(offsite.status, StatusCode::CREATED, "{:?}", offsite.body);
    assert_eq!(offsite.body["password_set"], true);
    let listed = read(&h, &admin, "/api/v1/backup/tasks").await;
    assert_eq!(listed.body.as_array().unwrap().len(), 2);
    assert!(!listed.body.to_string().contains("s3cret"), "never sent back");
    assert!(!offsite.body.to_string().contains("s3cret"));

    let taken = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", local("albums")).await;
    assert_problem(&taken, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let mut weekly = local("Weekly");
    weekly["schedule"] = json!({ "frequency": "weekly", "hour": 3, "minute": 0 });
    let refused = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", weekly).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let mut serverless = push("No server");
    serverless["server"] = Value::Null;
    let refused = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", serverless).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    // Changed without its password: it stays.
    let id = offsite.body["id"].as_i64().unwrap();
    let mut changed = push("Offsite nightly");
    changed["password"] = Value::Null;
    let updated = send(&h, &admin, Method::PUT, &format!("/api/v1/backup/tasks/{id}"), changed).await;
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(
        (updated.body["name"].as_str(), updated.body["password_set"].as_bool()),
        (Some("Offsite nightly"), Some(true))
    );
    assert_eq!(updated.body["folder"], "Offsite", "its folder stays");

    let readers = read(&h, &admin, "/api/v1/backup/shares/2/readers").await;
    assert_eq!(readers.body["accounts"], json!(["alice"]));
    assert_eq!(read(&h, &admin, "/api/v1/backup/shares/9/readers").await.status, StatusCode::NOT_FOUND);

    let removed = send(&h, &admin, Method::DELETE, &format!("/api/v1/backup/tasks/{id}"), Value::Null).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    assert_eq!(read(&h, &admin, &format!("/api/v1/backup/tasks/{id}")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_run_is_logged_and_its_version_restored() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    installed(&h, &admin).await;
    let made = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", local("Albums")).await;
    let id = made.body["id"].as_i64().unwrap();
    let stop = send(&h, &admin, Method::POST, &format!("/api/v1/backup/tasks/{id}/stop"), Value::Null).await;
    assert_problem(&stop, StatusCode::CONFLICT, "task_not_running");

    let run = send(&h, &admin, Method::POST, &format!("/api/v1/backup/tasks/{id}/run"), Value::Null).await;
    assert_eq!(run.status, StatusCode::ACCEPTED, "{:?}", run.body);
    assert_eq!(run.body["kind"], "backup.run");
    assert_eq!(finished(&h, &run.body).await.state, zendiloos_jobs::JobState::Succeeded);

    let runs = read(&h, &admin, &format!("/api/v1/backup/tasks/{id}/runs")).await;
    assert_eq!(runs.body[0]["outcome"], "succeeded");
    assert_eq!(runs.body[0]["trigger"], "manual");
    assert_eq!(runs.body[0]["by"], "alice");
    let all = read(&h, &admin, "/api/v1/backup/runs?limit=5").await;
    assert_eq!(all.body.as_array().unwrap().len(), 1);
    let run_id = runs.body[0]["id"].as_i64().unwrap();
    let transfers = read(&h, &admin, &format!("/api/v1/backup/runs/{run_id}/transfers")).await;
    assert_eq!(transfers.body["transfers"][0], json!({ "change": ">f+++++++++", "path": "a.jpg" }));
    assert_eq!(read(&h, &admin, "/api/v1/backup/runs/99/transfers").await.status, StatusCode::NOT_FOUND);

    let versions = read(&h, &admin, &format!("/api/v1/backup/tasks/{id}/versions")).await;
    assert_eq!(versions.status, StatusCode::OK, "{:?}", versions.body);
    let version = versions.body["versions"][0].as_i64().unwrap();
    let browsed =
        read(&h, &admin, &format!("/api/v1/backup/tasks/{id}/browse?version={version}&path=2024")).await;
    assert_eq!(browsed.status, StatusCode::OK, "{:?}", browsed.body);
    let task = read(&h, &admin, &format!("/api/v1/backup/tasks/{id}")).await;
    assert_eq!(task.body["last_run"]["outcome"], "succeeded");

    let restore = json!({ "version": version, "paths": ["2024"], "existing": "skip",
                          "to": { "kind": "share", "share_id": 2, "path": "restored" } });
    let restored =
        send(&h, &admin, Method::POST, &format!("/api/v1/backup/tasks/{id}/restore"), restore).await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{:?}", restored.body);
    assert_eq!(finished(&h, &restored.body).await.state, zendiloos_jobs::JobState::Succeeded);
    let into_server = json!({ "existing": "replace", "to": { "kind": "server", "path": "x" } });
    let refused =
        send(&h, &admin, Method::POST, &format!("/api/v1/backup/tasks/{id}/restore"), into_server).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn a_task_to_a_drive_needs_it_plugged_in_and_settings_have_bounds() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    installed(&h, &admin).await;
    let mut to_drive = local("To stick");
    to_drive["target"] = json!({ "kind": "drive", "drive": "1A2B-3C4D", "path": "nas" });
    to_drive["mode"] = json!({ "kind": "mirror", "keep_changed_days": null });
    to_drive["on_plug"] = json!(true);
    let made = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", to_drive).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["reachable"], false);
    let id = made.body["id"].as_i64().unwrap();
    let run = send(&h, &admin, Method::POST, &format!("/api/v1/backup/tasks/{id}/run"), Value::Null).await;
    assert_problem(&run, StatusCode::CONFLICT, "drive_missing");
    h.backup_drives.plug("1A2B-3C4D", "vfat", "boot:1");
    assert_eq!(read(&h, &admin, &format!("/api/v1/backup/tasks/{id}")).await.body["reachable"], true);

    let settings = read(&h, &admin, "/api/v1/backup/settings").await;
    assert_eq!(settings.body, json!({ "log_days": 90, "bwlimit_kib": 0 }));
    let saved = send(
        &h,
        &admin,
        Method::PUT,
        "/api/v1/backup/settings",
        json!({ "log_days": 30, "bwlimit_kib": 500 }),
    )
    .await;
    assert_eq!(saved.body, json!({ "log_days": 30, "bwlimit_kib": 500 }));
    let bad =
        send(&h, &admin, Method::PUT, "/api/v1/backup/settings", json!({ "log_days": 0, "bwlimit_kib": 0 }))
            .await;
    assert_problem(&bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let check = json!({ "server": { "transport": "daemon", "host": "down.lan", "port": 873, "user": "zendi", "module": "m" }, "path": "" });
    let failed = send(&h, &admin, Method::POST, "/api/v1/backup/servers/check", check).await;
    assert_problem(&failed, StatusCode::BAD_GATEWAY, "agent_failed");
    let check = json!({ "server": { "transport": "daemon", "host": "nas2.lan", "port": 873, "user": "zendi", "module": "m" }, "path": "" });
    assert_eq!(
        send(&h, &admin, Method::POST, "/api/v1/backup/servers/check", check).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_cloud_account_is_signed_in_to_and_an_encrypted_task_never_shows_its_password() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    installed(&h, &admin).await;
    let plugin = read(&h, &admin, "/api/v1/backup/plugin").await;
    assert_eq!(plugin.body["rclone"], true);
    let begun = send(
        &h,
        &admin,
        Method::POST,
        "/api/v1/backup/accounts/sign-in",
        json!({ "provider": "onedrive", "name": "Family OneDrive" }),
    )
    .await;
    assert_eq!(begun.status, StatusCode::OK, "{:?}", begun.body);
    let flow = begun.body["flow"].as_str().unwrap().to_owned();
    assert!(begun.body["url"].as_str().unwrap().contains(&format!("state={flow}")));
    let finish = format!("/api/v1/backup/accounts/sign-in/{flow}");
    let address = format!("http://localhost:53682/?code=good&state={flow}");
    let account = send(&h, &admin, Method::POST, &finish, json!({ "address": address })).await;
    assert_eq!(account.status, StatusCode::CREATED, "{:?}", account.body);
    assert_eq!(account.body["provider"], "onedrive");
    assert!(account.body.get("token").is_none());
    let again = send(&h, &admin, Method::POST, &finish, json!({ "address": address })).await;
    assert_problem(&again, StatusCode::CONFLICT, "cloud_sign_in_expired");
    let accounts = read(&h, &admin, "/api/v1/backup/accounts").await;
    assert_eq!(accounts.body.as_array().unwrap().len(), 1);

    let id = account.body["id"].as_i64().unwrap();
    let mut task = local("To the cloud");
    task["target"] = json!({ "kind": "cloud", "account_id": id, "path": "nas" });
    task["encrypted"] = json!(true);
    let refused = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", task.clone()).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    task["crypt_password"] = json!("correct horse battery staple");
    task["crypt_salt"] = json!("pepper");
    let made = send(&h, &admin, Method::POST, "/api/v1/backup/tasks", task).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!((made.body["kind"].as_str(), made.body["encrypted"].as_bool()), (Some("cloud"), Some(true)));
    let shown = made.body.to_string();
    assert!(!shown.contains("correct horse") && !shown.contains("pepper"), "{shown}");

    let gone = send(&h, &admin, Method::DELETE, &format!("/api/v1/backup/accounts/{id}"), Value::Null).await;
    assert_problem(&gone, StatusCode::CONFLICT, "account_in_use");
    assert_eq!(gone.body["detail"], "To the cloud");
    let task_id = made.body["id"].as_i64().unwrap();
    let removed =
        send(&h, &admin, Method::DELETE, &format!("/api/v1/backup/tasks/{task_id}"), Value::Null).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let gone = send(&h, &admin, Method::DELETE, &format!("/api/v1/backup/accounts/{id}"), Value::Null).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    let user = h.sign_in("bob", Role::User).await;
    assert_eq!(read(&h, &user, "/api/v1/backup/accounts").await.status, StatusCode::FORBIDDEN);
}
