//! Pools of another installation through the API: those the configuration
//! names that are not imported, found or missing, and others on the disks;
//! imported as their volume or adopted as another, by force only when
//! another machine used them last.

use axum::http::StatusCode;
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;
use zendiloos_storage::domain::{ImportablePool, KnownPool, VolumeNumber};

use crate::tests::{Harness, assert_problem, empty, get, job_done, json, post, with_cookie, with_csrf};

fn found(name: &str, id: u64, other_host: bool) -> ImportablePool {
    ImportablePool {
        name: name.into(),
        id,
        state: "ONLINE".into(),
        other_host,
        can_import: true,
        status: other_host.then(|| "The pool was last accessed by another system.".into()),
        action: Some("The pool can be imported using its name or numeric identifier.".into()),
        devices: vec![("ata-TEST_HDD_T3-part1".into(), "ONLINE".into())],
    }
}

/// tank, volume 1 with the share photos, is on the disks but not imported,
/// as on a machine installed afresh with the configuration restored; cold,
/// volume 2, is on no disk; stray is a pool the configuration does not name.
fn restored(h: &Harness) {
    *h.storage.tank_absent.lock().unwrap() = true;
    h.storage.known.lock().unwrap().extend([
        KnownPool {
            name: "tank".into(),
            volume: VolumeNumber::new(1).unwrap(),
            shares: vec!["photos".into()],
        },
        KnownPool { name: "cold".into(), volume: VolumeNumber::new(2).unwrap(), shares: Vec::new() },
    ]);
    h.storage
        .importable
        .lock()
        .unwrap()
        .extend([found("tank", 9_846_106_214_133_009_000, true), found("stray", 42, false)]);
}

#[tokio::test]
async fn administrators_see_the_pools_to_import_and_import_them() {
    let h = Harness::new();
    restored(&h);
    let admin = h.sign_in("alice", Role::Admin).await;
    let absent = h.send(empty(with_cookie(get("/api/v1/storage/pools/absent"), &admin))).await;
    assert_eq!(absent.status, StatusCode::OK, "{:?}", absent.body);
    assert_eq!(
        absent.body["known"],
        json!([{
            "pool": { "name": "tank", "volume": 1, "mount_point": "/volume1", "shares": ["photos"] },
            "found": {
                "name": "tank", "id": "9846106214133009000", "state": "ONLINE", "other_host": true,
                "can_import": true, "status": "The pool was last accessed by another system.",
                "action": "The pool can be imported using its name or numeric identifier.",
                "devices": [{ "name": "ata-TEST_HDD_T3-part1", "state": "ONLINE" }],
            },
        }])
    );
    assert_eq!(
        absent.body["missing"],
        json!([{ "name": "cold", "volume": 2, "mount_point": "/volume2", "shares": [] }])
    );
    assert_eq!(absent.body["foreign"][0]["name"], "stray");
    assert_eq!(absent.body["free_volume"], 3);

    let import = |body: Value| h.send(json(with_csrf(post("/api/v1/storage/pools/import"), &admin), body));
    let tank = || json!({ "name": "tank", "id": "9846106214133009000" });
    assert_problem(&import(tank()).await, StatusCode::CONFLICT, "other_host");
    let mut mistyped = tank();
    mistyped["force"] = json!(true);
    mistyped["confirm"] = json!("tnak");
    assert_problem(&import(mistyped).await, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");
    let odd = import(json!({ "name": "tank", "id": "-1" })).await;
    assert_problem(&odd, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let unknown = import(json!({ "name": "tank", "id": "7" })).await;
    assert_problem(&unknown, StatusCode::NOT_FOUND, "not_found");

    let mut forced = tank();
    forced["force"] = json!(true);
    forced["confirm"] = json!("tank");
    let started = import(forced).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    let job = job_done(&h, started.body["id"].as_i64().unwrap()).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded, "{:?}", job.error);

    let taken = import(json!({ "name": "stray", "id": "42", "volume": 1 })).await;
    assert_problem(&taken, StatusCode::CONFLICT, "volume_taken");
    let adopted = import(json!({ "name": "stray", "id": "42" })).await;
    assert_eq!(adopted.status, StatusCode::ACCEPTED);
    job_done(&h, adopted.body["id"].as_i64().unwrap()).await;
    assert_eq!(
        h.storage.calls(),
        [
            "import tank 9846106214133009000 1 adopt=false force=true",
            "mount tank 1 photos",
            "import stray 42 3 adopt=true force=false",
            "mount stray 3 ",
        ]
    );
    assert_eq!(*h.storage.volumes_back.lock().unwrap(), [1, 3], "Samba and NFS followed each");
    let imported: Vec<_> = h
        .identity
        .audit
        .entries()
        .into_iter()
        .filter(|entry| entry.action == "storage.import_pool")
        .map(|entry| entry.target)
        .collect();
    assert_eq!(imported, [Some("tank".to_owned()), Some("stray".to_owned())]);
}

#[tokio::test]
async fn importing_is_for_administrators_only() {
    let h = Harness::new();
    restored(&h);
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let absent = h.send(empty(with_cookie(get("/api/v1/storage/pools/absent"), &viewer))).await;
    assert_problem(&absent, StatusCode::FORBIDDEN, "forbidden");
    let body = json!({ "name": "stray", "id": "42" });
    let import = h.send(json(with_csrf(post("/api/v1/storage/pools/import"), &viewer), body)).await;
    assert_problem(&import, StatusCode::FORBIDDEN, "forbidden");
    assert!(h.storage.calls().is_empty());
}
