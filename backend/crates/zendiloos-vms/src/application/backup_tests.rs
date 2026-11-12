//! Snapshots, backups and restores, the host's devices, a TPM and Secure
//! Boot, over the fakes.

use std::time::Duration;

use zendiloos_jobs::JobState;
use zendiloos_kernel::{Actor, RequestContext, Timestamp};

use super::*;
use crate::domain::{BackupTarget, DiskBus, OsPreset, RestoreAs, SnapshotSchedule};
use crate::testing::{FakeVmHost, TestVms};

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

async fn finished(test: &TestVms, job: &Job) -> Job {
    for _ in 0..400 {
        let now = test.jobs.find(job.id).await.unwrap().unwrap();
        if now.finished_at.is_some() {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the job did not finish")
}

fn settings(name: &str) -> VmSettings {
    VmSettings {
        cpus: 1,
        memory_mib: 256,
        disks: vec![Disk { slot: 0, size_gib: 1, bus: DiskBus::Virtio, location: Location::named("") }],
        ..VmSettings::preset(name, OsPreset::Linux)
    }
}

async fn made(test: &TestVms, settings: VmSettings) -> String {
    let (view, job) = test.vms.create(NewVm { settings }, &alice(), &context()).await.unwrap();
    assert_eq!(finished(test, &job).await.state, JobState::Succeeded);
    view.vm.id
}

fn change_of(vm: &Vm) -> VmChange {
    let s = vm.settings.clone();
    VmChange {
        name: s.name,
        cpus: s.cpus,
        memory_mib: s.memory_mib,
        firmware: s.firmware,
        disks: s.disks,
        nics: s.nics,
        drives: s.drives,
        boot: s.boot,
        autostart: s.autostart,
        usb: s.usb,
        pci: s.pci,
        tpm: s.tpm,
        secure_boot: s.secure_boot,
    }
}

fn actions(test: &TestVms) -> Vec<&'static str> {
    test.audit.0.lock().unwrap().iter().map(|entry| entry.action).collect()
}

/// The fake clock stands still: snapshots taken one after another get the
/// next second's label, so each is newer.
#[tokio::test]
async fn snapshots_by_hand_roll_back_to_an_older_one_only_once_agreed() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, settings("web")).await;
    let first = test.vms.take_snapshot(&id, " before the update ", &alice(), &context()).await.unwrap();
    assert_eq!(first.name.as_deref(), Some("before the update"));
    assert_eq!(first.consistency, Consistency::Off);
    let second = test.vms.take_snapshot(&id, "", &alice(), &context()).await.unwrap();
    assert_ne!(first.label, second.label);
    assert!(test.vms.take_snapshot(&id, &"x".repeat(65), &alice(), &context()).await.is_err());
    let listed = test.vms.snapshots(&id, &context()).await.unwrap();
    let labels: Vec<&str> = listed.snapshots.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, [second.label.as_str(), first.label.as_str()], "newest first");

    test.host.set_state(&id, PowerState::Running);
    assert!(matches!(
        test.vms.rollback_snapshot(&id, &second.label, false, &alice(), &context()).await,
        Err(VmsError::Running)
    ));
    test.host.set_state(&id, PowerState::ShutOff);
    let refused = test.vms.rollback_snapshot(&id, &first.label, false, &alice(), &context()).await;
    assert!(
        matches!(&refused, Err(VmsError::NewerSnapshots(later)) if later.as_slice() == std::slice::from_ref(&second.label)),
        "{refused:?}"
    );
    let gone = test.vms.rollback_snapshot(&id, &first.label, true, &alice(), &context()).await.unwrap();
    assert_eq!(gone, std::slice::from_ref(&second.label));
    assert!(test.host.calls().contains(&format!("rollback {id} {} true", first.label)));
    assert!(matches!(
        test.vms.rollback_snapshot(&id, "zdl-2026-11-12_99.00.00", false, &alice(), &context()).await,
        Err(VmsError::NotFound)
    ));
    test.vms.delete_snapshot(&id, &first.label, &alice(), &context()).await.unwrap();
    assert!(test.vms.snapshots(&id, &context()).await.unwrap().snapshots.is_empty());
    assert!(matches!(
        test.vms.delete_snapshot(&id, &first.label, &alice(), &context()).await,
        Err(VmsError::NotFound)
    ));
    let audited = actions(&test);
    for action in ["vms.snapshot", "vms.rollback", "vms.snapshot_delete"] {
        assert!(audited.contains(&action), "{action}: {audited:?}");
    }
}

