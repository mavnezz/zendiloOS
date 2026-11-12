//! The network through the API: taken over from ifupdown in a job and kept
//! once confirmed, what would not be carried over named, interfaces
//! switched on and off; bridges, bonds and VLANs planned, made, changed and
//! removed, a bridge a VM is on kept.

use std::future::Future;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

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

async fn setup(h: &Harness, session: &Session) -> Value {
    let reply = h.send(empty(with_cookie(get("/api/v1/network/setup"), session))).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    reply.body
}

async fn take_over(h: &Harness, session: &Session) -> Reply {
    h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/takeover"), session))).await
}

/// What the Debian installer wrote: the card by DHCP.
const INSTALLER: &str = "auto lo\niface lo inet loopback\n\nallow-hotplug enp1s0\niface enp1s0 inet dhcp\n";

#[tokio::test]
async fn administrators_let_zendiloos_take_over_from_ifupdown() {
    let h = Harness::new();
    h.network_host.use_ifupdown(INSTALLER);
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;

    let seen = setup(&h, &viewer).await;
    assert_eq!((seen["managed"].clone(), seen["manager"].clone()), (json!(false), json!("ifupdown")));
    assert_eq!(
        seen["takeover"],
        json!({
            "interfaces": [{
                "name": "enp1s0",
                "ipv4": { "mode": "dhcp", "keep": "192.168.1.20" },
                "ipv6": "automatic",
                "dns": [],
            }],
            "refused": [],
            "installs_resolved": true,
            "ifupdown_running": true,
        })
    );
    assert_problem(&take_over(&h, &viewer).await, StatusCode::FORBIDDEN, "forbidden");
    // Nothing to change before it is taken over.
    let dhcp = json!({ "ipv4": { "mode": "dhcp" }, "ipv6": "automatic", "dns": [] });
    let refused = h
        .send(json(with_csrf(changing(Method::PUT, "/api/v1/network/interfaces/enp1s0"), &admin), dhcp))
        .await;
    assert_problem(&refused, StatusCode::CONFLICT, "refused");

    let started = take_over(&h, &admin).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    assert_eq!(started.body["kind"], "network.take_over");
    let job = finished(&h, &started.body).await;
    assert_eq!(job.state, zendiloos_jobs::JobState::Succeeded, "{:?}", job.error);
    assert_eq!(*h.network_host.installs.lock().unwrap(), 1, "systemd-resolved came first");

    let waiting = setup(&h, &viewer).await;
    assert_eq!((waiting["managed"].clone(), waiting["manager"].clone()), (json!(true), json!("networkd")));
    assert_eq!(waiting["takeover"], Value::Null);
    assert_eq!(
        (waiting["pending"]["takeover"].clone(), waiting["pending"]["interface"].clone()),
        (json!(true), json!(""))
    );
    assert_problem(&take_over(&h, &admin).await, StatusCode::CONFLICT, "change_pending");

    let confirmed = h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/confirm"), &admin))).await;
    assert_eq!(confirmed.status, StatusCode::NO_CONTENT);
    assert_eq!(setup(&h, &admin).await["pending"], Value::Null);
    let again = take_over(&h, &admin).await;
    assert_problem(&again, StatusCode::CONFLICT, "refused");
    assert!(again.body["detail"].as_str().unwrap().contains("already"), "{:?}", again.body);
}

#[tokio::test]
async fn what_would_not_be_carried_over_is_named_and_keeps_ifupdown() {
    let h = Harness::new();
    h.network_host.use_ifupdown("auto enp1s0\niface enp1s0 inet dhcp\n  post-up /usr/local/bin/hello\n");
    let admin = h.sign_in("alice", Role::Admin).await;
    let seen = setup(&h, &admin).await;
    assert_eq!(
        seen["takeover"]["refused"],
        json!([{ "place": "interfaces:2", "reason": "enp1s0: post-up: runs commands" }])
    );
    let refused = take_over(&h, &admin).await;
    assert_problem(&refused, StatusCode::CONFLICT, "not_carried_over");
    assert_eq!(refused.body["detail"], "interfaces:2: enp1s0: post-up: runs commands");
    assert!(h.network_host.taken_over.lock().unwrap().is_empty());

    // Where NetworkManager runs, it is only named.
    *h.network_host.network_manager.lock().unwrap() = true;
    let seen = setup(&h, &admin).await;
    assert_eq!((seen["manager"].clone(), seen["takeover"].clone()), (json!("network_manager"), Value::Null));
}

