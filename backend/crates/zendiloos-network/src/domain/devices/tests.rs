use super::*;
use crate::domain::{Address, DefaultRoute, Interface, LinkState};

fn card(name: &str, mac: &str, addresses: Vec<Address>) -> Interface {
    Interface {
        name: name.into(),
        kind: InterfaceKind::Ethernet,
        state: LinkState::Up,
        mac: Some(mac.into()),
        mtu: 1500,
        member_of: None,
        parent: None,
        addresses,
    }
}

fn address(ip: &str, prefix: u8, dynamic: bool) -> Address {
    Address { ip: ip.parse().unwrap(), prefix, scope: AddressScope::Global, dynamic }
}

/// enp1s0 by DHCP with the default route, enp2s0 without an address,
/// enp3s0 with a static one, and a wireless card.
fn network() -> Network {
    let lan = card(
        "enp1s0",
        "52:54:00:12:34:56",
        vec![address("192.168.1.20", 24, true), address("fe80::1", 64, false)],
    );
    let spare = card("enp2s0", "52:54:00:00:00:02", vec![]);
    let fixed = card("enp3s0", "52:54:00:00:00:03", vec![address("10.0.0.5", 24, false)]);
    let wireless = Interface { kind: InterfaceKind::Wireless, ..card("wlp1s0", "52:54:00:00:00:04", vec![]) };
    let route =
        DefaultRoute { gateway: "192.168.1.1".parse().unwrap(), interface: "enp1s0".into(), metric: 100 };
    Network::new(vec![lan, spare, fixed, wireless], vec![route], vec!["192.168.1.1".parse().unwrap()])
}