#[tokio::test]
async fn schedules_take_what_is_due_and_let_go_of_what_they_no_longer_keep() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, settings("db")).await;
    let hourly = SnapshotSchedule { frequency: Frequency::Hourly, keep: 1 };
    assert!(
        test.vms
            .set_snapshot_schedules(&id, vec![SnapshotSchedule { keep: 0, ..hourly }], &alice(), &context())
            .await
            .is_err()
    );
    test.vms.set_snapshot_schedules(&id, vec![hourly], &alice(), &context()).await.unwrap();
    assert_eq!(test.vms.snapshots(&id, &context()).await.unwrap().schedules, [hourly]);
    // An hourly one from long ago, and one by hand: the first look takes a
    // new one and lets the old hourly one go, never the one by hand.
    {
        let mut snapshots = test.host.snapshots.lock().unwrap();
        for (label, schedules) in
            [("zdl-2020-01-01_00.00.00", vec![Frequency::Hourly]), ("zdl-2020-01-01_01.00.00", vec![])]
        {
            snapshots.push(VmSnapshot {
                vm: id.clone(),
                label: label.into(),
                taken_at: crate::domain::label_time(label).unwrap(),
                name: None,
                schedules,
                consistency: Consistency::Off,
                disks: vec![0],
                used_bytes: 0,
                referenced_bytes: 0,
            });
        }
    }
    test.vms.take_due_snapshots().await;
    let left: Vec<String> =
        test.vms.snapshots(&id, &context()).await.unwrap().snapshots.into_iter().map(|s| s.label).collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(
        left.contains(&"zdl-2020-01-01_01.00.00".to_owned())
            && !left.contains(&"zdl-2020-01-01_00.00.00".to_owned())
    );
    // Taken just now: nothing is due.
    let calls = test.host.calls().len();
    test.vms.take_due_snapshots().await;
    assert_eq!(test.host.calls().len(), calls);
}

