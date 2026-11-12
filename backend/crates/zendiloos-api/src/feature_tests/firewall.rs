//! The firewall through the API: rules applied and kept once confirmed,
//! the browser never shut out by mistake, blocked services listed and told.

use axum::http::{Method, StatusCode, request};
use serde_json::{Value, json};
use zendiloos_firewall::domain::{ProjectPorts, Transport};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

/// A browser at 192.168.1.50 that reached the NAS at its address on
/// enp1s0, on 443, as nginx says.
fn from_lan(builder: request::Builder) -> request::Builder {
    builder
        .header("x-real-ip", "192.168.1.50")
        .header("x-server-addr", "192.168.1.10")
        .header("x-server-port", "443")
}

async fn put(h: &Harness, session: &Session, body: Value) -> Reply {
    h.send(json(from_lan(with_csrf(changing(Method::PUT, "/api/v1/security/firewall"), session)), body)).await
}

async fn confirm(h: &Harness, session: &Session) -> Reply {
    h.send(empty(with_csrf(changing(Method::POST, "/api/v1/security/firewall/confirm"), session))).await
}

async fn setup(h: &Harness, session: &Session) -> Reply {
    h.send(empty(with_cookie(get("/api/v1/security/firewall"), session))).await
}

fn rule(service: &str, source: &str, action: &str) -> Value {
    json!({ "enabled": true, "interface": null, "service": service, "source": source, "action": action })
}

#[tokio::test]
async fn rules_are_kept_once_confirmed() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let start = setup(&h, &admin).await;
    assert_eq!(start.status, StatusCode::OK, "{:?}", start.body);
    assert_eq!(start.body["current"], json!({ "enabled": false, "fallback": "allow", "rules": [] }));
    assert_eq!(start.body["confirm_within"], 90);
    assert_eq!(start.body["interfaces"], json!(["enp1s0"]));
    assert_eq!(start.body["local_networks"], json!(["192.168.1.0/24"]));
    let smb = start.body["services"].as_array().unwrap().iter().find(|s| s["key"] == "smb").unwrap().clone();
    assert_eq!(
        smb,
        json!({ "key": "smb", "service": "smb", "project": null, "on": true, "tcp": "139, 445", "udp": "137-138" })
    );

    let lan = json!({
        "enabled": true,
        "fallback": "deny",
        "rules": [
            rule("web_interface", "192.168.1.0/24", "allow"),
            { "enabled": true, "interface": "enp1s0", "service": "ports", "protocol": "tcp", "ports": "8000-8100, 9000",
              "source": "any", "action": "allow" },
        ],
    });
    let applied = put(&h, &admin, lan).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert_eq!(applied.body["pending"]["settings"]["rules"][1]["ports"], "8000-8100, 9000");
    assert_eq!(applied.body["current"]["enabled"], false, "kept only once confirmed");
    assert_eq!(applied.body["blocked"], json!(["smb"]));
    let again = put(&h, &admin, json!({ "enabled": false, "fallback": "allow", "rules": [] })).await;
    assert_problem(&again, StatusCode::CONFLICT, "change_pending");

    assert_eq!(confirm(&h, &admin).await.status, StatusCode::NO_CONTENT);
    let kept = setup(&h, &admin).await;
    assert_eq!(kept.body["current"]["fallback"], "deny");
    assert!(kept.body["pending"].is_null());
    assert_problem(&confirm(&h, &admin).await, StatusCode::CONFLICT, "refused");
    assert_eq!(h.firewall_host.calls(), ["apply 2 rules, confirm within 90", "confirm"]);
    let (told, _) = h.notifier.for_account(1, 50).await.unwrap();
    assert!(told.iter().any(|n| n.code == "firewall_blocked"), "{told:?}");
}

#[tokio::test]
async fn the_browser_is_not_shut_out_by_mistake() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let elsewhere = json!({ "enabled": true, "fallback": "deny", "rules": [rule("web_interface", "10.0.0.0/8", "allow")] });
    assert_problem(&put(&h, &admin, elsewhere.clone()).await, StatusCode::CONFLICT, "would_lock_out");
    // Another port than the one the browser came on.
    let other_port = json!({ "enabled": true, "fallback": "deny", "rules": [
        { "enabled": true, "service": "ports", "protocol": "tcp", "ports": "8443", "source": "any", "action": "allow" },
    ] });
    assert_problem(&put(&h, &admin, other_port).await, StatusCode::CONFLICT, "would_lock_out");
    assert!(h.firewall_host.calls().is_empty());
    // Meant: applied, going back unless confirmed.
    let mut meant = elsewhere;
    meant["lock_out"] = json!(true);
    assert_eq!(put(&h, &admin, meant).await.status, StatusCode::ACCEPTED);
    h.firewall_host.time_up();
    let back = setup(&h, &admin).await;
    assert!(back.body["pending"].is_null() && back.body["current"]["enabled"] == false, "{:?}", back.body);
}

