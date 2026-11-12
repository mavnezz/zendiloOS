use std::sync::Mutex;
use std::time::Duration;

use zendiloos_jobs::JobState;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};

use zendiloos_notify::testing::MemoryNotifications;

use super::*;
use crate::domain::{Ipv6Setting, Refusal, Standing};
use crate::testing::{DIGEST, FakeConfigurator, FakeUsers, MemorySettings, SettableNetwork, three_cards};

#[derive(Default)]
struct Audit(Mutex<Vec<(&'static str, Outcome, Option<String>)>>);

#[async_trait]
impl AuditLog for Audit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push((entry.action, entry.outcome, entry.target));
        Ok(())
    }
}

struct Setup {
    network: NetworkUseCases,
    host: Arc<FakeConfigurator>,
    source: Arc<SettableNetwork>,
    users: Arc<FakeUsers>,
    store: Arc<MemorySettings>,
    notifications: Arc<MemoryNotifications>,
    audit: Arc<Audit>,
    jobs: Jobs,
}

fn setup(host: FakeConfigurator) -> Setup {
    let host = Arc::new(host);
    let audit = Arc::new(Audit::default());
    let source = Arc::new(SettableNetwork::default());
    let users = Arc::new(FakeUsers::default());
    let store = Arc::new(MemorySettings::default());
    let notifications = Arc::new(MemoryNotifications::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let network = NetworkUseCases::new(NetworkPorts {
        source: source.clone(),
        configurator: host.clone(),
        users: users.clone(),
        store: store.clone(),
        jobs: jobs.clone(),
        notifier: Notifier::new(notifications.clone(), Arc::new(FixedClock)),
        audit: audit.clone(),
        clock: Arc::new(FixedClock),
    });
    Setup { network, host, source, users, store, notifications, audit, jobs }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

async fn finished(jobs: &Jobs, job: Job) -> Job {
    for _ in 0..200 {
        let current = jobs.find(job.id).await.unwrap().unwrap();
        if current.finished_at.is_some() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the job did not finish");
}

const INSTALLER: &str = "auto lo\niface lo inet loopback\n\nallow-hotplug enp1s0\niface enp1s0 inet dhcp\n";

#[tokio::test]
async fn ifupdowns_network_is_offered_for_a_takeover_with_what_it_would_set_up() {
    let s = setup(FakeConfigurator::with_ifupdown(INSTALLER));
    let request = RequestContext::default();
    let found = s.network.configure.setup(&request).await.unwrap();
    assert_eq!((found.managed, found.manager), (false, Manager::Ifupdown));
    let takeover = found.takeover.unwrap();
    assert!(takeover.installs_resolved && takeover.ifupdown_running && takeover.plan.refused.is_empty());
    assert_eq!(takeover.digest, DIGEST);
    assert_eq!(
        takeover.plan.interfaces,
        [InterfaceSettings {
            interface: "enp1s0".into(),
            // The address it has, asked for again.
            ipv4: Ipv4Setting::Dhcp { keep: Some("192.168.1.20".parse().unwrap()) },
            ipv6: Ipv6Setting::Automatic,
            dns: vec![],
        }]
    );

    // NetworkManager is only named.
    *s.host.network_manager.lock().unwrap() = true;
    let found = s.network.configure.setup(&request).await.unwrap();
    assert_eq!((found.manager, found.takeover), (Manager::NetworkManager, None));
    let refused = s.network.configure.take_over(&alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::Refused(why) if why.contains("NetworkManager")), "{refused}");
}

#[tokio::test]
async fn a_takeover_installs_resolved_first_and_then_waits_for_confirmation() {
    let s = setup(FakeConfigurator::with_ifupdown(INSTALLER));
    let request = RequestContext::default();
    let job = s.network.configure.take_over(&alice(), &request).await.unwrap();
    assert_eq!((job.kind.as_str(), job.target.as_deref()), ("network.take_over", Some("ifupdown")));
    let job = finished(&s.jobs, job).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    assert_eq!(*s.host.installs.lock().unwrap(), 1);
    assert_eq!(s.host.taken_over.lock().unwrap()[0][0].interface, "enp1s0");

    let found = s.network.configure.setup(&request).await.unwrap();
    assert!(found.managed && found.takeover.is_none());
    assert_eq!(found.pending.unwrap().change, Change::Takeover(vec!["enp1s0".into()]));
    let again = s.network.configure.take_over(&alice(), &request).await.unwrap_err();
    assert!(matches!(again, ConfigureError::Pending), "{again}");
    s.network.configure.confirm(&alice(), &request).await.unwrap();
    let again = s.network.configure.take_over(&alice(), &request).await.unwrap_err();
    assert!(matches!(&again, ConfigureError::Refused(why) if why.contains("already")), "{again}");

    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(
        audit,
        [
            ("network.take_over", Outcome::Success, Some("ifupdown".into())),
            ("network.confirm", Outcome::Success, Some("ifupdown".into())),
        ]
    );
}

#[tokio::test]
async fn what_would_not_be_carried_over_keeps_ifupdown() {
    let bridged = "auto br0\niface br0 inet dhcp\n  bridge_ports enp1s0\n";
    let s = setup(FakeConfigurator::with_ifupdown(bridged));
    let request = RequestContext::default();
    let refused = s.network.configure.take_over(&alice(), &request).await.unwrap_err();
    let ConfigureError::NotCarriedOver(why) = &refused else { panic!("{refused}") };
    assert_eq!(
        why,
        &[Refusal { place: "interfaces:2".into(), reason: "br0: bridge-ports: a bridge".into() }]
    );
    assert_eq!(refused.to_string(), "not carried over: interfaces:2: br0: bridge-ports: a bridge");
    assert!(s.host.taken_over.lock().unwrap().is_empty() && *s.host.installs.lock().unwrap() == 0);

    // ifupdown that did not bring the network up has nothing to give.
    let s = setup(FakeConfigurator::with_ifupdown(INSTALLER));
    s.host.ifupdown.lock().unwrap().as_mut().unwrap().active = false;
    let refused = s.network.configure.take_over(&alice(), &request).await.unwrap_err();
    assert!(
        matches!(&refused, ConfigureError::Refused(why) if why.contains("networking.service")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_takeover_the_host_refuses_fails_its_job_and_is_audited() {
    let s = setup(FakeConfigurator::with_ifupdown(INSTALLER));
    s.host.ifupdown.lock().unwrap().as_mut().unwrap().resolved_installed = true;
    let request = RequestContext::default();
    let job = s.network.configure.take_over(&alice(), &request).await.unwrap();
    // The files changed meanwhile.
    s.host.ifupdown.lock().unwrap().as_mut().unwrap().digest = "f".repeat(64);
    let job = finished(&s.jobs, job).await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.error.unwrap().code, "refused");
    assert_eq!(*s.host.installs.lock().unwrap(), 0, "installed already");
    assert_eq!(s.audit.0.lock().unwrap()[0].1, Outcome::Failure);
}

#[tokio::test]
async fn dhcp_asks_for_the_address_the_card_has() {
    let s = setup(FakeConfigurator::default());
    let request = RequestContext::default();
    let settings = InterfaceSettings {
        interface: "enp1s0".into(),
        ipv4: Ipv4Setting::Dhcp { keep: None },
        ipv6: Ipv6Setting::Off,
        dns: vec![],
    };
    let setup = s.network.configure.apply(settings, &alice(), &request).await.unwrap();
    assert_eq!(setup.pending.unwrap().change, Change::Interface("enp1s0".into()));
    let applied = s.host.applied.lock().unwrap()[0].clone();
    assert_eq!(applied.ipv4, Ipv4Setting::Dhcp { keep: Some("192.168.1.20".parse().unwrap()) });
    s.network.configure.confirm(&alice(), &request).await.unwrap();

    // A card switched off: nothing to keep.
    let off = InterfaceSettings {
        interface: "enp1s0".into(),
        ipv4: Ipv4Setting::Off,
        ipv6: Ipv6Setting::Off,
        dns: vec![],
    };
    s.network.configure.apply(off, &alice(), &request).await.unwrap();
    assert_eq!(s.host.applied.lock().unwrap()[1].ipv4, Ipv4Setting::Off);
    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(audit[1], ("network.confirm", Outcome::Success, Some("enp1s0".into())));
}

fn bridge(members: &[&str]) -> DeviceRequest {
    DeviceRequest::Make(NewDevice {
        name: "br0".into(),
        kind: crate::domain::DeviceKind::Bridge { stp: false },
        members: members.iter().map(|m| (*m).to_owned()).collect(),
        settings: None,
    })
}

/// A bridge over the card waits for confirmation as one change; the plan
/// says beforehand what becomes of the card, and the setup names the
/// device once it is there.
#[tokio::test]
async fn a_bridge_is_planned_then_made_as_one_change() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    let request = RequestContext::default();

    let plan = s.network.configure.plan(bridge(&["enp2s0", "enp1s0"]), &request).await.unwrap();
    assert_eq!(plan.devices[0].members, ["enp1s0", "enp2s0"], "the card with settings first");
    assert!(s.host.changes.lock().unwrap().is_empty(), "a plan changes nothing");

    let setup = s.network.configure.device(bridge(&["enp2s0", "enp1s0"]), &alice(), &request).await.unwrap();
    assert_eq!(
        setup.pending.unwrap().change,
        Change::Device("br0".into(), crate::domain::DeviceAction::Made)
    );
    assert_eq!(setup.devices[0].name, "br0");
    assert_eq!(setup.devices[0].members, ["enp1s0", "enp2s0"]);
    let again = s.network.configure.device(bridge(&["enp3s0"]), &alice(), &request).await.unwrap_err();
    assert!(matches!(again, ConfigureError::Pending), "{again}");
    s.network.configure.confirm(&alice(), &request).await.unwrap();

    // The card's settings are the bridge's now.
    let member = InterfaceSettings {
        interface: "enp1s0".into(),
        ipv4: Ipv4Setting::Dhcp { keep: None },
        ipv6: Ipv6Setting::Off,
        dns: vec![],
    };
    let refused = s.network.configure.apply(member, &alice(), &request).await.unwrap_err();
    assert!(
        matches!(&refused, ConfigureError::InvalidDevice(why) if why.0.contains("member of br0")),
        "{refused}"
    );

    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(audit[0], ("network.device_make", Outcome::Success, Some("br0".into())));
    assert_eq!(audit[1].0, "network.device_make");
    assert_eq!(audit[1].1, Outcome::Failure);
    assert_eq!(audit[2], ("network.confirm", Outcome::Success, Some("br0".into())));
    assert_eq!(audit[3], ("network.apply", Outcome::Failure, Some("enp1s0".into())));
}

/// A bridge a VM is on stays: the VMs say so through their port.
#[tokio::test]
async fn a_bridge_in_use_by_a_vm_is_not_removed() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    let request = RequestContext::default();
    s.network.configure.device(bridge(&["enp2s0"]), &alice(), &request).await.unwrap();
    s.network.configure.confirm(&alice(), &request).await.unwrap();
    s.users.0.lock().unwrap().push(("br0".into(), "web".into()));

    let remove = || DeviceRequest::Remove("br0".into());
    let refused = s.network.configure.device(remove(), &alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::InUse(users) if users == &["web"]), "{refused}");
    assert_eq!(refused.to_string(), "in use by web");
    assert!(s.network.configure.plan(remove(), &request).await.is_err());

    s.users.0.lock().unwrap().clear();
    let setup = s.network.configure.device(remove(), &alice(), &request).await.unwrap();
    assert!(setup.devices.is_empty());
    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(audit.last().unwrap(), &("network.device_remove", Outcome::Success, Some("br0".into())));
    assert_eq!(audit[audit.len() - 2], ("network.device_remove", Outcome::Failure, Some("br0".into())));
}

#[tokio::test]
async fn devices_are_checked_before_anything_is_written() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    let request = RequestContext::default();
    let refused = s.network.configure.device(bridge(&["enp9s0"]), &alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::InvalidDevice(why) if why.0.contains("enp9s0")), "{refused}");
    assert_eq!(job_error(refused).code, "invalid_input");
    assert_eq!(job_error(ConfigureError::InUse(vec!["web".into()])).code, "device_in_use");
    let change =
        DeviceRequest::Change(DeviceUpdate { name: "br0".into(), members: vec![], mode: None, stp: None });
    assert!(matches!(
        s.network.configure.plan(change, &request).await,
        Err(ConfigureError::InvalidDevice(_))
    ));
    assert!(s.host.changes.lock().unwrap().is_empty());
}

