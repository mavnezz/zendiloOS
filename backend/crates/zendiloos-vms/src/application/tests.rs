use std::time::Duration;

use tokio::io::AsyncWriteExt;
use zendiloos_jobs::JobState;
use zendiloos_kernel::{Actor, RequestContext, Timestamp};

use super::*;
use crate::domain::{DiskBus, IMAGE_UPLOAD_MAX, ImageOrigin, Network, NicModel, OsPreset};
use crate::testing::{FakeVmHost, MemoryImages, TestVms};

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

fn small(name: &str) -> NewVm {
    NewVm {
        settings: VmSettings {
            cpus: 1,
            memory_mib: 256,
            disks: vec![Disk { slot: 0, size_gib: 1, bus: DiskBus::Virtio }],
            ..VmSettings::preset(name, OsPreset::Linux)
        },
    }
}

/// An image of the library, ready: `00000000000a`, `debian.iso`.
fn debian() -> VmImage {
    VmImage {
        id: "00000000000a".into(),
        name: "debian.iso".into(),
        bytes: Some(4096),
        origin: ImageOrigin::Share { share: "media".into(), path: "isos/debian.iso".into() },
        volume: 1,
        added_at: Timestamp::from_unix_seconds(0),
        ready: true,
    }
}

fn with_debian() -> TestVms {
    TestVms::with_images(FakeVmHost::ready(), MemoryImages::with(vec![debian()]))
}

fn actions(test: &TestVms) -> Vec<&'static str> {
    test.audit.0.lock().unwrap().iter().map(|entry| entry.action).collect()
}

/// Made and ready: the VM's id.
async fn made(test: &TestVms, new: NewVm) -> String {
    let (view, job) = test.vms.create(new, &alice(), &context()).await.unwrap();
    assert_eq!(finished(test, &job).await.state, JobState::Succeeded);
    view.vm.id
}

/// What the VM has now, as a change asks for it.
fn change_of(vm: &Vm) -> VmChange {
    let settings = vm.settings.clone();
    VmChange {
        name: settings.name,
        cpus: settings.cpus,
        memory_mib: settings.memory_mib,
        firmware: settings.firmware,
        disks: settings.disks,
        nics: settings.nics,
        drives: settings.drives,
        boot: settings.boot,
        autostart: settings.autostart,
    }
}

#[tokio::test]
async fn the_plugin_installs_only_where_kvm_is() {
    let without = FakeVmHost::default();
    without.facts.lock().unwrap().kvm = false;
    let test = TestVms::new(without);
    assert!(matches!(test.vms.install(&alice(), &context()).await, Err(VmsError::NoKvm)));
    assert!(test.host.calls().is_empty());

    let test = TestVms::new(FakeVmHost::default());
    let plugin = test.vms.plugin(&context()).await.unwrap();
    assert!(plugin.facts.kvm && !plugin.facts.installed && plugin.facts.storage.is_none());
    assert_eq!(plugin.pools.len(), 2);
    assert!(matches!(test.vms.set_up("tank", &alice(), &context()).await, Err(VmsError::NotInstalled)));
    let job = test.vms.install(&alice(), &context()).await.unwrap();
    assert_eq!(job.kind, "vms.install");
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    assert!(matches!(test.vms.install(&alice(), &context()).await, Err(VmsError::Installed)));

    assert!(matches!(test.vms.set_up("nowhere", &alice(), &context()).await, Err(VmsError::PoolNotFound)));
    let plugin = test.vms.set_up("cold", &alice(), &context()).await.unwrap();
    assert_eq!(plugin.facts.storage, Some(StoragePlace { pool: "cold".into(), volume: 2 }));
    assert_eq!(actions(&test), ["vms.install", "vms.storage"]);
}

