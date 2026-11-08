//! Where each disk sits, through the API: the model offered and chosen,
//! a layout of one's own, a disk made to blink, the inventory of every
//! disk seen.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;
use zendiloos_storage::testing::TEST_MODEL;

use crate::tests::{
    Harness, Reply, Session, assert_problem, changing, empty, get, json, with_cookie, with_csrf,
};

async fn read(h: &Harness, session: &Session, uri: &str) -> Reply {
    h.send(empty(with_cookie(get(uri), session))).await
}

async fn send(h: &Harness, session: &Session, method: Method, uri: &str, body: Value) -> Reply {
    h.send(json(with_csrf(changing(method, uri), session), body)).await
}

async fn delete(h: &Harness, session: &Session, uri: &str) -> Reply {
    h.send(empty(with_csrf(changing(Method::DELETE, uri), session))).await
}

fn slots(reply: &Reply) -> Vec<(String, String, Value)> {
    reply.body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["slots"].as_array().unwrap().clone())
        .map(|slot| {
            (
                slot["id"].as_str().unwrap().to_owned(),
                slot["state"].as_str().unwrap().to_owned(),
                slot["disk"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_detected_model_is_offered_chosen_and_overridden() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;

    let none = read(&h, &viewer, "/api/v1/storage/enclosure").await;
    assert_eq!(none.status, StatusCode::OK, "{:?}", none.body);
    assert_eq!(none.body["machine"]["vendor"], "QEMU");
    assert!(none.body["detected"].is_null() && none.body["source"].is_null());
    assert_eq!(none.body["groups"], json!([]));
    assert_eq!(none.body["chassis"], json!({ "style": "plain", "panel": null }));

    h.storage.set_machine("TEST", "NAS 4");
    let offered = read(&h, &viewer, "/api/v1/storage/enclosure").await;
    assert_eq!(offered.body["detected"]["id"], TEST_MODEL);
    assert_eq!(
        offered.body["detected"]["groups"][0]["slots"][0]["binding"],
        json!({ "kind": "port", "value": "pci-0000:05:00.0-ata-1" })
    );
    assert!(offered.body["source"].is_null(), "offered, not chosen");

    let choose = json!({ "source": "model", "model": TEST_MODEL });
    assert_problem(
        &send(&h, &viewer, Method::PUT, "/api/v1/storage/enclosure", choose.clone()).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let chosen = send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", choose).await;
    assert_eq!(chosen.status, StatusCode::OK, "{:?}", chosen.body);
    assert_eq!(chosen.body["source"], "model");
    assert_eq!(chosen.body["chassis"]["style"], "tower");
    assert_eq!(chosen.body["chassis"]["panel"]["side"], "left");
    assert_eq!(
        slots(&chosen),
        [
            ("bay1".into(), "occupied".into(), json!("sda")),
            ("bay2".into(), "occupied".into(), json!("sdb")),
            ("bay3".into(), "occupied".into(), json!("sdc")),
            ("bay4".into(), "occupied".into(), json!("sdd")),
            ("m2-1".into(), "occupied".into(), json!("nvme0n1")),
            ("m2-2".into(), "empty".into(), Value::Null),
        ]
    );
    assert_eq!(chosen.body["unplaced"], json!(["sde"]));
    let bay2 = &chosen.body["groups"][0]["slots"][1];
    assert_eq!(
        (bay2["row"].clone(), bay2["column"].clone(), bay2["overridden"].clone()),
        (json!(0), json!(1), json!(false))
    );
    assert_eq!(chosen.body["groups"][1]["slots"][1]["row"], 1, "the M.2 slots stack");

    // sde by serial number into the free M.2 slot; bay 1 bound to nothing.
    let overridden = send(
        &h,
        &admin,
        Method::PUT,
        "/api/v1/storage/enclosure",
        json!({ "source": "model", "model": TEST_MODEL, "overrides": [
            { "slot": "m2-2", "binding": { "kind": "serial", "value": "T5" } },
            { "slot": "bay1", "binding": null },
        ] }),
    )
    .await;
    assert_eq!(overridden.status, StatusCode::OK, "{:?}", overridden.body);
    let placed = slots(&overridden);
    assert_eq!(placed[0], ("bay1".into(), "empty".into(), Value::Null));
    assert_eq!(placed[5], ("m2-2".into(), "occupied".into(), json!("sde")));
    assert_eq!(overridden.body["groups"][1]["slots"][1]["overridden"], true);
    assert_eq!(overridden.body["unplaced"], json!(["sda"]));
    assert_eq!(read(&h, &viewer, "/api/v1/storage/enclosure").await.body["unplaced"], json!(["sda"]));

    let wrong =
        json!({ "source": "model", "model": TEST_MODEL, "overrides": [{ "slot": "bay9", "binding": null }] });
    assert_problem(
        &send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", wrong).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    let unknown = json!({ "source": "model", "model": "acme-99" });
    assert_problem(
        &send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", unknown).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );

    let reset = delete(&h, &admin, "/api/v1/storage/enclosure").await;
    assert_eq!(reset.status, StatusCode::OK);
    assert!(reset.body["source"].is_null());
    assert_eq!(reset.body["detected"]["id"], TEST_MODEL, "offered again");
    assert_problem(
        &delete(&h, &viewer, "/api/v1/storage/enclosure").await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );

    let models = read(&h, &viewer, "/api/v1/storage/enclosure/models").await;
    assert_eq!(models.status, StatusCode::OK);
    assert_eq!(models.body[0]["id"], TEST_MODEL);
    assert_eq!(models.body[0]["dmi_vendor"], "TEST");
    assert_eq!(models.body[0]["verified"], false);
}

fn group(id: &str, rows: u8, columns: u8, form: &str, slots: Vec<Value>) -> Value {
    json!({ "id": id, "name": format!("Group {id}"), "rows": rows, "columns": columns,
            "orientation": "vertical", "form": form, "numbering": "left_to_right", "slots": slots })
}

#[tokio::test]
async fn a_layout_of_one_s_own_binds_by_port_and_serial_number() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let sata = (1..=4)
        .map(|n| {
            json!({ "id": format!("s{n}"), "label": format!("SATA {n}"),
                         "binding": { "kind": "port", "value": format!("pci-0000:05:00.0-ata-{n}") } })
        })
        .collect();
    let m2 = vec![json!({ "id": "nvme", "label": "M.2", "binding": { "kind": "serial", "value": "T5" } })];
    let own = json!({ "source": "custom", "groups": [group("sata", 1, 4, "inch_3_5", sata), group("m2", 1, 1, "m2", m2)] });
    let saved = send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", own).await;
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(saved.body["source"], "custom");
    assert!(saved.body["model"].is_null());
    assert_eq!(saved.body["chassis"]["style"], "plain");
    let placed = slots(&saved);
    assert_eq!(placed[3], ("s4".into(), "occupied".into(), json!("sdd")));
    assert_eq!(placed[4], ("nvme".into(), "occupied".into(), json!("sde")), "by serial number");
    assert_eq!(saved.body["unplaced"], json!(["nvme0n1"]));

    // A disk bound by serial number that is not there.
    let missing = json!({ "source": "custom", "groups": [group("m2", 1, 1, "m2",
        vec![json!({ "id": "nvme", "label": "M.2", "binding": { "kind": "serial", "value": "GONE" } })])] });
    let saved = send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", missing).await;
    assert_eq!(slots(&saved)[0], ("nvme".into(), "missing".into(), Value::Null));

    for bad in [
        json!({ "source": "custom", "groups": [] }),
        json!({ "source": "custom", "groups": [group("g", 1, 2, "m2", vec![json!({ "id": "a", "label": "A" })])] }),
        json!({ "source": "custom", "groups": [group("g", 1, 1, "m2", vec![json!({ "id": "a", "label": "" })])] }),
        json!({ "source": "custom", "groups": [group("g", 1, 1, "m2", vec![json!({ "id": "a", "label": "A",
            "binding": { "kind": "port", "value": "pci-0000:05:00.0-ata-1-part1" } })])] }),
    ] {
        let refused = send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", bad.clone()).await;
        assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    }
    let odd = json!({ "source": "custom", "groups": [group("g", 1, 1, "inch_5_25", vec![json!({ "id": "a", "label": "A" })])] });
    assert_eq!(
        send(&h, &admin, Method::PUT, "/api/v1/storage/enclosure", odd).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn a_disk_blinks_until_it_is_stopped() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    assert_eq!(read(&h, &viewer, "/api/v1/storage/locate").await.body, json!([]));

    assert_problem(
        &send(&h, &viewer, Method::POST, "/api/v1/storage/disks/sdc/locate", json!({})).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let started =
        send(&h, &admin, Method::POST, "/api/v1/storage/disks/sdc/locate", json!({ "seconds": 120 })).await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.body);
    assert_eq!(started.body["disk"], "sdc");
    let now = zendiloos_kernel::Clock::now(h.identity.clock.as_ref()).unix_seconds();
    assert_eq!(started.body["until"], now + 120);
    let blinking = read(&h, &viewer, "/api/v1/storage/locate").await;
    assert_eq!(blinking.body, json!([{ "disk": "sdc", "until": now + 120 }]));

    let default = send(&h, &admin, Method::POST, "/api/v1/storage/disks/sda/locate", json!({})).await;
    assert_eq!(default.body["until"], now + 60);
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/storage/disks/sda/locate", json!({ "seconds": 301 })).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/storage/disks/sdz/locate", json!({})).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );

    assert_problem(
        &delete(&h, &viewer, "/api/v1/storage/disks/sdc/locate").await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    assert_eq!(delete(&h, &admin, "/api/v1/storage/disks/sdc/locate").await.status, StatusCode::NO_CONTENT);
    assert_problem(
        &delete(&h, &admin, "/api/v1/storage/disks/sdc/locate").await,
        StatusCode::CONFLICT,
        "not_locating",
    );
    let left: Vec<Value> = read(&h, &viewer, "/api/v1/storage/locate").await.body.as_array().unwrap().clone();
    assert_eq!(left.iter().map(|s| s["disk"].clone()).collect::<Vec<_>>(), [json!("sda")]);
    assert!(!h.storage.bursts().is_empty(), "read from the disk");
}

#[tokio::test]
async fn every_disk_seen_is_kept_with_what_the_owner_noted() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    h.storage.remember("OLD1", 1_700_000_000);

    let listed = read(&h, &viewer, "/api/v1/storage/inventory").await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    let serials: Vec<_> =
        listed.body.as_array().unwrap().iter().map(|d| (d["serial"].clone(), d["disk"].clone())).collect();
    assert_eq!(serials.first().unwrap(), &(json!("T1"), json!("sda")));
    assert_eq!(serials.last().unwrap(), &(json!("OLD1"), Value::Null), "stored, no longer present");

    let note = json!({ "purchased": "2024-03-01", "warranty_until": "2027-03-01", "vendor": "Shop", "note": "RMA once" });
    assert_problem(
        &send(&h, &viewer, Method::PUT, "/api/v1/storage/inventory/T1", note.clone()).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let updated = send(&h, &admin, Method::PUT, "/api/v1/storage/inventory/T1", note).await;
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.body["warranty_until"], "2027-03-01");
    assert_eq!(updated.body["vendor"], "Shop");
    assert_eq!(updated.body["disk"], "sda");
    assert_eq!(updated.body["model"], "TEST HDD");
    let cleared =
        send(&h, &admin, Method::PUT, "/api/v1/storage/inventory/T1", json!({ "vendor": " " })).await;
    assert!(cleared.body["purchased"].is_null() && cleared.body["vendor"].is_null());
    let bad = json!({ "purchased": "2024-03-01", "warranty_until": "2023-01-01" });
    assert_problem(
        &send(&h, &admin, Method::PUT, "/api/v1/storage/inventory/T1", bad).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    assert_problem(
        &send(&h, &admin, Method::PUT, "/api/v1/storage/inventory/NOPE", json!({})).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );

    assert_problem(
        &delete(&h, &admin, "/api/v1/storage/inventory/T1").await,
        StatusCode::CONFLICT,
        "disk_present",
    );
    assert_problem(
        &delete(&h, &viewer, "/api/v1/storage/inventory/OLD1").await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    assert_eq!(delete(&h, &admin, "/api/v1/storage/inventory/OLD1").await.status, StatusCode::NO_CONTENT);
    assert_problem(
        &delete(&h, &admin, "/api/v1/storage/inventory/OLD1").await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
}

#[tokio::test]
async fn disks_say_where_they_are_plugged_in() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    let disks = read(&h, &viewer, "/api/v1/storage/disks").await;
    let sda = disks.body.as_array().unwrap().iter().find(|d| d["name"] == "sda").unwrap();
    assert_eq!(sda["id_path"], "pci-0000:05:00.0-ata-1.0");
}
