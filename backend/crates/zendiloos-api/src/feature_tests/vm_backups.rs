//! Through the API: a TPM and Secure Boot for Windows, the host's USB and
//! PCI devices, snapshots of a VM's disks with schedules and rollbacks,
//! backups into a share or onto a drive, canceled, restored as a new VM or
//! over the old one, and deleted; all for administrators only.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;
use zendiloos_vms::domain::PowerState;

use super::vms::{change_of, finished, read, ready, send, small};
use crate::tests::{Harness, Session, assert_problem};

/// A VM made from `body`, its job done: its id and what the API says of it.
async fn made(h: &Harness, admin: &Session, body: Value) -> (String, Value) {
    let created = send(h, admin, Method::POST, "/api/v1/vms", body).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{:?}", created.body);
    assert_eq!(finished(h, &created.body["job"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let id = created.body["vm"]["id"].as_str().unwrap().to_owned();
    let vm = read(h, admin, &format!("/api/v1/vms/{id}")).await.body;
    (id, vm)
}

#[tokio::test]
async fn windows_11_gets_a_tpm_and_secure_boot_and_the_hosts_devices_are_listed() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    ready(&h, &admin).await;
    let mut windows = small("win11");
    windows["os"] = json!("windows");
    windows["tpm"] = json!(true);
    windows["secure_boot"] = json!(true);
    let (id, vm) = made(&h, &admin, windows.clone()).await;
    assert_eq!((vm["tpm"].as_bool(), vm["secure_boot"].as_bool()), (Some(true), Some(true)));
    let mut bios = change_of(&vm);
    bios["firmware"] = json!("bios");
    let refused = send(&h, &admin, Method::PUT, &format!("/api/v1/vms/{id}"), bios).await;
    assert_problem(&refused, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert!(refused.body["detail"].as_str().unwrap().contains("Secure Boot needs UEFI"));

    assert_problem(&read(&h, &user, "/api/v1/vms/devices").await, StatusCode::FORBIDDEN, "forbidden");
    let devices = read(&h, &admin, "/api/v1/vms/devices").await;
    assert_eq!(devices.status, StatusCode::OK);
    assert_eq!(
        devices.body["iommu"],
        json!({ "on": true, "cmdline": ["intel_iommu=on"], "cpu_vendor": "intel" })
    );
    assert_eq!(devices.body["usb"][0]["id"], "046d:c31c");
    let gpu = &devices.body["pci"][0];
    assert_eq!(
        (gpu["address"].as_str(), gpu["vendor"].as_str(), gpu["class"].as_str()),
        (Some("0000:01:00.0"), Some("10de"), Some("030000"))
    );
    assert_eq!(devices.body["pci"][2]["needed"], json!({ "kind": "disk", "disk": "sda" }));

    // A graphics card with its sound, and the keyboard.
    let mut passed = small("gaming");
    passed["pci"] = json!(["0000:01:00.0"]);
    let half = send(&h, &admin, Method::POST, "/api/v1/vms", passed.clone()).await;
    assert_problem(&half, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    passed["pci"] = json!(["0000:01:00.0", "0000:01:00.1"]);
    passed["usb"] = json!(["046d:c31c"]);
    let (gaming, vm) = made(&h, &admin, passed).await;
    assert_eq!(vm["pci"], json!(["0000:01:00.0", "0000:01:00.1"]));
    assert_eq!(vm["usb"], json!(["046d:c31c"]));
    let mut bad = small("bad");
    bad["usb"] = json!(["keyboard"]);
    assert_problem(
        &send(&h, &admin, Method::POST, "/api/v1/vms", bad).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_input",
    );
    // While it runs, the keyboard is pulled out at once.
    h.vm_host.set_state(&gaming, PowerState::Running);
    let mut without = change_of(&vm);
    without["usb"] = json!([]);
    let changed = send(&h, &admin, Method::PUT, &format!("/api/v1/vms/{gaming}"), without).await;
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert!(h.vm_host.calls().contains(&format!("usb {gaming} +[] -[046d:c31c]")));
}

#[tokio::test]
async fn snapshots_are_taken_rolled_back_with_consent_scheduled_and_deleted() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    ready(&h, &admin).await;
    let (id, _) = made(&h, &admin, small("web")).await;
    let path = format!("/api/v1/vms/{id}/snapshots");
    assert_problem(&read(&h, &user, &path).await, StatusCode::FORBIDDEN, "forbidden");
    let first = send(&h, &admin, Method::POST, &path, json!({ "name": "before the update" })).await;
    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert_eq!(
        (first.body["name"].as_str(), first.body["consistency"].as_str()),
        (Some("before the update"), Some("off"))
    );
    let second = send(&h, &admin, Method::POST, &path, json!({})).await;
    let (first, second) =
        (first.body["label"].as_str().unwrap().to_owned(), second.body["label"].as_str().unwrap().to_owned());
    let listed = read(&h, &admin, &path).await.body;
    let labels: Vec<&str> =
        listed["snapshots"].as_array().unwrap().iter().map(|s| s["label"].as_str().unwrap()).collect();
    assert_eq!(labels, [second.as_str(), first.as_str()]);

    let rollback = |label: &str| format!("{path}/{label}/rollback");
    let newer = send(&h, &admin, Method::POST, &rollback(&first), json!({})).await;
    assert_problem(&newer, StatusCode::CONFLICT, "newer_snapshots");
    assert_eq!(newer.body["detail"], json!(second));
    h.vm_host.set_state(&id, PowerState::Running);
    assert_problem(
        &send(&h, &admin, Method::POST, &rollback(&second), json!({})).await,
        StatusCode::CONFLICT,
        "vm_running",
    );
    h.vm_host.set_state(&id, PowerState::ShutOff);
    let done = send(&h, &admin, Method::POST, &rollback(&first), json!({ "newer": true })).await;
    assert_eq!(done.status, StatusCode::OK, "{:?}", done.body);
    assert_eq!(done.body["removed"], json!([second]));

    let schedules = format!("{path}/schedules");
    let set = send(
        &h,
        &admin,
        Method::PUT,
        &schedules,
        json!({ "schedules": [{ "frequency": "daily", "keep": 7 }, { "frequency": "hourly", "keep": 24 }] }),
    )
    .await;
    assert_eq!(set.body, json!([{ "frequency": "hourly", "keep": 24 }, { "frequency": "daily", "keep": 7 }]));
    let none = send(
        &h,
        &admin,
        Method::PUT,
        &schedules,
        json!({ "schedules": [{ "frequency": "daily", "keep": 0 }] }),
    )
    .await;
    assert_problem(&none, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    assert_eq!(read(&h, &admin, &path).await.body["schedules"].as_array().unwrap().len(), 2);

    let delete = send(&h, &admin, Method::POST, &format!("{path}/{first}/delete"), Value::Null).await;
    assert_eq!(delete.status, StatusCode::NO_CONTENT);
    assert_problem(
        &send(&h, &admin, Method::POST, &format!("{path}/{first}/delete"), Value::Null).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_problem(
        &send(&h, &admin, Method::POST, &format!("{path}/../delete"), Value::Null).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
}

#[tokio::test]
async fn a_backup_goes_into_a_share_and_comes_back_as_a_new_vm_or_over_the_old() {
    let h = Harness::new();
    let admin = h.sign_in("alice", Role::Admin).await;
    let user = h.sign_in("bob", Role::User).await;
    ready(&h, &admin).await;
    let (id, vm) = made(&h, &admin, small("office")).await;
    let backup = format!("/api/v1/vms/{id}/backups");
    let to_share = json!({ "target": { "kind": "share", "share_id": 1 }, "folder": "vm-backups" });
    assert_problem(
        &send(&h, &user, Method::POST, &backup, to_share.clone()).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    let nowhere = json!({ "target": { "kind": "share", "share_id": 9 } });
    assert_problem(
        &send(&h, &admin, Method::POST, &backup, nowhere).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let job = send(&h, &admin, Method::POST, &backup, to_share.clone()).await;
    assert_eq!(job.status, StatusCode::ACCEPTED, "{:?}", job.body);
    assert_eq!(finished(&h, &job.body).await.state, zendiloos_jobs::JobState::Succeeded);
    let listed = read(&h, &admin, "/api/v1/vms/backups").await.body;
    let first = &listed[0];
    assert_eq!(
        (first["vm"].as_str(), first["vm_name"].as_str(), first["place"].as_str()),
        (Some(id.as_str()), Some("office"), Some("backups"))
    );
    assert_eq!(first["target"], json!({ "kind": "share", "share_id": 1 }));
    assert!(first["vm_exists"].as_bool().unwrap());
    assert_eq!(first["files"][0]["part"], json!({ "kind": "disk", "slot": 0, "volsize": 1u64 << 30 }));
    assert_eq!(first["files"][1]["part"], json!({ "kind": "nvram" }));
    let backup_id = first["id"].as_i64().unwrap();

    let restore = format!("/api/v1/vms/backups/{backup_id}/restore");
    let new =
        send(&h, &admin, Method::POST, &restore, json!({ "as": { "kind": "new", "name": "office copy" } }))
            .await;
    assert_eq!(new.status, StatusCode::ACCEPTED, "{:?}", new.body);
    assert_eq!(finished(&h, &new.body["job"]).await.state, zendiloos_jobs::JobState::Succeeded);
    let copy = read(&h, &admin, &format!("/api/v1/vms/{}", new.body["vm"].as_str().unwrap())).await.body;
    assert_eq!(copy["name"], "office copy");
    assert_ne!(copy["nics"][0]["mac"], vm["nics"][0]["mac"]);

    let over = |confirm: &str| json!({ "as": { "kind": "over", "confirm": confirm } });
    assert_problem(
        &send(&h, &admin, Method::POST, &restore, over("other")).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "confirmation_mismatch",
    );
    h.vm_host.set_state(&id, PowerState::Running);
    assert_problem(
        &send(&h, &admin, Method::POST, &restore, over("office")).await,
        StatusCode::CONFLICT,
        "vm_running",
    );
    h.vm_host.set_state(&id, PowerState::ShutOff);
    let back = send(&h, &admin, Method::POST, &restore, over("office")).await;
    assert_eq!(back.status, StatusCode::ACCEPTED, "{:?}", back.body);
    assert_eq!(back.body["vm"], json!(id));
    assert_eq!(finished(&h, &back.body["job"]).await.state, zendiloos_jobs::JobState::Succeeded);

    // A backup onto a drive, canceled while it runs.
    let hold = std::sync::Arc::new(tokio::sync::Notify::new());
    *h.vm_host.hold_work.lock().unwrap() = Some(hold.clone());
    let to_drive = json!({ "target": { "kind": "drive", "drive": "1234-ABCD" } });
    let job = send(&h, &admin, Method::POST, &backup, to_drive).await;
    assert_eq!(job.status, StatusCode::ACCEPTED, "{:?}", job.body);
    for _ in 0..100 {
        if h.vm_host.calls().iter().any(|call| call.starts_with(&format!("back_up {id} 1234-ABCD"))) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let busy = send(&h, &admin, Method::POST, &backup, to_share).await;
    assert_problem(&busy, StatusCode::CONFLICT, "refused");
    let cancel = send(&h, &admin, Method::POST, &format!("{backup}/cancel"), Value::Null).await;
    assert_eq!(cancel.status, StatusCode::NO_CONTENT);
    let ended = finished(&h, &job.body).await;
    assert_eq!(ended.error.map(|error| error.code), Some("canceled".to_owned()));
    assert_problem(
        &send(&h, &admin, Method::POST, &format!("{backup}/cancel"), Value::Null).await,
        StatusCode::CONFLICT,
        "refused",
    );
    assert_eq!(
        read(&h, &admin, "/api/v1/vms/backups").await.body.as_array().unwrap().len(),
        1,
        "nothing recorded"
    );

    let delete =
        send(&h, &admin, Method::POST, &format!("/api/v1/vms/backups/{backup_id}/delete"), Value::Null).await;
    assert_eq!(delete.status, StatusCode::NO_CONTENT);
    assert_eq!(read(&h, &admin, "/api/v1/vms/backups").await.body, json!([]));
    assert_problem(
        &send(&h, &admin, Method::POST, &restore, over("office")).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
}