/// A bridge made without members is an internal network for VMs; one with
/// a card is not. A card a VM is on directly is not taken into a bridge
/// or bond: it would carry nothing of its own any more.
#[tokio::test]
async fn a_bridge_without_members_is_an_internal_network_and_a_cards_vms_keep_it() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    let request = RequestContext::default();
    let internal = DeviceRequest::Make(NewDevice {
        name: "vmnet0".into(),
        kind: crate::domain::DeviceKind::Bridge { stp: false },
        members: vec![],
        settings: None,
    });
    let setup = s.network.configure.device(internal, &alice(), &request).await.unwrap();
    assert_eq!(setup.internal, ["vmnet0"]);
    s.network.configure.confirm(&alice(), &request).await.unwrap();

    s.users.0.lock().unwrap().push(("enp2s0".into(), "lab".into()));
    let refused =
        s.network.configure.device(bridge(&["enp1s0", "enp2s0"]), &alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::InUse(users) if users == &["lab"]), "{refused}");
    assert!(s.network.configure.plan(bridge(&["enp2s0"]), &request).await.is_err());
    let into = DeviceRequest::Change(DeviceUpdate {
        name: "vmnet0".into(),
        members: vec!["enp2s0".into()],
        mode: None,
        stp: None,
    });
    assert!(matches!(s.network.configure.plan(into, &request).await, Err(ConfigureError::InUse(_))));

    // A card nobody is on goes in; the bridge is the LAN's then, no internal network.
    let into = DeviceRequest::Change(DeviceUpdate {
        name: "vmnet0".into(),
        members: vec!["enp3s0".into()],
        mode: None,
        stp: None,
    });
    let setup = s.network.configure.device(into, &alice(), &request).await.unwrap();
    assert!(setup.internal.is_empty(), "{:?}", setup.internal);
}

