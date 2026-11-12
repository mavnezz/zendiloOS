//! Virtual machines through the API: the plugin refused without KVM,
//! installed and set up; VMs made in a job with several disks, cards and
//! drives, changed (hardware only while off), started, stopped and removed
//! with their name typed again; the image library filled from a share and
//! by an upload; the load on the live WebSocket; the console over its
//! WebSocket; all for administrators only.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

pub(super) async fn finished(h: &Harness, job: &Value) -> zendiloos_jobs::Job {
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

pub(super) async fn send(h: &Harness, session: &Session, method: Method, uri: &str, body: Value) -> Reply {
    let builder = with_csrf(changing(method, uri), session);
    h.send(if body.is_null() { empty(builder) } else { json(builder, body) }).await
}

pub(super) async fn read(h: &Harness, session: &Session, uri: &str) -> Reply {
    h.send(empty(with_cookie(get(uri), session))).await
}

/// Installed and set up on `tank`.
pub(super) async fn ready(h: &Harness, admin: &Session) {
    let install = send(h, admin, Method::POST, "/api/v1/vms/plugin/install", Value::Null).await;
    assert_eq!(install.status, StatusCode::ACCEPTED, "{:?}", install.body);
    assert_eq!(finished(h, &install.body).await.state, zendiloos_jobs::JobState::Succeeded);
    let storage = send(h, admin, Method::POST, "/api/v1/vms/plugin/storage", json!({ "pool": "tank" })).await;
    assert_eq!(storage.status, StatusCode::OK, "{:?}", storage.body);
}

pub(super) fn small(name: &str) -> Value {
    json!({ "name": name, "os": "linux", "cpus": 1, "memory_mib": 256, "firmware": "uefi",
            "disks": [{ "slot": 0, "size_gib": 1, "bus": "virtio" }],
            "nics": [{ "slot": 0, "model": "virtio", "network": { "kind": "user" } }],
            "drives": [{ "slot": 0, "image": null }],
            "boot": [{ "kind": "drive", "slot": 0 }, { "kind": "disk", "slot": 0 }] })
}

/// What a VM has now, as a change asks for it.
pub(super) fn change_of(vm: &Value) -> Value {
    let nics: Vec<Value> = vm["nics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|nic| json!({ "slot": nic["slot"], "model": nic["model"], "network": nic["network"] }))
        .collect();
    json!({ "name": vm["name"], "cpus": vm["cpus"], "memory_mib": vm["memory_mib"], "firmware": vm["firmware"],
            "disks": vm["disks"], "nics": nics, "drives": vm["drives"], "boot": vm["boot"],
            "autostart": vm["autostart"], "usb": vm["usb"], "pci": vm["pci"], "tpm": vm["tpm"],
            "secure_boot": vm["secure_boot"] })
}

/// An ISO copied into the library from share 1: its id.
async fn imported(h: &Harness, admin: &Session, path: &str) -> String {
    let added =
        send(h, admin, Method::POST, "/api/v1/vms/images", json!({ "share_id": 1, "path": path })).await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{:?}", added.body);
    let job = finished(h, &added.body["job"]).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded, "{:?}", job.error);
    added.body["image"]["id"].as_str().unwrap().to_owned()
}

async fn upload(h: &Harness, admin: &Session, name: &str, length: usize, body: &'static [u8]) -> Reply {
    let request = with_csrf(changing(Method::POST, &format!("/api/v1/vms/images/upload?name={name}")), admin)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, length.to_string())
        .body(Body::from(body))
        .unwrap();
    h.send(request).await
}