#[tokio::test]
async fn a_backup_runs_as_a_job_is_recorded_and_restores_as_a_new_vm_or_over_the_old() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, VmSettings { tpm: true, ..settings("office") }).await;
    let share = BackupTarget::Share { share_id: 1 };
    assert!(matches!(
        test.vms.back_up(&id, BackupTarget::Share { share_id: 9 }, "", &alice(), &context()).await,
        Err(VmsError::NotFound)
    ));
    assert!(test.vms.back_up(&id, share.clone(), "a/../b", &alice(), &context()).await.is_err());
    let job = test.vms.back_up(&id, share.clone(), "/vm-backups/", &alice(), &context()).await.unwrap();
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    let backups = test.vms.backups().await.unwrap();
    assert_eq!(backups.len(), 1);
    let backup = &backups[0].backup;
    assert!(backups[0].vm_exists);
    assert!(backup.folder.starts_with(&format!("vm-backups/{id}/")), "{}", backup.folder);
    assert_eq!((backup.vm_name.as_str(), backup.settings.tpm), ("office", true));
    assert_eq!(backup.place.shown(), "backups");
    assert!(backup.bytes() > 0);

    // As a new VM: a name of its own, new MAC addresses.
    let (copy, job) = test
        .vms
        .restore(
            backup.id,
            RestoreAs::New { name: "office copy".into(), pool: String::new() },
            &alice(),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    let copied = test.vms.get(&copy, &context()).await.unwrap().vm;
    assert!(copied.ready && copied.settings.tpm);
    let original = test.vms.get(&id, &context()).await.unwrap().vm;
    assert_ne!(copied.settings.nics[0].mac, original.settings.nics[0].mac);
    assert!(
        test.host
            .calls()
            .iter()
            .any(|call| call.starts_with(&format!("restore {copy} 1 {} over=false", backup.folder)))
    );
    assert!(
        matches!(
            test.vms
                .restore(
                    backup.id,
                    RestoreAs::New { name: "office copy".into(), pool: String::new() },
                    &alice(),
                    &context()
                )
                .await,
            Err(VmsError::Invalid(_))
        ),
        "the name is taken"
    );

    // Over the old one: shut off, its name typed again; a disk added since goes.
    let mut grown = change_of(&original);
    grown.disks.push(Disk { slot: 1, size_gib: 2, bus: DiskBus::Virtio, location: Location::named("") });
    test.vms.update(&id, grown, None, &alice(), &context()).await.unwrap();
    let over = || RestoreAs::Over { confirm: "office".into() };
    assert!(matches!(
        test.vms.restore(backup.id, RestoreAs::Over { confirm: "other".into() }, &alice(), &context()).await,
        Err(VmsError::ConfirmMismatch)
    ));
    test.host.set_state(&id, PowerState::Running);
    assert!(matches!(
        test.vms.restore(backup.id, over(), &alice(), &context()).await,
        Err(VmsError::Running)
    ));
    test.host.set_state(&id, PowerState::ShutOff);
    let (same, job) = test.vms.restore(backup.id, over(), &alice(), &context()).await.unwrap();
    assert_eq!(same, id);
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    let restored = test.vms.get(&id, &context()).await.unwrap().vm;
    assert_eq!(restored.settings.disks.len(), 1, "back to one disk");
    assert_eq!(
        restored.settings.nics[0].mac, original.settings.nics[0].mac,
        "its cards keep their addresses"
    );
    assert!(
        test.host
            .calls()
            .iter()
            .any(|call| call.starts_with(&format!("restore {id} 1 {} over=true [1]", backup.folder)))
    );

    // A failed restore as a new VM leaves nothing.
    *test.host.fail_restore.lock().unwrap() = Some("the image is shorter".into());
    let (gone, job) = test
        .vms
        .restore(
            backup.id,
            RestoreAs::New { name: "broken".into(), pool: String::new() },
            &alice(),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(finished(&test, &job).await.state, JobState::Failed);
    assert!(matches!(test.vms.get(&gone, &context()).await, Err(VmsError::NotFound)));

    // The share renamed: the backup is found where it is now; gone, only
    // the record goes with the backup.
    *test.shares.0.lock().unwrap() = Some(BackupShare { name: "archive".into(), volume: 2 });
    let (_, job) = test
        .vms
        .restore(
            backup.id,
            RestoreAs::New { name: "office again".into(), pool: String::new() },
            &alice(),
            &context(),
        )
        .await
        .unwrap();
    finished(&test, &job).await;
    *test.shares.0.lock().unwrap() = None;
    assert!(matches!(
        test.vms
            .restore(
                backup.id,
                RestoreAs::New { name: "nowhere".into(), pool: String::new() },
                &alice(),
                &context()
            )
            .await,
        Err(VmsError::NotFound)
    ));
    let calls = test.host.calls().len();
    test.vms.delete_backup(backup.id, &alice(), &context()).await.unwrap();
    assert!(test.vms.backups().await.unwrap().is_empty());
    assert_eq!(test.host.calls().len(), calls, "nothing to remove on the host");
    let audited = actions(&test);
    for action in ["vms.backup", "vms.restore", "vms.backup_delete"] {
        assert!(audited.contains(&action), "{action}: {audited:?}");
    }
}

#[tokio::test]
async fn a_backup_can_be_canceled_and_keeps_the_vm_from_changing_meanwhile() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, settings("big")).await;
    let hold = std::sync::Arc::new(tokio::sync::Notify::new());
    *test.host.hold_work.lock().unwrap() = Some(hold.clone());
    let job = test
        .vms
        .back_up(&id, BackupTarget::Drive { drive: "1234-ABCD".into() }, "", &alice(), &context())
        .await
        .unwrap();
    for _ in 0..100 {
        if test.host.calls().iter().any(|call| call.starts_with("back_up")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let vm = test.vms.get(&id, &context()).await.unwrap().vm;
    let busy = test.vms.update(&id, change_of(&vm), None, &alice(), &context()).await;
    assert!(matches!(&busy, Err(VmsError::Refused(why)) if why.contains("being backed up")), "{busy:?}");
    assert!(
        test.vms.back_up(&id, BackupTarget::Share { share_id: 1 }, "", &alice(), &context()).await.is_err()
    );
    test.vms.cancel_work(&id, &alice(), &context()).await.unwrap();
    let ended = finished(&test, &job).await;
    assert_eq!(ended.state, JobState::Failed);
    assert_eq!(ended.error.as_ref().map(|error| error.code.as_str()), Some("canceled"));
    assert!(test.host.calls().contains(&format!("end_work {id}")));
    assert!(test.vms.backups().await.unwrap().is_empty(), "nothing recorded");
    assert!(test.vms.cancel_work(&id, &alice(), &context()).await.is_err(), "nothing runs any more");
    assert!(test.vms.update(&id, change_of(&vm), None, &alice(), &context()).await.is_ok());
    assert!(
        test.vms
            .back_up(&id, BackupTarget::Drive { drive: "../x".into() }, "", &alice(), &context())
            .await
            .is_err()
    );
    drop(hold);
    let _ = Timestamp::from_unix_seconds(0);
}

#[tokio::test]
async fn the_hosts_devices_go_to_a_vm_as_its_rules_say() {
    let test = TestVms::new(FakeVmHost::ready());
    let keyboard = UsbDevice { vendor: 0x046d, product: 0xc31c };
    let stick = UsbDevice { vendor: 0x0781, product: 0x5583 };
    let devices = test.vms.host_devices(&context()).await.unwrap();
    assert!(devices.iommu.on && devices.usb.len() == 1);
    let half = VmSettings { pci: vec!["0000:01:00.0".into()], ..settings("gpu") };
    let refused = test.vms.create(NewVm { settings: half }, &alice(), &context()).await;
    assert!(
        matches!(&refused, Err(VmsError::Invalid(why)) if why.0.contains("0000:01:00.1 too")),
        "{refused:?}"
    );
    let unplugged = VmSettings { usb: vec![stick], ..settings("gpu") };
    assert!(test.vms.create(NewVm { settings: unplugged }, &alice(), &context()).await.is_err());
    let whole = VmSettings {
        pci: vec!["0000:01:00.1".into(), "0000:01:00.0".into()],
        usb: vec![keyboard],
        ..settings("gpu")
    };
    let id = made(&test, whole).await;
    let vm = test.vms.get(&id, &context()).await.unwrap().vm;
    assert_eq!(vm.settings.pci, ["0000:01:00.0", "0000:01:00.1"], "in order");
    // While it runs: USB devices plug in and out, PCI devices do not change.
    test.host.set_state(&id, PowerState::Running);
    let mut without = change_of(&vm);
    without.usb.clear();
    test.vms.update(&id, without.clone(), None, &alice(), &context()).await.unwrap();
    assert!(test.host.calls().contains(&format!("usb {id} +[] -[046d:c31c]")));
    without.pci.clear();
    assert!(matches!(
        test.vms.update(&id, without, None, &alice(), &context()).await,
        Err(VmsError::Running)
    ));
}

#[tokio::test]
async fn a_tpm_and_secure_boot_need_their_packages_which_the_plugin_brings() {
    let host = FakeVmHost::ready();
    {
        let mut facts = host.facts.lock().unwrap();
        facts.swtpm = false;
        facts.secure_boot = false;
    }
    let test = TestVms::new(host);
    let windows = VmSettings { cpus: 1, memory_mib: 256, ..VmSettings::preset("win", OsPreset::Windows) };
    let refused = test.vms.create(NewVm { settings: windows.clone() }, &alice(), &context()).await;
    assert!(matches!(&refused, Err(VmsError::Invalid(why)) if why.0.contains("swtpm")), "{refused:?}");
    // Installed before VMs had a TPM: the plugin installs again for it.
    let job = test.vms.install(&alice(), &context()).await.unwrap();
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    {
        let mut facts = test.host.facts.lock().unwrap();
        facts.swtpm = true;
        facts.secure_boot = true;
    }
    assert!(matches!(test.vms.install(&alice(), &context()).await, Err(VmsError::Installed)));
    let id = made(&test, windows).await;
    let vm = test.vms.get(&id, &context()).await.unwrap().vm;
    assert!(vm.settings.tpm && vm.settings.secure_boot);
    let mut bios = change_of(&vm);
    bios.firmware = Firmware::Bios;
    assert!(matches!(
        test.vms.update(&id, bios, None, &alice(), &context()).await,
        Err(VmsError::Invalid(_))
    ));
}