#[tokio::test]
async fn a_vm_is_made_in_a_job_with_its_devices_and_macs_of_their_own() {
    let test = with_debian();
    let mut new = small(" Debian ");
    new.settings.disks.push(Disk { slot: 1, size_gib: 8, bus: DiskBus::Sata });
    new.settings.nics.push(Nic {
        slot: 1,
        model: NicModel::E1000e,
        network: Network::Bridge("br0".into()),
        // What a client sends of a MAC address is not taken.
        mac: "52:54:00:ff:ff:ff".into(),
    });
    new.settings.drives = vec![Drive { slot: 0, image: Some("00000000000a".into()) }];
    let (view, job) = test.vms.create(new, &alice(), &context()).await.unwrap();
    assert_eq!(view.state, PowerState::Creating);
    assert_eq!(view.vm.settings.name, "Debian", "kept trimmed");
    assert_eq!(view.vm.id, "000000000001");
    assert_eq!((view.vm.pool.as_str(), view.vm.volume), ("tank", 1));
    let macs: Vec<&str> = view.vm.settings.nics.iter().map(|nic| nic.mac.as_str()).collect();
    assert_eq!(macs, ["52:54:00:00:00:01", "52:54:00:00:00:02"]);
    assert_eq!(job.kind, "vms.create");
    assert!(matches!(
        test.vms.control(&view.vm.id, VmAction::Start, &alice(), &context()).await,
        Err(VmsError::Creating)
    ));

    let done = finished(&test, &job).await;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(test.host.calls(), ["define 000000000001"]);
    let listed = test.vms.list(&context()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].vm.ready);
    assert_eq!(listed[0].vm.settings.disks.len(), 2);
    assert_eq!(listed[0].state, PowerState::ShutOff);
    assert_eq!(actions(&test), ["vms.create"]);

    // A second VM's card never gets the first's address.
    let second = made(&test, small("second")).await;
    let second = test.vms.get(&second, &context()).await.unwrap();
    assert_eq!(second.vm.settings.nics[0].mac, "52:54:00:00:00:03");
}

#[tokio::test]
async fn what_breaks_the_rules_is_refused_before_anything_is_made() {
    let test = TestVms::new(FakeVmHost::default());
    assert!(matches!(test.vms.create(small("a"), &alice(), &context()).await, Err(VmsError::NotInstalled)));
    test.host.facts.lock().unwrap().installed = true;
    assert!(matches!(test.vms.create(small("a"), &alice(), &context()).await, Err(VmsError::NotSetUp)));
    let test = with_debian();

    let mut cpus = small("a");
    cpus.settings.cpus = 3;
    assert!(matches!(test.vms.create(cpus, &alice(), &context()).await, Err(VmsError::Invalid(_))));
    let mut bridge = small("a");
    bridge.settings.nics[0].network = Network::Bridge("br9".into());
    assert!(matches!(test.vms.create(bridge, &alice(), &context()).await, Err(VmsError::Invalid(_))));
    let mut unknown = small("a");
    unknown.settings.drives[0].image = Some("00000000000b".into());
    let refused = test.vms.create(unknown, &alice(), &context()).await;
    assert!(matches!(refused, Err(VmsError::Invalid(Invalid(text))) if text.contains("not in the library")));
    assert!(test.vms.list(&context()).await.unwrap().is_empty());
    assert!(test.host.calls().is_empty());

    made(&test, small("Same")).await;
    assert!(matches!(test.vms.create(small("same"), &alice(), &context()).await, Err(VmsError::Invalid(_))));
}

#[tokio::test]
async fn an_image_still_being_added_goes_into_no_drive() {
    let test = TestVms::with_images(
        FakeVmHost::ready(),
        MemoryImages::with(vec![VmImage { ready: false, ..debian() }]),
    );
    let mut new = small("a");
    new.settings.drives[0].image = Some("00000000000a".into());
    assert!(matches!(test.vms.create(new, &alice(), &context()).await, Err(VmsError::Invalid(_))));
}

