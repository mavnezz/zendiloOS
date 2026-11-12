//! Interfaces as the kernel reports them, and which of them matter to
//! someone looking after a NAS.

use std::net::{IpAddr, Ipv4Addr};

pub mod devices;
pub mod ifupdown;
pub mod kept;
mod settings;

pub use devices::{
    Becomes, BondMode, Configuration, Device, DeviceAction, DeviceKind, DeviceUpdate, Edit, Effect,
    InvalidDevice, LinkFile, MAX_MEMBERS, NetworkdFile, NewDevice, Planning, Subject,
};
pub use ifupdown::{IfupdownFile, Refusal, TakeoverPlan, takeover_plan};
pub use kept::{Difference, DifferenceKind, Part, differences, restoring};
pub use settings::{
    InterfaceSettings, InvalidSettings, Ipv4Setting, Ipv6Setting, MAX_DNS, is_interface_name,
};

/// What kind of device an interface is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceKind {
    /// A network card, or one the hypervisor emulates.
    Ethernet,
    Loopback,
    Bridge,
    Bond,
    Vlan,
    Wireless,
    /// Anything else the kernel names, e.g. `veth`, `tun`, `wireguard`.
    Other(String),
}

/// Whether an interface can carry traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Up,
    /// Switched on, but no cable or no partner.
    NoCarrier,
    Down,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressScope {
    Global,
    /// IPv6 site-local, deprecated but still seen (e.g. from QEMU).
    Site,
    /// Only valid on the link, e.g. `fe80::…`.
    Link,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub ip: IpAddr,
    pub prefix: u8,
    pub scope: AddressScope,
    /// Handed out by DHCP or router advertisements rather than configured.
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub kind: InterfaceKind,
    pub state: LinkState,
    /// Hardware address, if it has one.
    pub mac: Option<String>,
    pub mtu: u32,
    /// The bridge or bond it belongs to; its addresses are on that one.
    pub member_of: Option<String>,
    /// A VLAN's: the interface it is on.
    pub parent: Option<String>,
    pub addresses: Vec<Address>,
}

impl Interface {
    /// Whether to show it: not the loopback, not the plumbing of containers
    /// and VMs (`veth`, `tun`/`tap`, Docker's bridges).
    pub fn matters(&self) -> bool {
        match &self.kind {
            InterfaceKind::Loopback => false,
            InterfaceKind::Other(kind) => !matches!(kind.as_str(), "veth" | "tun" | "tap" | "dummy"),
            InterfaceKind::Bridge => !is_docker_bridge(&self.name),
            _ => true,
        }
    }
}

