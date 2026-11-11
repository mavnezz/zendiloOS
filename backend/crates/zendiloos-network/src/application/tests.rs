use std::sync::Mutex;
use std::time::Duration;

use zendiloos_jobs::JobState;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};

use super::*;
use crate::domain::{Ipv6Setting, Refusal};
use crate::testing::{DIGEST, FakeConfigurator, FixedNetwork};

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
    audit: Arc<Audit>,
    jobs: Jobs,
}

fn setup(host: FakeConfigurator) -> Setup {
    let host = Arc::new(host);
    let audit = Arc::new(Audit::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let network = NetworkUseCases::new(NetworkPorts {
        source: Arc::new(FixedNetwork),
        configurator: host.clone(),
        jobs: jobs.clone(),
        audit: audit.clone(),
        clock: Arc::new(FixedClock),
    });
    Setup { network, host, audit, jobs }
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
