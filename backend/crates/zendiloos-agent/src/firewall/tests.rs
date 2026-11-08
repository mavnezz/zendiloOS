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
    FirewallConfig { rules: FirewallRules::new(rules).unwrap(), fallback }
}

#[test]
fn the_file_says_exactly_the_rules() {
    let off = ruleset(None);
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
    let text = ruleset(Some(&config(rules, FirewallVerdict::Drop)));
    assert!(text.starts_with(HEADER));
    let (input, forward) = text.split_once("chain forward").unwrap();
    for line in [
        "\t\ttype filter hook input priority filter - 5; policy accept;\n",
        "\t\tct state established,related accept\n",
        "\t\tiif \"lo\" accept\n",
        "\t\tiifname \"br-*\" accept\n",
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
    for line in [
        "\t\ttype filter hook forward priority filter - 5; policy accept;\n",
        "\t\tiifname \"veth*\" accept\n",
        "\t\tct status dnat jump published\n",
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
    let accepting = ruleset(Some(&config(vec![], FirewallVerdict::Accept)));
    assert!(accepting.ends_with("\t\taccept\n\t}\n}\n"), "{accepting}");
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

#[tokio::test]
async fn rules_stay_once_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    // Never on: staying off writes nothing.
    assert_eq!(firewall.apply(None, None).await.unwrap(), Response::Done);
    assert!(!dir.path().join("firewall").exists() && calls(dir.path()).is_empty());

    let within = ConfirmWithin::seconds(30).unwrap();
    let closed = config(vec![], FirewallVerdict::Drop);
    assert_eq!(firewall.apply(Some(&closed), Some(within)).await.unwrap(), Response::Done);
    assert!(in_force(dir.path()).contains("\t\tdrop\n"));
    let Response::FirewallState { pending: Some(pending) } = firewall.state().await else { panic!() };
    assert!(pending.revert_at > now());
    assert!(matches!(firewall.apply(Some(&closed), Some(within)).await, Err(OperationError::Conflict)));
    // Brought up to date while waiting: what returns stays what was before.
    let open = config(vec![], FirewallVerdict::Accept);
    firewall.apply(Some(&open), None).await.unwrap();
    assert!(in_force(dir.path()).ends_with("\t\taccept\n\t}\n}\n"));
    assert_eq!(
        fs::read_to_string(dir.path().join("firewall/zendiloos.nft.previous")).unwrap(),
        ruleset(None)
    );
    assert_eq!(firewall.confirm().await.unwrap(), Response::Done);
    assert_eq!(firewall.state().await, Response::FirewallState { pending: None });
    assert!(!dir.path().join("firewall/zendiloos.nft.previous").exists());
    assert!(!dir.path().join("firewall/zendiloos.nft.check").exists());
    assert!(firewall.confirm().await.is_err(), "nothing waits");
    assert_eq!(
        calls(dir.path()),
        ["reload-or-restart zendiloos-firewall.service", "reload-or-restart zendiloos-firewall.service"]
    );
    // Off again: the table goes.
    firewall.apply(None, None).await.unwrap();
    assert_eq!(in_force(dir.path()), ruleset(None));
}

#[tokio::test]
async fn rules_nft_refuses_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    fs::write(dir.path().join("fail"), "").unwrap();
    let failed =
        firewall.apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok()).await;
    assert_eq!(
        failed.unwrap_err().to_string(),
        "/etc/zendiloos/firewall/zendiloos.nft.check:9:3-9: Error: syntax error"
    );
    assert!(!dir.path().join("firewall/zendiloos.nft").exists());
    assert!(!dir.path().join("firewall/zendiloos.nft.previous").exists());
    assert_eq!(firewall.state().await, Response::FirewallState { pending: None });
    assert!(calls(dir.path()).is_empty());
}

#[tokio::test]
async fn rules_not_confirmed_go_back_by_themselves_and_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let firewall = firewall(dir.path());
    let open = config(vec![], FirewallVerdict::Accept);
    firewall.apply(Some(&open), None).await.unwrap();
    let before = in_force(dir.path());
    firewall
        .apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok())
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
    firewall.apply(Some(&config(vec![], FirewallVerdict::Accept)), None).await.unwrap();
    let before = in_force(dir.path());
    // systemctl fails from now on.
    let systemctl = dir.path().join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\necho 'Job failed' >&2\nexit 1\n").unwrap();
    let failed =
        firewall.apply(Some(&config(vec![], FirewallVerdict::Drop)), ConfirmWithin::seconds(30).ok()).await;
    assert!(failed.is_err());
    assert_eq!(in_force(dir.path()), before);
    assert_eq!(firewall.state().await, Response::FirewallState { pending: None });
}
