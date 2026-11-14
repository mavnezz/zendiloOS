//! The UPS through the API: what is installed, a UPS set and read, the
//! passwords never sent back, NUT installed in a job, the scanner's
//! devices; changed by administrators only.

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;
use zendiloos_ups::application::HostFacts;
use zendiloos_ups::domain::{Found, Source};

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

async fn send(h: &Harness, session: &Session, method: Method, uri: &str, body: Value) -> Reply {
    let builder = with_csrf(changing(method, uri), session);
    h.send(if body.is_null() { empty(builder) } else { json(builder, body) }).await
}

async fn read(h: &Harness, session: &Session) -> Reply {
    h.send(empty(with_cookie(get("/api/v1/ups"), session))).await
}

fn usb() -> Value {
    json!({ "kind": "usb", "driver": "usbhid-ups", "vendor_id": "0463", "product_id": "FFFF" })
}

#[tokio::test]
async fn without_a_ups_the_page_says_what_can_be_set() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let overview = read(&h, &viewer).await;
    assert_eq!(overview.status, StatusCode::OK, "{:?}", overview.body);
    assert_eq!(
        overview.body,
        json!({
            "installed": true,
            "snmp": true,
            "settings": { "source": null, "shutdown": { "kind": "low_battery" }, "cut_power": false, "serve": null },
            "reading": null,
            "drivers": [
                "usbhid-ups", "nutdrv_qx", "blazer_usb", "bcmxcp_usb", "tripplite_usb", "richcomm_usb",
                "riello_usb", "nutdrv_atcl_usb"
            ],
            "name": "ups",
            "port": 3493,
            "default_account": "monuser",
        })
    );
    // Looking is for whoever may see the system; changing for administrators.
    let set = send(
        &h,
        &viewer,
        Method::PUT,
        "/api/v1/ups",
        json!({ "source": null, "shutdown": { "kind": "low_battery" } }),
    )
    .await;
    assert_problem(&set, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(
        &send(&h, &viewer, Method::POST, "/api/v1/ups/scan", Value::Null).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    assert_problem(
        &send(&h, &viewer, Method::POST, "/api/v1/ups/install", Value::Null).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
}

#[tokio::test]
async fn an_administrator_sets_a_usb_ups_and_reads_what_it_says() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    h.ups_link.says("OB DISCHRG");

    let body = json!({
        "source": usb(),
        "shutdown": { "kind": "after_minutes", "minutes": 10 },
        "cut_power": true,
        "serve": { "user": "monuser", "password": "for-others" },
    });
    let set = send(&h, &admin, Method::PUT, "/api/v1/ups", body).await;
    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(
        set.body["settings"],
        json!({
            "source": { "kind": "usb", "driver": "usbhid-ups", "vendor_id": "0463", "product_id": "ffff", "serial": null },
            "shutdown": { "kind": "after_minutes", "minutes": 10 },
            "cut_power": true,
            "serve": { "user": "monuser", "password_set": true },
        })
    );
    assert_eq!(set.body["reading"]["on_battery"], true);
    assert_eq!(set.body["reading"]["status"], "OB DISCHRG");
    assert_eq!(set.body["reading"]["charge_percent"], 80);
    assert!(!set.body.to_string().contains("for-others"), "the password is never sent back");

    let applied = h.ups_host.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    assert!(
        matches!(&applied[0].settings.source, Some(Source::Usb { driver, .. }) if driver == "usbhid-ups")
    );
    assert_eq!(
        applied[0].settings.serve.as_ref().map(|account| account.password.as_str()),
        Some("for-others")
    );
    let audit = h.identity.audit.entries();
    assert!(audit.iter().any(|entry| entry.action == "ups.update" && entry.target.as_deref() == Some("usb")));

    // Changed without the password: the one kept stays.
    let again = json!({
        "source": usb(),
        "shutdown": { "kind": "charge_below", "percent": 30 },
        "serve": { "user": "monuser" },
    });
    let set = send(&h, &admin, Method::PUT, "/api/v1/ups", again).await;
    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(set.body["settings"]["shutdown"], json!({ "kind": "charge_below", "percent": 30 }));
    assert_eq!(set.body["settings"]["cut_power"], false);
    let applied = h.ups_host.applied.lock().unwrap().clone();
    assert_eq!(
        applied[1].settings.serve.as_ref().map(|account| account.password.as_str()),
        Some("for-others")
    );

    // No UPS any more: NUT is switched off.
    let off = send(
        &h,
        &admin,
        Method::PUT,
        "/api/v1/ups",
        json!({ "source": null, "shutdown": { "kind": "low_battery" } }),
    )
    .await;
    assert_eq!(off.status, StatusCode::OK, "{:?}", off.body);
    assert_eq!(off.body["settings"]["source"], Value::Null);
    assert_eq!(off.body["reading"], Value::Null);
    assert_eq!(h.ups_host.applied.lock().unwrap().last().unwrap().settings.source, None);
}

#[tokio::test]
async fn what_does_not_fit_is_told_and_changes_nothing() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let set = |body: Value| send(&h, &admin, Method::PUT, "/api/v1/ups", body);

    let driver = set(
        json!({ "source": { "kind": "usb", "driver": "dummy-ups" }, "shutdown": { "kind": "low_battery" } }),
    )
    .await;
    assert_problem(&driver, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let charge = set(json!({ "source": usb(), "shutdown": { "kind": "charge_below", "percent": 2 } })).await;
    assert_problem(&charge, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert_eq!(charge.body["detail"], "the charge to shut down at is from 5 to 95 percent");
    let server = json!({ "kind": "server", "host": "nas.local", "user": "monuser" });
    let no_password = set(json!({ "source": server, "shutdown": { "kind": "low_battery" } })).await;
    assert_problem(&no_password, StatusCode::UNPROCESSABLE_ENTITY, "ups_password_needed");
    let quote = json!({ "kind": "server", "host": "nas.local", "user": "monuser", "password": "two words" });
    assert_problem(
        &set(json!({ "source": quote, "shutdown": { "kind": "low_battery" } })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let unknown =
        set(json!({ "source": usb(), "shutdown": { "kind": "low_battery" }, "standby": true })).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", unknown.body);

    *h.ups_host.refuses.lock().unwrap() = Some("the driver did not take it".into());
    let refused = set(json!({ "source": usb(), "shutdown": { "kind": "low_battery" } })).await;
    assert_problem(&refused, StatusCode::CONFLICT, "refused");
    assert_eq!(refused.body["detail"], "the driver did not take it");

    assert!(h.ups_host.applied.lock().unwrap().is_empty());
    assert_eq!(read(&h, &admin).await.body["settings"]["source"], Value::Null);
}

#[tokio::test]
async fn another_devices_ups_is_followed_with_an_account_whose_password_stays_here() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    h.ups_link.says("OL");
    let server = json!({ "kind": "server", "host": "NAS.local", "user": "monuser", "password": "secret" });
    let set = send(
        &h,
        &admin,
        Method::PUT,
        "/api/v1/ups",
        json!({ "source": server, "shutdown": { "kind": "low_battery" } }),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(
        set.body["settings"]["source"],
        json!({ "kind": "server", "host": "nas.local", "port": 3493, "ups": "ups", "user": "monuser", "password_set": true })
    );
    assert!(!set.body.to_string().contains("secret"));
    let asked = h.ups_link.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!((asked.host.as_str(), asked.port, asked.ups.as_str()), ("nas.local", 3493, "ups"));
    // Only a UPS of the NAS's own can be offered on.
    let serve = json!({
        "source": { "kind": "server", "host": "nas.local", "user": "monuser" },
        "shutdown": { "kind": "low_battery" },
        "serve": { "user": "others", "password": "pw" },
    });
    let refused = send(&h, &admin, Method::PUT, "/api/v1/ups", serve).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
}

#[tokio::test]
async fn nut_is_installed_in_a_job_and_the_scanner_finds_what_is_plugged_in() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    *h.ups_host.facts.lock().unwrap() = HostFacts { installed: false, snmp: false };

    assert_eq!(read(&h, &admin).await.body["installed"], false);
    let early = send(
        &h,
        &admin,
        Method::PUT,
        "/api/v1/ups",
        json!({ "source": usb(), "shutdown": { "kind": "low_battery" } }),
    )
    .await;
    assert_problem(&early, StatusCode::CONFLICT, "ups_not_installed");
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/ups/scan", Value::Null).await,
        StatusCode::CONFLICT,
        "ups_not_installed",
    );

    let install = send(&h, &admin, Method::POST, "/api/v1/ups/install", Value::Null).await;
    assert_eq!(install.status, StatusCode::ACCEPTED, "{:?}", install.body);
    assert_eq!(install.body["kind"], "ups.install");
    let id = zendiloos_jobs::JobId::new(install.body["id"].as_i64().unwrap());
    let mut done = None;
    for _ in 0..400 {
        let job = h.jobs.find(id).await.unwrap().unwrap();
        if job.finished_at.is_some() {
            done = Some(job);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(done.expect("the job did not finish").state, zendiloos_jobs::JobState::Succeeded);
    assert_eq!(read(&h, &admin).await.body["installed"], true);
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/ups/install", Value::Null).await,
        StatusCode::CONFLICT,
        "ups_installed",
    );

    *h.ups_host.found.lock().unwrap() = vec![
        Found {
            driver: "usbhid-ups".into(),
            vendor_id: Some("0463".into()),
            product_id: Some("FFFF".into()),
            vendor: Some("EATON".into()),
            product: Some("Ellipse ECO 650".into()),
            serial: None,
        },
        Found {
            driver: "some-new-driver".into(),
            vendor_id: None,
            product_id: None,
            vendor: None,
            product: None,
            serial: None,
        },
    ];
    let scan = send(&h, &admin, Method::POST, "/api/v1/ups/scan", Value::Null).await;
    assert_eq!(scan.status, StatusCode::OK, "{:?}", scan.body);
    assert_eq!(scan.body["devices"][0]["product"], "Ellipse ECO 650");
    assert_eq!(scan.body["devices"][0]["supported"], true);
    assert_eq!(scan.body["devices"][1]["supported"], false, "a driver zendiloOS does not offer");
}
