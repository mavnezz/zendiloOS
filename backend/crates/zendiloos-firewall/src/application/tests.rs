use std::sync::Mutex as StdMutex;

use zendiloos_notify::testing::MemoryNotifications;

use super::*;
use crate::domain::{KnownService, ProjectPorts, Protocol, Rule, Service, Transport, parse_ports};
use crate::testing::{FakeFirewallHost, FixedFacts, MemoryFirewall};

#[derive(Default)]
struct Audit(StdMutex<Vec<(&'static str, String)>>);

#[async_trait]
impl AuditLog for Audit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push((entry.action, entry.target.unwrap_or_default()));
        Ok(())
    }
}

struct Fixed;

impl Clock for Fixed {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(1_800_000_000)
    }
}

struct Setup {
    firewall: Firewall,
    repository: Arc<MemoryFirewall>,
    facts: Arc<FixedFacts>,
    host: Arc<FakeFirewallHost>,
    notifications: Arc<MemoryNotifications>,
    audit: Arc<Audit>,
}

fn setup() -> Setup {
    let repository = Arc::new(MemoryFirewall::default());
    let facts = Arc::new(FixedFacts::default());
    let host = Arc::new(FakeFirewallHost::default());
    host.set_now(Fixed.now());
    let notifications = Arc::new(MemoryNotifications::default());
    let audit = Arc::new(Audit::default());
    let firewall = Firewall::new(FirewallPorts {
        repository: repository.clone(),
        facts: facts.clone(),
        host: host.clone(),
        notifier: Notifier::new(notifications.clone(), Arc::new(Fixed)),
        audit: audit.clone(),
        clock: Arc::new(Fixed),
    });
    Setup { firewall, repository, facts, host, notifications, audit }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

/// A browser in the local network on the interface's address, at 443.
fn browser() -> Connection {
    Connection {
        client: Some("192.168.1.50".parse().unwrap()),
        server: Some("192.168.1.10".parse().unwrap()),
        port: Some(443),
    }
}

fn allow(service: Service, source: &str) -> Rule {
    Rule {
        enabled: true,
        interface: None,
        service,
        source: Source::parse(source).unwrap(),
        action: Action::Allow,
    }
}

fn closed(rules: Vec<Rule>) -> FirewallSettings {
    FirewallSettings { enabled: true, fallback: Action::Deny, rules }
}

fn codes(notifications: &MemoryNotifications) -> Vec<String> {
    notifications
        .all()
        .iter()
        .map(|n| format!("{} {}", n.code, n.params.get("service").cloned().unwrap_or_default()))
        .collect()
}

#[tokio::test]
async fn a_change_is_kept_once_confirmed() {
    let s = setup();
    let web = allow(Service::Known(KnownService::WebInterface), "192.168.1.0/24");
    let settings = closed(vec![web]);
    s.firewall.apply(settings.clone(), browser(), false, &alice(), &context()).await.unwrap();
    assert_eq!(s.host.calls(), ["apply 1 rules, confirm Some(90)"]);
    let view = s.firewall.view(&context()).await.unwrap();
    assert_eq!(view.current, FirewallSettings::default(), "not saved before it is confirmed");
    let (pending, until) = view.pending.unwrap();
    assert_eq!((pending, until.unix_seconds()), (settings.clone(), 1_800_000_090));
    assert_eq!(view.blocked, [Exposed::Known(KnownService::Smb)], "of what is in force");
    assert_eq!(view.interfaces, ["enp1s0"]);
    assert_eq!(view.local_networks, [Source::parse("192.168.1.0/24").unwrap()]);

    // One change at a time.
    let again = s.firewall.apply(settings.clone(), browser(), false, &alice(), &context()).await;
    assert!(matches!(again, Err(FirewallError::Pending)), "{again:?}");
    // A refresh with nothing new asks the host nothing.
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().len(), 1);
    assert!(s.notifications.all().is_empty(), "nothing told while it waits");

    s.firewall.confirm(&alice(), &context()).await.unwrap();
    assert_eq!(s.repository.settings().await.unwrap(), settings);
    assert_eq!(codes(&s.notifications), ["firewall_blocked smb"]);
    let nothing = s.firewall.confirm(&alice(), &context()).await;
    assert!(matches!(nothing, Err(FirewallError::Refused(_))), "{nothing:?}");
    let actions: Vec<_> = s.audit.0.lock().unwrap().iter().map(|(a, t)| format!("{a}: {t}")).collect();
    assert_eq!(
        actions,
        ["firewall.apply: on, 1 rules, otherwise deny", "firewall.confirm: on, 1 rules, otherwise deny"]
    );
}

#[tokio::test]
async fn a_change_not_confirmed_goes_back_and_is_forgotten() {
    let s = setup();
    let settings = closed(vec![allow(Service::All, "any")]);
    s.firewall.apply(settings, browser(), false, &alice(), &context()).await.unwrap();
    s.host.time_up();
    assert!(s.firewall.view(&context()).await.unwrap().pending.is_none());
    let late = s.firewall.confirm(&alice(), &context()).await;
    assert!(matches!(late, Err(FirewallError::Refused(_))), "{late:?}");
    // What went back is given again, as saved: off.
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().last().unwrap(), "apply 0 rules, confirm None");
    assert_eq!(s.host.rules(), None);
    assert_eq!(s.repository.settings().await.unwrap(), FirewallSettings::default());
}

