use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use zendiloos_agent_proto::{FirewallPortRange, FirewallRules, InterfaceName};

use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn tcp(ranges: &[(u16, u16)]) -> FirewallTraffic {
    let ranges = ranges.iter().map(|(from, to)| FirewallPortRange::new(*from, *to).unwrap()).collect();
    FirewallTraffic::Tcp { ports: FirewallPorts::new(ranges).unwrap() }
}

fn udp(port: u16) -> FirewallTraffic {
    FirewallTraffic::Udp {
        ports: FirewallPorts::new(vec![FirewallPortRange::new(port, port).unwrap()]).unwrap(),
    }
}

fn rule(
    source: FirewallSource,
    traffic: FirewallTraffic,
    forward: bool,
    verdict: FirewallVerdict,
) -> FirewallRule {
    FirewallRule { interface: None, source, traffic, forward, verdict }
}

fn config(rules: Vec<FirewallRule>, fallback: FirewallVerdict) -> FirewallConfig {
    FirewallConfig {
        rules: FirewallRules::new(rules).unwrap(),
        fallback,
        bridges: vec![],
        own_bridges: vec![],
    }
}

fn chain<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text.find(&format!("chain {name} {{")).expect("chain");
    let rest = &text[start..];
    &rest[..rest.find("\n\t}").expect("end of chain")]
}

#[test]
fn the_file_says_exactly_the_rules() {
    let off = ruleset(None, Phase::Kept);
    assert!(off.ends_with("table inet zendiloos_firewall\ndelete table inet zendiloos_firewall\n"), "{off}");
    assert!(!off.contains("chain"));

    let lan = FirewallSource::network(ip("192.168.1.0"), 24).unwrap();
    let rules = vec![
        FirewallRule {
            interface: Some(InterfaceName::parse("enp1s0").unwrap()),
            ..rule(lan, tcp(&[(139, 139), (445, 445)]), false, FirewallVerdict::Accept)
        },
        rule(
            FirewallSource::network(ip("10.0.0.7"), 32).unwrap(),
            FirewallTraffic::All {},
            true,
            FirewallVerdict::Drop,
        ),
        rule(FirewallSource::Any, tcp(&[(8080, 8081)]), true, FirewallVerdict::Accept),
        rule(
            FirewallSource::range(ip("fd00::1"), ip("fd00::9")).unwrap(),
            udp(3478),
            true,
            FirewallVerdict::Accept,
        ),
        rule(FirewallSource::Any, FirewallTraffic::Ping {}, false, FirewallVerdict::Accept),
        rule(lan, FirewallTraffic::Ping {}, false, FirewallVerdict::Accept),
        rule(
            FirewallSource::network(ip("fd00::"), 8).unwrap(),
            FirewallTraffic::Ping {},
            true,
            FirewallVerdict::Drop,
        ),
    ];
    let mut with_bridges = config(rules, FirewallVerdict::Drop);
    with_bridges.bridges = vec![DockerBridge::parse("br-0123456789ab").unwrap()];
    with_bridges.own_bridges = vec![InterfaceName::parse("br-lan").unwrap()];
    let text = ruleset(Some(&with_bridges), Phase::Kept);
    assert!(text.starts_with(HEADER));
    let (input, forward) = text.split_once("chain forward").unwrap();
    for line in [
        "\t\ttype filter hook input priority filter - 5; policy accept;\n",
        "\t\tct state established,related accept\n",
        "\t\tiif \"lo\" accept\n",
        "\t\tiifname { \"br-0123456789ab\", \"docker0\" } accept\n",
        "\t\tudp sport 67 udp dport 68 accept\n",
        "\t\tiifname \"enp1s0\" ip saddr 192.168.1.0/24 tcp dport { 139, 445 } accept\n",
        "\t\tip saddr 10.0.0.7 drop\n",
        "\t\ttcp dport { 8080-8081 } accept\n",
        "\t\tip6 saddr fd00::1-fd00::9 udp dport { 3478 } accept\n",
        "\t\tmeta l4proto icmp icmp type echo-request accept\n",
        "\t\tmeta l4proto ipv6-icmp icmpv6 type echo-request accept\n",
        "\t\tip saddr 192.168.1.0/24 icmp type echo-request accept\n",
        "\t\tip6 saddr fd00::/8 icmpv6 type echo-request drop\n",
        "\t\tdrop\n\t}\n",
    ] {
        assert!(input.contains(line), "{line:?} in {input}");
    }
    assert!(!text.contains("br-*"), "no bridge passes for Docker's by its name's start: {text}");
    for line in [
        "\t\ttype filter hook forward priority filter - 5; policy accept;\n",
        "\t\tiifname { \"br-0123456789ab\", \"docker0\" } accept\n",
        "\t\tiifname \"veth*\" accept\n",
        "\t\tct status dnat jump published\n",
        "\t\tmeta iifkind \"bridge\" iifname != { \"br-lan\" } accept\n",
        "\t\tmeta oifkind \"bridge\" oifname != { \"br-lan\" } drop\n\t}\n",
        "\t\tip saddr 10.0.0.7 drop\n",
        "\t\tmeta l4proto tcp ct original proto-dst { 8080-8081 } accept\n",
        "\t\tip6 saddr fd00::1-fd00::9 meta l4proto udp ct original proto-dst { 3478 } accept\n",
        "\t\tdrop\n\t}\n}\n",
    ] {
        assert!(forward.contains(line), "{line:?} in {forward}");
    }
    let (_, published) = forward.split_once("chain published").unwrap();
    assert!(
        !published.contains("139") && !published.contains("icmp"),
        "only rules that forward: {published}"
    );
    let accepting = ruleset(Some(&config(vec![], FirewallVerdict::Accept)), Phase::Kept);
    assert!(accepting.ends_with("\t\taccept\n\t}\n}\n"), "{accepting}");
    assert!(accepting.contains("\t\tiifname { \"docker0\" } accept\n"), "{accepting}");
}

