use std::sync::Mutex as StdMutex;

use zendiloos_notify::testing::MemoryNotifications;

use super::*;
use crate::domain::{
    KnownService, ProjectPorts, Protocol, Rule, Service, Traffic, Transport, compile, parse_ports,
};
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
    // At start the host is given the rules kept (off).
    s.firewall.refresh(&context()).await.unwrap();
    s.firewall.apply(settings.clone(), browser(), false, &alice(), &context()).await.unwrap();
    assert_eq!(s.host.calls(), ["keep 0 rules", "apply 1 rules, confirm within 90"]);
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
    assert_eq!(s.host.calls().len(), 2);
    assert!(s.notifications.all().is_empty(), "nothing told while it waits");
    // A port changing while it waits: the change and what returns, both.
    s.facts.set(|facts| facts.web_ports = vec![80, 443, 8443]);
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().last().unwrap(), "keep 0 rules, 1 waiting");

    s.firewall.confirm(&alice(), &context()).await.unwrap();
    assert_eq!(s.repository.settings().await.unwrap(), settings);
    assert_eq!(codes(&s.notifications), ["firewall_blocked smb"]);
    // The host keeps what it was given: nothing to ask it.
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().last().unwrap(), "confirm");
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
    assert_eq!(s.host.calls().last().unwrap(), "keep 0 rules");
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
    assert_eq!(s.host.calls(), ["keep 1 rules"], "the project publishes nothing yet");
    s.facts.set(|facts| {
        facts.projects = vec![ProjectPorts {
            name: "nextcloud".into(),
            ports: vec![(Transport::Tcp, 8080), (Transport::Udp, 3478)],
        }];
    });
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().last().unwrap(), "keep 3 rules");
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
    assert_eq!(s.host.calls(), ["keep 1 rules"]);
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

/// zendiloosd started again, with the same ports.
fn restarted(s: &Setup) -> Firewall {
    Firewall::new(FirewallPorts {
        repository: s.repository.clone(),
        facts: s.facts.clone(),
        host: s.host.clone(),
        notifier: Notifier::new(s.notifications.clone(), Arc::new(Fixed)),
        audit: s.audit.clone(),
        clock: Arc::new(Fixed),
    })
}

fn site(port: u16) -> Vec<ProjectPorts> {
    vec![ProjectPorts { name: "site".into(), ports: vec![(Transport::Tcp, port)] }]
}

/// A change waits only in zendiloosd's memory. Restarted within the 90
/// seconds, it cannot confirm it any more: the host ends it at the first
/// refresh and enforces the rules kept, with the ports of now (the project
/// moved from 8080 to 9090 meanwhile), not the older file its timer would
/// have put back.
#[tokio::test]
async fn a_restart_while_a_change_waits_brings_the_host_to_the_rules_kept() {
    let s = setup();
    let saved = closed(vec![
        allow(Service::Known(KnownService::WebInterface), "any"),
        allow(Service::Project("site".into()), "any"),
    ]);
    s.repository.save(&saved).await.unwrap();
    s.facts.set(|facts| facts.projects = site(8080));
    s.firewall.refresh(&context()).await.unwrap();
    s.firewall
        .apply(closed(vec![allow(Service::All, "any")]), browser(), false, &alice(), &context())
        .await
        .unwrap();
    s.facts.set(|facts| facts.projects = site(9090));
    let after = restarted(&s);
    after.refresh(&context()).await.unwrap();
    assert_eq!(s.host.state(&context()).await.unwrap().pending_until, None, "ended");
    // The agent's timer has nothing left to put back.
    s.host.time_up();
    after.refresh(&context()).await.unwrap();
    let facts = s.facts.facts(&context()).await;
    assert_eq!(s.host.rules(), compile(&saved, &facts));
    assert_ne!(s.host.calls().last().unwrap(), "went back");
    let late = after.confirm(&alice(), &context()).await;
    assert!(matches!(late, Err(FirewallError::Refused(_))), "{late:?}");
    assert_eq!(s.repository.settings().await.unwrap(), saved);
}