#[tokio::test]
async fn an_interface_is_switched_off_and_on_again() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let put = |body: Value| {
        h.send(json(with_csrf(changing(Method::PUT, "/api/v1/network/interfaces/enp1s0"), &admin), body))
    };
    let confirm = || h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/confirm"), &admin)));

    let with_dns = json!({ "ipv4": { "mode": "off" }, "ipv6": "off", "dns": ["9.9.9.9"] });
    assert_problem(&put(with_dns).await, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let off = put(json!({ "ipv4": { "mode": "off" }, "ipv6": "off" })).await;
    assert_eq!(off.status, StatusCode::ACCEPTED, "{:?}", off.body);
    assert_eq!(confirm().await.status, StatusCode::NO_CONTENT);
    let on = put(json!({ "ipv4": { "mode": "dhcp" }, "ipv6": "automatic" })).await;
    assert_eq!(on.status, StatusCode::ACCEPTED, "{:?}", on.body);
    assert_eq!(confirm().await.status, StatusCode::NO_CONTENT);

    let applied = h.network_host.applied.lock().unwrap().clone();
    assert_eq!(applied[0].ipv4, zendiloos_network::domain::Ipv4Setting::Off);
    // DHCP asks for the address the card has.
    assert_eq!(
        applied[1].ipv4,
        zendiloos_network::domain::Ipv4Setting::Dhcp { keep: Some("192.168.1.20".parse().unwrap()) }
    );
}

fn post_json(h: &Harness, session: &Session, path: &str, body: Value) -> impl Future<Output = Reply> {
    h.send(json(with_csrf(changing(Method::POST, path), session), body))
}

