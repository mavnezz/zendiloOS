//! The network settings of a restored configuration backup beside this
//! machine's (docs/architecture.md, 4.15): interface by interface and
//! device by device, whether they can be applied here. Interfaces have
//! other names on other hardware, so only those whose names this machine
//! has are offered; each is applied as an ordinary change that goes back
//! unless confirmed, never by itself.

use super::devices::{Configuration, Device, DeviceKind, Planning};
use super::{InterfaceSettings, Ipv4Setting};

/// How a backup's interface or device stands here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// It can be applied here.
    Applicable,
    /// The files here say so already.
    InForce,
    /// There is no interface of this name here (an interface), or not
    /// every member or parent it is on (a device): it is skipped.
    Missing,
    /// In the backup a member of this bridge or bond, whose settings are
    /// the device's: it comes with the device.
    MemberThere(String),
    /// Here a member of this bridge or bond: its settings are the device's.
    MemberHere(String),
    /// Here an interface or device of this name is something else.
    Taken,
}

/// An interface's settings in the backup, and how they stand here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredLink {
    pub settings: InterfaceSettings,
    pub standing: Standing,
}

/// A bridge, bond or VLAN the backup's machine had, and how it stands here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredDevice {
    pub device: Device,
    pub standing: Standing,
}

/// The backup's network settings beside this machine's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoredConfiguration {
    /// By interface name.
    pub links: Vec<RestoredLink>,
    /// By name.
    pub devices: Vec<RestoredDevice>,
}

/// The same settings, whatever address DHCP asks for again.
pub fn same_settings(a: &InterfaceSettings, b: &InterfaceSettings) -> bool {
    let plain = |settings: &InterfaceSettings| {
        let mut settings = settings.clone();
        if let Ipv4Setting::Dhcp { keep } = &mut settings.ipv4 {
            *keep = None;
        }
        settings
    };
    plain(a) == plain(b)
}