/// "Deny the project `site`" is kept; `site` starts publishing 8080 while a
/// change waits and zendiloosd restarts: after the agent's time is up the
/// project's port is denied, as the rules kept say.
#[tokio::test]
async fn a_restart_while_a_change_waits_keeps_a_projects_denial() {
    let s = setup();
    let deny_site = Rule { action: Action::Deny, ..allow(Service::Project("site".into()), "any") };
    let saved = FirewallSettings { enabled: true, fallback: Action::Allow, rules: vec![deny_site] };
    s.repository.save(&saved).await.unwrap();
    s.firewall.refresh(&context()).await.unwrap();
    let mut change = saved.clone();
    change.rules.push(allow(Service::Known(KnownService::Ssh), "any"));
    s.firewall.apply(change, browser(), false, &alice(), &context()).await.unwrap();

    s.facts.set(|facts| facts.projects = site(8080));
    let after = restarted(&s);
    after.refresh(&context()).await.unwrap();
    s.host.time_up();
    after.refresh(&context()).await.unwrap();

    let facts = s.facts.facts(&context()).await;
    assert_eq!(s.host.rules(), compile(&saved, &facts), "calls: {:?}", s.host.calls());
    assert!(s.host.rules().unwrap().rules.iter().any(|r| r.action == Action::Deny));
}

/// A change begun before a restart is ended before a new one begins.
#[tokio::test]
async fn a_change_begun_before_a_restart_does_not_block_a_new_one() {
    let s = setup();
    s.firewall.refresh(&context()).await.unwrap();
    s.firewall
        .apply(closed(vec![allow(Service::All, "any")]), browser(), false, &alice(), &context())
        .await
        .unwrap();
    let after = restarted(&s);
    let settings = closed(vec![allow(Service::Known(KnownService::WebInterface), "any")]);
    after.apply(settings.clone(), browser(), false, &alice(), &context()).await.unwrap();
    assert_eq!(after.view(&context()).await.unwrap().pending.unwrap().0, settings);
    assert_eq!(s.host.calls()[2..], ["keep 0 rules", "apply 1 rules, confirm within 90"]);
}

/// The host's timer fires between zendiloosd's look at it (`settle`) and
/// the rules it then sends for the change it believes waits.
struct GoesBackMidway(Arc<FakeFirewallHost>, std::sync::atomic::AtomicBool);

#[async_trait]
impl FirewallHost for GoesBackMidway {
    async fn state(&self, context: &RequestContext) -> Result<HostState, PortError> {
        self.0.state(context).await
    }

    async fn begin(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: u16,
        context: &RequestContext,
    ) -> Result<HostState, HostError> {
        self.0.begin(ruleset, confirm_within, context).await
    }

    async fn keep(&self, rules: &HostRules, context: &RequestContext) -> Result<HostState, HostError> {
        if self.1.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.0.time_up();
        }
        self.0.keep(rules, context).await
    }

    async fn confirm(&self, context: &RequestContext) -> Result<(), HostError> {
        self.0.confirm(context).await
    }
}

/// The host decides, at once with its timer: rules for a change that just
/// went back are not put in force; the rules kept are, and zendiloosd knows
/// at once that nothing waits.
#[tokio::test]
async fn a_refresh_racing_the_timer_leaves_the_rules_kept_in_force() {
    let s = setup();
    let host = Arc::new(GoesBackMidway(s.host.clone(), std::sync::atomic::AtomicBool::new(false)));
    let firewall = Firewall::new(FirewallPorts {
        repository: s.repository.clone(),
        facts: s.facts.clone(),
        host: host.clone(),
        notifier: Notifier::new(s.notifications.clone(), Arc::new(Fixed)),
        audit: s.audit.clone(),
        clock: Arc::new(Fixed),
    });
    firewall.refresh(&context()).await.unwrap();
    let unconfirmed = closed(vec![allow(Service::Known(KnownService::WebInterface), "192.168.1.0/24")]);
    firewall.apply(unconfirmed, browser(), false, &alice(), &context()).await.unwrap();
    // Something changes (a web interface port waits), so the refresh asks the host.
    s.facts.set(|facts| facts.web_ports = vec![80, 443, 8443]);
    host.1.store(true, std::sync::atomic::Ordering::SeqCst);
    firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.state(&context()).await.unwrap().pending_until, None);
    assert_eq!(s.host.rules(), None, "the rules kept: off");
    assert!(firewall.view(&context()).await.unwrap().pending.is_none());
    // Nothing left to bring up to date.
    let calls = s.host.calls().len();
    firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.calls().len(), calls);
}