/// A NAS whose own LAN interface is a bridge named `br-lan` (the network
/// context shows it as one of the machine's): a rule denying everything on
/// it, with a denying fallback, keeps its clients out, both from the NAS
/// and from the ports Docker publishes.
#[test]
fn a_lan_bridge_named_br_dash_is_filtered() {
    let deny_lan = FirewallRule {
        interface: Some(InterfaceName::parse("br-lan").unwrap()),
        source: FirewallSource::Any,
        traffic: FirewallTraffic::All {},
        forward: true,
        verdict: FirewallVerdict::Drop,
    };
    let mut lan = config(vec![deny_lan], FirewallVerdict::Drop);
    lan.own_bridges = vec![InterfaceName::parse("br-lan").unwrap()];
    let text = ruleset(Some(&lan), Phase::Kept);
    let input = chain(&text, "input");
    let rule = input.find("iifname \"br-lan\" drop").expect("the rule");
    for (at, line) in input.match_indices(" accept").map(|(at, _)| (at, &input[..at])) {
        let line = line.rsplit('\n').next().unwrap_or_default();
        assert!(at > rule || !line.contains("br-"), "accepted before the rule: {line:?} in\n{input}");
    }
    let forward = chain(&text, "forward");
    let jump = forward.find("ct status dnat jump published").expect("the jump");
    assert!(!forward[..jump].contains("br-lan") && !forward.contains("br-*"), "{forward}");
    assert!(chain(&text, "published").contains("iifname \"br-lan\" drop"));
}

