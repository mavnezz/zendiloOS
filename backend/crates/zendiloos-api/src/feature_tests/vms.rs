//! Virtual machines through the API: the plugin refused without KVM,
//! installed and set up; VMs made in a job, started, stopped and removed
//! with their name typed again; the console over its WebSocket; all for
//! administrators only.

use std::time::Duration;

use axum::http::{Method, StatusCode, header};
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

/// Installed and set up on `tank`.
async fn ready(h: &Harness, admin: &Session) {
    let install = send(h, admin, Method::POST, "/api/v1/vms/plugin/install", Value::Null).await;
    assert_eq!(install.status, StatusCode::ACCEPTED, "{:?}", install.body);
    assert_eq!(finished(h, &install.body).await.state, zendiloos_jobs::JobState::Succeeded);
    let storage = send(h, admin, Method::POST, "/api/v1/vms/plugin/storage", json!({ "pool": "tank" })).await;
    assert_eq!(storage.status, StatusCode::OK, "{:?}", storage.body);
}

fn small(name: &str) -> Value {
    json!({ "name": name, "os": "linux", "cpus": 1, "memory_mib": 256, "firmware": "uefi", "disk_gib": 1,
            "network": { "kind": "user" } })
}

#[tokio::test]
async fn the_plugin_says_what_the_machine_offers_and_needs_kvm() {
    let h = Harness::new();
    let user = h.sign_in("bob", Role::User).await;
    assert_problem(&read(&h, &user, "/api/v1/vms/plugin").await, StatusCode::FORBIDDEN, "forbidden");
    let admin = h.sign_in("alice", Role::Admin).await;

    let plugin = read(&h, &admin, "/api/v1/vms/plugin").await;
    assert_eq!(plugin.status, StatusCode::OK);
    assert_eq!(plugin.body["hardware"], json!({ "kvm": true, "cpu_virtualization": "vmx", "iommu": false }));
    assert_eq!((plugin.body["installed"].as_bool(), plugin.body["cores"].as_u64()), (Some(false), Some(2)));
    assert_eq!(plugin.body["storage"], Value::Null);
    assert_eq!(
        plugin.body["pools"],
        json!([{ "pool": "tank", "volume": 1 }, { "pool": "cold", "volume": 2 }])
    );
    assert_eq!(plugin.body["bridges"], json!(["br0"]));
    let early = send(&h, &admin, Method::POST, "/api/v1/vms/plugin/storage", json!({ "pool": "tank" })).await;
    assert_problem(&early, StatusCode::CONFLICT, "vms_not_installed");
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", small("a")).await,
        StatusCode::CONFLICT,
        "vms_not_installed",
    );

    h.vm_host.facts.lock().unwrap().kvm = false;
    let refused = send(&h, &admin, Method::POST, "/api/v1/vms/plugin/install", Value::Null).await;
    assert_problem(&refused, StatusCode::CONFLICT, "kvm_missing");
    h.vm_host.facts.lock().unwrap().kvm = true;

    ready(&h, &admin).await;
    let plugin = read(&h, &admin, "/api/v1/vms/plugin").await;
    assert_eq!(plugin.body["storage"], json!({ "pool": "tank", "volume": 1 }));
    assert!(plugin.body["installed"].as_bool().unwrap());
    let again = send(&h, &admin, Method::POST, "/api/v1/vms/plugin/install", Value::Null).await;
    assert_problem(&again, StatusCode::CONFLICT, "vms_installed");
    let nowhere =
        send(&h, &admin, Method::POST, "/api/v1/vms/plugin/storage", json!({ "pool": "nowhere" })).await;
    assert_problem(&nowhere, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn a_vm_is_made_run_and_removed() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    ready(&h, &admin).await;

    let too_big = json!({ "name": "big", "os": "linux", "cpus": 8, "memory_mib": 256, "firmware": "uefi",
                          "disk_gib": 1, "network": { "kind": "user" } });
    let refused = send(&h, &admin, Method::POST, "/api/v1/vms", too_big).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let mut text = small("text");
    text["iso"] = json!({ "share_id": 1, "path": "notes/readme.txt" });
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", text).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let mut denied = small("denied");
    denied["iso"] = json!({ "share_id": 2, "path": "isos/x.iso" });
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", denied).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "iso_denied",
    );
    let unknown =
        send(&h, &admin, Method::POST, "/api/v1/vms", json!({ "name": "x", "xml": "<domain/>" })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "unknown fields are refused");

    let mut new = small("Debian");
    new["iso"] = json!({ "share_id": 1, "path": "isos/debian.iso" });
    new["autostart"] = json!(true);
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", new).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{:?}", created.body);
    let vm = &created.body["vm"];
    assert_eq!(vm["state"], "creating");
    assert_eq!(vm["iso"], json!({ "share": "media", "path": "isos/debian.iso" }));
    let id = vm["id"].as_str().unwrap().to_owned();
    let job = finished(&h, &created.body["job"]).await;
    assert_eq!(job.kind, "vms.create");
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded, "{:?}", job.error);
    let taken = send(&h, &admin, Method::POST, "/api/v1/vms", small("debian")).await;
    assert_problem(&taken, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let listed = read(&h, &admin, "/api/v1/vms").await;
    assert_eq!(listed.body.as_array().unwrap().len(), 1);
    assert_eq!(listed.body[0]["state"], "shut_off");
    assert_eq!(listed.body[0]["autostart"], true);
    assert_eq!(listed.body[0]["network"], json!({ "kind": "user" }));

    let path = |action: &str| format!("/api/v1/vms/{id}/{action}");
    let started = send(&h, &admin, Method::POST, &path("start"), Value::Null).await;
    assert_eq!((started.status, started.body["state"].as_str()), (StatusCode::OK, Some("running")));
    let twice = send(&h, &admin, Method::POST, &path("start"), Value::Null).await;
    assert_problem(&twice, StatusCode::CONFLICT, "refused");
    assert_eq!(send(&h, &admin, Method::POST, &path("restart"), Value::Null).await.status, StatusCode::OK);
    let off = send(&h, &admin, Method::POST, &path("force-off"), Value::Null).await;
    assert_eq!(off.body["state"], "shut_off");
    let autostart = send(&h, &admin, Method::PUT, &path("autostart"), json!({ "autostart": false })).await;
    assert_eq!(autostart.body["autostart"], false);

    let wrong = send(&h, &admin, Method::POST, &path("delete"), json!({ "confirm": "debian" })).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");
    let deleted = send(&h, &admin, Method::POST, &path("delete"), json!({ "confirm": "Debian" })).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_problem(&read(&h, &admin, &format!("/api/v1/vms/{id}")).await, StatusCode::NOT_FOUND, "not_found");
    let calls = h.vm_host.calls();
    assert!(calls.contains(&format!("copy_iso {id} media/isos/debian.iso")), "{calls:?}");
    assert_eq!(calls.last().unwrap(), &format!("remove {id}"));

    let audit = read(&h, &admin, "/api/v1/logs/audit").await;
    let text = audit.body.to_string();
    for action in ["vms.install", "vms.storage", "vms.create", "vms.start", "vms.force_off", "vms.delete"] {
        assert!(text.contains(action), "{action} not in the audit log");
    }
}