/// At boot Docker answers late: until it does, the projects' ports saved
/// when it last answered stand for them, so a rule about a project holds
/// from the first refresh.
#[tokio::test]
async fn a_projects_rule_holds_from_the_start_with_the_ports_saved() {
    let s = setup();
    let saved = closed(vec![allow(Service::Project("site".into()), "any")]);
    s.repository.save(&saved).await.unwrap();
    s.facts.set(|facts| facts.projects = site(8080));
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.repository.known_projects().await.unwrap(), site(8080), "saved once Docker answered");

    // zendiloosd starts again; Docker has not answered yet.
    s.facts.set(|facts| (facts.projects, facts.projects_unknown) = (vec![], true));
    let after = restarted(&s);
    after.refresh(&context()).await.unwrap();
    let rules = s.host.rules().unwrap().rules;
    assert!(rules.iter().any(|r| r.traffic == Traffic::Tcp(parse_ports("8080").unwrap())), "{rules:?}");
    // Docker answers: what it says counts, and is saved.
    s.facts.set(|facts| (facts.projects, facts.projects_unknown) = (site(9090), false));
    after.refresh(&context()).await.unwrap();
    assert!(
        s.host.rules().unwrap().rules.iter().any(|r| r.traffic == Traffic::Tcp(parse_ports("9090").unwrap()))
    );
    assert_eq!(s.repository.known_projects().await.unwrap(), site(9090));
}

/// What a restored backup brought: the web interface from its own network
/// only, everything else denied.
fn restored(s: &Setup, source: &str) -> FirewallSettings {
    let settings = closed(vec![allow(Service::Known(KnownService::WebInterface), source)]);
    *s.repository.restored.lock().unwrap() =
        Some(RestoredFirewall { settings: settings.clone(), restored_at: Timestamp::from_unix_seconds(7) });
    settings
}

#[tokio::test]
async fn a_restored_backups_rules_come_in_force_only_once_confirmed() {
    let s = setup();
    let none = s.firewall.apply_restored(browser(), false, &alice(), &context()).await;
    assert!(matches!(none, Err(FirewallError::Refused(_))), "{none:?}");
    let backed_up = restored(&s, "192.168.1.0/24");
    let view = s.firewall.view(&context()).await.unwrap();
    assert_eq!(view.restored.as_ref().map(|r| &r.settings), Some(&backed_up));
    assert_eq!(view.current, FirewallSettings::default(), "the defaults hold meanwhile");

    // Not confirmed: the rules from before (the defaults) are back, the
    // backup's still offered.
    s.firewall.apply_restored(browser(), false, &alice(), &context()).await.unwrap();
    assert!(s.firewall.view(&context()).await.unwrap().pending.is_some());
    s.host.time_up();
    s.firewall.refresh(&context()).await.unwrap();
    assert_eq!(s.host.rules(), None);
    assert!(s.firewall.view(&context()).await.unwrap().restored.is_some());

    // Confirmed: this machine's, offered no more.
    s.firewall.apply_restored(browser(), false, &alice(), &context()).await.unwrap();
    s.firewall.confirm(&alice(), &context()).await.unwrap();
    assert_eq!(s.repository.settings().await.unwrap(), backed_up);
    let view = s.firewall.view(&context()).await.unwrap();
    assert_eq!(view.restored, None);
}

#[tokio::test]
async fn a_restored_backups_rules_that_shut_this_browser_out_are_refused_or_forgotten() {
    let s = setup();
    restored(&s, "10.20.0.0/16");
    let refused = s.firewall.apply_restored(browser(), false, &alice(), &context()).await;
    assert!(matches!(refused, Err(FirewallError::WouldLockOut)), "another network's: {refused:?}");
    assert!(s.host.calls().is_empty());
    s.firewall.forget_restored(&alice(), &context()).await.unwrap();
    assert_eq!(s.firewall.view(&context()).await.unwrap().restored, None);
    let actions: Vec<&str> = s.audit.0.lock().unwrap().iter().map(|(action, _)| *action).collect();
    assert_eq!(actions, ["firewall.forget_restored"]);
}
