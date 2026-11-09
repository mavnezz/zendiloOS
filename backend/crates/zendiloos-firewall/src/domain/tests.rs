use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn ports(text: &str) -> Vec<PortRange> {
    parse_ports(text).unwrap()
}

fn rule(service: Service, source: &str, action: Action) -> Rule {
    Rule { enabled: true, interface: None, service, source: Source::parse(source).unwrap(), action }
}

fn on(fallback: Action, rules: Vec<Rule>) -> FirewallSettings {
    FirewallSettings { enabled: true, fallback, rules }
}

fn facts() -> Facts {
    Facts {
        ssh_on: true,
        smb_on: true,
        projects: vec![ProjectPorts {
            name: "nextcloud".into(),
            ports: vec![(Transport::Tcp, 8080), (Transport::Tcp, 8081), (Transport::Udp, 3478)],
        }],
        ..Facts::default()
    }
}

#[test]
fn words_read_back_as_they_are_written() {
    for action in [Action::Allow, Action::Deny] {
        assert_eq!(Action::parse(action.as_str()), Some(action));
    }
    for transport in [Transport::Tcp, Transport::Udp] {
        assert_eq!(Transport::parse(transport.as_str()), Some(transport));
    }
    for protocol in [Protocol::Tcp, Protocol::Udp, Protocol::Both] {
        assert_eq!(Protocol::parse(protocol.as_str()), Some(protocol));
    }
    for service in KnownService::ALL {
        assert_eq!(KnownService::parse(service.key()), Some(service));
    }
    assert_eq!((Action::parse("drop"), Transport::parse("icmp"), Protocol::parse("all")), (None, None, None));
    assert!(Protocol::Both.carries(Transport::Udp) && !Protocol::Tcp.carries(Transport::Udp));
    assert!(Protocol::Udp.carries(Transport::Udp) && !Protocol::Udp.carries(Transport::Tcp));
}

#[test]
fn ports_are_numbers_and_ranges() {
    assert_eq!(ports_text(&ports(" 80, 443  8000-8100,")), "80, 443, 8000-8100");
    for wrong in ["", " , ", "0", "65536", "http", "90-80", "1-", "-5", "1-2-3"] {
        assert!(parse_ports(wrong).is_err(), "{wrong:?}");
    }
    let many = (1..=33).map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    assert!(parse_ports(&many).is_err());
    let range = PortRange::new(10, 20).unwrap();
    assert!(range.contains(10) && range.contains(20) && !range.contains(21));
    assert_eq!((range.from(), range.to()), (10, 20));
    assert!(PortRange::new(0, 5).is_err());
    assert_eq!(merged(ports("30, 1-5, 6, 4-8, 10, 65535, 65534")), ports("1-8, 10, 30, 65534-65535"));
}