#[tokio::test]
async fn the_console_relays_rfb_over_a_websocket_of_the_same_origin() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    ready(&h, &admin).await;
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", small("box")).await;
    finished(&h, &created.body["job"]).await;
    let id = created.body["vm"]["id"].as_str().unwrap().to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let open = |origin: &str| {
        let mut request = format!("ws://{address}/api/v1/vms/{id}/console").into_client_request().unwrap();
        request.headers_mut().insert(header::COOKIE, admin.cookie.parse().unwrap());
        request.headers_mut().insert(header::ORIGIN, origin.parse().unwrap());
        request.headers_mut().insert(header::SEC_WEBSOCKET_PROTOCOL, "binary".parse().unwrap());
        request
    };
    // Another site cannot open it with the visitor's cookie.
    assert!(tokio_tungstenite::connect_async(open("https://evil.example")).await.is_err());

    let next = async |socket: &mut tokio_tungstenite::WebSocketStream<_>| {
        tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap()
    };
    // Not running: closed with the reason.
    let (mut socket, _) = tokio_tungstenite::connect_async(open(&format!("http://{address}"))).await.unwrap();
    match next(&mut socket).await {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4000);
            assert_eq!(frame.reason.as_str(), "the virtual machine is not running");
        }
        other => panic!("not closed: {other:?}"),
    }

    send(&h, &admin, Method::POST, &format!("/api/v1/vms/{id}/start"), Value::Null).await;
    let (mut socket, response) =
        tokio_tungstenite::connect_async(open(&format!("http://{address}"))).await.unwrap();
    assert_eq!(response.headers()[header::SEC_WEBSOCKET_PROTOCOL], "binary");
    assert_eq!(next(&mut socket).await, Message::Binary(b"RFB 003.008\n".to_vec().into()));
    socket.send(Message::Binary(b"RFB 003.008\n".to_vec().into())).await.unwrap();
    assert_eq!(
        next(&mut socket).await,
        Message::Binary(b"RFB 003.008\n".to_vec().into()),
        "echoed by the fake"
    );
    assert!(h.vm_host.calls().contains(&format!("console {id}")));
}