fn networkd(name: &str, text: &str) -> NetworkdFile {
    NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() }
}

/// enp1s0 by DHCP and enp2s0 fixed, as the agent wrote them.
fn confirmed_files() -> Vec<NetworkdFile> {
    vec![
        networkd("enp1s0.network", "[Match]\nName=enp1s0\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n"),
        networkd("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nAddress=10.0.0.5/24\n"),
    ]
}

fn drift_codes(s: &Setup) -> Vec<String> {
    s.notifications
        .all()
        .into_iter()
        .filter(|n| n.code == "network_drift")
        .map(|n| n.params["names"].clone())
        .collect()
}

#[tokio::test]
async fn the_first_look_keeps_the_files_and_later_ones_tell_what_differs_once() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    *s.host.files.lock().unwrap() = confirmed_files();
    let request = RequestContext::default();
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Kept);
    assert_eq!(s.store.0.lock().unwrap().as_ref().unwrap().files, confirmed_files());
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Same);
    assert!(s.network.configure.setup(&request).await.unwrap().drift.is_none());

    // enp2s0's file lost, enp1s0's changed by hand.
    s.host.files.lock().unwrap().retain(|file| !file.name.contains("enp2s0"));
    s.host.files.lock().unwrap()[0].text = "[Match]\nName=enp1s0\n[Network]\nAddress=10.9.9.9/24\n".into();
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Differs(2));
    assert_eq!(drift_codes(&s), ["enp1s0, enp2s0"]);
    // Told once, also after a restart.
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Differs(2));
    assert_eq!(drift_codes(&s).len(), 1);
    let drift = s.network.configure.setup(&request).await.unwrap().drift.unwrap();
    assert_eq!(
        drift.differences,
        [
            Difference { part: Part::Interface, name: "enp1s0".into(), kind: DifferenceKind::Changed },
            Difference { part: Part::Interface, name: "enp2s0".into(), kind: DifferenceKind::Missing },
        ]
    );
    assert_eq!(drift.kept_at, FixedClock.now());

    // Nothing else changes meanwhile: it would be planned on files
    // zendiloOS did not write.
    let settings = InterfaceSettings {
        interface: "enp3s0".into(),
        ipv4: Ipv4Setting::Dhcp { keep: None },
        ipv6: Ipv6Setting::Off,
        dns: vec![],
    };
    let refused = s.network.configure.apply(settings, &alice(), &request).await.unwrap_err();
    assert!(matches!(refused, ConfigureError::Drifted), "{refused}");
    assert_eq!(job_error(refused).code, "network_drift");
    let refused = s.network.configure.plan(bridge(&["enp2s0"]), &request).await.unwrap_err();
    assert!(matches!(refused, ConfigureError::Drifted), "{refused}");

    // Written back, as one change that waits for confirmation.
    let restored = s.network.configure.restore(&alice(), &request).await.unwrap();
    assert_eq!(restored.pending.unwrap().change, Change::Restore);
    assert!(restored.drift.is_none(), "not told while a change waits");
    let edit = s.host.changes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(edit.subject, Subject::Restore);
    let written: Vec<&str> = edit.links.iter().map(|link| link.settings.interface.as_str()).collect();
    assert_eq!(written, ["enp1s0", "enp2s0"]);
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Pending);
    s.network.configure.confirm(&alice(), &request).await.unwrap();
    assert!(s.network.configure.setup(&request).await.unwrap().drift.is_none());
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::Same);
    assert_eq!(s.store.0.lock().unwrap().as_ref().unwrap().said, None, "told again should it differ again");
    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(
        audit.iter().map(|(action, outcome, _)| (*action, *outcome)).collect::<Vec<_>>(),
        [
            ("network.apply", Outcome::Failure),
            ("network.restore", Outcome::Success),
            ("network.confirm", Outcome::Success)
        ]
    );
    // Nothing differs: nothing to write back.
    let refused = s.network.configure.restore(&alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::Refused(why) if why.contains("already")), "{refused}");
}