#[tokio::test]
async fn a_failed_creation_leaves_nothing_behind() {
    let test = TestVms::new(FakeVmHost::ready());
    *test.host.fail_define.lock().unwrap() = Some("virsh: no such firmware".into());
    let (view, job) = test.vms.create(small("broken"), &alice(), &context()).await.unwrap();
    let done = finished(&test, &job).await;
    assert_eq!(done.state, JobState::Failed);
    assert_eq!(done.error.unwrap().detail.as_deref(), Some("virsh: no such firmware"));
    assert_eq!(test.host.calls(), [format!("define {}", view.vm.id), format!("remove {}", view.vm.id)]);
    assert!(test.vms.list(&context()).await.unwrap().is_empty(), "the record goes too");
}

#[tokio::test]
async fn vms_run_and_stop_and_say_so() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, small("box")).await;
    let id = id.as_str();
    assert!(matches!(test.vms.open_console(id, &alice(), &context()).await, Err(VmsError::NotRunning)));

    let started = test.vms.control(id, VmAction::Start, &alice(), &context()).await.unwrap();
    assert_eq!(started.state, PowerState::Running);
    let again = test.vms.control(id, VmAction::Start, &alice(), &context()).await;
    assert!(matches!(again, Err(VmsError::Refused(reason)) if reason.contains("already active")));
    let off = test.vms.control(id, VmAction::ForceOff, &alice(), &context()).await.unwrap();
    assert_eq!(off.state, PowerState::ShutOff);

    let on = test.vms.set_autostart(id, true, &alice(), &context()).await.unwrap();
    assert!(on.vm.settings.autostart);
    assert!(matches!(test.vms.get("../etc", &context()).await, Err(VmsError::NotFound)));
    assert!(matches!(test.vms.get("00000000000f", &context()).await, Err(VmsError::NotFound)));
    let logged = actions(&test);
    assert_eq!(logged, ["vms.create", "vms.start", "vms.start", "vms.force_off", "vms.autostart_on"]);
    let failed = test.audit.0.lock().unwrap()[2].outcome;
    assert_eq!(failed, Outcome::Failure, "the refused start is recorded as failed");
}

#[tokio::test]
async fn hardware_changes_only_while_the_vm_is_off() {
    let test = with_debian();
    let id = made(&test, small("box")).await;
    let vm = test.vms.get(&id, &context()).await.unwrap().vm;
    let mac = vm.settings.nics[0].mac.clone();

    // Off: two vCPUs, 512 MiB, the disk grown, a second one and a card.
    let mut change = change_of(&vm);
    change.cpus = 2;
    change.memory_mib = 512;
    change.disks = vec![
        Disk { slot: 0, size_gib: 2, bus: DiskBus::Virtio },
        Disk { slot: 1, size_gib: 4, bus: DiskBus::Sata },
    ];
    change.nics.push(Nic { slot: 1, model: NicModel::Virtio, network: Network::User, mac: String::new() });
    change.nics[0].mac.clear();
    let changed = test.vms.update(&id, change.clone(), None, &alice(), &context()).await.unwrap();
    assert_eq!((changed.vm.settings.cpus, changed.vm.settings.memory_mib), (2, 512));
    assert_eq!(changed.vm.settings.nics[0].mac, mac, "a card keeps its address");
    assert_ne!(changed.vm.settings.nics[1].mac, mac);
    assert_eq!(test.host.calls().last().unwrap(), &format!("update {id} []"));

    // Running: hardware is refused, and nothing is asked of the host.
    test.vms.control(&id, VmAction::Start, &alice(), &context()).await.unwrap();
    let calls = test.host.calls().len();
    let more = VmChange { cpus: 1, ..change_of(&changed.vm) };
    assert!(matches!(test.vms.update(&id, more, None, &alice(), &context()).await, Err(VmsError::Running)));
    let firmware = VmChange { firmware: Firmware::Bios, ..change_of(&changed.vm) };
    assert!(matches!(
        test.vms.update(&id, firmware, None, &alice(), &context()).await,
        Err(VmsError::Running)
    ));
    assert_eq!(test.host.calls().len(), calls);

    // …but its name, autostart and the image in a drive change at once.
    let mut live = change_of(&changed.vm);
    live.name = "renamed".into();
    live.autostart = true;
    live.drives[0].image = Some("00000000000a".into());
    let renamed = test.vms.update(&id, live.clone(), None, &alice(), &context()).await.unwrap();
    assert_eq!(renamed.vm.settings.name, "renamed");
    assert!(renamed.vm.settings.autostart);
    assert_eq!(renamed.state, PowerState::Running);
    assert_eq!(
        test.host.calls()[calls..],
        [format!("media {id} 0 00000000000a"), format!("autostart {id} true")]
    );
    live.drives[0].image = None;
    test.vms.update(&id, live, None, &alice(), &context()).await.unwrap();
    assert_eq!(test.host.calls().last().unwrap(), &format!("media {id} 0 eject"));
    assert!(actions(&test).iter().filter(|action| **action == "vms.change").count() >= 3);
}