fn file(name: &str, text: &str) -> NetworkdFile {
    NetworkdFile { name: name.into(), text: text.into() }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn dhcp(interface: &str, keep: Option<&str>) -> InterfaceSettings {
    InterfaceSettings {
        interface: interface.into(),
        ipv4: Ipv4Setting::Dhcp { keep: keep.map(|ip| ip.parse().unwrap()) },
        ipv6: Ipv6Setting::Automatic,
        dns: vec![],
    }
}

fn bridge(name: &str, members: &[&str]) -> NewDevice {
    NewDevice {
        name: name.into(),
        kind: DeviceKind::Bridge { stp: false },
        members: names(members),
        settings: None,
    }
}

fn bond(name: &str, members: &[&str]) -> NewDevice {
    NewDevice {
        name: name.into(),
        kind: DeviceKind::Bond { mode: BondMode::ActiveBackup },
        members: names(members),
        settings: None,
    }
}

fn vlan(name: &str, parent: &str, id: u16) -> NewDevice {
    NewDevice {
        name: name.into(),
        kind: DeviceKind::Vlan { parent: parent.into(), id },
        members: vec![],
        settings: None,
    }
}

/// The files the agent writes for `edit`, as zendiloosd reads them back.
fn written(before: &Configuration, edit: &Edit) -> Configuration {
    let mut configuration = before.clone();
    configuration
        .devices
        .retain(|d| !edit.removed.contains(&d.name) && !edit.devices.iter().any(|n| n.name == d.name));
    configuration.devices.extend(edit.devices.iter().cloned());
    configuration.links.retain(|l| {
        !edit.removed.contains(&l.settings.interface)
            && !edit.links.iter().any(|n| n.settings.interface == l.settings.interface)
    });
    configuration.links.extend(edit.links.iter().cloned());
    configuration
}

fn message(result: Result<Edit, InvalidDevice>) -> String {
    result.unwrap_err().0
}

#[test]
fn zendiloos_files_are_read_back() {
    let files = [
        file(
            "05-zendiloos-br0.netdev",
            "# Written by zendiloOS\n# Members: enp2s0 bond0 gone\n[NetDev]\nName=br0\nKind=bridge\nMACAddress=52:54:00:00:00:02\n\n[Bridge]\nSTP=yes\n",
        ),
        file(
            "05-zendiloos-bond0.netdev",
            "[NetDev]\nName=bond0\nKind=bond\n\n[Bond]\nMode=802.3ad\nMIIMonitorSec=100ms\n",
        ),
        file("05-zendiloos-enp1s0.10.netdev", "[NetDev]\nName=enp1s0.10\nKind=vlan\n\n[VLAN]\nId=10\n"),
        file("05-zendiloos-odd.netdev", "[NetDev]\nName=other\nKind=bridge\n"),
        file("05-zendiloos-strange.netdev", "[NetDev]\nName=strange\nKind=wireguard\n"),
        file(
            "05-zendiloos-br0.network",
            "[Match]\nName=br0\n\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\nDNS=9.9.9.9\n\n[DHCPv4]\nClientIdentifier=mac\nRequestAddress=192.168.1.20\n",
        ),
        file("05-zendiloos-enp2s0.network", "[Match]\nName=enp2s0\n\n[Network]\nBridge=br0\nDHCP=no\n"),
        file("05-zendiloos-enp3s0.network", "[Match]\nName=enp3s0\n\n[Network]\nBridge=br0\nDHCP=no\n"),
        file("05-zendiloos-bond0.network", "[Match]\nName=bond0\n[Network]\nBridge=br0\n"),
        file(
            "05-zendiloos-enp1s0.network",
            "[Match]\nName=enp1s0\n\n[Network]\nDHCP=no\nIPv6AcceptRA=no\nAddress=10.0.0.5/24\nGateway=10.0.0.1\nVLAN=enp1s0.10\n",
        ),
        file(
            "05-zendiloos-enp1s0.10.network",
            "[Match]\nName=enp1s0.10\n[Link]\nRequiredForOnline=no\n[Network]\nDHCP=no\n",
        ),
        file(
            "05-zendiloos-enp4s0.network",
            "[Match]\nName=enp4s0\n[Link]\nActivationPolicy=down\n[Network]\nDHCP=no\n",
        ),
        file("10-netplan-enp9s0.network", "[Match]\nName=enp9s0\n[Network]\nDHCP=ipv4\n"),
    ];
    let configuration = Configuration::read(&files);
    let devices: Vec<&str> = configuration.devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(devices, ["bond0", "br0", "enp1s0.10"], "files not of zendiloOS's form are passed by");
    let br0 = configuration.device("br0").unwrap();
    assert_eq!(br0.kind, DeviceKind::Bridge { stp: true });
    assert_eq!(br0.members, ["enp2s0", "bond0", "enp3s0"], "in the noted order, then the others");
    assert_eq!(br0.mac.as_deref(), Some("52:54:00:00:00:02"));
    assert_eq!(configuration.device("bond0").unwrap().kind, DeviceKind::Bond { mode: BondMode::Lacp });
    assert_eq!(
        configuration.device("enp1s0.10").unwrap().kind,
        DeviceKind::Vlan { parent: "enp1s0".into(), id: 10 }
    );

    let link = |name: &str| configuration.link(name).unwrap();
    assert_eq!(
        link("br0").settings,
        InterfaceSettings { dns: vec!["9.9.9.9".parse().unwrap()], ..dhcp("br0", Some("192.168.1.20")) }
    );
    assert!(link("br0").keep_up && !link("enp1s0").keep_up);
    assert_eq!(link("enp2s0").port_of.as_deref(), Some("br0"));
    assert_eq!(
        link("enp1s0").settings.ipv4,
        Ipv4Setting::Static {
            address: "10.0.0.5".parse().unwrap(),
            prefix: 24,
            gateway: Some("10.0.0.1".parse().unwrap())
        }
    );
    assert_eq!(link("enp1s0").vlans, ["enp1s0.10"]);
    assert!(link("enp4s0").settings.switched_off() && !link("enp4s0").keep_up);
    assert!(configuration.link("enp9s0").is_none());
}

#[test]
fn a_bridge_over_the_card_takes_its_settings_and_its_mac() {
    let network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };

    let edit = planning.make(bridge("br0", &["enp1s0"])).unwrap();

    assert_eq!(edit.subject, Subject::Device("br0".into(), DeviceAction::Made));
    assert_eq!(
        edit.devices,
        [Device {
            name: "br0".into(),
            kind: DeviceKind::Bridge { stp: false },
            members: names(&["enp1s0"]),
            mac: Some("52:54:00:12:34:56".into()),
        }]
    );
    // DHCP asks for the address the card has; IPv6 as it had.
    assert_eq!(edit.links[0].settings, dhcp("br0", Some("192.168.1.20")));
    assert!(edit.links[0].keep_up);
    assert_eq!(edit.links[1].port_of.as_deref(), Some("br0"));
    assert!(edit.links[1].settings.switched_off());
    assert_eq!(
        edit.effects,
        [
            Effect { interface: "br0".into(), becomes: Becomes::Settings(dhcp("br0", Some("192.168.1.20"))) },
            Effect {
                interface: "enp1s0".into(),
                becomes: Becomes::Member { device: "br0".into(), moved: true, dropped: false }
            },
        ]
    );
}