/// With Docker the NAS forwards IPv4. A host of the LAN that routes a
/// container network through the NAS reaches a container's port directly,
/// without Docker's address translation: that new connection meets a drop.
#[test]
fn new_forwarded_connections_without_dnat_meet_the_rules_or_a_drop() {
    let text = ruleset(Some(&config(vec![], FirewallVerdict::Accept)), Phase::Kept);
    let forward = chain(&text, "forward");
    let after_jump = &forward[forward.find("ct status dnat jump published").expect("the jump")..];
    assert!(
        after_jump.lines().skip(1).any(|line| line.trim() == "meta oifkind \"bridge\" drop"),
        "a new connection to a container that was not translated falls through to `policy accept`:\n{forward}"
    );
}

/// Docker makes a bridge when a project starts, and names it as a network
/// says (`com.docker.network.bridge.name`): the rules written before know
/// neither. What comes from outside meets the drop on its way to any
/// bridge, by the kernel's kind of device, but the machine's own; its
/// containers still reach each other, and the LAN still reaches the
/// machine's own bridge.
#[test]
fn a_bridge_docker_made_since_or_named_otherwise_is_kept_from_outside() {
    let mut open = config(vec![], FirewallVerdict::Accept);
    open.own_bridges = vec![InterfaceName::parse("virbr0").unwrap(), InterfaceName::parse("br-lan").unwrap()];
    let text = ruleset(Some(&open), Phase::Kept);
    let forward = chain(&text, "forward");
    let after_jump: Vec<&str> = forward[forward.find("ct status dnat jump published").expect("the jump")..]
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .collect();
    assert_eq!(
        after_jump,
        [
            "ct status dnat jump published",
            "meta iifkind \"bridge\" iifname != { \"br-lan\", \"virbr0\" } accept",
            "meta oifkind \"bridge\" oifname != { \"br-lan\", \"virbr0\" } drop",
        ],
        "{forward}"
    );
    assert!(!forward.contains("oifname { \"docker0\" }"), "no list of names known when written: {forward}");
}

/// A frame a bridge of the machine's own forwards between its ports (br0
/// in and out, as `br_netfilter` hands it over) matches nothing that drops
/// it after the jump: neither while br0 is listed as the machine's own nor
/// before it is, with any fallback.
#[test]
fn frames_within_the_machines_own_bridge_pass() {
    for (own, fallback) in
        [(true, FirewallVerdict::Drop), (false, FirewallVerdict::Drop), (true, FirewallVerdict::Accept)]
    {
        let mut lan = config(vec![], fallback);
        if own {
            lan.own_bridges = vec![InterfaceName::parse("br0").unwrap()];
        }
        let text = ruleset(Some(&lan), Phase::Kept);
        let forward = chain(&text, "forward");
        let rules: Vec<&str> = forward[forward.find("ct status dnat jump published").expect("the jump")..]
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.starts_with('#') && !line.is_empty() && *line != "}")
            .collect();
        // The first rule that matches br0 → br0 decides.
        let decides = rules.iter().find(|rule| {
            let from_other = rule.starts_with("meta iifkind \"bridge\"") && !rule.contains("\"br0\"");
            let to_other = rule.starts_with("meta oifkind \"bridge\"") && !rule.contains("\"br0\"");
            from_other || to_other
        });
        match decides {
            Some(rule) => assert!(rule.ends_with(" accept"), "own {own}: {rule} in\n{forward}"),
            None => assert!(
                forward.contains("policy accept;"),
                "own {own}: falls through to the policy\n{forward}"
            ),
        }
    }
}

/// While a change waits, what others send on connections opened before it
/// meets the rules (a browser's open HTTP/2 connection carries its
/// confirmation); answers to the NAS's own connections still pass at once.
#[test]
fn while_a_change_waits_connections_opened_before_meet_the_rules() {
    let rules = vec![rule(FirewallSource::Any, tcp(&[(443, 443)]), false, FirewallVerdict::Accept)];
    let waiting = ruleset(Some(&config(rules.clone(), FirewallVerdict::Drop)), Phase::Waiting);
    let input = chain(&waiting, "input");
    assert!(!input.contains("ct state established,related accept"), "{input}");
    let reply = input.find("\t\tct state established ct direction reply accept\n").expect("answers");
    assert!(input.find("\t\tct state related accept\n").is_some());
    let rule = input.find("tcp dport { 443 } accept").expect("the rule");
    assert!(reply < rule);
    let kept = ruleset(Some(&config(rules, FirewallVerdict::Drop)), Phase::Kept);
    for name in ["input", "forward"] {
        assert!(chain(&kept, name).contains("\t\tct state established,related accept\n"), "{kept}");
    }
    assert_eq!(chain(&waiting, "published"), chain(&kept, "published"));
    assert_eq!(ruleset(None, Phase::Waiting), ruleset(None, Phase::Kept));
}