#[test]
fn sources_are_addresses_networks_and_ranges() {
    assert_eq!(Source::parse(" any ").unwrap(), Source::Any);
    assert_eq!(Source::parse("").unwrap(), Source::Any);
    assert_eq!(Source::parse("192.168.1.7").unwrap(), Source::Address(ip("192.168.1.7")));
    assert_eq!(Source::parse("::ffff:192.168.1.7").unwrap(), Source::Address(ip("192.168.1.7")));
    let lan = Source::parse("192.168.1.77/24").unwrap();
    assert_eq!(lan.to_string(), "192.168.1.0/24", "host bits dropped");
    assert_eq!(Source::parse("10.0.0.1/32").unwrap(), Source::Address(ip("10.0.0.1")));
    assert_eq!(Source::parse("0.0.0.0/0").unwrap().to_string(), "0.0.0.0/0");
    assert_eq!(Source::parse("fd00:1::5/64").unwrap().to_string(), "fd00:1::/64");
    let range = Source::parse("192.168.1.10 - 192.168.1.20").unwrap();
    assert_eq!(range.to_string(), "192.168.1.10-192.168.1.20");
    assert_eq!(Source::parse("10.0.0.1-10.0.0.1").unwrap(), Source::Address(ip("10.0.0.1")));
    for wrong in
        ["192.168.1", "10.0.0.0/33", "fd00::/129", "10.0.0.0/x", "10.0.0.9-10.0.0.1", "10.0.0.1-fd00::1"]
    {
        assert!(Source::parse(wrong).is_err(), "{wrong:?}");
    }

    assert!(lan.contains(ip("192.168.1.200")) && !lan.contains(ip("192.168.2.1")));
    assert!(lan.contains(ip("::ffff:192.168.1.5")), "seen by an IPv6 socket");
    assert!(!lan.contains(ip("fd00::1")));
    assert!(range.contains(ip("192.168.1.15")) && !range.contains(ip("192.168.1.21")));
    assert!(!range.contains(ip("fd00::1")));
    assert!(Source::Any.contains(ip("fd00::1")) && Source::Any.is_any());
    assert!(Source::Address(ip("10.0.0.1")).contains(ip("10.0.0.1")));
    assert!(Source::parse("fd00:1::/64").unwrap().contains(ip("fd00:1::abcd")));
}

#[test]
fn settings_are_checked_rule_by_rule() {
    let good = on(
        Action::Deny,
        vec![
            Rule {
                interface: Some("enp1s0".into()),
                ..rule(Service::Known(KnownService::Smb), "any", Action::Allow)
            },
            rule(Service::Project("nextcloud".into()), "192.168.1.0/24", Action::Allow),
            rule(Service::Ports { protocol: Protocol::Both, ports: ports("8000-8100") }, "any", Action::Deny),
        ],
    );
    assert_eq!(good.clone().check().unwrap(), good);
    // A network as stored before: host bits dropped.
    let loose = on(
        Action::Allow,
        vec![Rule {
            source: Source::Network { address: ip("10.1.2.3"), prefix: 8 },
            ..rule(Service::All, "any", Action::Deny)
        }],
    );
    assert_eq!(loose.check().unwrap().rules[0].source.to_string(), "10.0.0.0/8");
    let wrong = |rule: Rule| on(Action::Allow, vec![rule]).check().unwrap_err().0;
    let base = rule(Service::All, "any", Action::Allow);
    assert!(wrong(Rule { interface: Some("lo".into()), ..base.clone() }).starts_with("rule 1: "));
    assert!(
        wrong(Rule { interface: Some("a very long interface".into()), ..base.clone() }).contains("interface")
    );
    assert!(
        wrong(Rule { service: Service::Project("Next Cloud".into()), ..base.clone() }).contains("project")
    );
    assert!(
        wrong(Rule { service: Service::Ports { protocol: Protocol::Tcp, ports: vec![] }, ..base.clone() })
            .contains("port")
    );
    let many = vec![PortRange::single(1).unwrap(); MAX_PORTS + 1];
    assert!(
        wrong(Rule { service: Service::Ports { protocol: Protocol::Tcp, ports: many }, ..base.clone() })
            .contains("at most")
    );
    assert!(
        wrong(Rule { source: Source::Network { address: ip("10.0.0.0"), prefix: 40 }, ..base.clone() })
            .contains("prefix")
    );
    assert!(
        wrong(Rule { source: Source::Range { from: ip("10.0.0.9"), to: ip("10.0.0.1") }, ..base.clone() })
            .contains("backwards")
    );
    let too_many = on(Action::Allow, vec![base; MAX_RULES + 1]);
    assert!(too_many.check().is_err());
    assert_eq!(FirewallSettings::default().fallback, Action::Allow);
    assert!(!FirewallSettings::default().enabled);
}