/// The first member given that has settings gives them, and comes first;
/// the others' go.
#[test]
fn the_member_with_settings_comes_first() {
    let network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };

    let edit = planning.make(bridge("br0", &["enp2s0", "enp3s0", "enp1s0"])).unwrap();

    assert_eq!(edit.devices[0].members, ["enp3s0", "enp2s0", "enp1s0"]);
    assert_eq!(edit.devices[0].mac.as_deref(), Some("52:54:00:00:00:03"));
    // A static address keeps the name servers in use.
    assert_eq!(
        edit.links[0].settings,
        InterfaceSettings {
            interface: "br0".into(),
            ipv4: Ipv4Setting::Static { address: "10.0.0.5".parse().unwrap(), prefix: 24, gateway: None },
            ipv6: Ipv6Setting::Off,
            dns: vec!["192.168.1.1".parse().unwrap()],
        }
    );
    let fates: Vec<(&str, &Becomes)> =
        edit.effects[1..].iter().map(|e| (e.interface.as_str(), &e.becomes)).collect();
    let member = |moved, dropped| Becomes::Member { device: "br0".into(), moved, dropped };
    assert_eq!(
        fates,
        [
            ("enp3s0", &member(true, false)),
            ("enp2s0", &member(false, false)),
            ("enp1s0", &member(false, true))
        ]
    );
}

/// A bridge for VMs only has nobody's settings and no address, but stays up.
#[test]
fn a_bridge_without_members_is_up_without_an_address() {
    let network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let edit = planning.make(bridge("vms", &[])).unwrap();
    assert_eq!((edit.devices[0].mac.clone(), edit.links.len()), (None, 1));
    assert!(edit.links[0].settings.switched_off() && edit.links[0].keep_up);
    // A card without settings gives none either.
    let edit = planning.make(bridge("br0", &["enp2s0"])).unwrap();
    assert!(edit.links[0].settings.switched_off());
    assert_eq!(edit.devices[0].mac.as_deref(), Some("52:54:00:00:00:02"));
}

#[test]
fn names_are_interface_names_and_free() {
    let network = network();
    let configuration =
        Configuration::read(&[file("05-zendiloos-br0.netdev", "[NetDev]\nName=br0\nKind=bridge\n")]);
    let planning = Planning { network: &network, configuration: &configuration };
    for (name, why) in [
        ("", "no interface name"),
        ("bridge-for-vms-1", "no interface name"),
        ("br 0", "no interface name"),
        ("-br", "no interface name"),
        ("lo", "no interface name"),
        ("docker0", "Docker uses"),
        ("br-0123456789ab", "Docker uses"),
        ("veth1", "Docker uses"),
        ("enp1s0", "there is an interface enp1s0 already"),
        ("br0", "there is an interface br0 already"),
    ] {
        let refused = message(planning.make(bridge(name, &[])));
        assert!(refused.contains(why), "{name:?}: {refused}");
    }
    assert!(planning.make(bridge("br-lan", &[])).is_ok());
    assert!(planning.make(bridge("vmbr.15_chars", &[])).is_ok(), "15 characters");
}