/// The web interface behind a reverse proxy in a container, published on
/// 443: the browser's open connection to the proxy is forwarded by Docker,
/// and the proxy reaches nginx over a Docker bridge. A change whose rules
/// no longer let anyone reach 443 must not get its confirmation through
/// that connection: forwarded, it meets the rules while the change waits,
/// as the input chain's connections do.
#[test]
fn while_a_change_waits_forwarded_connections_opened_before_meet_the_rules() {
    let web_interface_only = rule(FirewallSource::Any, tcp(&[(8443, 8443)]), false, FirewallVerdict::Accept);
    let waiting = ruleset(Some(&config(vec![web_interface_only], FirewallVerdict::Drop)), Phase::Waiting);
    let forward = chain(&waiting, "forward");
    let jump = forward.find("ct status dnat jump published").expect("the jump");
    let before_the_rules: Vec<&str> = forward[..jump].lines().map(str::trim).collect();
    assert!(
        !before_the_rules.contains(&"ct state established,related accept"),
        "the browser's side of a forwarded connection passes without meeting the rules:\n{forward}"
    );
    assert!(before_the_rules.contains(&"ct state established ct direction reply accept"), "{forward}");
    assert!(before_the_rules.contains(&"ct state related accept"), "{forward}");
    // Nothing that forwards lets 443 in: the fallback decides.
    assert_eq!(chain(&waiting, "published").trim_end(), "chain published {\n\t\tdrop");
}