#[tokio::test]
async fn rules_that_would_shut_out_the_browser_are_refused_unless_meant() {
    let s = setup();
    let lan_only = closed(vec![allow(Service::Known(KnownService::WebInterface), "192.168.1.0/24")]);
    let elsewhere = Connection { client: Some("10.0.0.5".parse().unwrap()), ..browser() };
    let refused = s.firewall.apply(lan_only.clone(), elsewhere, false, &alice(), &context()).await;
    assert!(matches!(refused, Err(FirewallError::WouldLockOut)), "{refused:?}");
    assert!(s.host.calls().is_empty());
    // The interface the browser came in on counts.
    let other_interface = closed(vec![Rule {
        interface: Some("enp2s0".into()),
        ..allow(Service::Known(KnownService::WebInterface), "any")
    }]);
    let refused = s.firewall.apply(other_interface, browser(), false, &alice(), &context()).await;
    assert!(matches!(refused, Err(FirewallError::WouldLockOut)));
    // Meant: applied, and going back by itself if nobody confirms.
    s.firewall.apply(lan_only, elsewhere, true, &alice(), &context()).await.unwrap();
    assert_eq!(s.host.calls().len(), 1);
    let invalid = closed(vec![Rule { interface: Some("lo".into()), ..allow(Service::All, "any") }]);
    let invalid = s.firewall.apply(invalid, browser(), false, &alice(), &context()).await;
    assert!(matches!(invalid, Err(FirewallError::Invalid(_))));
}

#[tokio::test]
async fn the_host_follows_the_services_ports() {
    let s = setup();
    s.repository
        .save(&closed(vec![
            allow(Service::Known(KnownService::WebInterface), "any"),
            allow(Service::Project("nextcloud".into()), "any"),
        ]))
        .await
        .unwrap();
    // At start the host is given the rules once.
    s.firewall.refresh(&context()).await.unwrap();
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls(), ["apply 1 rules, confirm None"], "the project publishes nothing yet");
    s.facts.set(|facts| {
        facts.projects = vec![ProjectPorts {
            name: "nextcloud".into(),
            ports: vec![(Transport::Tcp, 8080), (Transport::Udp, 3478)],
        }];
    });
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().last().unwrap(), "apply 3 rules, confirm None");
    // SMB stays blocked, told once.
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(codes(&s.notifications), ["firewall_blocked smb"]);
    // Off and on again: told again.
    s.facts.set(|facts| facts.smb_on = false);
    s.firewall.refresh(&context()).await.unwrap();
    assert!(s.repository.told_blocked().await.unwrap().is_empty());
    s.facts.set(|facts| facts.smb_on = true);
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(codes(&s.notifications).len(), 2);
}

#[tokio::test]
async fn a_project_blocked_is_told_with_its_name() {
    let s = setup();
    s.facts.set(|facts| {
        facts.smb_on = false;
        facts.projects = vec![ProjectPorts { name: "media".into(), ports: vec![(Transport::Tcp, 8096)] }];
    });
    let ports = Service::Ports { protocol: Protocol::Tcp, ports: parse_ports("80, 443").unwrap() };
    s.repository.save(&closed(vec![allow(ports, "any")])).await.unwrap();
    s.firewall.refresh(&context()).await.unwrap();
    let told = s.notifications.all();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].params.get("service").map(String::as_str), Some("project:media"));
    assert_eq!(told[0].params.get("project").map(String::as_str), Some("media"));
    assert_eq!(told[0].level, Level::Warning);
}

#[tokio::test]
async fn the_host_failing_is_said_and_tried_again() {
    let s = setup();
    s.host.fail(Some("nft: syntax error"));
    let failed = s.firewall.apply(closed(vec![]), Connection::default(), false, &alice(), &context()).await;
    assert!(matches!(failed, Err(FirewallError::HostFailed(ref reason)) if reason == "nft: syntax error"));
    assert!(s.firewall.view(&context()).await.unwrap().pending.is_none());
    s.repository.save(&closed(vec![allow(Service::All, "any")])).await.unwrap();
    assert!(s.firewall.refresh(&context()).await.is_err());
    s.host.fail(None);
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls(), ["apply 1 rules, confirm None"]);
    let audit = s.audit.0.lock().unwrap().clone();
    assert_eq!(audit, [("firewall.apply", "on, 0 rules, otherwise deny".to_owned())]);
}

#[tokio::test]
async fn a_nudge_wakes_whoever_waits() {
    let s = setup();
    let waiting = tokio::spawn({
        let firewall = s.firewall.clone();
        async move { firewall.nudged().await }
    });
    tokio::task::yield_now().await;
    s.firewall.nudge();
    tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
    assert_eq!(describe(&FirewallSettings::default()), "off");
    let allowing = FirewallSettings { enabled: true, ..FirewallSettings::default() };
    assert_eq!(describe(&allowing), "on, 0 rules, otherwise allow");
    let error: FirewallError = HostError::Conflict.into();
    assert!(matches!(error, FirewallError::Pending));
    let error: FirewallError = HostError::Refused("no".into()).into();
    assert!(matches!(error, FirewallError::Refused(_)));
    let error: FirewallError = HostError::Port(PortError::new("agent", "gone")).into();
    assert!(matches!(error, FirewallError::Port(_)));
}