#[test]
fn members_are_free_wired_cards_or_devices_made_here() {
    let mut network = network();
    network
        .interfaces
        .push(Interface { member_of: Some("br9".into()), ..card("enp4s0", "52:54:00:00:00:05", vec![]) });
    network.interfaces.push(Interface {
        parent: Some("enp5s0".into()),
        kind: InterfaceKind::Vlan,
        ..card("enp5s0.7", "52:54:00:00:00:06", vec![])
    });
    network.interfaces.push(card("enp5s0", "52:54:00:00:00:06", vec![]));
    network
        .interfaces
        .push(Interface { kind: InterfaceKind::Bond, ..card("bond9", "52:54:00:00:00:07", vec![]) });
    let configuration = Configuration::read(&[
        file("05-zendiloos-bond0.netdev", "[NetDev]\nName=bond0\nKind=bond\n[Bond]\nMode=active-backup\n"),
        file("05-zendiloos-enp6s0.network", "[Match]\nName=enp6s0\n[Network]\nDHCP=ipv4\nVLAN=enp6s0.3\n"),
    ]);
    network
        .interfaces
        .push(Interface { kind: InterfaceKind::Bond, ..card("bond0", "52:54:00:00:00:08", vec![]) });
    network.interfaces.push(card("enp6s0", "52:54:00:00:00:09", vec![]));
    let planning = Planning { network: &network, configuration: &configuration };
    for (members, why) in [
        (&["enp9s0"][..], "there is no interface enp9s0"),
        (&["enp2s0", "enp2s0"], "enp2s0 is named twice"),
        (&["wlp1s0"], "wlp1s0 cannot be in a bridge"),
        (&["bond9"], "bond9 cannot be in a bridge"),
        (&["enp4s0"], "enp4s0 is a member of br9 already"),
        (&["enp5s0"], "enp5s0 carries the VLAN enp5s0.7"),
        (&["enp6s0"], "enp6s0 carries the VLAN enp6s0.3"),
        (&["br7"], "there is no interface br7"),
    ] {
        let refused = message(planning.make(bridge("br1", members)));
        assert!(refused.contains(why), "{members:?}: {refused}");
    }
    let many: Vec<String> = (0..=MAX_MEMBERS).map(|i| format!("eth{i}")).collect();
    let refused = message(planning.make(NewDevice { members: many, ..bridge("br1", &[]) }));
    assert!(refused.contains("at most 8 members"), "{refused}");

    // A bond made here can be in a bridge; a bond is made of cards only.
    let edit = planning.make(bridge("br1", &["bond0", "enp2s0"])).unwrap();
    assert_eq!(edit.links[1].port_of.as_deref(), Some("br1"));
    assert_eq!(edit.devices[0].mac.as_deref(), Some("52:54:00:00:00:08"), "the bond's address");
    assert!(message(planning.make(bond("bond1", &["bond0"]))).contains("bond0 is no wired card"));
    assert!(message(planning.make(bond("bond1", &[]))).contains("at least one member"));
    let given = NewDevice { settings: Some(dhcp("br1", None)), ..bridge("br1", &[]) };
    assert!(message(planning.make(given)).contains("takes the settings of its first member"));
}