#[tokio::test]
async fn the_plugin_says_what_the_machine_offers_and_needs_kvm() {
    let h = Harness::new();
    let user = h.sign_in("bob", Role::User).await;
    assert_problem(&read(&h, &user, "/api/v1/vms/plugin").await, StatusCode::FORBIDDEN, "forbidden");
    let admin = h.sign_in("alice", Role::Admin).await;

    let plugin = read(&h, &admin, "/api/v1/vms/plugin").await;
    assert_eq!(plugin.status, StatusCode::OK);
    assert_eq!(
        plugin.body["hardware"],
        json!({
            "architecture": "amd64", "architecture_supported": true,
            "kvm": true, "cpu_virtualization": "vmx", "iommu": false, "swtpm": true, "secure_boot": true
        })
    );
    assert_eq!((plugin.body["installed"].as_bool(), plugin.body["cores"].as_u64()), (Some(false), Some(2)));
    assert_eq!(plugin.body["storage"], Value::Null);
    let pools: Vec<(&str, &str)> = plugin.body["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pool| (pool["pool"].as_str().unwrap(), pool["health"].as_str().unwrap()))
        .collect();
    assert_eq!(pools, [("tank", "online"), ("cold", "online")]);
    assert_eq!(plugin.body["bridges"], json!(["br0"]));
    assert_eq!(plugin.body["internal_networks"], json!(["vmnet0"]));
    assert_eq!(plugin.body["cards"], json!(["enp1s0", "bond0"]));
    assert_eq!(plugin.body["locations"], json!([]));
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

    // Not offered on arm64 yet, and said so.
    h.vm_host.facts.lock().unwrap().architecture = "arm64".into();
    let plugin = read(&h, &admin, "/api/v1/vms/plugin").await;
    assert_eq!(plugin.body["hardware"]["architecture_supported"], json!(false));
    let refused = send(&h, &admin, Method::POST, "/api/v1/vms/plugin/install", Value::Null).await;
    assert_problem(&refused, StatusCode::CONFLICT, "vms_architecture");
    assert_eq!(refused.body["detail"], json!("arm64"));
    h.vm_host.facts.lock().unwrap().architecture = "amd64".into();

    ready(&h, &admin).await;
    let plugin = read(&h, &admin, "/api/v1/vms/plugin").await;
    assert_eq!(plugin.body["storage"], json!({ "pool": "tank", "volume": 1 }));
    assert_eq!(plugin.body["locations"], json!([{ "pool": "tank", "volume": 1 }]));
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

    let mut too_big = small("big");
    too_big["cpus"] = json!(8);
    let refused = send(&h, &admin, Method::POST, "/api/v1/vms", too_big).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let mut unknown_image = small("image");
    unknown_image["drives"] = json!([{ "slot": 0, "image": "00000000000f" }]);
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", unknown_image).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let unknown =
        send(&h, &admin, Method::POST, "/api/v1/vms", json!({ "name": "x", "xml": "<domain/>" })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "unknown fields are refused");
    let mut mac = small("mac");
    mac["nics"][0]["mac"] = json!("52:54:00:ff:ff:ff");
    let chosen = send(&h, &admin, Method::POST, "/api/v1/vms", mac).await;
    assert_eq!(chosen.status, StatusCode::UNPROCESSABLE_ENTITY, "a MAC address is not the client's to give");

    let image = imported(&h, &admin, "isos/debian.iso").await;
    let mut new = small("Debian");
    new["disks"] =
        json!([{ "slot": 0, "size_gib": 1, "bus": "virtio" }, { "slot": 1, "size_gib": 4, "bus": "sata" }]);
    new["nics"] = json!([{ "slot": 0, "model": "virtio", "network": { "kind": "user" } },
                         { "slot": 1, "model": "e1000e", "network": { "kind": "bridge", "bridge": "br0" } }]);
    new["drives"] = json!([{ "slot": 0, "image": image }, { "slot": 1, "image": null }]);
    new["autostart"] = json!(true);
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", new).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{:?}", created.body);
    let vm = &created.body["vm"];
    assert_eq!(vm["state"], "creating");
    assert_eq!(vm["disks"].as_array().unwrap().len(), 2);
    assert_eq!(vm["nics"][1]["network"], json!({ "kind": "bridge", "bridge": "br0" }));
    let macs = [vm["nics"][0]["mac"].as_str().unwrap(), vm["nics"][1]["mac"].as_str().unwrap()];
    assert!(macs.iter().all(|mac| mac.starts_with("52:54:00:")) && macs[0] != macs[1], "{macs:?}");
    assert_eq!(vm["drives"][0], json!({ "slot": 0, "image": image }));
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
    assert_eq!(
        listed.body[0]["boot"],
        json!([{ "kind": "drive", "slot": 0 }, { "kind": "disk", "slot": 0 }])
    );

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
    assert!(calls.contains(&format!("copy_image {image} media/isos/debian.iso")), "{calls:?}");
    assert_eq!(calls.last().unwrap(), &format!("remove {id}"));
    let library = read(&h, &admin, "/api/v1/vms/images").await;
    assert_eq!(library.body.as_array().unwrap().len(), 1, "the image stays");

    let audit = read(&h, &admin, "/api/v1/logs/audit").await;
    let text = audit.body.to_string();
    for action in [
        "vms.install",
        "vms.storage",
        "vms.image_add",
        "vms.create",
        "vms.start",
        "vms.force_off",
        "vms.delete",
    ] {
        assert!(text.contains(action), "{action} not in the audit log");
    }
}

