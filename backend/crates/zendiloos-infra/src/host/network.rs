//! The machine's network through `ip -json`, run unprivileged (reading
//! needs no capability, only netlink), and the name servers from
//! `resolv.conf`. Nothing from a request reaches the command line.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use zendiloos_exec::{Limits, Program};
use zendiloos_kernel::PortError;
use zendiloos_network::application::NetworkSource;
use zendiloos_network::domain::{
    Address, AddressScope, DefaultRoute, Interface, InterfaceKind, LinkState, Network, name_servers,
    only_stub,
};

const WHAT: &str = "network";

/// Where `ip` is looked for if no path is configured: Debian, then other
/// common layouts (for development machines).
const CANDIDATES: [&str; 4] = ["/usr/sbin/ip", "/sbin/ip", "/usr/bin/ip", "/run/current-system/sw/bin/ip"];

const RESOLV_CONF: &str = "/etc/resolv.conf";
/// The real servers behind systemd-resolved's stub.
const RESOLVED_CONF: &str = "/run/systemd/resolve/resolv.conf";

pub struct IpNetworkReader {
    program: Option<Program>,
}

impl IpNetworkReader {
    /// `path` if given, otherwise the first of the usual places that
    /// exists. Without one, reading fails with a clear error.
    pub fn new(path: Option<&Path>) -> Self {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| CANDIDATES.iter().map(PathBuf::from).find(|candidate| candidate.exists()));
        Self { program: path.and_then(|path| Program::new(path).ok()) }
    }

    pub fn path(&self) -> Option<&Path> {
        self.program.as_ref().map(Program::path)
    }

    async fn ip(&self, args: &[&str]) -> Result<String, PortError> {
        let program = self.program.as_ref().ok_or_else(|| PortError::new(WHAT, "ip not found"))?;
        let limits = Limits { timeout: Duration::from_secs(10), max_stdout: 4 * 1024 * 1024 };
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let output = zendiloos_exec::run(program, args, limits)
            .await
            .and_then(|output| output.success(program))
            .map_err(|e| PortError::new(WHAT, e))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[async_trait]
impl NetworkSource for IpNetworkReader {
    async fn network(&self) -> Result<Network, PortError> {
        let addresses = self.ip(&["-json", "-details", "address", "show"]).await?;
        let routes = self.ip(&["-json", "route", "show", "default"]).await?;
        let routes6 = self.ip(&["-json", "-6", "route", "show", "default"]).await?;
        let interfaces = parse_interfaces(&addresses).map_err(|e| PortError::new(WHAT, e))?;
        let mut default_routes = parse_routes(&routes).map_err(|e| PortError::new(WHAT, e))?;
        default_routes.extend(parse_routes(&routes6).map_err(|e| PortError::new(WHAT, e))?);
        Ok(Network::new(interfaces, default_routes, read_name_servers().await))
    }
}

/// The servers in `/etc/resolv.conf`, or systemd-resolved's real ones if
/// that only names its stub. A missing file means none.
async fn read_name_servers() -> Vec<IpAddr> {
    let read = |path| async move { tokio::fs::read_to_string(path).await.unwrap_or_default() };
    let servers = name_servers(&read(RESOLV_CONF).await);
    if only_stub(&servers) { name_servers(&read(RESOLVED_CONF).await) } else { servers }
}

#[derive(Debug, thiserror::Error)]
#[error("unexpected ip output: {0}")]
pub struct ParseError(String);

#[derive(Deserialize)]
struct Link {
    ifname: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    mtu: u32,
    #[serde(default)]
    operstate: String,
    #[serde(default)]
    link_type: String,
    address: Option<String>,
    master: Option<String>,
    /// The interface a VLAN (or a veth's peer) is on.
    link: Option<String>,
    linkinfo: Option<LinkInfo>,
    #[serde(default)]
    addr_info: Vec<AddrInfo>,
}

#[derive(Deserialize)]
struct LinkInfo {
    info_kind: Option<String>,
}

#[derive(Deserialize)]
struct AddrInfo {
    local: Option<String>,
    prefixlen: Option<u8>,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    dynamic: bool,
}

fn kind(link: &Link) -> InterfaceKind {
    if link.link_type == "loopback" {
        return InterfaceKind::Loopback;
    }
    match link.linkinfo.as_ref().and_then(|info| info.info_kind.as_deref()) {
        Some("bridge") => InterfaceKind::Bridge,
        Some("bond") => InterfaceKind::Bond,
        Some("vlan") => InterfaceKind::Vlan,
        Some(other) => InterfaceKind::Other(other.to_owned()),
        // Predictable names of wireless cards start with `wl`.
        None if link.ifname.starts_with("wl") => InterfaceKind::Wireless,
        None if link.link_type == "ether" => InterfaceKind::Ethernet,
        None => InterfaceKind::Other(link.link_type.clone()),
    }
}

fn state(link: &Link) -> LinkState {
    let flag = |name: &str| link.flags.iter().any(|f| f == name);
    match link.operstate.as_str() {
        "UP" => LinkState::Up,
        "LOWERLAYERDOWN" => LinkState::NoCarrier,
        _ if flag("NO-CARRIER") => LinkState::NoCarrier,
        "DOWN" if flag("UP") => LinkState::NoCarrier,
        "DOWN" => LinkState::Down,
        // Virtual devices often report no state, but say whether they work.
        _ if flag("LOWER_UP") => LinkState::Up,
        _ => LinkState::Unknown,
    }
}

fn scope(scope: &str) -> AddressScope {
    match scope {
        "link" => AddressScope::Link,
        "host" => AddressScope::Host,
        "site" => AddressScope::Site,
        _ => AddressScope::Global,
    }
}

/// `ip -json -details address show`.
pub fn parse_interfaces(json: &str) -> Result<Vec<Interface>, ParseError> {
    let links: Vec<Link> = serde_json::from_str(json).map_err(|e| ParseError(e.to_string()))?;
    let mut interfaces = Vec::with_capacity(links.len());
    for link in links {
        let mut addresses = Vec::new();
        for info in &link.addr_info {
            let (Some(local), Some(prefix)) = (&info.local, info.prefixlen) else { continue };
            let ip = local.parse().map_err(|_| ParseError(format!("address {local:?}")))?;
            addresses.push(Address { ip, prefix, scope: scope(&info.scope), dynamic: info.dynamic });
        }
        let mac = link.address.as_ref().filter(|_| link.link_type == "ether").cloned();
        let kind = kind(&link);
        interfaces.push(Interface {
            parent: link.link.clone().filter(|_| kind == InterfaceKind::Vlan),
            kind,
            state: state(&link),
            mac,
            mtu: link.mtu,
            member_of: link.master.clone(),
            addresses,
            name: link.ifname,
        });
    }
    Ok(interfaces)
}

#[derive(Deserialize)]
struct Route {
    gateway: Option<String>,
    dev: Option<String>,
    #[serde(default)]
    metric: u32,
}

/// `ip -json route show default`; routes without a gateway (point to
/// point) are left out.
pub fn parse_routes(json: &str) -> Result<Vec<DefaultRoute>, ParseError> {
    // No routes at all print nothing, not `[]`.
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let routes: Vec<Route> = serde_json::from_str(json).map_err(|e| ParseError(e.to_string()))?;
    let mut defaults = Vec::new();
    for route in routes {
        let (Some(gateway), Some(interface)) = (route.gateway, route.dev) else { continue };
        let gateway = gateway.parse().map_err(|_| ParseError(format!("gateway {gateway:?}")))?;
        defaults.push(DefaultRoute { gateway, interface, metric: route.metric });
    }
    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ip -json -details address show` in the Debian 13 test VM: QEMU's
    /// NAT card and a second card bridged into a LAN.
    const VM: &str = include_str!("fixtures/ip-address-vm.json");
    /// The same on a workstation that is itself a VM: its card is in a
    /// bridge, beside Docker's bridges and a container's veth.
    const BRIDGE: &str = include_str!("fixtures/ip-address-bridge.json");
    const ROUTES: &str = include_str!("fixtures/ip-route-vm.json");
    const ROUTES6: &str = include_str!("fixtures/ip-route6-vm.json");

    #[test]
    fn reads_cards_with_their_addresses() {
        let interfaces = parse_interfaces(VM).unwrap();
        let lan = interfaces.iter().find(|i| i.name == "enp0s5").unwrap();
        assert_eq!(lan.kind, InterfaceKind::Ethernet);
        assert_eq!(lan.state, LinkState::Up);
        assert_eq!(lan.mac.as_deref(), Some("52:54:00:5a:d1:05"));
        assert_eq!(lan.mtu, 1500);
        assert_eq!(
            lan.addresses[0],
            Address { ip: [192, 168, 1, 90].into(), prefix: 24, scope: AddressScope::Global, dynamic: true }
        );
        assert_eq!(lan.addresses[1].scope, AddressScope::Link);
        assert!(!lan.addresses[1].dynamic);
        let lo = interfaces.iter().find(|i| i.name == "lo").unwrap();
        assert_eq!((lo.kind.clone(), lo.mac.clone()), (InterfaceKind::Loopback, None));
    }

    #[test]
    fn shows_the_bridge_and_its_card_but_not_dockers_plumbing() {
        let interfaces = parse_interfaces(BRIDGE).unwrap();
        let network = Network::new(interfaces, vec![], vec![]);
        let names: Vec<&str> = network.interfaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["br0", "ens3"]);
        let card = &network.interfaces[1];
        assert_eq!(card.member_of.as_deref(), Some("br0"));
        assert!(card.addresses.is_empty());
        assert_eq!(network.interfaces[0].kind, InterfaceKind::Bridge);
        assert_eq!(network.interfaces[0].addresses[0].ip.to_string(), "192.168.1.9");
        let docker = parse_interfaces(BRIDGE).unwrap().into_iter().find(|i| i.name == "docker0").unwrap();
        assert_eq!(docker.state, LinkState::NoCarrier);
    }

    #[test]
    fn reads_default_routes_of_both_families() {
        let mut routes = parse_routes(ROUTES).unwrap();
        routes.extend(parse_routes(ROUTES6).unwrap());
        let network = Network::new(vec![], routes, vec![]);
        let gateways: Vec<(String, &str, u32)> = network
            .default_routes
            .iter()
            .map(|r| (r.gateway.to_string(), r.interface.as_str(), r.metric))
            .collect();
        assert_eq!(
            gateways,
            [
                ("10.0.2.2".into(), "enp0s4", 100),
                ("fe80::2".into(), "enp0s4", 100),
                ("192.168.1.1".into(), "enp0s5", 200)
            ]
        );
        assert_eq!(parse_routes("").unwrap(), []);
    }

    /// A VLAN names the interface it is on; a veth's peer is no parent.
    #[test]
    fn a_vlan_is_on_its_parent() {
        let json = r#"[
            {"ifname":"enp1s0.10","link":"enp1s0","link_type":"ether","address":"52:54:00:12:34:56","linkinfo":{"info_kind":"vlan"}},
            {"ifname":"veth1","link":"if5","link_type":"ether","linkinfo":{"info_kind":"veth"}}
        ]"#;
        let interfaces = parse_interfaces(json).unwrap();
        assert_eq!(interfaces[0].parent.as_deref(), Some("enp1s0"));
        assert_eq!(interfaces[0].kind, InterfaceKind::Vlan);
        assert_eq!(interfaces[1].parent, None);
    }

    #[test]
    fn refuses_garbage() {
        assert!(parse_interfaces("not json").is_err());
        assert!(
            parse_interfaces(r#"[{"ifname":"x","addr_info":[{"local":"300.1.1.1","prefixlen":8}]}]"#)
                .is_err()
        );
    }
}