/// Docker names its bridges `docker0` and `br-` plus 12 hex digits.
pub(crate) fn is_docker_bridge(name: &str) -> bool {
    name == "docker0"
        || name
            .strip_prefix("br-")
            .is_some_and(|id| id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultRoute {
    pub gateway: IpAddr,
    pub interface: String,
    /// Lower wins when there are several.
    pub metric: u32,
}

/// The machine's connections, as shown to administrators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Network {
    /// Only those that [matter](Interface::matters), by name.
    pub interfaces: Vec<Interface>,
    /// The preferred one first.
    pub default_routes: Vec<DefaultRoute>,
    pub name_servers: Vec<IpAddr>,
    /// Docker's bridges, by name (not shown): the firewall lets containers
    /// on them reach the NAS.
    pub docker_bridges: Vec<String>,
    /// The machine's own bridges, by name: those with a member other than a
    /// container's end of a `veth` pair (a network card, a bond, a VM's
    /// `tap`). Docker's hold only containers, whatever they are named; the
    /// firewall keeps connections from outside off every bridge but these.
    pub own_bridges: Vec<String>,
}

impl Network {
    pub fn new(
        interfaces: Vec<Interface>,
        mut default_routes: Vec<DefaultRoute>,
        name_servers: Vec<IpAddr>,
    ) -> Self {
        let mut docker_bridges: Vec<String> = interfaces
            .iter()
            .filter(|i| i.kind == InterfaceKind::Bridge && is_docker_bridge(&i.name))
            .map(|i| i.name.clone())
            .collect();
        docker_bridges.sort_unstable();
        let mut own_bridges: Vec<String> = interfaces
            .iter()
            .filter(|i| i.kind == InterfaceKind::Bridge && !is_docker_bridge(&i.name))
            .filter(|bridge| {
                interfaces.iter().any(|member| {
                    member.member_of.as_deref() == Some(bridge.name.as_str())
                        && member.kind != InterfaceKind::Other("veth".into())
                })
            })
            .map(|i| i.name.clone())
            .collect();
        own_bridges.sort_unstable();
        let mut interfaces: Vec<Interface> = interfaces.into_iter().filter(Interface::matters).collect();
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        for interface in &mut interfaces {
            // Global addresses first, IPv4 before IPv6.
            interface.addresses.sort_by_key(|a| (a.scope != AddressScope::Global, a.ip.is_ipv6()));
        }
        default_routes.sort_by_key(|route| (route.metric, route.gateway.is_ipv6()));
        Self { interfaces, default_routes, name_servers, docker_bridges, own_bridges }
    }
}

impl Network {
    /// The IPv4 address DHCP gave `interface`, if it has one: asked for
    /// again when its settings change, so it stays where the server agrees.
    pub fn dynamic_address(&self, interface: &str) -> Option<Ipv4Addr> {
        let interface = self.interfaces.iter().find(|i| i.name == interface)?;
        interface.addresses.iter().find_map(|address| match address.ip {
            IpAddr::V4(ip) if address.dynamic && address.scope == AddressScope::Global => Some(ip),
            _ => None,
        })
    }
}

/// Name servers from a `resolv.conf`; `127.0.0.53` is systemd-resolved's
/// stub, which says nothing about the real ones.
pub fn name_servers(resolv_conf: &str) -> Vec<IpAddr> {
    resolv_conf
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|rest| rest.trim().parse::<IpAddr>().ok())
        .collect()
}