#[tokio::test]
async fn files_changed_on_purpose_are_kept_as_the_settings() {
    let s = setup(FakeConfigurator::default());
    *s.host.files.lock().unwrap() = confirmed_files();
    let request = RequestContext::default();
    s.network.configure.look_at_files(&request).await.unwrap();
    s.host
        .files
        .lock()
        .unwrap()
        .push(networkd("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nDHCP=ipv4\n"));
    let drift = s.network.configure.setup(&request).await.unwrap().drift.unwrap();
    assert_eq!(drift.differences[0].kind, DifferenceKind::Added);

    let kept = s.network.configure.keep(&alice(), &request).await.unwrap();
    assert!(kept.drift.is_none());
    assert_eq!(s.store.0.lock().unwrap().as_ref().unwrap().files.len(), 3);
    assert_eq!(s.audit.0.lock().unwrap()[0].0, "network.keep_files");
    assert!(s.host.changes.lock().unwrap().is_empty(), "nothing written");
}

#[tokio::test]
async fn a_confirmed_change_becomes_the_settings_and_one_that_waits_is_no_difference() {
    let s = setup(FakeConfigurator::default());
    *s.host.files.lock().unwrap() = confirmed_files();
    let request = RequestContext::default();
    s.network.configure.look_at_files(&request).await.unwrap();
    let settings = InterfaceSettings {
        interface: "enp1s0".into(),
        ipv4: Ipv4Setting::Static { address: "192.168.1.10".parse().unwrap(), prefix: 24, gateway: None },
        ipv6: Ipv6Setting::Off,
        dns: vec![],
    };
    let waiting = s.network.configure.apply(settings, &alice(), &request).await.unwrap();
    assert!(waiting.drift.is_none() && waiting.pending.is_some());
    // Not confirmed yet: the settings are as they were.
    assert_eq!(s.store.0.lock().unwrap().as_ref().unwrap().files, confirmed_files());
    s.network.configure.confirm(&alice(), &request).await.unwrap();
    let kept = s.store.0.lock().unwrap().clone().unwrap();
    assert!(kept.files.iter().any(|file| file.text.contains("Address=192.168.1.10/24")), "{:?}", kept.files);
    assert!(s.network.configure.setup(&request).await.unwrap().drift.is_none());
}