#[test]
fn a_bond_takes_its_first_members_mac_and_settings() {
    let network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let edit = planning.make(bond("bond0", &["enp2s0", "enp1s0"])).unwrap();
    assert_eq!(edit.devices[0].members, ["enp1s0", "enp2s0"]);
    assert_eq!(edit.devices[0].mac.as_deref(), Some("52:54:00:12:34:56"));
    assert_eq!(edit.devices[0].kind, DeviceKind::Bond { mode: BondMode::ActiveBackup });
    assert_eq!(edit.links[0].settings, dhcp("bond0", Some("192.168.1.20")));
    assert!(edit.links[1..].iter().all(|link| link.port_of.as_deref() == Some("bond0")));
    assert_eq!(BondMode::parse("802.3ad"), Some(BondMode::Lacp));
    assert_eq!(BondMode::BalanceAlb.as_str(), "balance-alb");
    assert_eq!(BondMode::parse("broadcast"), None);
}

#[test]
fn a_vlan_has_its_own_settings_and_its_parent_carries_it() {
    let network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };

    let edit = planning.make(vlan("enp1s0.10", "enp1s0", 10)).unwrap();
    assert_eq!(edit.devices[0].kind, DeviceKind::Vlan { parent: "enp1s0".into(), id: 10 });
    // The parent keeps what it has, now carrying the VLAN.
    assert_eq!(edit.links[0].settings, dhcp("enp1s0", Some("192.168.1.20")));
    assert_eq!(edit.links[0].vlans, ["enp1s0.10"]);
    // Off by default, up to carry others.
    assert!(edit.links[1].settings.switched_off() && edit.links[1].keep_up);
    assert_eq!(edit.effects[1].becomes, Becomes::Parent { vlan: "enp1s0.10".into(), carries: true });

    let own = NewDevice { settings: Some(dhcp("ignored", None)), ..vlan("lab", "enp2s0", 20) };
    assert_eq!(planning.make(own).unwrap().links[1].settings, dhcp("lab", None));
    let bad = InterfaceSettings {
        ipv4: Ipv4Setting::Static { address: "10.1.1.0".parse().unwrap(), prefix: 24, gateway: None },
        ..dhcp("lab", None)
    };
    assert!(
        message(planning.make(NewDevice { settings: Some(bad), ..vlan("lab", "enp2s0", 20) }))
            .contains("10.1.1.0")
    );
    for (id, parent, why) in [
        (0, "enp1s0", "from 1 to 4094"),
        (4095, "enp1s0", "from 1 to 4094"),
        (5, "enp9s0", "there is no interface enp9s0"),
        (5, "wlp1s0", "not on wlp1s0"),
    ] {
        let refused = message(planning.make(vlan("v", parent, id)));
        assert!(refused.contains(why), "{id} on {parent}: {refused}");
    }
    let with_members = NewDevice { members: names(&["enp2s0"]), ..vlan("v", "enp1s0", 5) };
    assert!(message(planning.make(with_members)).contains("no members"));

    // Once there: the same id twice on one parent, a VLAN on a member.
    let after = written(&configuration, &edit);
    let mut network = network.clone();
    network.interfaces.push(Interface {
        kind: InterfaceKind::Vlan,
        parent: Some("enp1s0".into()),
        ..card("enp1s0.10", "52:54:00:12:34:56", vec![])
    });
    let planning = Planning { network: &network, configuration: &after };
    assert!(message(planning.make(vlan("again", "enp1s0", 10))).contains("carries the VLAN 10 already"));
    let bridged = written(&after, &planning.make(bridge("br0", &["enp2s0"])).unwrap());
    let planning = Planning { network: &network, configuration: &bridged };
    assert!(
        message(planning.make(vlan("v", "enp2s0", 5)))
            .contains("enp2s0 is a member of br0: put the VLAN on br0")
    );
    // Parents of VLANs keep them when their settings change.
    let settings = planning.settings(dhcp("enp1s0", None)).unwrap();
    assert_eq!(settings.links[0].vlans, ["enp1s0.10"]);
}