/// How `backup`'s interfaces and devices stand on this machine, as
/// `planning` sees it.
pub fn restored_beside(backup: &Configuration, planning: &Planning<'_>) -> RestoredConfiguration {
    let here = |name: &str| planning.network.interfaces.iter().any(|interface| interface.name == name);
    let links = backup
        .links
        .iter()
        .map(|link| {
            let name = link.settings.interface.as_str();
            let standing = if let Some(device) = &link.port_of {
                Standing::MemberThere(device.clone())
            } else if !here(name) {
                Standing::Missing
            } else if let Some(master) = planning.master_of(name) {
                Standing::MemberHere(master)
            } else if planning.settings_of(name).is_some_and(|now| same_settings(&now, &link.settings)) {
                Standing::InForce
            } else {
                Standing::Applicable
            };
            RestoredLink { settings: link.settings.clone(), standing }
        })
        .collect();
    let devices = backup
        .devices
        .iter()
        .map(|device| {
            let lower: Vec<&str> = match &device.kind {
                DeviceKind::Vlan { parent, .. } => vec![parent.as_str()],
                _ => device.members.iter().map(String::as_str).collect(),
            };
            let standing = match planning.configuration.device(&device.name) {
                Some(ours) if ours.kind == device.kind && ours.members == device.members => Standing::InForce,
                Some(_) => Standing::Taken,
                None if here(&device.name) => Standing::Taken,
                None if lower.iter().any(|name| name.is_empty() || !here(name)) => Standing::Missing,
                None => Standing::Applicable,
            };
            RestoredDevice { device: device.clone(), standing }
        })
        .collect();
    RestoredConfiguration { links, devices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Address, AddressScope, Interface, InterfaceKind, Ipv6Setting, LinkState, Network, NetworkdFile,
    };

    fn card(name: &str, address: Option<&str>) -> Interface {
        Interface {
            name: name.into(),
            kind: InterfaceKind::Ethernet,
            state: LinkState::Up,
            mac: Some("52:54:00:12:34:56".into()),
            mtu: 1500,
            member_of: None,
            parent: None,
            addresses: address
                .map(|ip| Address {
                    ip: ip.parse().unwrap(),
                    prefix: 24,
                    scope: AddressScope::Global,
                    dynamic: true,
                })
                .into_iter()
                .collect(),
        }
    }

    fn file(name: &str, text: &str) -> NetworkdFile {
        NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() }
    }

    /// The backed-up machine: enp1s0 fixed, enp2s0 by DHCP, eno1 by DHCP
    /// (another card's name), br0 of enp3s0 and enp4s0 with a VLAN on it.
    fn backup() -> Configuration {
        Configuration::read(&[
            file(
                "enp1s0.network",
                "[Match]\nName=enp1s0\n[Network]\nAddress=192.168.1.20/24\nGateway=192.168.1.1\nDNS=192.168.1.1\n",
            ),
            file("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n"),
            file("eno1.network", "[Match]\nName=eno1\n[Network]\nDHCP=ipv4\n"),
            file(
                "br0.netdev",
                "# Members: enp3s0 enp4s0\n[NetDev]\nName=br0\nKind=bridge\n[Bridge]\nSTP=no\n",
            ),
            file("br0.network", "[Match]\nName=br0\n[Network]\nDHCP=ipv4\nVLAN=br0.10\n"),
            file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nBridge=br0\n"),
            file("enp4s0.network", "[Match]\nName=enp4s0\n[Network]\nBridge=br0\n"),
            file("br0.10.netdev", "[NetDev]\nName=br0.10\nKind=vlan\n[VLAN]\nId=10\n"),
            file("br0.10.network", "[Match]\nName=br0.10\n"),
        ])
    }

    #[test]
    fn only_what_has_its_names_here_can_be_applied() {
        let network = Network {
            interfaces: vec![
                card("enp1s0", Some("192.168.1.50")),
                card("enp2s0", Some("10.0.0.5")),
                card("enp3s0", None),
            ],
            ..Network::default()
        };
        let here = Configuration::read(&[file(
            "enp2s0.network",
            "[Match]\nName=enp2s0\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n",
        )]);
        let planning = Planning { network: &network, configuration: &here };
        let restored = restored_beside(&backup(), &planning);
        let standing: Vec<(&str, Standing)> = restored
            .links
            .iter()
            .map(|link| (link.settings.interface.as_str(), link.standing.clone()))
            .collect();
        assert_eq!(
            standing,
            [
                ("br0", Standing::Missing),
                ("br0.10", Standing::Missing),
                ("eno1", Standing::Missing),
                ("enp1s0", Standing::Applicable),
                ("enp2s0", Standing::InForce),
                ("enp3s0", Standing::MemberThere("br0".into())),
                ("enp4s0", Standing::MemberThere("br0".into())),
            ]
        );
        let devices: Vec<(&str, Standing)> = restored
            .devices
            .iter()
            .map(|device| (device.device.name.as_str(), device.standing.clone()))
            .collect();
        assert_eq!(
            devices,
            [("br0", Standing::Missing), ("br0.10", Standing::Missing)],
            "enp4s0 is not here"
        );
    }

    #[test]
    fn a_device_is_made_where_its_members_are_and_left_where_it_is() {
        let network = Network {
            interfaces: vec![card("enp1s0", None), card("enp3s0", None), card("enp4s0", None)],
            ..Network::default()
        };
        let none = Configuration::default();
        let planning = Planning { network: &network, configuration: &none };
        let restored = restored_beside(&backup(), &planning);
        assert_eq!(restored.devices[0].standing, Standing::Applicable, "br0's members are here");
        assert_eq!(restored.devices[1].standing, Standing::Missing, "its VLAN waits for br0");

        let mut network = network;
        network.interfaces.push(card("br0", None));
        network.interfaces[1].member_of = Some("br0".into());
        let ours = Configuration::read(&[
            file(
                "br0.netdev",
                "# Members: enp3s0 enp4s0\n[NetDev]\nName=br0\nKind=bridge\n[Bridge]\nSTP=no\n",
            ),
            file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nBridge=br0\n"),
            file("enp4s0.network", "[Match]\nName=enp4s0\n[Network]\nBridge=br0\n"),
        ]);
        let planning = Planning { network: &network, configuration: &ours };
        let restored = restored_beside(&backup(), &planning);
        assert_eq!(restored.devices[0].standing, Standing::InForce);
        assert_eq!(restored.devices[1].standing, Standing::Applicable, "br0 carries it now");
        let br0 = restored.links.iter().find(|link| link.settings.interface == "br0").unwrap();
        assert_eq!(br0.standing, Standing::Applicable, "its own settings follow it");

        let other = Configuration::read(&[file(
            "br0.netdev",
            "[NetDev]\nName=br0\nKind=bond\n[Bond]\nMode=active-backup\n",
        )]);
        let planning = Planning { network: &network, configuration: &other };
        assert_eq!(restored_beside(&backup(), &planning).devices[0].standing, Standing::Taken);
    }

    #[test]
    fn settings_are_the_same_whatever_address_dhcp_asks_for() {
        let dhcp = |keep: Option<&str>| InterfaceSettings {
            interface: "enp1s0".into(),
            ipv4: Ipv4Setting::Dhcp { keep: keep.map(|ip| ip.parse().unwrap()) },
            ipv6: Ipv6Setting::Automatic,
            dns: Vec::new(),
        };
        assert!(same_settings(&dhcp(None), &dhcp(Some("10.0.0.5"))));
        let mut off = dhcp(None);
        off.ipv6 = Ipv6Setting::Off;
        assert!(!same_settings(&dhcp(None), &off));
    }
}