#[test]
fn names_are_checked() {
    for good in ["nextcloud", "my-app_2", "0ad"] {
        assert!(is_project_name(good), "{good}");
    }
    for bad in ["", "-x", "_x", "Nextcloud", "a b", &"a".repeat(64)] {
        assert!(!is_project_name(bad), "{bad:?}");
    }
    for good in ["enp1s0", "bond0.10", "br_lan", "wlan-0"] {
        assert!(is_interface_name(good), "{good}");
    }
    for bad in ["", "lo", ".x", "-x", "eth 0", "eth0\"", "abcdefghijklmnop"] {
        assert!(!is_interface_name(bad), "{bad:?}");
    }
}

#[test]
fn services_stand_for_the_ports_in_use() {
    let facts = Facts { web_ports: vec![8080, 8443], ssh_port: 2222, ..facts() };
    let reach = |service: Service| service.reach(&facts);
    assert_eq!(reach(Service::Known(KnownService::WebInterface)).tcp, ports("8080, 8443"));
    assert_eq!(reach(Service::Known(KnownService::Ssh)).tcp, ports("2222"));
    let smb = reach(Service::Known(KnownService::Smb));
    assert_eq!((smb.tcp, smb.udp), (ports("139, 445"), ports("137-138")));
    let nfs = reach(Service::Known(KnownService::Nfs));
    assert_eq!(nfs.tcp, nfs.udp);
    assert!(nfs.tcp.iter().any(|r| r.contains(2049)) && nfs.tcp.iter().any(|r| r.contains(20048)));
    // lockd's port is not one Linux hands out for outgoing connections
    // (`ip_local_port_range`, 32768 to 60999 by default).
    assert!(nfs.tcp.iter().any(|r| r.contains(32767)));
    assert!(NFS_PORTS.iter().all(|port| !(32768..=60999).contains(port)), "{NFS_PORTS:?}");
    let discovery = reach(Service::Known(KnownService::Discovery));
    assert!(discovery.has(Transport::Udp, 5353) && discovery.has(Transport::Tcp, 5357));
    assert!(!discovery.has(Transport::Tcp, 5353));
    assert!(reach(Service::Known(KnownService::Ping)).ping);
    let project = reach(Service::Project("nextcloud".into()));
    assert_eq!((project.tcp, project.udp), (ports("8080-8081"), ports("3478")));
    assert_eq!(reach(Service::Project("gone".into())), Reach::default());
    let custom = reach(Service::Ports { protocol: Protocol::Udp, ports: ports("53, 50-60") });
    assert_eq!((custom.tcp, custom.udp), (vec![], ports("50-60")));
    assert!(reach(Service::All).has(Transport::Udp, 1));
    assert!(Service::All.forwards() && Service::Project("x".into()).forwards());
    assert!(!Service::Known(KnownService::Smb).forwards());
}

#[test]
fn rules_compile_to_what_the_host_matches() {
    assert_eq!(compile(&FirewallSettings::default(), &facts()), None, "off");
    let settings = on(
        Action::Deny,
        vec![
            rule(Service::Known(KnownService::Smb), "192.168.1.0/24", Action::Allow),
            Rule { enabled: false, ..rule(Service::All, "any", Action::Allow) },
            rule(Service::Project("nextcloud".into()), "any", Action::Allow),
            rule(Service::Project("stopped".into()), "any", Action::Allow),
            Rule { interface: Some("eth1".into()), ..rule(Service::All, "10.0.0.1", Action::Deny) },
            rule(Service::Known(KnownService::Ping), "any", Action::Allow),
        ],
    );
    let ruleset = compile(&settings, &facts()).unwrap();
    assert_eq!(ruleset.fallback, Action::Deny);
    let lan = Source::parse("192.168.1.0/24").unwrap();
    let compiled = |interface: Option<&str>,
                    source: Source,
                    traffic: Traffic,
                    forward: bool,
                    action: Action| {
        Compiled { interface: interface.map(str::to_owned), source, traffic, forward, action }
    };
    assert_eq!(
        ruleset.rules,
        [
            compiled(None, lan, Traffic::Tcp(ports("139, 445")), false, Action::Allow),
            compiled(None, lan, Traffic::Udp(ports("137-138")), false, Action::Allow),
            compiled(None, Source::Any, Traffic::Tcp(ports("8080-8081")), true, Action::Allow),
            compiled(None, Source::Any, Traffic::Udp(ports("3478")), true, Action::Allow),
            compiled(Some("eth1"), Source::Address(ip("10.0.0.1")), Traffic::All, true, Action::Deny),
            compiled(None, Source::Any, Traffic::Ping, false, Action::Allow),
        ]
    );
    assert!(ruleset.bridges.is_empty());
    // Docker's bridges and the machine's own go along, in order, once each.
    let bridges = Facts {
        bridges: vec!["br-b".into(), "br-a".into(), "br-b".into()],
        own_bridges: vec!["br-lan".into(), "br-dmz".into()],
        ..facts()
    };
    let with_bridges = compile(&settings, &bridges).unwrap();
    assert_eq!(with_bridges.bridges, ["br-a", "br-b"]);
    assert_eq!(with_bridges.own_bridges, ["br-dmz", "br-lan"]);
}