async fn confirm(h: &Harness, session: &Session) {
    let confirmed =
        h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/confirm"), session))).await;
    assert_eq!(confirmed.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_bridge_is_planned_made_and_kept_once_confirmed() {
    let h = Harness::new();
    h.network_source.set(zendiloos_network::testing::three_cards());
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let bridge = json!({ "kind": "bridge", "name": "br0", "members": ["enp2s0", "enp1s0"] });

    let plan =
        post_json(&h, &viewer, "/api/v1/network/devices/plan", json!({ "action": "make", "device": bridge }))
            .await;
    assert_problem(&plan, StatusCode::FORBIDDEN, "forbidden");
    let plan =
        post_json(&h, &admin, "/api/v1/network/devices/plan", json!({ "action": "make", "device": bridge }))
            .await;
    assert_eq!(plan.status, StatusCode::OK, "{:?}", plan.body);
    assert_eq!(
        plan.body,
        json!({
            "device": {
                "name": "br0", "kind": "bridge", "members": ["enp1s0", "enp2s0"], "parent": null, "vlan_id": null,
                "mode": null, "stp": false, "mac": "52:54:00:12:34:56",
            },
            "interfaces": [
                { "interface": "br0", "becomes": { "fate": "settings", "settings": {
                    "ipv4": { "mode": "dhcp", "keep": "192.168.1.20" }, "ipv6": "off", "dns": [] } } },
                { "interface": "enp1s0", "becomes": { "fate": "member", "device": "br0", "moved": true, "dropped": false } },
                { "interface": "enp2s0", "becomes": { "fate": "member", "device": "br0", "moved": false, "dropped": false } },
            ],
        })
    );
    assert!(h.network_host.changes.lock().unwrap().is_empty(), "a plan changes nothing");

    assert_problem(
        &post_json(&h, &viewer, "/api/v1/network/devices", bridge.clone()).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let made = post_json(&h, &admin, "/api/v1/network/devices", bridge.clone()).await;
    assert_eq!(made.status, StatusCode::ACCEPTED, "{:?}", made.body);
    assert_eq!(
        made.body["pending"],
        json!({ "interface": "br0", "takeover": false, "restore": false, "device": "made", "revert_at": 120 })
    );
    assert_eq!(made.body["devices"][0]["members"], json!(["enp1s0", "enp2s0"]));
    assert_problem(
        &post_json(&h, &admin, "/api/v1/network/devices", bridge).await,
        StatusCode::CONFLICT,
        "change_pending",
    );
    confirm(&h, &admin).await;
    let seen = setup(&h, &viewer).await;
    assert_eq!((seen["pending"].clone(), seen["devices"][0]["name"].clone()), (Value::Null, json!("br0")));

    // A member's settings are the bridge's.
    let dhcp = json!({ "ipv4": { "mode": "dhcp" }, "ipv6": "automatic" });
    let member = h
        .send(json(with_csrf(changing(Method::PUT, "/api/v1/network/interfaces/enp1s0"), &admin), dhcp))
        .await;
    assert_problem(&member, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert!(member.body["detail"].as_str().unwrap().contains("member of br0"), "{:?}", member.body);

    // A member taken out is switched off.
    let fewer = h
        .send(json(
            with_csrf(changing(Method::PUT, "/api/v1/network/devices/br0"), &admin),
            json!({ "members": ["enp1s0"], "stp": true }),
        ))
        .await;
    assert_eq!(fewer.status, StatusCode::ACCEPTED, "{:?}", fewer.body);
    assert_eq!(fewer.body["pending"]["device"], "changed");
    let change = h.network_host.changes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(change.devices[0].kind, zendiloos_network::domain::DeviceKind::Bridge { stp: true });
    assert!(
        change.links.iter().any(|link| link.settings.interface == "enp2s0" && link.settings.switched_off())
    );
    confirm(&h, &admin).await;
}

/// A VM on a bridge keeps it; removed once it is not, the bridge gives its
/// settings back to its first member.
#[tokio::test]
async fn a_bridge_a_vm_is_on_stays() {
    let h = Harness::new();
    h.network_source.set(zendiloos_network::testing::three_cards());
    let admin = h.sign_in("alice", Role::Admin).await;
    let bridge = json!({ "kind": "bridge", "name": "br0", "members": ["enp1s0"] });
    assert_eq!(post_json(&h, &admin, "/api/v1/network/devices", bridge).await.status, StatusCode::ACCEPTED);
    confirm(&h, &admin).await;
    h.network_users.0.lock().unwrap().push(("br0".into(), "web".into()));
    let remove = || h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/network/devices/br0"), &admin)));

    let refused = remove().await;
    assert_problem(&refused, StatusCode::CONFLICT, "device_in_use");
    assert_eq!(refused.body["detail"], "web");
    let plan =
        post_json(&h, &admin, "/api/v1/network/devices/plan", json!({ "action": "remove", "name": "br0" }))
            .await;
    assert_problem(&plan, StatusCode::CONFLICT, "device_in_use");

    h.network_users.0.lock().unwrap().clear();
    let plan =
        post_json(&h, &admin, "/api/v1/network/devices/plan", json!({ "action": "remove", "name": "br0" }))
            .await;
    assert_eq!(plan.body["device"], Value::Null);
    assert_eq!(plan.body["interfaces"][0]["interface"], "enp1s0");
    assert_eq!(plan.body["interfaces"][0]["becomes"]["fate"], "settings");
    let removed = remove().await;
    assert_eq!(removed.status, StatusCode::ACCEPTED, "{:?}", removed.body);
    assert_eq!(removed.body["pending"]["device"], "removed");
    assert_eq!(removed.body["devices"], json!([]));
}

#[tokio::test]
async fn bonds_and_vlans_are_made_and_what_is_wrong_is_named() {
    let h = Harness::new();
    h.network_source.set(zendiloos_network::testing::three_cards());
    let admin = h.sign_in("alice", Role::Admin).await;
    let invalid = |body: Value, why: &'static str| {
        let h = &h;
        let admin = &admin;
        async move {
            let reply = post_json(h, admin, "/api/v1/network/devices", body).await;
            assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
            let detail = reply.body["detail"].as_str().unwrap_or_default().to_owned();
            assert!(detail.contains(why), "{why}: {detail}");
        }
    };
    invalid(json!({ "kind": "bridge", "name": "enp1s0" }), "there is an interface enp1s0 already").await;
    invalid(json!({ "kind": "bridge", "name": "a-name-far-too-long" }), "no interface name").await;
    invalid(
        json!({ "kind": "bridge", "name": "br0", "members": ["enp9s0"] }),
        "there is no interface enp9s0",
    )
    .await;
    invalid(json!({ "kind": "bond", "name": "bond0", "members": [], "mode": "lacp" }), "at least one member")
        .await;
    invalid(json!({ "kind": "vlan", "name": "v", "parent": "enp1s0", "id": 4095 }), "from 1 to 4094").await;
    let odd = post_json(
        &h,
        &admin,
        "/api/v1/network/devices",
        json!({ "kind": "bond", "name": "b", "members": [], "mode": "broadcast" }),
    )
    .await;
    assert_problem(&odd, StatusCode::UNPROCESSABLE_ENTITY, "invalid_body");
    assert!(h.network_host.changes.lock().unwrap().is_empty());

    let bond =
        json!({ "kind": "bond", "name": "bond0", "members": ["enp2s0", "enp3s0"], "mode": "active_backup" });
    let made = post_json(&h, &admin, "/api/v1/network/devices", bond).await;
    assert_eq!(made.status, StatusCode::ACCEPTED, "{:?}", made.body);
    assert_eq!(made.body["devices"][0]["mode"], "active_backup");
    confirm(&h, &admin).await;
    let vlan = json!({
        "kind": "vlan", "name": "enp1s0.10", "parent": "enp1s0", "id": 10,
        "settings": { "ipv4": { "mode": "static", "address": "10.10.0.2", "prefix": 24, "gateway": null }, "ipv6": "off" },
    });
    let made = post_json(&h, &admin, "/api/v1/network/devices", vlan).await;
    assert_eq!(made.status, StatusCode::ACCEPTED, "{:?}", made.body);
    let change = h.network_host.changes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(change.links[0].vlans, ["enp1s0.10"], "the parent carries it");
    assert_eq!(
        change.links[1].settings.ipv4,
        zendiloos_network::domain::Ipv4Setting::Static {
            address: "10.10.0.2".parse().unwrap(),
            prefix: 24,
            gateway: None
        }
    );
    confirm(&h, &admin).await;
    let devices = setup(&h, &admin).await["devices"].clone();
    assert_eq!(
        devices[1],
        json!({
            "name": "enp1s0.10", "kind": "vlan", "members": [], "parent": "enp1s0", "vlan_id": 10,
            "mode": null, "stp": null, "mac": null,
        })
    );
    let changed = h
        .send(json(
            with_csrf(changing(Method::PUT, "/api/v1/network/devices/bond0"), &admin),
            json!({ "members": ["enp2s0", "enp3s0"], "mode": "lacp" }),
        ))
        .await;
    assert_eq!(changed.status, StatusCode::ACCEPTED, "{:?}", changed.body);
    let unknown = h
        .send(json(
            with_csrf(changing(Method::PUT, "/api/v1/network/devices/br7"), &admin),
            json!({ "members": [] }),
        ))
        .await;
    assert_problem(&unknown, StatusCode::CONFLICT, "change_pending");
}

/// zendiloOS's files changed outside it: told on the setup, other changes
/// refused, written back as a change with confirmation, or kept as they are.
#[tokio::test]
async fn files_changed_outside_zendiloos_are_restored_or_kept() {
    use zendiloos_network::domain::NetworkdFile;
    let h = Harness::new();
    h.network_source.set(zendiloos_network::testing::three_cards());
    let file =
        |name: &str, text: &str| NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() };
    let confirmed = vec![file("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nAddress=10.0.0.5/24\n")];
    *h.network_host.files.lock().unwrap() = confirmed.clone();
    // As zendiloosd's start keeps them.
    *h.network_settings.0.lock().unwrap() = Some(zendiloos_network::application::KeptSettings {
        files: confirmed,
        kept_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
        said: None,
    });
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    assert_eq!(setup(&h, &viewer).await["drift"], Value::Null);

    // Lost, and a file written by hand.
    *h.network_host.files.lock().unwrap() =
        vec![file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nDHCP=ipv4\n")];
    let drift = setup(&h, &viewer).await["drift"].clone();
    assert_eq!(
        drift["differences"],
        json!([
            { "part": "interface", "name": "enp2s0", "kind": "missing" },
            { "part": "interface", "name": "enp3s0", "kind": "added" },
        ])
    );
    let dhcp = json!({ "ipv4": { "mode": "dhcp" }, "ipv6": "automatic", "dns": [] });
    let refused = h
        .send(json(with_csrf(changing(Method::PUT, "/api/v1/network/interfaces/enp1s0"), &admin), dhcp))
        .await;
    assert_problem(&refused, StatusCode::CONFLICT, "network_drift");
    let restore = || h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/restore"), &admin)));
    let keep =
        |session| h.send(empty(with_csrf(changing(Method::POST, "/api/v1/network/keep-files"), session)));
    assert_problem(&keep(&viewer).await, StatusCode::FORBIDDEN, "forbidden");

    let restored = restore().await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{:?}", restored.body);
    assert_eq!(
        (restored.body["pending"]["restore"].clone(), restored.body["drift"].clone()),
        (json!(true), Value::Null)
    );
    assert_problem(&restore().await, StatusCode::CONFLICT, "change_pending");
    confirm(&h, &admin).await;
    assert_eq!(setup(&h, &viewer).await["drift"], Value::Null, "the files say the settings again");
    let names: Vec<String> = h.network_host.files.lock().unwrap().iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["05-zendiloos-enp2s0.network"]);

    // Changed again, on purpose: kept.
    h.network_host
        .files
        .lock()
        .unwrap()
        .push(file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nDHCP=ipv4\n"));
    assert!(setup(&h, &viewer).await["drift"].is_object());
    let kept = keep(&admin).await;
    assert_eq!(kept.status, StatusCode::OK, "{:?}", kept.body);
    assert_eq!(kept.body["drift"], Value::Null);
    assert_eq!(h.network_settings.0.lock().unwrap().as_ref().unwrap().files.len(), 2);
    let refused = restore().await;
    assert_problem(&refused, StatusCode::CONFLICT, "refused");
}

/// A restored backup's network: listed beside this machine's, applied one
/// interface at a time with confirmation, cards of other names skipped,
/// forgotten when asked.
#[tokio::test]
async fn a_restored_backups_network_is_applied_where_its_names_match() {
    use zendiloos_network::domain::NetworkdFile;
    let h = Harness::new();
    h.network_source.set(zendiloos_network::testing::three_cards());
    let file =
        |name: &str, text: &str| NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() };
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let restored = |session| h.send(empty(with_cookie(get("/api/v1/network/restored"), session)));
    assert_problem(&restored(&viewer).await, StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(restored(&admin).await.body, Value::Null, "no restore");

    *h.network_settings.1.lock().unwrap() = Some(zendiloos_network::application::RestoredSettings {
        files: vec![
            file("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nAddress=10.0.0.5/24\nDNS=10.0.0.1\n"),
            file("eno1.network", "[Match]\nName=eno1\n[Network]\nDHCP=ipv4\n"),
        ],
        restored_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
    });
    let listed = restored(&admin).await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!({
            "restored_at": 1_793_000_000,
            "interfaces": [
                {
                    "interface": "eno1", "standing": "missing", "device": null,
                    "settings": { "ipv4": { "mode": "dhcp", "keep": null }, "ipv6": "off", "dns": [] },
                },
                {
                    "interface": "enp2s0", "standing": "applicable", "device": null,
                    "settings": {
                        "ipv4": { "mode": "static", "address": "10.0.0.5", "prefix": 24, "gateway": null },
                        "ipv6": "off", "dns": ["10.0.0.1"],
                    },
                },
            ],
            "devices": [],
        })
    );
    let apply = |name: &str| {
        h.send(json(
            with_csrf(changing(Method::POST, "/api/v1/network/restored/apply"), &admin),
            json!({ "name": name }),
        ))
    };
    assert_problem(&apply("eno1").await, StatusCode::CONFLICT, "refused");
    let applied = apply("enp2s0").await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert_eq!(applied.body["pending"]["interface"], "enp2s0");
    confirm(&h, &admin).await;
    assert_eq!(restored(&admin).await.body["interfaces"][1]["standing"], "in_force");

    let forget = h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/network/restored"), &admin))).await;
    assert_eq!(forget.status, StatusCode::NO_CONTENT);
    assert_eq!(restored(&admin).await.body, Value::Null);
}