/// A VLAN on a bridge that is a member of that bridge would carry itself.
#[test]
fn no_device_ends_up_on_itself() {
    let mut network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let made = written(&configuration, &planning.make(bridge("br0", &["enp2s0"])).unwrap());
    network
        .interfaces
        .push(Interface { kind: InterfaceKind::Bridge, ..card("br0", "52:54:00:00:00:02", vec![]) });
    let planning = Planning { network: &network, configuration: &made };
    let tagged = written(&made, &planning.make(vlan("br0.5", "br0", 5)).unwrap());
    network.interfaces.push(Interface {
        kind: InterfaceKind::Vlan,
        parent: Some("br0".into()),
        ..card("br0.5", "52:54:00:00:00:02", vec![])
    });
    let planning = Planning { network: &network, configuration: &tagged };
    let update =
        DeviceUpdate { name: "br0".into(), members: names(&["enp2s0", "br0.5"]), mode: None, stp: None };
    assert!(message(planning.change(update)).contains("br0 would end up on itself"));
}

#[test]
fn members_and_modes_change_and_who_leaves_is_switched_off() {
    let mut network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let made = written(&configuration, &planning.make(bond("bond0", &["enp1s0", "enp2s0"])).unwrap());
    for member in ["enp1s0", "enp2s0"] {
        network.interfaces.iter_mut().find(|i| i.name == member).unwrap().member_of = Some("bond0".into());
    }
    network
        .interfaces
        .push(Interface { kind: InterfaceKind::Bond, ..card("bond0", "52:54:00:12:34:56", vec![]) });
    let planning = Planning { network: &network, configuration: &made };

    let update = DeviceUpdate {
        name: "bond0".into(),
        members: names(&["enp1s0", "enp3s0"]),
        mode: Some(BondMode::Lacp),
        stp: None,
    };
    let edit = planning.change(update).unwrap();
    assert_eq!(edit.subject, Subject::Device("bond0".into(), DeviceAction::Changed));
    assert_eq!(edit.devices[0].kind, DeviceKind::Bond { mode: BondMode::Lacp });
    assert_eq!(edit.devices[0].members, ["enp1s0", "enp3s0"]);
    assert_eq!(edit.devices[0].mac.as_deref(), Some("52:54:00:12:34:56"), "the device's address stays");
    let fates: Vec<(&str, &Becomes)> =
        edit.effects.iter().map(|e| (e.interface.as_str(), &e.becomes)).collect();
    assert_eq!(
        fates,
        [
            ("enp3s0", &Becomes::Member { device: "bond0".into(), moved: false, dropped: true }),
            ("enp2s0", &Becomes::SwitchedOff),
        ]
    );
    assert!(
        edit.links
            .iter()
            .any(|l| l.settings.interface == "enp2s0" && l.settings.switched_off() && l.port_of.is_none())
    );

    let same =
        DeviceUpdate { name: "bond0".into(), members: names(&["enp1s0", "enp2s0"]), mode: None, stp: None };
    assert!(message(planning.change(same)).contains("stays as it is"));
    let stp = DeviceUpdate {
        stp: Some(true),
        ..DeviceUpdate { name: "bond0".into(), members: names(&["enp1s0"]), mode: None, stp: None }
    };
    assert!(message(planning.change(stp)).contains("a bond has no STP"));
    let none = DeviceUpdate { name: "bond0".into(), members: vec![], mode: None, stp: None };
    assert!(message(planning.change(none)).contains("at least one member"));
    let theirs = DeviceUpdate { name: "enp1s0".into(), members: vec![], mode: None, stp: None };
    assert!(message(planning.change(theirs)).contains("no bridge, bond or VLAN zendiloOS made"));
}