#[test]
fn the_guard_sees_whether_the_caller_still_gets_in() {
    let facts = facts();
    let caller =
        Caller { address: Some(ip("192.168.1.20")), interface: Some("enp1s0".into()), port: Some(443) };
    let web = || Service::Known(KnownService::WebInterface);
    // Off, unknown callers and the machine itself always get in.
    assert!(admits(&FirewallSettings::default(), &facts, &caller));
    let closed = on(Action::Deny, vec![]);
    assert!(!admits(&closed, &facts, &caller));
    assert!(admits(&closed, &facts, &Caller::default()));
    assert!(admits(&closed, &facts, &Caller { address: Some(ip("::1")), ..caller.clone() }));
    // The first rule that matches decides.
    let lan_only = on(Action::Deny, vec![rule(web(), "192.168.1.0/24", Action::Allow)]);
    assert!(admits(&lan_only, &facts, &caller));
    assert!(!admits(&lan_only, &facts, &Caller { address: Some(ip("10.0.0.5")), ..caller.clone() }));
    let shut_out = on(
        Action::Allow,
        vec![rule(Service::All, "192.168.1.20", Action::Deny), rule(web(), "any", Action::Allow)],
    );
    assert!(!admits(&shut_out, &facts, &caller));
    // A rule for other ports does not match; the port taken is the caller's.
    let other = on(
        Action::Deny,
        vec![rule(Service::Ports { protocol: Protocol::Tcp, ports: ports("8443") }, "any", Action::Allow)],
    );
    assert!(!admits(&other, &facts, &caller));
    assert!(admits(&other, &facts, &Caller { port: Some(8443), ..caller.clone() }));
    assert!(!admits(&other, &facts, &Caller { port: None, ..caller.clone() }), "443 by default");
    // A rule for another interface does not match; for an unknown one
    // only a denying rule counts.
    let elsewhere =
        on(Action::Deny, vec![Rule { interface: Some("eth1".into()), ..rule(web(), "any", Action::Allow) }]);
    assert!(!admits(&elsewhere, &facts, &caller));
    let here = on(
        Action::Deny,
        vec![Rule { interface: Some("enp1s0".into()), ..rule(web(), "any", Action::Allow) }],
    );
    assert!(admits(&here, &facts, &caller));
    assert!(!admits(&here, &facts, &Caller { interface: None, ..caller.clone() }));
    let deny_there =
        on(Action::Allow, vec![Rule { interface: Some("eth1".into()), ..rule(web(), "any", Action::Deny) }]);
    assert!(admits(&deny_there, &facts, &caller));
    assert!(!admits(&deny_there, &facts, &Caller { interface: None, ..caller }));
}

