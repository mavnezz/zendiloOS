//! The network through the API: taken over from ifupdown in a job and kept
//! once confirmed, what would not be carried over named, interfaces
//! switched on and off.

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
