use std::time::Duration;

use zendiloos_jobs::JobState;
use zendiloos_kernel::{Actor, RequestContext};

use super::*;
use crate::domain::{Network, OsPreset};
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

fn small(name: &str) -> NewVm {
    NewVm {
        settings: VmSettings {
            cpus: 1,
            memory_mib: 256,
            disk_gib: 1,
            ..VmSettings::preset(name, OsPreset::Linux)
        },
        iso: None,
    }
}

fn actions(test: &TestVms) -> Vec<&'static str> {
    test.audit.0.lock().unwrap().iter().map(|entry| entry.action).collect()
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
async fn a_vm_is_made_in_a_job_with_its_iso_copied_first() {
    let test = TestVms::new(FakeVmHost::ready());
    let new =
        NewVm { iso: Some(IsoChoice { share_id: 1, path: "isos/debian.iso".into() }), ..small(" Debian ") };
    let (view, job) = test.vms.create(new, &alice(), &context()).await.unwrap();
    assert_eq!(view.state, PowerState::Creating);
    assert_eq!(view.vm.settings.name, "Debian", "kept trimmed");
    assert_eq!(view.vm.id, "000000000001");
    assert_eq!((view.vm.pool.as_str(), view.vm.volume), ("tank", 1));
    assert_eq!(view.vm.iso, Some(IsoOrigin { share: "media".into(), path: "isos/debian.iso".into() }));
    assert_eq!(job.kind, "vms.create");
    assert!(matches!(
        test.vms.control(&view.vm.id, VmAction::Start, &alice(), &context()).await,
        Err(VmsError::Creating)
    ));

    let done = finished(&test, &job).await;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(test.host.calls(), ["copy_iso 000000000001 media/isos/debian.iso", "define 000000000001"]);
    let listed = test.vms.list(&context()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].vm.ready);
    assert_eq!(listed[0].state, PowerState::ShutOff);
    assert_eq!(actions(&test), ["vms.create"]);
}

#[tokio::test]
async fn what_breaks_the_rules_is_refused_before_anything_is_made() {
    let test = TestVms::new(FakeVmHost::default());
    assert!(matches!(test.vms.create(small("a"), &alice(), &context()).await, Err(VmsError::NotInstalled)));
    test.host.facts.lock().unwrap().installed = true;
    assert!(matches!(test.vms.create(small("a"), &alice(), &context()).await, Err(VmsError::NotSetUp)));
    let test = TestVms::new(FakeVmHost::ready());

    let mut cpus = small("a");
    cpus.settings.cpus = 3;
    assert!(matches!(test.vms.create(cpus, &alice(), &context()).await, Err(VmsError::Invalid(_))));
    let mut bridge = small("a");
    bridge.settings.network = Network::Bridge("br9".into());
    assert!(matches!(test.vms.create(bridge, &alice(), &context()).await, Err(VmsError::Invalid(_))));
    let text = NewVm { iso: Some(IsoChoice { share_id: 1, path: "notes.txt".into() }), ..small("a") };
    assert!(matches!(test.vms.create(text, &alice(), &context()).await, Err(VmsError::Invalid(_))));
    let denied = NewVm { iso: Some(IsoChoice { share_id: 2, path: "x.iso".into() }), ..small("a") };
    assert!(matches!(test.vms.create(denied, &alice(), &context()).await, Err(VmsError::IsoDenied)));
    assert!(test.vms.list(&context()).await.unwrap().is_empty());
    assert!(test.host.calls().is_empty());

    let (_, job) = test.vms.create(small("Same"), &alice(), &context()).await.unwrap();
    finished(&test, &job).await;
    assert!(matches!(test.vms.create(small("same"), &alice(), &context()).await, Err(VmsError::Invalid(_))));
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
    let (view, job) = test.vms.create(small("box"), &alice(), &context()).await.unwrap();
    finished(&test, &job).await;
    let id = view.vm.id.as_str();
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
async fn consoles_are_counted_per_account() {
    let test = TestVms::new(FakeVmHost::ready());
    let (view, job) = test.vms.create(small("box"), &alice(), &context()).await.unwrap();
    finished(&test, &job).await;
    let id = view.vm.id.as_str();
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
async fn deleting_needs_the_name_typed_again() {
    let test = TestVms::new(FakeVmHost::ready());
    let (view, job) = test.vms.create(small("Büro"), &alice(), &context()).await.unwrap();
    finished(&test, &job).await;
    let id = view.vm.id.as_str();
    assert!(matches!(
        test.vms.delete(id, "büro", &alice(), &context()).await,
        Err(VmsError::ConfirmMismatch)
    ));
    assert!(matches!(test.vms.delete(id, "", &alice(), &context()).await, Err(VmsError::ConfirmMismatch)));
    test.vms.delete(id, " Büro ", &alice(), &context()).await.unwrap();
    assert!(test.vms.list(&context()).await.unwrap().is_empty());
    assert_eq!(test.host.calls().last().unwrap(), &format!("remove {id}"));
    assert!(matches!(test.vms.delete(id, "Büro", &alice(), &context()).await, Err(VmsError::NotFound)));
}