#[tokio::test]
async fn a_disk_goes_only_with_the_name_typed_again_and_none_shrinks() {
    let test = TestVms::new(FakeVmHost::ready());
    let mut new = small("box");
    new.settings.disks.push(Disk { slot: 1, size_gib: 4, bus: DiskBus::Sata });
    let id = made(&test, new).await;
    let vm = test.vms.get(&id, &context()).await.unwrap().vm;

    let mut fewer = change_of(&vm);
    fewer.disks.retain(|disk| disk.slot == 0);
    for confirm in [None, Some("BOX"), Some("")] {
        let refused = test.vms.update(&id, fewer.clone(), confirm, &alice(), &context()).await;
        assert!(matches!(refused, Err(VmsError::ConfirmMismatch)), "{confirm:?}");
    }
    test.vms.update(&id, fewer.clone(), Some(" box "), &alice(), &context()).await.unwrap();
    assert_eq!(test.host.calls().last().unwrap(), &format!("update {id} [1]"));

    let mut smaller = fewer;
    smaller.disks[0].size_gib = 0;
    assert!(matches!(
        test.vms.update(&id, smaller.clone(), None, &alice(), &context()).await,
        Err(VmsError::Invalid(_))
    ));
    let mut grown = change_of(&test.vms.get(&id, &context()).await.unwrap().vm);
    grown.disks[0].size_gib = 3;
    test.vms.update(&id, grown.clone(), None, &alice(), &context()).await.unwrap();
    grown.disks[0].size_gib = 2;
    let shrunk = test.vms.update(&id, grown, None, &alice(), &context()).await;
    assert!(matches!(shrunk, Err(VmsError::Invalid(Invalid(text))) if text.contains("only grows")));

    let other = made(&test, small("other")).await;
    let taken =
        VmChange { name: "BOX".into(), ..change_of(&test.vms.get(&other, &context()).await.unwrap().vm) };
    assert!(matches!(
        test.vms.update(&other, taken, None, &alice(), &context()).await,
        Err(VmsError::Invalid(_))
    ));
}

#[tokio::test]
async fn consoles_are_counted_per_account() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, small("box")).await;
    let id = id.as_str();
    test.vms.control(id, VmAction::Start, &alice(), &context()).await.unwrap();

    let mut open = Vec::new();
    for _ in 0..CONSOLES_PER_ACCOUNT {
        open.push(test.vms.open_console(id, &alice(), &context()).await.unwrap());
    }
    let refused = test.vms.open_console(id, &alice(), &context()).await;
    assert!(matches!(refused, Err(VmsError::Refused(reason)) if reason.contains("consoles are open")));
    let bob = Actor { account_id: 2, name: "bob".into() };
    assert!(test.vms.open_console(id, &bob, &context()).await.is_ok(), "another account has its own");
    let mut first = open.remove(0);
    assert_eq!(first.name, "box");
    assert_eq!(first.session.output.recv().await.unwrap(), b"RFB 003.008\n");
    drop(first);
    assert!(test.vms.open_console(id, &alice(), &context()).await.is_ok(), "a seat came free");
}