#[tokio::test]
async fn nothing_is_kept_or_compared_where_zendiloos_does_not_manage_the_network() {
    let s = setup(FakeConfigurator::with_ifupdown(INSTALLER));
    let request = RequestContext::default();
    assert_eq!(s.network.configure.look_at_files(&request).await.unwrap(), Looked::NotManaged);
    assert!(s.store.0.lock().unwrap().is_none());
    for refused in [
        s.network.configure.restore(&alice(), &request).await.unwrap_err(),
        s.network.configure.keep(&alice(), &request).await.unwrap_err(),
    ] {
        assert!(
            matches!(&refused, ConfigureError::Refused(why) if why.contains("does not manage")),
            "{refused}"
        );
    }

    // Managed without settings kept yet: nothing to restore.
    let s = setup(FakeConfigurator::default());
    let refused = s.network.configure.restore(&alice(), &request).await.unwrap_err();
    assert!(matches!(&refused, ConfigureError::Refused(why) if why.contains("keeps no")), "{refused}");
    // While a change waits, neither.
    s.network.configure.look_at_files(&request).await.unwrap();
    s.network.configure.device(bridge(&["enp2s0"]), &alice(), &request).await.ok();
    *s.host.pending.lock().unwrap() =
        Some(PendingChange { change: Change::Restore, revert_at: Timestamp::from_unix_seconds(1) });
    for refused in [
        s.network.configure.restore(&alice(), &request).await.unwrap_err(),
        s.network.configure.keep(&alice(), &request).await.unwrap_err(),
    ] {
        assert!(matches!(refused, ConfigureError::Pending), "{refused}");
    }
}

