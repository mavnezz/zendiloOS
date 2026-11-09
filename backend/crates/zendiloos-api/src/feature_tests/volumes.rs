//! Docker's volumes through the API: listed with who uses them, removed,
//! backed up on a plan or at once, and restored, with the refusals; their
//! backups as a whole, also of volumes deleted, and backups found again in
//! a folder.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Session, assert_problem, changing, empty, get, json, post, with_cookie, with_csrf,
};

async fn admin(h: &Harness) -> Session {
    h.sign_in("alice", Role::Admin).await
}

/// The Docker app's job `id` once it ended, as stored.
async fn ended(h: &Harness, id: &Value) -> zendiloos_jobs::Job {
    let id = id.as_i64().unwrap();
    for _ in 0..400 {
        if let Some(job) =
            h.docker_jobs.all().into_iter().find(|job| job.id.get() == id && job.finished_at.is_some())
        {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("job {id} did not end");
}

fn plan() -> Value {
    json!({ "share_id": 1, "folder": "backups", "frequency": "weekly", "weekday": 6, "hour": 3 })
}

#[tokio::test]
async fn volumes_are_listed_with_their_users_sizes_and_plans() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let saved = h
        .send(json(
            with_csrf(changing(Method::PUT, "/api/v1/docker/volumes/kuma_data/backup-plan"), &admin),
            plan(),
        ))
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(
        (saved.body["frequency"].clone(), saved.body["weekday"].clone(), saved.body["keep"].clone()),
        (json!("weekly"), json!(6), json!(7))
    );
    assert_eq!(saved.body["stop_containers"], true, "on unless said otherwise");

    let listed = h.send(empty(with_cookie(get("/api/v1/docker/volumes"), &admin))).await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    let volumes = listed.body.as_array().unwrap();
    let names: Vec<&str> = volumes.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["kuma_data", "nfs_media", "orphan", "stack_db"]);
    let kuma = &volumes[0];
    assert_eq!(kuma["project"], "kuma");
    assert_eq!(kuma["used_by"], json!([{ "name": "kuma-1", "running": true }]));
    assert_eq!(kuma["plan"]["folder"], "backups");
    assert_eq!(kuma["last_backup"], Value::Null);
    assert_eq!(
        (volumes[1]["backed_up_here"].clone(), volumes[2]["used_by"].clone()),
        (json!(false), json!([]))
    );

    let sizes = h.send(empty(with_cookie(get("/api/v1/docker/volumes/sizes"), &admin))).await;
    assert_eq!(sizes.status, StatusCode::OK, "{:?}", sizes.body);
    assert_eq!(sizes.body[0], json!({ "name": "kuma_data", "bytes": 4096, "complete": true }));
    let shown = h.send(empty(with_cookie(get("/api/v1/docker/volumes/kuma_data/backup-plan"), &admin))).await;
    assert_eq!(shown.body["hour"], 3);
    let none = h.send(empty(with_cookie(get("/api/v1/docker/volumes/orphan/backup-plan"), &admin))).await;
    assert_problem(&none, StatusCode::NOT_FOUND, "not_found");

    // Who reads the backups' share, and with it the volumes' contents.
    let readers = h.send(empty(with_cookie(get("/api/v1/docker/backup-shares/1/readers"), &admin))).await;
    assert_eq!(readers.status, StatusCode::OK, "{:?}", readers.body);
    assert_eq!(
        readers.body,
        json!({ "accounts": ["alice"], "groups": ["family"], "nfs": ["192.168.1.0/24"] })
    );
    let other = h.send(empty(with_cookie(get("/api/v1/docker/backup-shares/9/readers"), &admin))).await;
    assert_problem(&other, StatusCode::FORBIDDEN, "share_denied");
}

#[tokio::test]
async fn only_unused_volumes_are_removed() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let used =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/volumes/kuma_data"), &admin))).await;
    assert_problem(&used, StatusCode::CONFLICT, "refused");
    let gone =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/volumes/ghost"), &admin))).await;
    assert_problem(&gone, StatusCode::NOT_FOUND, "not_found");
    let removed =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/volumes/orphan"), &admin))).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    let pruned = h.send(empty(with_csrf(post("/api/v1/docker/volumes/prune"), &admin))).await;
    assert_eq!(pruned.status, StatusCode::OK, "{:?}", pruned.body);
    assert!(pruned.body["text"].as_str().unwrap().contains("reclaimed"));
}