/// The rules in `dir`; `nft -c` fails while `fail` is there, and
/// systemctl writes what it was asked into `calls`.
fn firewall(dir: &Path) -> Arc<Firewall> {
    let executable = |path: &Path, text: String| {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    let nft = dir.join("nft");
    executable(
        &nft,
        format!(
            "#!/bin/sh\nif [ -f {} ]; then echo '/etc/zendiloos/firewall/zendiloos.nft.check:9:3-9: Error: syntax error'; exit 1; fi\n",
            dir.join("fail").display()
        ),
    );
    executable(
        &dir.join("systemd-run"),
        format!("#!/bin/sh\nwhile [ \"$1\" != {} ]; do shift; done\nexec \"$@\"\n", nft.display()),
    );
    executable(
        &dir.join("systemctl"),
        format!("#!/bin/sh\necho \"$@\" >> {}\n", dir.join("calls").display()),
    );
    let tools = Tools {
        systemctl: dir.join("systemctl"),
        systemd_run: dir.join("systemd-run"),
        nft,
        ..Tools::default()
    };
    Arc::new(Firewall::new(&tools, FirewallPaths { directory: dir.join("firewall") }).unwrap())
}

fn calls(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn in_force(dir: &Path) -> String {
    fs::read_to_string(dir.join("firewall/zendiloos.nft")).unwrap()
}

fn previous(dir: &Path) -> Option<String> {
    fs::read_to_string(dir.join("firewall/zendiloos.nft.previous")).ok()
}

const NOTHING_WAITS: Response = Response::FirewallState { pending: None };

fn waits(answer: &Response) -> bool {
    matches!(answer, Response::FirewallState { pending: Some(p) } if p.revert_at > now())
}

#[tokio::test]
async fn rules_stay_once_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    // Never on: staying off writes nothing.
    assert_eq!(firewall.apply(None, None, None).await.unwrap(), NOTHING_WAITS);
    assert!(!dir.path().join("firewall").exists() && calls(dir.path()).is_empty());

    let within = ConfirmWithin::seconds(30).unwrap();
    let closed = config(vec![], FirewallVerdict::Drop);
    assert!(waits(&firewall.apply(Some(&closed), Some(within), None).await.unwrap()));
    assert_eq!(in_force(dir.path()), ruleset(Some(&closed), Phase::Waiting));
    assert!(waits(&firewall.state().await));
    assert!(matches!(firewall.apply(Some(&closed), Some(within), None).await, Err(OperationError::Conflict)));
    let both = WaitingFirewall { config: None };
    assert!(matches!(firewall.apply(None, Some(within), Some(&both)).await, Err(OperationError::Refused(_))));
    // Brought up to date while waiting: the change's rules stay in force,
    // what returns is the rules kept as they are now.
    let open = config(vec![], FirewallVerdict::Accept);
    let mut closed_now = closed.clone();
    closed_now.bridges = vec![DockerBridge::parse("br-0123456789ab").unwrap()];
    let waiting = WaitingFirewall { config: Some(closed_now.clone()) };
    assert!(waits(&firewall.apply(Some(&open), None, Some(&waiting)).await.unwrap()));
    assert_eq!(in_force(dir.path()), ruleset(Some(&closed_now), Phase::Waiting));
    assert_eq!(previous(dir.path()).unwrap(), ruleset(Some(&open), Phase::Kept));
    // Confirmed: the change's rules, without checking connections opened
    // before them.
    assert_eq!(firewall.confirm().await.unwrap(), Response::Done);
    assert_eq!(firewall.state().await, NOTHING_WAITS);
    assert_eq!(in_force(dir.path()), ruleset(Some(&closed_now), Phase::Kept));
    assert_eq!(previous(dir.path()), None);
    assert!(!dir.path().join("firewall/zendiloos.nft.check").exists());
    assert!(firewall.confirm().await.is_err(), "nothing waits");
    assert_eq!(calls(dir.path()), ["reload-or-restart zendiloos-firewall.service"; 3]);
    // Rules for a change that no longer waits: the rules kept go in force.
    assert_eq!(firewall.apply(Some(&open), None, Some(&waiting)).await.unwrap(), NOTHING_WAITS);
    assert_eq!(in_force(dir.path()), ruleset(Some(&open), Phase::Kept));
    // Off again: the table goes.
    firewall.apply(None, None, None).await.unwrap();
    assert_eq!(in_force(dir.path()), ruleset(None, Phase::Kept));
}

/// zendiloosd keeps a change waiting in memory only. Once it restarted, it
/// cannot confirm the change any more and names none waiting: the change
/// ends there, with the rules kept in force, and its timer puts nothing
/// older back later.
#[tokio::test]
async fn a_change_nobody_can_confirm_any_more_ends_with_the_rules_kept() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    let open = config(vec![], FirewallVerdict::Accept);
    firewall.apply(Some(&open), None, None).await.unwrap();
    let closed = config(vec![], FirewallVerdict::Drop);
    let begun = firewall.apply(Some(&closed), ConfirmWithin::seconds(30).ok(), None).await.unwrap();
    assert!(waits(&begun));

    let mut open_now = open.clone();
    open_now.bridges = vec![DockerBridge::parse("br-0123456789ab").unwrap()];
    assert_eq!(firewall.apply(Some(&open_now), None, None).await.unwrap(), NOTHING_WAITS);
    assert_eq!(in_force(dir.path()), ruleset(Some(&open_now), Phase::Kept));
    assert_eq!(previous(dir.path()), None);
    assert_eq!(firewall.state().await, NOTHING_WAITS);
    // Nothing older is left to put back.
    firewall.restore().await;
    assert_eq!(in_force(dir.path()), ruleset(Some(&open_now), Phase::Kept));
    assert_eq!(calls(dir.path()).len(), 3);
}