fn restored_file(name: &str, text: &str) -> NetworkdFile {
    NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() }
}

/// What a restored backup brought: enp1s0 fixed, eno1 (a card of another
/// machine) by DHCP, br0 of enp2s0 and enp3s0.
fn restored_backup(s: &Setup) {
    *s.store.1.lock().unwrap() = Some(RestoredSettings {
        files: vec![
            restored_file(
                "enp1s0.network",
                "[Match]\nName=enp1s0\n[Network]\nAddress=192.168.1.30/24\nGateway=192.168.1.1\n",
            ),
            restored_file("eno1.network", "[Match]\nName=eno1\n[Network]\nDHCP=ipv4\n"),
            restored_file(
                "br0.netdev",
                "# Members: enp2s0 enp3s0\n[NetDev]\nName=br0\nKind=bridge\n[Bridge]\nSTP=no\n",
            ),
            restored_file("br0.network", "[Match]\nName=br0\n[Network]\nDHCP=ipv4\n"),
            restored_file("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nBridge=br0\n"),
            restored_file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nBridge=br0\n"),
        ],
        restored_at: Timestamp::from_unix_seconds(500),
    });
}

#[tokio::test]
async fn a_restored_backups_network_is_applied_where_its_names_are_here_and_confirmed() {
    let s = setup(FakeConfigurator::default());
    s.source.set(three_cards());
    let request = RequestContext::default();
    assert_eq!(s.network.configure.restored(&request).await.unwrap(), None);
    let none = s.network.configure.apply_restored("enp1s0", &alice(), &request).await.unwrap_err();
    assert!(matches!(none, ConfigureError::Refused(_)), "{none:?}");

    restored_backup(&s);
    let restored = s.network.configure.restored(&request).await.unwrap().unwrap();
    assert_eq!(restored.restored_at, Timestamp::from_unix_seconds(500));
    let standing: Vec<(&str, Standing)> = restored
        .configuration
        .links
        .iter()
        .map(|link| (link.settings.interface.as_str(), link.standing.clone()))
        .collect();
    assert_eq!(
        standing,
        [
            ("br0", Standing::Missing),
            ("eno1", Standing::Missing),
            ("enp1s0", Standing::Applicable),
            ("enp2s0", Standing::MemberThere("br0".into())),
            ("enp3s0", Standing::MemberThere("br0".into())),
        ]
    );
    assert_eq!(restored.configuration.devices[0].standing, Standing::Applicable);

    // Skipped: no such card here; a member comes with its device.
    for (name, why) in
        [("eno1", "no interface eno1"), ("enp2s0", "make br0"), ("wlan9", "no settings of wlan9")]
    {
        let refused = s.network.configure.apply_restored(name, &alice(), &request).await.unwrap_err();
        assert!(
            matches!(&refused, ConfigureError::Refused(reason) if reason.contains(why)),
            "{name}: {refused:?}"
        );
    }
    assert!(s.host.changes.lock().unwrap().is_empty(), "nothing changed");

    // An interface: an ordinary change, waiting to be confirmed.
    let waiting = s.network.configure.apply_restored("enp1s0", &alice(), &request).await.unwrap();
    assert_eq!(waiting.pending.unwrap().change, Change::Interface("enp1s0".into()));
    let applied = s.host.applied.lock().unwrap()[0].clone();
    assert_eq!(
        applied.ipv4,
        Ipv4Setting::Static {
            address: [192, 168, 1, 30].into(),
            prefix: 24,
            gateway: Some([192, 168, 1, 1].into())
        }
    );
    let busy = s.network.configure.apply_restored("br0", &alice(), &request).await.unwrap_err();
    assert!(matches!(busy, ConfigureError::Pending), "one change at a time: {busy:?}");
    s.network.configure.confirm(&alice(), &request).await.unwrap();
    let after = s.network.configure.restored(&request).await.unwrap().unwrap();
    assert_eq!(after.configuration.links[2].standing, Standing::InForce, "the files say it now");

    // A bridge: made as the backup had it.
    let made = s.network.configure.apply_restored("br0", &alice(), &request).await.unwrap();
    assert_eq!(made.pending.unwrap().change, Change::Device("br0".into(), DeviceAction::Made));
    let edit = s.host.changes.lock().unwrap().last().unwrap().clone();
    assert_eq!(edit.devices[0].members, ["enp2s0", "enp3s0"]);

    s.network.configure.forget_restored(&alice(), &request).await.unwrap();
    assert_eq!(s.network.configure.restored(&request).await.unwrap(), None);
    let audit: Vec<&str> = s.audit.0.lock().unwrap().iter().map(|(action, ..)| *action).collect();
    assert_eq!(
        audit,
        [
            "network.apply",
            "network.device_make",
            "network.confirm",
            "network.device_make",
            "network.forget_restored"
        ]
    );
}