#[tokio::test]
async fn plans_are_checked() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let put = |volume: &str, body: Value| {
        let path = format!("/api/v1/docker/volumes/{volume}/backup-plan");
        let request = json(with_csrf(changing(Method::PUT, &path), &admin), body);
        h.send(request)
    };
    let no_weekday = put("orphan", json!({ "share_id": 1, "frequency": "weekly", "hour": 3 })).await;
    assert_problem(&no_weekday, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let no_day = put("orphan", json!({ "share_id": 1, "frequency": "monthly", "hour": 3 })).await;
    assert_problem(&no_day, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let late = put("orphan", json!({ "share_id": 1, "frequency": "daily", "hour": 24 })).await;
    assert_problem(&late, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let denied = put("orphan", json!({ "share_id": 2, "frequency": "daily", "hour": 3 })).await;
    assert_problem(&denied, StatusCode::FORBIDDEN, "share_denied");
    let elsewhere = put("nfs_media", plan()).await;
    assert_problem(&elsewhere, StatusCode::CONFLICT, "refused");
    let ghost = put("ghost", plan()).await;
    assert_problem(&ghost, StatusCode::NOT_FOUND, "not_found");
    let unknown = put("orphan", json!({ "share_id": 1, "frequency": "daily", "hour": 3, "minute": 5 })).await;
    assert!(unknown.status.is_client_error(), "{:?}", unknown.status);

    let monthly = put(
        "orphan",
        json!({ "share_id": 1, "frequency": "monthly", "day": 31, "hour": 2, "keep": 3,
                                        "stop_containers": false }),
    )
    .await;
    assert_eq!(monthly.status, StatusCode::OK, "{:?}", monthly.body);
    assert_eq!(
        (monthly.body["day"].clone(), monthly.body["stop_containers"].clone()),
        (json!(31), json!(false))
    );
    let path = "/api/v1/docker/volumes/orphan/backup-plan";
    let ended = h.send(empty(with_csrf(changing(Method::DELETE, path), &admin))).await;
    assert_eq!(ended.status, StatusCode::NO_CONTENT);
    let again = h.send(empty(with_csrf(changing(Method::DELETE, path), &admin))).await;
    assert_problem(&again, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn a_volume_is_backed_up_at_once_and_restored() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let none = h.send(empty(with_csrf(post("/api/v1/docker/volumes/kuma_data/backups"), &admin))).await;
    assert_problem(&none, StatusCode::NOT_FOUND, "not_found");
    h.send(json(
        with_csrf(changing(Method::PUT, "/api/v1/docker/volumes/kuma_data/backup-plan"), &admin),
        plan(),
    ))
    .await;

    let started = h.send(empty(with_csrf(post("/api/v1/docker/volumes/kuma_data/backups"), &admin))).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    assert_eq!(started.body["kind"], "containers.volume_backup");
    assert_eq!(ended(&h, &started.body["id"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let listed = h.send(empty(with_cookie(get("/api/v1/docker/volumes/kuma_data/backups"), &admin))).await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    let backup = &listed.body[0];
    assert_eq!(backup["state"], "succeeded");
    assert!(backup["archive"].as_str().unwrap().starts_with("backups/kuma_data/kuma_data-"), "{backup}");
    assert_eq!(
        (backup["sha256"].as_str().unwrap().len(), backup["started_by"].clone()),
        (64, json!("alice"))
    );
    let id = backup["id"].as_i64().unwrap();
    let volumes = h.send(empty(with_cookie(get("/api/v1/docker/volumes"), &admin))).await;
    assert_eq!(volumes.body[0]["last_backup"]["id"], id);

    let restore = |body: Value| {
        let request = json(with_csrf(post(&format!("/api/v1/docker/backups/{id}/restore")), &admin), body);
        h.send(request)
    };
    let in_use = restore(json!({ "target": "own_name" })).await;
    assert_problem(&in_use, StatusCode::CONFLICT, "volume_in_use");
    assert_eq!(in_use.body["detail"], "kuma-1");
    let nameless = restore(json!({ "target": "existing" })).await;
    assert_problem(&nameless, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let missing = restore(json!({ "target": "existing", "volume": "ghost" })).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");
    let restored = restore(json!({ "target": "own_name", "stop_containers": true })).await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{:?}", restored.body);
    assert_eq!(restored.body["kind"], "containers.volume_restore");
    assert_eq!(ended(&h, &restored.body["id"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let into = restore(json!({ "target": "existing", "volume": "orphan" })).await;
    assert_eq!(ended(&h, &into.body["id"]).await.state, zendiloos_jobs::JobState::Succeeded);
    assert!(
        h.archiver
            .calls()
            .iter()
            .any(|call| call.starts_with("restore orphan from docker/backups/kuma_data/"))
    );
    let unknown = h
        .send(json(
            with_csrf(post("/api/v1/docker/backups/999/restore"), &admin),
            json!({ "target": "own_name" }),
        ))
        .await;
    assert_problem(&unknown, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn volumes_are_for_those_who_manage_containers() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    for path in [
        "/api/v1/docker/volumes",
        "/api/v1/docker/volumes/sizes",
        "/api/v1/docker/volumes/x1/backups",
        "/api/v1/docker/backups",
    ] {
        assert_problem(
            &h.send(empty(with_cookie(get(path), &viewer))).await,
            StatusCode::FORBIDDEN,
            "forbidden",
        );
    }
    for (method, path, body) in [
        (Method::POST, "/api/v1/docker/volumes/prune", json!({})),
        (Method::DELETE, "/api/v1/docker/volumes/x1", json!({})),
        (Method::PUT, "/api/v1/docker/volumes/x1/backup-plan", plan()),
        (Method::POST, "/api/v1/docker/volumes/x1/backups", json!({})),
        (Method::POST, "/api/v1/docker/backups/1/restore", json!({ "target": "own_name" })),
        (Method::DELETE, "/api/v1/docker/backups/1", json!({})),
        (Method::POST, "/api/v1/docker/volumes/x1/backup-plan/resume", json!({})),
        (Method::POST, "/api/v1/docker/backups/import", json!({ "share_id": 1, "folder": "" })),
    ] {
        let reply = h.send(json(with_csrf(changing(method, path), &viewer), body)).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
}

#[tokio::test]
async fn the_backups_of_a_deleted_volume_are_listed_restored_and_removed() {
    let h = Harness::new();
    let admin = admin(&h).await;
    h.send(json(
        with_csrf(changing(Method::PUT, "/api/v1/docker/volumes/orphan/backup-plan"), &admin),
        plan(),
    ))
    .await;
    let started = h.send(empty(with_csrf(post("/api/v1/docker/volumes/orphan/backups"), &admin))).await;
    assert_eq!(ended(&h, &started.body["id"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let removed =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/volumes/orphan"), &admin))).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);

    let groups = h.send(empty(with_cookie(get("/api/v1/docker/backups"), &admin))).await;
    assert_eq!(groups.status, StatusCode::OK, "{:?}", groups.body);
    let orphan = &groups.body[0];
    assert_eq!(
        (orphan["volume"].clone(), orphan["exists"].clone(), orphan["plan"].clone()),
        (json!("orphan"), json!(false), Value::Null),
        "removed in zendiloOS, with its plan"
    );
    let backup = &orphan["backups"][0];
    assert_eq!((backup["state"].clone(), backup["imported"].clone()), (json!("succeeded"), json!(false)));
    let id = backup["id"].as_i64().unwrap();

    let restored = h
        .send(json(
            with_csrf(post(&format!("/api/v1/docker/backups/{id}/restore")), &admin),
            json!({ "target": "own_name" }),
        ))
        .await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{:?}", restored.body);
    assert_eq!(ended(&h, &restored.body["id"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let groups = h.send(empty(with_cookie(get("/api/v1/docker/backups"), &admin))).await;
    assert_eq!(groups.body[0]["exists"], true, "made again");

    let path = format!("/api/v1/docker/backups/{id}");
    let gone = h.send(empty(with_csrf(changing(Method::DELETE, &path), &admin))).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{:?}", gone.body);
    assert!(h.archiver.calls().iter().any(|call| call.starts_with("remove docker/backups/orphan/")));
    let again = h.send(empty(with_csrf(changing(Method::DELETE, &path), &admin))).await;
    assert_problem(&again, StatusCode::NOT_FOUND, "not_found");
    let groups = h.send(empty(with_cookie(get("/api/v1/docker/backups"), &admin))).await;
    assert_eq!(groups.body, json!([]));
}

#[tokio::test]
async fn a_plan_is_resumed_only_for_a_volume_that_is_there() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let resume = |volume: &str| {
        let path = format!("/api/v1/docker/volumes/{volume}/backup-plan/resume");
        h.send(empty(with_csrf(post(&path), &admin)))
    };
    assert_problem(&resume("stack_db").await, StatusCode::NOT_FOUND, "not_found");
    h.send(json(
        with_csrf(changing(Method::PUT, "/api/v1/docker/volumes/stack_db/backup-plan"), &admin),
        plan(),
    ))
    .await;
    let resumed = resume("stack_db").await;
    assert_eq!(resumed.status, StatusCode::OK, "{:?}", resumed.body);
    assert_eq!(resumed.body["paused"], false);
    h.engine.volumes.lock().unwrap().retain(|volume| volume.name != "stack_db");
    let gone = resume("stack_db").await;
    assert_problem(&gone, StatusCode::CONFLICT, "refused");
    let groups = h.send(empty(with_cookie(get("/api/v1/docker/backups"), &admin))).await;
    let group = &groups.body[0];
    assert_eq!(
        (group["volume"].clone(), group["exists"].clone(), group["plan"]["paused"].clone()),
        (json!("stack_db"), json!(false), json!(false))
    );
}

#[tokio::test]
async fn backups_found_again_in_a_folder_are_imported_in_a_job() {
    use zendiloos_containers::application::ports::{FoundArchive, ScannedArchives};
    use zendiloos_containers::domain::{BackupFile, WrittenBackup};

    let h = Harness::new();
    let admin = admin(&h).await;
    let file = |path: &str, inode: u64| BackupFile {
        path: path.into(),
        bytes: 10,
        device: 1,
        inode,
        modified: Some(1),
    };
    *h.archiver.found.lock().unwrap() = ScannedArchives {
        found: vec![FoundArchive {
            volume: "lost".into(),
            backed_up_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
            written: WrittenBackup {
                archive: file("old/lost/lost-20261027-030000.tar.zst", 5),
                manifest: file("old/lost/lost-20261027-030000.json", 6),
                sha256: "ef".repeat(32),
                data_bytes: 100,
                entries: Some(2),
                manifest_json: r#"{"volume":"lost","driver":"local","labels":{}}"#.into(),
            },
        }],
        refused: vec![(
            "old/lost/lost-20261026-030000.tar.zst".into(),
            "the archive: writable by others".into(),
        )],
    };
    let import = |body: Value| h.send(json(with_csrf(post("/api/v1/docker/backups/import"), &admin), body));

    let started = import(json!({ "share_id": 1, "folder": "old" })).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    assert_eq!(started.body["kind"], "containers.volume_import");
    let job = ended(&h, &started.body["id"]).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded);
    assert!(
        job.output.contains(
            &"Refused old/lost/lost-20261026-030000.tar.zst: the archive: writable by others".to_owned()
        ),
        "{:?}",
        job.output
    );
    let groups = h.send(empty(with_cookie(get("/api/v1/docker/backups"), &admin))).await;
    let lost = groups.body.as_array().unwrap().iter().find(|group| group["volume"] == "lost").unwrap();
    assert_eq!((lost["exists"].clone(), lost["plan"].clone()), (json!(false), Value::Null));
    assert_eq!(
        (lost["backups"][0]["imported"].clone(), lost["backups"][0]["started_by"].clone()),
        (json!(true), Value::Null)
    );

    assert_problem(
        &import(json!({ "share_id": 2, "folder": "" })).await,
        StatusCode::FORBIDDEN,
        "share_denied",
    );
    assert_problem(
        &import(json!({ "share_id": 1, "folder": "a/../b" })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
}