/// The timer goes back while holding the lock: a refresh for the change
/// that reaches the agent just then finds it gone and puts the rules kept
/// in force, never the change's rules with nothing left to undo them.
#[tokio::test]
async fn rules_for_a_change_that_just_went_back_are_not_put_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    let open = config(vec![], FirewallVerdict::Accept);
    firewall.apply(Some(&open), None, None).await.unwrap();
    let closed = config(vec![], FirewallVerdict::Drop);
    let begun = firewall.apply(Some(&closed), ConfirmWithin::seconds(30).ok(), None).await.unwrap();
    assert!(waits(&begun));
    // What the timer does when the time is up, under the lock.
    {
        let mut pending = firewall.pending.lock().await;
        pending.take().unwrap().timer.abort();
        firewall.restore().await;
    }
    assert_eq!(firewall.state().await, NOTHING_WAITS);
    assert_eq!(in_force(dir.path()), ruleset(Some(&open), Phase::Kept));

    let waiting = WaitingFirewall { config: Some(closed) };
    assert_eq!(firewall.apply(Some(&open), None, Some(&waiting)).await.unwrap(), NOTHING_WAITS);
    assert_eq!(in_force(dir.path()), ruleset(Some(&open), Phase::Kept));
}

#[tokio::test]
async fn rules_nft_refuses_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    fs::write(dir.path().join("fail"), "").unwrap();
    let failed = firewall
        .apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok(), None)
        .await;
    assert_eq!(
        failed.unwrap_err().to_string(),
        "/etc/zendiloos/firewall/zendiloos.nft.check:9:3-9: Error: syntax error"
    );
    assert!(!dir.path().join("firewall/zendiloos.nft").exists());
    assert!(!dir.path().join("firewall/zendiloos.nft.previous").exists());
    assert_eq!(firewall.state().await, NOTHING_WAITS);
    assert!(calls(dir.path()).is_empty());
}

#[tokio::test]
async fn rules_not_confirmed_go_back_by_themselves_and_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    let open = config(vec![], FirewallVerdict::Accept);
    firewall.apply(Some(&open), None, None).await.unwrap();
    let before = in_force(dir.path());
    firewall
        .apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok(), None)
        .await
        .unwrap();
    // What the timer does when the time is up.
    firewall.pending.lock().await.take().unwrap().timer.abort();
    firewall.restore().await;
    assert_eq!(in_force(dir.path()), before);
    assert!(!dir.path().join("firewall/zendiloos.nft.previous").exists());
    assert_eq!(calls(dir.path()).len(), 3, "loaded: open, closed, open again");

    // A restart before the change was confirmed.
    fs::write(dir.path().join("firewall/zendiloos.nft"), "closed").unwrap();
    fs::write(dir.path().join("firewall/zendiloos.nft.previous"), "open").unwrap();
    firewall.restore_unconfirmed().await;
    assert_eq!(in_force(dir.path()), "open");
    assert!(!dir.path().join("firewall/zendiloos.nft.previous").exists());
    assert_eq!(calls(dir.path()).len(), 4);
    // Nothing waiting: nothing happens.
    firewall.restore_unconfirmed().await;
    assert_eq!(calls(dir.path()).len(), 4);
}

#[tokio::test]
async fn rules_that_cannot_be_loaded_go_back_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    firewall.apply(Some(&config(vec![], FirewallVerdict::Accept)), None, None).await.unwrap();
    let before = in_force(dir.path());
    // systemctl fails from now on.
    let systemctl = dir.path().join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\necho 'Job failed' >&2\nexit 1\n").unwrap();
    let failed = firewall
        .apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok(), None)
        .await;
    assert!(failed.is_err());
    assert_eq!(in_force(dir.path()), before);
    assert_eq!(firewall.state().await, NOTHING_WAITS);
}