#[test]
fn a_device_removed_gives_its_settings_back_to_its_first_member() {
    let mut network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let made = written(&configuration, &planning.make(bridge("br0", &["enp2s0", "enp1s0"])).unwrap());
    // The bridge has the card's address now.
    let lan = network.interfaces.iter_mut().find(|i| i.name == "enp1s0").unwrap();
    lan.addresses.clear();
    lan.member_of = Some("br0".into());
    network.interfaces.iter_mut().find(|i| i.name == "enp2s0").unwrap().member_of = Some("br0".into());
    network.interfaces.push(Interface {
        kind: InterfaceKind::Bridge,
        ..card("br0", "52:54:00:12:34:56", vec![address("192.168.1.21", 24, true)])
    });
    let planning = Planning { network: &network, configuration: &made };

    let edit = planning.remove("br0", &[]).unwrap();
    assert_eq!((edit.removed.clone(), edit.devices.len()), (names(&["br0"]), 0));
    assert_eq!(
        edit.links[0].settings,
        dhcp("enp1s0", Some("192.168.1.21")),
        "the address it has now, asked for"
    );
    assert!(edit.links[1].settings.switched_off() && edit.links[1].settings.interface == "enp2s0");
    assert_eq!(edit.effects[1].becomes, Becomes::SwitchedOff);

    assert!(message(planning.remove("br0", &names(&["web", "mail"]))).contains("br0 is in use by web, mail"));
    assert!(message(planning.remove("enp1s0", &[])).contains("no bridge, bond or VLAN zendiloOS made"));
    // Members and parents go last.
    let tagged = written(&made, &planning.make(vlan("br0.5", "br0", 5)).unwrap());
    let planning = Planning { network: &network, configuration: &tagged };
    assert!(message(planning.remove("br0", &[])).contains("br0 carries the VLAN br0.5: remove it first"));
    let edit = planning.remove("br0.5", &[]).unwrap();
    assert!(edit.links[0].vlans.is_empty() && edit.links[0].settings.interface == "br0");
    assert_eq!(edit.effects[0].becomes, Becomes::Parent { vlan: "br0.5".into(), carries: false });
    assert!(message(planning.settings(dhcp("enp1s0", None))).contains("enp1s0 is a member of br0"));
}

#[test]
fn a_member_in_a_bond_in_a_bridge_stays_where_it_is() {
    let mut network = network();
    let configuration = Configuration::default();
    let planning = Planning { network: &network, configuration: &configuration };
    let bonded = written(&configuration, &planning.make(bond("bond0", &["enp1s0", "enp2s0"])).unwrap());
    network
        .interfaces
        .push(Interface { kind: InterfaceKind::Bond, ..card("bond0", "52:54:00:12:34:56", vec![]) });
    let planning = Planning { network: &network, configuration: &bonded };
    let edit = planning.make(bridge("br0", &["bond0"])).unwrap();
    assert_eq!(
        edit.links[0].settings,
        dhcp("br0", Some("192.168.1.20")),
        "the bond's settings, which were the card's"
    );
    let bridged = written(&bonded, &edit);
    let planning = Planning { network: &network, configuration: &bridged };
    assert!(message(planning.remove("bond0", &[])).contains("bond0 is a member of br0: take it out first"));
    assert!(
        message(planning.make(bridge("br1", &["enp1s0"]))).contains("enp1s0 is a member of bond0 already")
    );

    // Out of the bridge, the bond stays up without an address, where a
    // card would be switched off.
    let update = DeviceUpdate { name: "br0".into(), members: vec![], mode: None, stp: None };
    let edit = planning.change(update).unwrap();
    let out = edit.links.iter().find(|link| link.settings.interface == "bond0").unwrap();
    assert!(out.keep_up && out.port_of.is_none() && out.settings.switched_off(), "{out:?}");
    assert_eq!(edit.effects[0].becomes, Becomes::Settings(off("bond0")));
    // The bridge removed gives the bond its settings back, and the bond
    // stays a device made here.
    let edit = planning.remove("br0", &[]).unwrap();
    assert_eq!(edit.links[0].settings, dhcp("bond0", Some("192.168.1.20")));
    assert!(edit.links[0].keep_up);
    assert_eq!(edit.effects[0].becomes, Becomes::Settings(dhcp("bond0", Some("192.168.1.20"))));
}