#[tokio::test]
async fn a_vm_changes_its_hardware_only_while_it_is_off() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    ready(&h, &admin).await;
    let image = imported(&h, &admin, "isos/debian.iso").await;
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", small("box")).await;
    finished(&h, &created.body["job"]).await;
    let id = created.body["vm"]["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/v1/vms/{id}");
    let vm = read(&h, &admin, &uri).await.body;
    let mac = vm["nics"][0]["mac"].clone();

    // Off: vCPUs and memory, a second disk.
    let mut change = change_of(&vm);
    change["cpus"] = json!(2);
    change["memory_mib"] = json!(512);
    change["disks"] =
        json!([{ "slot": 0, "size_gib": 2, "bus": "virtio" }, { "slot": 1, "size_gib": 4, "bus": "sata" }]);
    let changed = send(&h, &admin, Method::PUT, &uri, change.clone()).await;
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert_eq!((changed.body["cpus"].as_u64(), changed.body["memory_mib"].as_u64()), (Some(2), Some(512)));
    assert_eq!(changed.body["nics"][0]["mac"], mac, "a card keeps its address");
    let mut shrunk = change.clone();
    shrunk["disks"][0]["size_gib"] = json!(1);
    assert_problem(
        &send(&h, &admin, Method::PUT, &uri, shrunk).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let mut fewer = change.clone();
    fewer["disks"] = json!([{ "slot": 0, "size_gib": 2, "bus": "virtio" }]);
    assert_problem(
        &send(&h, &admin, Method::PUT, &uri, fewer.clone()).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "confirmation_mismatch",
    );

    // Running: hardware is refused; the name and a medium change.
    send(&h, &admin, Method::POST, &format!("{uri}/start"), Value::Null).await;
    let mut more = change.clone();
    more["cpus"] = json!(1);
    assert_problem(&send(&h, &admin, Method::PUT, &uri, more).await, StatusCode::CONFLICT, "vm_running");
    fewer["confirm"] = json!("box");
    assert_problem(&send(&h, &admin, Method::PUT, &uri, fewer).await, StatusCode::CONFLICT, "vm_running");
    let mut live = change.clone();
    live["name"] = json!("renamed");
    live["drives"] = json!([{ "slot": 0, "image": image }]);
    let inserted = send(&h, &admin, Method::PUT, &uri, live.clone()).await;
    assert_eq!(inserted.status, StatusCode::OK, "{:?}", inserted.body);
    assert_eq!(
        (inserted.body["name"].as_str(), inserted.body["state"].as_str()),
        (Some("renamed"), Some("running"))
    );
    assert_eq!(h.vm_host.calls().last().unwrap(), &format!("media {id} 0 {image}"));
    live["drives"] = json!([{ "slot": 0, "image": null }]);
    assert_eq!(send(&h, &admin, Method::PUT, &uri, live).await.status, StatusCode::OK);
    assert_eq!(h.vm_host.calls().last().unwrap(), &format!("media {id} 0 eject"));
    let unknown = json!({ "name": "x", "cpus": 1, "os": "windows" });
    assert_eq!(send(&h, &admin, Method::PUT, &uri, unknown).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(read(&h, &admin, "/api/v1/logs/audit").await.body.to_string().contains("vms.change"));
}

#[tokio::test]
async fn the_library_takes_isos_from_shares_and_uploads_and_keeps_those_in_use() {
    let h = Harness::new();
    let user = h.sign_in("bob", Role::User).await;
    assert_problem(&read(&h, &user, "/api/v1/vms/images").await, StatusCode::FORBIDDEN, "forbidden");
    let admin = h.sign_in("alice", Role::Admin).await;
    let early = upload(&h, &admin, "x.iso", 5, b"hello").await;
    assert_problem(&early, StatusCode::CONFLICT, "vms_not_installed");
    ready(&h, &admin).await;

    let denied = json!({ "share_id": 2, "path": "isos/x.iso" });
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms/images", denied).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "iso_denied",
    );
    let text = json!({ "share_id": 1, "path": "notes/readme.txt" });
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms/images", text).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let debian = imported(&h, &admin, "isos/debian.iso").await;
    let again = json!({ "share_id": 1, "path": "isos/debian.iso" });
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms/images", again).await,
        StatusCode::CONFLICT,
        "name_taken",
    );

    // From the browser: exactly what Content-Length says.
    let uploaded = upload(&h, &admin, "alpine-virt.iso", 5, b"hello").await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{:?}", uploaded.body);
    assert_eq!(uploaded.body["origin"], json!({ "kind": "upload", "file": "alpine-virt.iso" }));
    assert_eq!((uploaded.body["bytes"].as_u64(), uploaded.body["ready"].as_bool()), (Some(5), Some(true)));
    let short = upload(&h, &admin, "short.iso", 10, b"hello").await;
    assert_problem(&short, StatusCode::BAD_REQUEST, "bad_request");
    let no_length = with_csrf(changing(Method::POST, "/api/v1/vms/images/upload?name=y.iso"), &admin)
        .body(Body::from("hello"))
        .unwrap();
    assert_problem(&h.send(no_length).await, StatusCode::BAD_REQUEST, "bad_request");
    let huge = upload(&h, &admin, "huge.iso", 33 << 30, b"").await;
    assert_problem(&huge, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let listed = read(&h, &admin, "/api/v1/vms/images").await;
    let names: Vec<&str> =
        listed.body.as_array().unwrap().iter().map(|image| image["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["debian.iso", "alpine-virt.iso"], "what did not come whole is gone");

    // An image in a VM's drive stays, the VM named.
    let mut new = small("web");
    new["drives"] = json!([{ "slot": 0, "image": debian }]);
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", new).await;
    finished(&h, &created.body["job"]).await;
    let listed = read(&h, &admin, "/api/v1/vms/images").await;
    assert_eq!(listed.body[0]["used_by"], json!(["web"]));
    let image_uri = format!("/api/v1/vms/images/{debian}");
    let in_use = send(&h, &admin, Method::DELETE, &image_uri, Value::Null).await;
    assert_problem(&in_use, StatusCode::CONFLICT, "image_in_use");
    assert_eq!(in_use.body["detail"], "web");

    let renamed = send(&h, &admin, Method::PUT, &image_uri, json!({ "name": "Debian 13" })).await;
    assert_eq!((renamed.status, renamed.body["name"].as_str()), (StatusCode::OK, Some("Debian 13")));
    assert_eq!(renamed.body["used_by"], json!(["web"]));
    let clash = send(&h, &admin, Method::PUT, &image_uri, json!({ "name": "ALPINE-VIRT.ISO" })).await;
    assert_problem(&clash, StatusCode::CONFLICT, "name_taken");

    let alpine = uploaded.body["id"].as_str().unwrap();
    let gone = send(&h, &admin, Method::DELETE, &format!("/api/v1/vms/images/{alpine}"), Value::Null).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert_problem(
        &send(&h, &admin, Method::DELETE, &format!("/api/v1/vms/images/{alpine}"), Value::Null).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let audit = read(&h, &admin, "/api/v1/logs/audit").await.body.to_string();
    for action in ["vms.image_add", "vms.image_upload", "vms.image_rename", "vms.image_delete"] {
        assert!(audit.contains(action), "{action} not in the audit log");
    }
}

#[tokio::test]
async fn the_load_of_running_vms_comes_on_the_live_websocket_for_administrators() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    ready(&h, &admin).await;
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", small("box")).await;
    finished(&h, &created.body["job"]).await;
    let id = created.body["vm"]["id"].as_str().unwrap().to_owned();
    send(&h, &admin, Method::POST, &format!("/api/v1/vms/{id}/start"), Value::Null).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let connect = async |session: &Session| {
        let mut request = format!("ws://{address}/api/v1/ws").into_client_request().unwrap();
        request.headers_mut().insert(header::COOKIE, session.cookie.parse().unwrap());
        request.headers_mut().insert(header::ORIGIN, format!("http://{address}").parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let subscribe = json!({ "type": "subscribe", "topic": "vm_stats" }).to_string();
        socket.send(Message::Text(subscribe.into())).await.unwrap();
        socket
    };

    let mut socket = connect(&admin).await;
    let sample = loop {
        let message =
            tokio::time::timeout(Duration::from_secs(10), socket.next()).await.unwrap().unwrap().unwrap();
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["type"] == "vm_stats" {
            break value["data"].clone();
        }
    };
    assert_eq!((sample["running"].as_u64(), sample["total"].as_u64()), (Some(1), Some(1)));
    assert_eq!(sample["vms"][0]["id"], id.as_str());
    assert_eq!(sample["vms"][0]["memory_bytes"], 256 << 20);
    assert_eq!(sample["host_cores"], 2);

    // Someone without the permission is told nothing.
    let mut theirs = connect(&user).await;
    let nothing = tokio::time::timeout(Duration::from_millis(500), theirs.next()).await;
    assert!(nothing.is_err(), "{nothing:?}");
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

/// Locations: a second one added, a VM with a disk on each, what each holds
/// listed, a disk that would move refused, and a location removed only once
/// empty with its pool's name typed again.
#[tokio::test]
async fn disks_go_on_locations_which_go_only_once_empty() {
    let h = Harness::new();
    let user = h.sign_in("bob", Role::User).await;
    assert_problem(&read(&h, &user, "/api/v1/vms/locations").await, StatusCode::FORBIDDEN, "forbidden");
    let admin = h.sign_in("alice", Role::Admin).await;
    ready(&h, &admin).await;
    let added = send(&h, &admin, Method::POST, "/api/v1/vms/locations", json!({ "pool": "cold" })).await;
    assert_eq!(added.status, StatusCode::OK, "{:?}", added.body);
    assert_eq!(
        added.body["locations"],
        json!([{ "pool": "tank", "volume": 1 }, { "pool": "cold", "volume": 2 }])
    );
    assert_eq!(added.body["storage"], json!({ "pool": "tank", "volume": 1 }), "the library stays");
    let nowhere = send(&h, &admin, Method::POST, "/api/v1/vms/locations", json!({ "pool": "lost" })).await;
    assert_problem(&nowhere, StatusCode::NOT_FOUND, "not_found");

    let mut new = small("two");
    new["disks"] = json!([{ "slot": 0, "size_gib": 1, "bus": "virtio" },
                          { "slot": 1, "size_gib": 4, "bus": "sata", "pool": "cold" }]);
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", new).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{:?}", created.body);
    finished(&h, &created.body["job"]).await;
    let id = created.body["vm"]["id"].as_str().unwrap().to_owned();
    let vm = read(&h, &admin, &format!("/api/v1/vms/{id}")).await.body;
    assert_eq!(vm["disks"][0]["pool"], "tank");
    assert_eq!((vm["disks"][1]["pool"].as_str(), vm["disks"][1]["volume"].as_u64()), (Some("cold"), Some(2)));
    let mut lost = small("lost");
    lost["disks"][0]["pool"] = json!("lost");
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", lost).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );

    let listed = read(&h, &admin, "/api/v1/vms/locations").await;
    assert_eq!(listed.status, StatusCode::OK);
    let cold = &listed.body[1];
    assert_eq!(
        (cold["pool"].as_str(), cold["library"].as_bool(), cold["health"].as_str()),
        (Some("cold"), Some(false), Some("online"))
    );
    assert_eq!(
        cold["vms"],
        json!([{ "id": id, "name": "two", "disks": [1], "used_bytes": 1u64 << 30,
                                      "referenced_bytes": 1u64 << 29, "size_bytes": 4u64 << 30 }])
    );
    assert!(listed.body[0]["library"].as_bool().unwrap() && cold["used_bytes"].as_u64().is_some());

    // As GET shows the disks, back in a change: kept where they are; moved: refused.
    let same = send(&h, &admin, Method::PUT, &format!("/api/v1/vms/{id}"), change_of(&vm)).await;
    assert_eq!(same.status, StatusCode::OK, "{:?}", same.body);
    let mut moved = change_of(&vm);
    moved["disks"][1]["pool"] = json!("tank");
    let refused = send(&h, &admin, Method::PUT, &format!("/api/v1/vms/{id}"), moved).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let remove = |confirm: &str| json!({ "confirm": confirm });
    let wrong = send(&h, &admin, Method::POST, "/api/v1/vms/locations/cold/delete", remove("tank")).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch");
    let used = send(&h, &admin, Method::POST, "/api/v1/vms/locations/cold/delete", remove("cold")).await;
    assert_problem(&used, StatusCode::CONFLICT, "location_in_use");
    assert_eq!(used.body["detail"], "two");
    let library = send(&h, &admin, Method::POST, "/api/v1/vms/locations/tank/delete", remove("tank")).await;
    assert_problem(&library, StatusCode::CONFLICT, "location_in_use");
    assert_eq!(library.body["detail"], "two, the image library");
    let none = send(&h, &admin, Method::POST, "/api/v1/vms/locations/lost/delete", remove("lost")).await;
    assert_problem(&none, StatusCode::NOT_FOUND, "not_found");

    send(&h, &admin, Method::POST, &format!("/api/v1/vms/{id}/delete"), json!({ "confirm": "two" })).await;
    let gone = send(&h, &admin, Method::POST, "/api/v1/vms/locations/cold/delete", remove("cold")).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{:?}", gone.body);
    let listed = read(&h, &admin, "/api/v1/vms/locations").await;
    assert_eq!(listed.body.as_array().unwrap().len(), 1);

    // Only the VMs' entries of the audit log, searched.
    let log = read(&h, &admin, "/api/v1/logs/audit?prefix=vms.&search=COLD").await;
    let actions: Vec<&str> =
        log.body["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert_eq!(
        actions,
        ["vms.location_remove", "vms.location_add"],
        "refusals before the host are not done: {:?}",
        log.body
    );
    let long = read(&h, &admin, &format!("/api/v1/logs/audit?search={}", "x".repeat(101))).await;
    assert_problem(&long, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

/// Cards directly on a card and on an internal network; one on an internal
/// network the machine has not is refused.
#[tokio::test]
async fn cards_go_directly_on_a_card_and_on_internal_networks() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    ready(&h, &admin).await;
    let mut new = small("net");
    new["nics"] = json!([{ "slot": 0, "model": "virtio", "network": { "kind": "direct", "card": "enp1s0" } },
                         { "slot": 1, "model": "e1000e", "network": { "kind": "internal", "network": "vmnet0" } }]);
    let created = send(&h, &admin, Method::POST, "/api/v1/vms", new.clone()).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{:?}", created.body);
    assert_eq!(created.body["vm"]["nics"][0]["network"], json!({ "kind": "direct", "card": "enp1s0" }));
    assert_eq!(created.body["vm"]["nics"][1]["network"], json!({ "kind": "internal", "network": "vmnet0" }));
    let mut wrong = new.clone();
    wrong["name"] = json!("wrong");
    wrong["nics"] =
        json!([{ "slot": 0, "model": "virtio", "network": { "kind": "internal", "network": "br0" } }]);
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", wrong.clone()).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    wrong["nics"] = json!([{ "slot": 0, "model": "virtio", "network": { "kind": "direct", "card": "enp1s0", "mode": "vepa" } }]);
    assert_eq!(
        send(&h, &admin, Method::POST, "/api/v1/vms", wrong).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}