#[tokio::test]
async fn rules_are_checked_and_only_administrators_change_them() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    for (rule, detail) in [
        (rule("all", "192.168.1.300", "deny"), "no IP address"),
        (
            json!({ "enabled": true, "service": "ports", "ports": "0", "source": "any", "action": "deny" }),
            "port",
        ),
        (
            json!({ "enabled": true, "service": "project", "source": "any", "action": "deny" }),
            "names the project",
        ),
        (
            json!({ "enabled": true, "service": "project", "project": "Bad Name", "source": "any", "action": "deny" }),
            "project",
        ),
        (
            json!({ "enabled": true, "interface": "lo", "service": "all", "source": "any", "action": "deny" }),
            "interface",
        ),
    ] {
        let reply = put(&h, &admin, json!({ "enabled": true, "fallback": "allow", "rules": [rule] })).await;
        assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
        assert!(reply.body["detail"].as_str().unwrap().contains(detail), "{:?}", reply.body);
    }
    let unknown =
        put(&h, &admin, json!({ "enabled": true, "fallback": "allow", "rules": [], "extra": 1 })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);
    h.firewall_host.fail(Some("nft: syntax error"));
    let failed = put(&h, &admin, json!({ "enabled": true, "fallback": "allow", "rules": [] })).await;
    assert_problem(&failed, StatusCode::BAD_GATEWAY, "firewall_failed");

    let user = h.sign_in("bob", Role::User).await;
    assert_problem(&setup(&h, &user).await, StatusCode::FORBIDDEN, "forbidden");
    let refused = put(&h, &user, json!({ "enabled": false, "fallback": "allow", "rules": [] })).await;
    assert_problem(&refused, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(&confirm(&h, &user).await, StatusCode::FORBIDDEN, "forbidden");
}

#[tokio::test]
async fn projects_are_services_and_blocked_ones_are_listed() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    h.firewall_facts.set(|facts| {
        facts.projects = vec![ProjectPorts { name: "media".into(), ports: vec![(Transport::Tcp, 8096)] }];
    });
    let rules = json!({ "enabled": true, "fallback": "allow", "rules": [
        { "enabled": true, "service": "project", "project": "media", "source": "any", "action": "deny" },
        rule("ping", "any", "allow"),
    ] });
    let applied = put(&h, &admin, rules).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert_eq!(applied.body["blocked"], json!(["project:media"]));
    let media = applied.body["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "project:media")
        .unwrap()
        .clone();
    assert_eq!(
        media,
        json!({ "key": "project:media", "service": "project", "project": "media", "on": true, "tcp": "8096", "udp": "" })
    );
    assert_eq!(applied.body["pending"]["settings"]["rules"][0]["project"], "media");
    assert_eq!(applied.body["pending"]["settings"]["rules"][1]["service"], "ping");
    // What returns unless it is confirmed was not given yet: given once.
    h.firewall.refresh(&Default::default()).await.unwrap();
    assert_eq!(h.firewall_host.calls().last().unwrap(), "keep 0 rules, 2 waiting");
    h.firewall.refresh(&Default::default()).await.unwrap();
    assert_eq!(h.firewall_host.calls().len(), 2, "nothing new while it waits");
}

/// A restored backup's rules: shown beside the defaults in force, applied
/// as any change, refused where they would shut this browser out, kept
/// once confirmed and offered no more.
#[tokio::test]
async fn a_restored_backups_rules_are_applied_with_confirmation() {
    use zendiloos_firewall::application::RestoredFirewall;
    use zendiloos_firewall::domain::{Action, FirewallSettings, KnownService, Rule, Service, Source};
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    assert_eq!(setup(&h, &admin).await.body["restored"], Value::Null);
    let backed_up = |source: &str| FirewallSettings {
        enabled: true,
        fallback: Action::Deny,
        rules: vec![Rule {
            enabled: true,
            interface: None,
            service: Service::Known(KnownService::WebInterface),
            source: Source::parse(source).unwrap(),
            action: Action::Allow,
        }],
    };
    *h.firewall_store.restored.lock().unwrap() = Some(RestoredFirewall {
        settings: backed_up("10.20.0.0/16"),
        restored_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
    });
    let shown = setup(&h, &admin).await.body;
    assert_eq!(shown["restored"]["restored_at"], 1_793_000_000);
    let first = &shown["restored"]["settings"]["rules"][0];
    assert_eq!(
        (first["service"].clone(), first["source"].clone()),
        (json!("web_interface"), json!("10.20.0.0/16"))
    );
    assert_eq!(shown["current"]["enabled"], false, "the defaults are in force");

    let apply = |body: Value| {
        h.send(json(
            from_lan(with_csrf(changing(Method::POST, "/api/v1/security/firewall/restored/apply"), &admin)),
            body,
        ))
    };
    assert_problem(&apply(json!({})).await, StatusCode::CONFLICT, "would_lock_out");
    *h.firewall_store.restored.lock().unwrap() = Some(RestoredFirewall {
        settings: backed_up("192.168.1.0/24"),
        restored_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
    });
    let applied = apply(json!({})).await;
    assert_eq!(applied.status, StatusCode::ACCEPTED, "{:?}", applied.body);
    assert!(applied.body["pending"].is_object(), "waits for confirmation");
    assert_eq!(confirm(&h, &admin).await.status, StatusCode::NO_CONTENT);
    let kept = setup(&h, &admin).await.body;
    assert_eq!((kept["current"]["enabled"].clone(), kept["restored"].clone()), (json!(true), Value::Null));

    *h.firewall_store.restored.lock().unwrap() = Some(RestoredFirewall {
        settings: backed_up("10.20.0.0/16"),
        restored_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_000_000),
    });
    let forget = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/security/firewall/restored"), &admin)))
        .await;
    assert_eq!(forget.status, StatusCode::NO_CONTENT);
    assert_eq!(setup(&h, &admin).await.body["restored"], Value::Null);
}