#[tokio::test]
async fn deleting_needs_the_name_typed_again_and_keeps_the_images() {
    let test = with_debian();
    let mut new = small("Büro");
    new.settings.drives[0].image = Some("00000000000a".into());
    let id = made(&test, new).await;
    let id = id.as_str();
    assert!(matches!(
        test.vms.delete(id, "büro", &alice(), &context()).await,
        Err(VmsError::ConfirmMismatch)
    ));
    assert!(matches!(test.vms.delete(id, "", &alice(), &context()).await, Err(VmsError::ConfirmMismatch)));
    test.vms.delete(id, " Büro ", &alice(), &context()).await.unwrap();
    assert!(test.vms.list(&context()).await.unwrap().is_empty());
    assert_eq!(test.host.calls().last().unwrap(), &format!("remove {id}"));
    assert!(matches!(test.vms.delete(id, "Büro", &alice(), &context()).await, Err(VmsError::NotFound)));
    assert_eq!(test.vms.images(&context()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_iso_from_a_share_is_copied_into_the_library_in_a_job() {
    let test = TestVms::new(FakeVmHost::ready());
    let choice = |path: &str| IsoChoice { share_id: 1, path: path.into() };
    let (image, job) =
        test.vms.import_image(choice("isos/debian.iso"), None, &alice(), &context()).await.unwrap();
    assert_eq!((image.name.as_str(), image.ready, image.bytes), ("debian.iso", false, None));
    assert_eq!(image.origin, ImageOrigin::Share { share: "media".into(), path: "isos/debian.iso".into() });
    assert_eq!(job.kind, "vms.image_add");
    assert_eq!(finished(&test, &job).await.state, JobState::Succeeded);
    let library = test.vms.images(&context()).await.unwrap();
    assert_eq!(library.len(), 1);
    assert!(library[0].image.ready && library[0].used_by.is_empty());
    assert_eq!(library[0].image.bytes, Some(4096));
    assert_eq!(test.host.calls(), [format!("copy_image {} media/isos/debian.iso", image.id)]);

    // The same name twice is refused; another name is not.
    let twice = test.vms.import_image(choice("isos/debian.iso"), None, &alice(), &context()).await;
    assert!(matches!(twice, Err(VmsError::NameTaken)));
    let (_, again) = test
        .vms
        .import_image(choice("isos/debian.iso"), Some(" Debian 13 "), &alice(), &context())
        .await
        .unwrap();
    finished(&test, &again).await;
    for (path, refused) in [("notes.txt", "invalid"), ("isos/missing.iso", "missing")] {
        let result = test.vms.import_image(choice(path), None, &alice(), &context()).await;
        assert!(matches!(result, Err(VmsError::Invalid(_) | VmsError::NotFound)), "{refused}");
    }
    let denied = IsoChoice { share_id: 2, path: "x.iso".into() };
    assert!(matches!(
        test.vms.import_image(denied, None, &alice(), &context()).await,
        Err(VmsError::IsoDenied)
    ));

    // A copy that fails leaves nothing.
    let (broken, job) =
        test.vms.import_image(choice("isos/broken.iso"), None, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&test, &job).await.state, JobState::Failed);
    assert!(test.vms.images(&context()).await.unwrap().iter().all(|view| view.image.id != broken.id));
    assert_eq!(test.host.calls().last().unwrap(), &format!("delete_image {}", broken.id));
    assert_eq!(actions(&test), ["vms.image_add", "vms.image_add", "vms.image_add"]);
}

#[tokio::test]
async fn an_image_in_a_drive_stays_and_the_refusal_names_the_vms() {
    let test = with_debian();
    let mut new = small("web");
    new.settings.drives[0].image = Some("00000000000a".into());
    let web = made(&test, new.clone()).await;
    new.settings.name = "mail".into();
    let mail = made(&test, new).await;

    let library = test.vms.images(&context()).await.unwrap();
    assert_eq!(library[0].used_by, ["web", "mail"]);
    let refused = test.vms.delete_image("00000000000a", &alice(), &context()).await;
    assert!(matches!(&refused, Err(VmsError::ImageInUse(names)) if names == &["web", "mail"]));
    assert_eq!(refused.unwrap_err().to_string(), "the image is in use by web, mail");

    let renamed =
        test.vms.rename_image("00000000000a", " Debian 13 netinst ", &alice(), &context()).await.unwrap();
    assert_eq!(renamed.name, "Debian 13 netinst");
    assert!(matches!(
        test.vms.rename_image("00000000000a", "", &alice(), &context()).await,
        Err(VmsError::Invalid(_))
    ));
    assert!(matches!(
        test.vms.rename_image("../etc", "x", &alice(), &context()).await,
        Err(VmsError::NotFound)
    ));

    test.vms.delete(&mail, "mail", &alice(), &context()).await.unwrap();
    let web_vm = test.vms.get(&web, &context()).await.unwrap().vm;
    let mut ejected = change_of(&web_vm);
    ejected.drives[0].image = None;
    test.vms.update(&web, ejected, None, &alice(), &context()).await.unwrap();
    test.vms.delete_image("00000000000a", &alice(), &context()).await.unwrap();
    assert!(test.vms.images(&context()).await.unwrap().is_empty());
    assert_eq!(test.host.calls().last().unwrap(), "delete_image 00000000000a");
    assert!(actions(&test).contains(&"vms.image_delete") && actions(&test).contains(&"vms.image_rename"));
}

#[tokio::test]
async fn the_size_of_an_iso_a_vm_had_before_the_library_is_looked_up() {
    let test = TestVms::with_images(
        FakeVmHost::ready(),
        MemoryImages::with(vec![VmImage { bytes: None, ..debian() }]),
    );
    test.host.files.lock().unwrap().insert("00000000000a".into(), 123_456);
    let library = test.vms.images(&context()).await.unwrap();
    assert_eq!(library[0].image.bytes, Some(123_456));
    assert_eq!(test.images.find("00000000000a").await.unwrap().unwrap().bytes, Some(123_456), "and kept");
}

#[tokio::test]
async fn an_upload_is_kept_only_whole() {
    let test = TestVms::new(FakeVmHost::ready());
    let mut upload = test.vms.begin_upload("alpine-virt.iso", 5, &context()).await.unwrap();
    assert_eq!(upload.ticket.bytes(), 5);
    upload.file.write_all(b"hello").await.unwrap();
    upload.file.flush().await.unwrap();
    drop(upload.file);
    let image = test.vms.finish_upload(upload.ticket, &alice(), &context()).await.unwrap();
    assert_eq!((image.bytes, image.ready), (Some(5), true));
    assert_eq!(image.origin, ImageOrigin::Upload { file: "alpine-virt.iso".into() });
    assert!(test.vms.images(&context()).await.unwrap()[0].image.ready);

    let mut short = test.vms.begin_upload("short.iso", 10, &context()).await.unwrap();
    short.file.write_all(b"abc").await.unwrap();
    short.file.flush().await.unwrap();
    drop(short.file);
    let refused = test.vms.finish_upload(short.ticket, &alice(), &context()).await;
    assert!(matches!(refused, Err(VmsError::Refused(reason)) if reason.contains("3 of 10")));
    assert_eq!(test.vms.images(&context()).await.unwrap().len(), 1, "what did not come whole goes");

    let aborted = test.vms.begin_upload("aborted.iso", 10, &context()).await.unwrap();
    drop(aborted.file);
    test.vms.abort_upload(aborted.ticket, &alice(), &context()).await;
    assert_eq!(test.vms.images(&context()).await.unwrap().len(), 1);

    for bytes in [0, IMAGE_UPLOAD_MAX + 1] {
        assert!(matches!(test.vms.begin_upload("x.iso", bytes, &context()).await, Err(VmsError::Invalid(_))));
    }
    assert!(matches!(
        test.vms.begin_upload("ALPINE-virt.iso", 5, &context()).await,
        Err(VmsError::NameTaken)
    ));
    assert_eq!(actions(&test), ["vms.image_upload", "vms.image_upload", "vms.image_upload"]);
    let outcomes: Vec<Outcome> = test.audit.0.lock().unwrap().iter().map(|entry| entry.outcome).collect();
    assert_eq!(outcomes, [Outcome::Success, Outcome::Failure, Outcome::Failure]);
}

#[tokio::test]
async fn images_cut_off_by_a_restart_go() {
    let test = TestVms::with_images(
        FakeVmHost::ready(),
        MemoryImages::with(vec![
            debian(),
            VmImage { id: "00000000000b".into(), name: "half".into(), ready: false, ..debian() },
        ]),
    );
    assert_eq!(test.vms.discard_unfinished_images(&context()).await.unwrap(), 1);
    let left: Vec<String> =
        test.vms.images(&context()).await.unwrap().into_iter().map(|view| view.image.id).collect();
    assert_eq!(left, ["00000000000a"]);
    assert_eq!(test.host.calls(), ["delete_image 00000000000b"]);
    let refused = test.vms.delete_image("00000000000b", &alice(), &context()).await;
    assert!(matches!(refused, Err(VmsError::NotFound)));
}

#[tokio::test(start_paused = true)]
async fn the_load_is_looked_at_while_someone_watches() {
    let test = TestVms::new(FakeVmHost::ready());
    let id = made(&test, small("box")).await;
    made(&test, small("idle")).await;
    test.vms.control(&id, VmAction::Start, &alice(), &context()).await.unwrap();

    let mut watching = test.vms.watch_stats();
    assert!(test.vms.stats_running());
    let first = watching.next().await.unwrap();
    assert_eq!((first.running, first.total), (1, 2));
    assert_eq!(first.vms[0].cpu_percent, None, "a rate needs two looks");
    assert_eq!(first.memory_assigned_bytes, 256 << 20);
    assert_eq!((first.host_memory_bytes, first.host_cores), (2 << 30, 2));
    assert_eq!(first.vms[0].memory_used_bytes, Some(128 << 20));

    // A second watcher shares the rounds.
    let second = test.vms.watch_stats();
    let next = watching.next().await.unwrap();
    assert_eq!(second.latest(), Some(next.clone()));
    // 3 s of CPU time in 3 s on one vCPU: all of it, half the machine.
    assert_eq!(next.vms[0].cpu_percent, Some(100.0));
    assert_eq!(next.cpu_percent, 50.0);
    assert_eq!(next.vms[0].received_bytes_per_second, Some(1000));
    assert_eq!(next.vms[0].sent_bytes_per_second, Some(500));
    let looks = test.host.looks.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(looks, 2, "one look per round for all VMs and watchers");

    drop(watching);
    drop(second);
    tokio::time::sleep(STATS_EVERY * 3).await;
    assert!(!test.vms.stats_running(), "nobody watches: no more rounds");
    let after = test.host.looks.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(STATS_EVERY * 3).await;
    assert_eq!(test.host.looks.load(std::sync::atomic::Ordering::SeqCst), after);

    let again = test.vms.watch_stats();
    assert_eq!(again.latest(), None, "nothing old is shown");
    assert!(test.vms.stats_running());
}