/// Whether `servers` is only systemd-resolved's stub.
pub fn only_stub(servers: &[IpAddr]) -> bool {
    !servers.is_empty() && servers.iter().all(|ip| *ip == IpAddr::from([127, 0, 0, 53]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(name: &str, kind: InterfaceKind) -> Interface {
        Interface {
            name: name.into(),
            kind,
            state: LinkState::Up,
            mac: None,
            mtu: 1500,
            member_of: None,
            parent: None,
            addresses: vec![],
        }
    }

    #[test]
    fn container_plumbing_is_left_out() {
        let shown = |name, kind| interface(name, kind).matters();
        assert!(shown("enp1s0", InterfaceKind::Ethernet));
        assert!(shown("br0", InterfaceKind::Bridge));
        assert!(shown("bond0", InterfaceKind::Bond));
        assert!(shown("wg0", InterfaceKind::Other("wireguard".into())));
        assert!(!shown("lo", InterfaceKind::Loopback));
        assert!(!shown("docker0", InterfaceKind::Bridge));
        assert!(!shown("br-9b4ffb8b3e6d", InterfaceKind::Bridge));
        assert!(shown("br-lan", InterfaceKind::Bridge));
        assert!(!shown("veth1234", InterfaceKind::Other("veth".into())));
    }

    #[test]
    fn dockers_bridges_are_known_by_name_and_only_they() {
        let network = Network::new(
            vec![
                interface("br-lan", InterfaceKind::Bridge),
                interface("docker0", InterfaceKind::Bridge),
                interface("br-9b4ffb8b3e6d", InterfaceKind::Bridge),
                interface("br-0123456789ab", InterfaceKind::Other("veth".into())),
                interface("enp1s0", InterfaceKind::Ethernet),
            ],
            vec![],
            vec![],
        );
        assert_eq!(network.docker_bridges, ["br-9b4ffb8b3e6d", "docker0"]);
        let shown: Vec<&str> = network.interfaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(shown, ["br-lan", "enp1s0"]);
    }

    /// A bridge is the machine's own when something besides containers is
    /// in it: a network card, or a VM. A Docker network named otherwise
    /// (`com.docker.network.bridge.name`) holds only containers' `veth`s,
    /// and an empty bridge holds nobody to keep out.
    #[test]
    fn own_bridges_are_those_with_more_than_containers_in_them() {
        let member = |name: &str, kind: InterfaceKind, bridge: &str| Interface {
            member_of: Some(bridge.into()),
            ..interface(name, kind)
        };
        let network = Network::new(
            vec![
                interface("br-lan", InterfaceKind::Bridge),
                member("enp1s0", InterfaceKind::Ethernet, "br-lan"),
                member("veth1", InterfaceKind::Other("veth".into()), "br-lan"),
                interface("virbr0", InterfaceKind::Bridge),
                member("vnet0", InterfaceKind::Other("tun".into()), "virbr0"),
                interface("proxy0", InterfaceKind::Bridge),
                member("veth2", InterfaceKind::Other("veth".into()), "proxy0"),
                interface("br0", InterfaceKind::Bridge),
                interface("docker0", InterfaceKind::Bridge),
                member("enp2s0", InterfaceKind::Ethernet, "docker0"),
            ],
            vec![],
            vec![],
        );
        assert_eq!(network.own_bridges, ["br-lan", "virbr0"]);
    }

    #[test]
    fn the_preferred_route_and_global_addresses_come_first() {
        let address =
            |ip: &str, scope| Address { ip: ip.parse().unwrap(), prefix: 64, scope, dynamic: false };
        let mut card = interface("eth0", InterfaceKind::Ethernet);
        card.addresses = vec![
            address("fe80::1", AddressScope::Link),
            address("2001:db8::1", AddressScope::Global),
            address("192.168.1.9", AddressScope::Global),
        ];
        let route = |gateway: &str, metric| DefaultRoute {
            gateway: gateway.parse().unwrap(),
            interface: "eth0".into(),
            metric,
        };

        let network =
            Network::new(vec![card], vec![route("10.0.0.1", 200), route("192.168.1.1", 100)], vec![]);

        let ips: Vec<String> = network.interfaces[0].addresses.iter().map(|a| a.ip.to_string()).collect();
        assert_eq!(ips, ["192.168.1.9", "2001:db8::1", "fe80::1"]);
        assert_eq!(network.default_routes[0].gateway.to_string(), "192.168.1.1");
    }

    #[test]
    fn the_address_dhcp_gave_is_known() {
        let address =
            |ip: &str, scope, dynamic| Address { ip: ip.parse().unwrap(), prefix: 24, scope, dynamic };
        let mut card = interface("enp2s0", InterfaceKind::Ethernet);
        card.addresses = vec![
            address("fe80::1", AddressScope::Link, false),
            address("2001:db8::5", AddressScope::Global, true),
            address("10.0.0.5", AddressScope::Global, false),
            address("192.168.1.127", AddressScope::Global, true),
        ];
        let network = Network::new(vec![card, interface("enp3s0", InterfaceKind::Ethernet)], vec![], vec![]);
        assert_eq!(network.dynamic_address("enp2s0"), Some("192.168.1.127".parse().unwrap()));
        assert_eq!(network.dynamic_address("enp3s0"), None);
        assert_eq!(network.dynamic_address("eth9"), None);
    }

    #[test]
    fn reads_name_servers_and_sees_through_the_stub() {
        let stub = "# comment\nnameserver 127.0.0.53\noptions edns0 trust-ad\nsearch .\n";
        assert!(only_stub(&name_servers(stub)));
        let real = "nameserver 1.1.1.1\nnameserver 2606:4700:4700::1111\n# Too many\nnameserver 10.0.2.3\n";
        let servers = name_servers(real);
        assert_eq!(servers.len(), 3);
        assert!(!only_stub(&servers));
        assert!(!only_stub(&[]));
    }
}