#[test]
fn blocked_services_are_those_nobody_reaches() {
    let facts = facts();
    let keys =
        |settings: &FirewallSettings| blocked(settings, &facts).iter().map(Exposed::key).collect::<Vec<_>>();
    assert!(keys(&FirewallSettings::default()).is_empty(), "off");
    assert!(keys(&on(Action::Allow, vec![])).is_empty());
    assert_eq!(keys(&on(Action::Deny, vec![])), ["web_interface", "smb", "ssh", "project:nextcloud"]);
    // Allowed to someone: not blocked; denied to some: still reachable.
    let some = on(
        Action::Deny,
        vec![
            rule(Service::Known(KnownService::WebInterface), "192.168.1.0/24", Action::Allow),
            rule(Service::Known(KnownService::Smb), "10.0.0.1", Action::Deny),
            rule(Service::Known(KnownService::Smb), "any", Action::Allow),
            rule(Service::Ports { protocol: Protocol::Tcp, ports: ports("8081") }, "any", Action::Allow),
            rule(Service::Known(KnownService::Ssh), "any", Action::Allow),
        ],
    );
    assert!(keys(&some).is_empty(), "{:?}", keys(&some));
    // Denied to anyone first: blocked, even with the fallback allowing.
    let denied = on(
        Action::Allow,
        vec![
            rule(Service::Known(KnownService::Ssh), "any", Action::Deny),
            rule(Service::Known(KnownService::Ssh), "any", Action::Allow),
            rule(Service::Project("nextcloud".into()), "any", Action::Deny),
            Rule {
                interface: Some("eth1".into()),
                ..rule(Service::Known(KnownService::Smb), "any", Action::Deny)
            },
            rule(Service::Ports { protocol: Protocol::Tcp, ports: ports("445") }, "any", Action::Deny),
        ],
    );
    assert_eq!(keys(&denied), ["ssh", "project:nextcloud"], "445 alone leaves 139 for SMB");
    // The NAS's own services do not reach containers.
    let own = on(Action::Deny, vec![rule(Service::Known(KnownService::Ping), "any", Action::Allow)]);
    assert!(keys(&own).contains(&"project:nextcloud".to_owned()));
    let everything =
        on(Action::Deny, vec![Rule { enabled: false, ..rule(Service::All, "any", Action::Allow) }]);
    assert_eq!(keys(&everything).len(), 4, "a rule switched off does nothing");

    let all = Facts { nfs_on: true, discovery_on: true, ..facts.clone() };
    assert_eq!(exposed(&all).len(), 6);
    for service in exposed(&all) {
        assert_eq!(Exposed::parse(&service.key()), Some(service));
    }
    assert_eq!(Exposed::parse("ping"), None);
    assert_eq!(Exposed::parse("project:Bad Name"), None);
    assert_eq!(Exposed::parse("nothing"), None);
}

#[test]
fn local_networks_come_from_the_interfaces() {
    let interfaces = vec![
        LocalInterface {
            name: "enp1s0".into(),
            addresses: vec![(ip("192.168.1.20"), 24), (ip("fe80::1"), 64), (ip("fd00:1::20"), 64)],
        },
        LocalInterface {
            name: "enp2s0".into(),
            addresses: vec![(ip("192.168.1.21"), 24), (ip("169.254.3.3"), 16)],
        },
        LocalInterface { name: "lo".into(), addresses: vec![(ip("127.0.0.1"), 8)] },
    ];
    let networks: Vec<String> = local_networks(&interfaces).iter().map(ToString::to_string).collect();
    assert_eq!(networks, ["192.168.1.0/24", "fd00:1::/64"]);
    assert_eq!(interface_of(&interfaces, ip("192.168.1.21")).as_deref(), Some("enp2s0"));
    assert_eq!(interface_of(&interfaces, ip("::ffff:192.168.1.20")).as_deref(), Some("enp1s0"));
    assert_eq!(interface_of(&interfaces, ip("10.0.0.1")), None);
}
