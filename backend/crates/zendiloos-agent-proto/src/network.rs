//! Network settings the agent writes as a systemd-networkd file, and the
//! network taken over from ifupdown. Every value is checked while it is
//! read, so the file can only say what these types allow.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// An interface's name as the kernel allows it, and nothing a file could
/// misread: `enp1s0`, `eth0`, `br0`, `bond0.10`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InterfaceName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an interface name")]
pub struct InvalidInterfaceName;

impl InterfaceName {
    pub fn parse(name: &str) -> Result<Self, InvalidInterfaceName> {
        let valid = (1..=15).contains(&name.len())
            && name != "lo"
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && !name.starts_with(['-', '.']);
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidInterfaceName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InterfaceName {
    type Error = InvalidInterfaceName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<InterfaceName> for String {
    fn from(name: InterfaceName) -> Self {
        name.0
    }
}

/// An IPv4 prefix length, 1 to 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Prefix(u8);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a prefix length is from 1 to 32")]
pub struct InvalidPrefix;

impl Prefix {
    pub fn new(length: u8) -> Result<Self, InvalidPrefix> {
        if (1..=32).contains(&length) { Ok(Self(length)) } else { Err(InvalidPrefix) }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Prefix {
    type Error = InvalidPrefix;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Prefix> for u8 {
    fn from(prefix: Prefix) -> Self {
        prefix.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ipv4Config {
    /// From DHCP. `request` is the address to ask for, the one the
    /// interface has now, so it stays where the server agrees.
    Dhcp {
        #[serde(default)]
        request: Option<Ipv4Addr>,
    },
    Static {
        address: Ipv4Addr,
        prefix: Prefix,
        gateway: Option<Ipv4Addr>,
    },
    /// No IPv4 address; with IPv6 off too, the interface is switched off.
    Off {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ipv6Config {
    /// From router advertisements, and DHCPv6 where they say so.
    Automatic,
    Off,
}

/// Up to three name servers; none: those DHCP and the router give.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<IpAddr>", into = "Vec<IpAddr>")]
pub struct NameServers(Vec<IpAddr>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most three name servers")]
pub struct InvalidNameServers;

impl NameServers {
    pub fn new(servers: Vec<IpAddr>) -> Result<Self, InvalidNameServers> {
        if servers.len() <= 3 { Ok(Self(servers)) } else { Err(InvalidNameServers) }
    }

    pub fn as_slice(&self) -> &[IpAddr] {
        &self.0
    }
}

impl TryFrom<Vec<IpAddr>> for NameServers {
    type Error = InvalidNameServers;

    fn try_from(value: Vec<IpAddr>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NameServers> for Vec<IpAddr> {
    fn from(servers: NameServers) -> Self {
        servers.0
    }
}

/// What an interface is a port of: `Bridge=` or `Bond=` in its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortOf {
    Bridge { name: InterfaceName },
    Bond { name: InterfaceName },
}

impl PortOf {
    pub fn name(&self) -> &InterfaceName {
        match self {
            Self::Bridge { name } | Self::Bond { name } => name,
        }
    }
}

/// The VLANs an interface carries (`VLAN=` in its file): each once, at
/// most [`Vlans::MAX`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<InterfaceName>", into = "Vec<InterfaceName>")]
pub struct Vlans(Vec<InterfaceName>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most 32 VLANs, each once")]
pub struct InvalidVlans;

impl Vlans {
    pub const MAX: usize = 32;

    pub fn new(vlans: Vec<InterfaceName>) -> Result<Self, InvalidVlans> {
        if vlans.len() <= Self::MAX && distinct(&vlans) { Ok(Self(vlans)) } else { Err(InvalidVlans) }
    }

    pub fn as_slice(&self) -> &[InterfaceName] {
        &self.0
    }
}

impl TryFrom<Vec<InterfaceName>> for Vlans {
    type Error = InvalidVlans;

    fn try_from(value: Vec<InterfaceName>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Vlans> for Vec<InterfaceName> {
    fn from(vlans: Vlans) -> Self {
        vlans.0
    }
}

fn distinct(names: &[InterfaceName]) -> bool {
    names.iter().enumerate().all(|(i, name)| !names[..i].contains(name))
}

/// One interface's systemd-networkd file: its addresses, or the bridge or
/// bond it is a port of, and the VLANs on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NetworkConfigFields", into = "NetworkConfigFields")]
pub struct NetworkConfig {
    pub interface: InterfaceName,
    pub ipv4: Ipv4Config,
    pub ipv6: Ipv6Config,
    pub dns: NameServers,
    /// A port of a bridge or bond: then it has no addresses of its own
    /// (IPv4 and IPv6 off, no name servers).
    pub port_of: Option<PortOf>,
    pub vlans: Vlans,
    /// Without addresses it stays up instead of being switched off: a
    /// bridge, bond or VLAN zendiloOS made, which is there to carry
    /// others. One that carries VLANs or is a port stays up anyway.
    pub keep_up: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkConfigFields {
    interface: InterfaceName,
    ipv4: Ipv4Config,
    ipv6: Ipv6Config,
    dns: NameServers,
    #[serde(default)]
    port_of: Option<PortOf>,
    #[serde(default)]
    vlans: Vlans,
    #[serde(default)]
    keep_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidNetworkConfig {
    #[error("a port of a bridge or bond has no addresses of its own")]
    PortWithAddresses,
    #[error("an interface is no port of itself and carries no VLAN of its own name")]
    Itself,
}

impl NetworkConfig {
    /// An interface with addresses of its own (or none, switched off),
    /// carrying no VLANs.
    pub fn addresses(interface: InterfaceName, ipv4: Ipv4Config, ipv6: Ipv6Config, dns: NameServers) -> Self {
        Self { interface, ipv4, ipv6, dns, port_of: None, vlans: Vlans::default(), keep_up: false }
    }

    /// Neither IPv4 nor IPv6 nor anything to carry: switched off (down).
    pub fn switched_off(&self) -> bool {
        matches!(self.ipv4, Ipv4Config::Off {})
            && self.ipv6 == Ipv6Config::Off
            && self.port_of.is_none()
            && self.vlans.as_slice().is_empty()
            && !self.keep_up
    }
}

impl TryFrom<NetworkConfigFields> for NetworkConfig {
    type Error = InvalidNetworkConfig;

    fn try_from(fields: NetworkConfigFields) -> Result<Self, Self::Error> {
        let addressless = matches!(fields.ipv4, Ipv4Config::Off {})
            && fields.ipv6 == Ipv6Config::Off
            && fields.dns.as_slice().is_empty();
        if fields.port_of.is_some() && !addressless {
            return Err(InvalidNetworkConfig::PortWithAddresses);
        }
        let itself = fields.port_of.as_ref().is_some_and(|port| *port.name() == fields.interface)
            || fields.vlans.as_slice().contains(&fields.interface);
        if itself {
            return Err(InvalidNetworkConfig::Itself);
        }
        Ok(Self {
            interface: fields.interface,
            ipv4: fields.ipv4,
            ipv6: fields.ipv6,
            dns: fields.dns,
            port_of: fields.port_of,
            vlans: fields.vlans,
            keep_up: fields.keep_up,
        })
    }
}

impl From<NetworkConfig> for NetworkConfigFields {
    fn from(config: NetworkConfig) -> Self {
        Self {
            interface: config.interface,
            ipv4: config.ipv4,
            ipv6: config.ipv6,
            dns: config.dns,
            port_of: config.port_of,
            vlans: config.vlans,
            keep_up: config.keep_up,
        }
    }
}

/// A MAC address a device zendiloOS makes is given: six bytes in lowercase
/// hex with colons, unicast, not all zero.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MacAddress(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a unicast MAC address")]
pub struct InvalidMacAddress;

impl MacAddress {
    pub fn parse(text: &str) -> Result<Self, InvalidMacAddress> {
        let text = text.to_ascii_lowercase();
        let parts: Vec<&str> = text.split(':').collect();
        let hex = parts.len() == 6
            && parts.iter().all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()));
        if !hex {
            return Err(InvalidMacAddress);
        }
        let first = u8::from_str_radix(parts[0], 16).map_err(|_| InvalidMacAddress)?;
        if first & 1 == 1 || parts.iter().all(|part| *part == "00") {
            return Err(InvalidMacAddress);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MacAddress {
    type Error = InvalidMacAddress;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<MacAddress> for String {
    fn from(mac: MacAddress) -> Self {
        mac.0
    }
}

/// A VLAN's id: 1 to 4094 (0 and 4095 are reserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct VlanId(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a VLAN id is from 1 to 4094")]
pub struct InvalidVlanId;

impl VlanId {
    pub fn new(id: u16) -> Result<Self, InvalidVlanId> {
        if (1..=4094).contains(&id) { Ok(Self(id)) } else { Err(InvalidVlanId) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for VlanId {
    type Error = InvalidVlanId;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VlanId> for u16 {
    fn from(id: VlanId) -> Self {
        id.0
    }
}

/// How a bond spreads its traffic over its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BondMode {
    /// One member carries, the others wait for it to fail.
    ActiveBackup,
    /// IEEE 802.3ad (LACP), negotiated with the switch.
    Lacp,
    /// Adaptive load balancing, no help from the switch needed.
    BalanceAlb,
}

/// What a device zendiloOS makes is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetdevKind {
    Bridge {
        stp: bool,
    },
    Bond {
        mode: BondMode,
    },
    /// On the interface whose file names it among its VLANs.
    Vlan {
        id: VlanId,
    },
}

/// A device zendiloOS makes, as its `.netdev` file says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NetdevFields", into = "NetdevFields")]
pub struct Netdev {
    /// Not one of Docker's names, nor a `veth`'s.
    pub name: InterfaceName,
    pub kind: NetdevKind,
    /// The first member's, so DHCP sees the same machine; none for a VLAN
    /// (the parent's) or a bridge without members (one of systemd's).
    pub mac: Option<MacAddress>,
    /// Of a bridge or bond, in order: the first gets the settings back
    /// when it goes. Written into the file as a note for zendiloosd; the
    /// members' own files say what they are a port of.
    pub members: Vec<InterfaceName>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetdevFields {
    name: InterfaceName,
    kind: NetdevKind,
    #[serde(default)]
    mac: Option<MacAddress>,
    #[serde(default)]
    members: Vec<InterfaceName>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidNetdev {
    #[error("Docker's names and veth's are not for devices zendiloOS makes")]
    Name,
    #[error("at most 16 members, each once, never the device itself, and none for a VLAN")]
    Members,
}

impl Netdev {
    pub const MAX_MEMBERS: usize = 16;

    /// Whether `name` is free for a device zendiloOS makes: neither one
    /// Docker would make nor one the firewall takes for a container's
    /// (`veth…`).
    pub fn allowed_name(name: &InterfaceName) -> bool {
        crate::DockerBridge::parse(name.as_str()).is_err() && !name.as_str().starts_with("veth")
    }
}

impl TryFrom<NetdevFields> for Netdev {
    type Error = InvalidNetdev;

    fn try_from(fields: NetdevFields) -> Result<Self, Self::Error> {
        if !Self::allowed_name(&fields.name) {
            return Err(InvalidNetdev::Name);
        }
        let vlan = matches!(fields.kind, NetdevKind::Vlan { .. });
        if fields.members.len() > Self::MAX_MEMBERS
            || !distinct(&fields.members)
            || fields.members.contains(&fields.name)
            || (vlan && !fields.members.is_empty())
        {
            return Err(InvalidNetdev::Members);
        }
        Ok(Self { name: fields.name, kind: fields.kind, mac: fields.mac, members: fields.members })
    }
}

impl From<Netdev> for NetdevFields {
    fn from(netdev: Netdev) -> Self {
        Self { name: netdev.name, kind: netdev.kind, mac: netdev.mac, members: netdev.members }
    }
}

/// A change of the network as one: devices made or changed, devices
/// removed, interfaces' files written. Kept or undone as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NetworkEditFields", into = "NetworkEditFields")]
pub struct NetworkEdit {
    /// What it is, while it waits.
    pub change: NetworkChange,
    /// `.netdev` files written: devices made, or made anew where what
    /// they are changed (systemd-networkd changes no device it made).
    pub netdevs: Vec<Netdev>,
    /// Devices zendiloOS made that go: their files, and their links.
    pub removed: Vec<InterfaceName>,
    /// `.network` files written.
    pub links: Vec<NetworkConfig>,
    /// `.network` files of zendiloOS's that go, of interfaces that are no
    /// device of its: written outside zendiloOS, its settings have none.
    pub unset: Vec<InterfaceName>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkEditFields {
    change: NetworkChange,
    #[serde(default)]
    netdevs: Vec<Netdev>,
    #[serde(default)]
    removed: Vec<InterfaceName>,
    #[serde(default)]
    links: Vec<NetworkConfig>,
    #[serde(default)]
    unset: Vec<InterfaceName>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "a change makes, changes or removes up to 16 devices and writes or removes up to 32 interfaces' files, \
     each once, none for a device that goes"
)]
pub struct InvalidNetworkEdit;

impl NetworkEdit {
    pub const MAX_DEVICES: usize = 16;
    pub const MAX_LINKS: usize = 32;
}

impl TryFrom<NetworkEditFields> for NetworkEdit {
    type Error = InvalidNetworkEdit;

    fn try_from(fields: NetworkEditFields) -> Result<Self, Self::Error> {
        let devices: Vec<InterfaceName> = fields
            .netdevs
            .iter()
            .map(|netdev| netdev.name.clone())
            .chain(fields.removed.iter().cloned())
            .collect();
        let links: Vec<InterfaceName> = fields
            .links
            .iter()
            .map(|link| link.interface.clone())
            .chain(fields.unset.iter().cloned())
            .collect();
        let valid = devices.len() <= Self::MAX_DEVICES
            && links.len() <= Self::MAX_LINKS
            && (!devices.is_empty() || !links.is_empty())
            && distinct(&devices)
            && distinct(&links)
            && !fields.removed.iter().any(|gone| links.contains(gone))
            && !fields.unset.iter().any(|gone| devices.contains(gone))
            && fields.removed.iter().all(Netdev::allowed_name);
        if !valid {
            return Err(InvalidNetworkEdit);
        }
        Ok(Self {
            change: fields.change,
            netdevs: fields.netdevs,
            removed: fields.removed,
            links: fields.links,
            unset: fields.unset,
        })
    }
}

impl From<NetworkEdit> for NetworkEditFields {
    fn from(edit: NetworkEdit) -> Self {
        Self {
            change: edit.change,
            netdevs: edit.netdevs,
            removed: edit.removed,
            links: edit.links,
            unset: edit.unset,
        }
    }
}

/// One of zendiloOS's systemd-networkd files as it is, for zendiloosd to
/// read: `05-zendiloos-<name>.network` or `.netdev`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkdFile {
    pub name: String,
    pub text: String,
}

/// How long a new configuration waits to be confirmed before the old one
/// returns: 30 seconds to 10 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct ConfirmWithin(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("confirmation is waited for 30 to 600 seconds")]
pub struct InvalidConfirmWithin;

impl ConfirmWithin {
    pub fn seconds(seconds: u16) -> Result<Self, InvalidConfirmWithin> {
        if (30..=600).contains(&seconds) { Ok(Self(seconds)) } else { Err(InvalidConfirmWithin) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for ConfirmWithin {
    type Error = InvalidConfirmWithin;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::seconds(value)
    }
}

impl From<ConfirmWithin> for u16 {
    fn from(value: ConfirmWithin) -> Self {
        value.0
    }
}

/// What a change waiting to be confirmed changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkChange {
    /// One interface's settings.
    Interface { interface: InterfaceName },
    /// The network, taken over from ifupdown: these interfaces were set up.
    Takeover { interfaces: Vec<InterfaceName> },
    /// A bridge, bond or VLAN made, changed or removed, with what that
    /// changed of other interfaces.
    Device { device: InterfaceName, action: DeviceAction },
    /// zendiloOS's settings written back, as its database keeps them,
    /// where its files were changed or lost outside it.
    Restore {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAction {
    Made,
    Changed,
    Removed,
}

/// A change that waits to be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingNetwork {
    pub change: NetworkChange,
    /// When the old configuration returns, in Unix seconds.
    pub revert_at: i64,
}

/// One of the files ifupdown reads, by its path below `/etc/network`:
/// `interfaces`, or `interfaces.d/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IfupdownFile {
    pub path: String,
    /// Empty if it was not read.
    pub text: String,
    /// Why it was not read: a link (which may lead out of `/etc/network`),
    /// not a plain file, too large, not text.
    #[serde(default)]
    pub skipped: Option<String>,
}

/// Where ifupdown manages the network: its files, for zendiloosd to read,
/// and what a takeover needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ifupdown {
    /// `/etc/network/interfaces`, then the files directly in
    /// `interfaces.d`, by name.
    pub files: Vec<IfupdownFile>,
    /// Of the files as read: a takeover names it, and is refused if they
    /// changed since.
    pub digest: FilesDigest,
    /// `networking.service` is active: ifupdown brought the network up.
    pub active: bool,
    /// systemd-resolved is installed; a takeover needs it.
    pub resolved_installed: bool,
}

/// A SHA-256 in lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FilesDigest(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a SHA-256 in lowercase hex")]
pub struct InvalidFilesDigest;

impl FilesDigest {
    pub fn parse(text: &str) -> Result<Self, InvalidFilesDigest> {
        let valid =
            text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if valid { Ok(Self(text.to_owned())) } else { Err(InvalidFilesDigest) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FilesDigest {
    type Error = InvalidFilesDigest;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<FilesDigest> for String {
    fn from(digest: FilesDigest) -> Self {
        digest.0
    }
}

/// The settings a takeover writes: each interface once, at most
/// [`TakeoverConfigs::MAX`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<NetworkConfig>", into = "Vec<NetworkConfig>")]
pub struct TakeoverConfigs(Vec<NetworkConfig>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most 32 interfaces, each once")]
pub struct InvalidTakeoverConfigs;

impl TakeoverConfigs {
    pub const MAX: usize = 32;

    pub fn new(configs: Vec<NetworkConfig>) -> Result<Self, InvalidTakeoverConfigs> {
        let twice =
            configs.iter().enumerate().any(|(i, c)| configs[..i].iter().any(|o| o.interface == c.interface));
        if configs.len() <= Self::MAX && !twice { Ok(Self(configs)) } else { Err(InvalidTakeoverConfigs) }
    }

    pub fn as_slice(&self) -> &[NetworkConfig] {
        &self.0
    }
}

impl TryFrom<Vec<NetworkConfig>> for TakeoverConfigs {
    type Error = InvalidTakeoverConfigs;

    fn try_from(value: Vec<NetworkConfig>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TakeoverConfigs> for Vec<NetworkConfig> {
    fn from(configs: TakeoverConfigs) -> Self {
        configs.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_checked_while_they_are_read() {
        let good = r#"{"interface":"enp1s0","ipv4":{"mode":"static","address":"192.168.1.10","prefix":24,"gateway":"192.168.1.1"},"ipv6":"off","dns":["192.168.1.1"]}"#;
        let config: NetworkConfig = serde_json::from_str(good).unwrap();
        assert_eq!(config.interface.as_str(), "enp1s0");
        for bad in [
            good.replace("enp1s0", "enp1s0\\n[Network]"),
            good.replace("enp1s0", "lo"),
            good.replace("\"prefix\":24", "\"prefix\":0"),
            good.replace("192.168.1.10", "192.168.1.10/24"),
            good.replace(r#"["192.168.1.1"]"#, r#"["1.1.1.1","1.0.0.1","8.8.8.8","9.9.9.9"]"#),
            good.replace(r#""ipv6":"off""#, r#""ipv6":"off","extra":1"#),
        ] {
            assert!(serde_json::from_str::<NetworkConfig>(&bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<ConfirmWithin>("10").is_err());
        assert_eq!(serde_json::from_str::<ConfirmWithin>("120").unwrap().get(), 120);

        // DHCP may name the address to ask for; off takes nothing.
        let dhcp: Ipv4Config = serde_json::from_str(r#"{"mode":"dhcp"}"#).unwrap();
        assert_eq!(dhcp, Ipv4Config::Dhcp { request: None });
        let asking: Ipv4Config =
            serde_json::from_str(r#"{"mode":"dhcp","request":"192.168.1.127"}"#).unwrap();
        assert_eq!(asking, Ipv4Config::Dhcp { request: Some("192.168.1.127".parse().unwrap()) });
        assert_eq!(serde_json::from_str::<Ipv4Config>(r#"{"mode":"off"}"#).unwrap(), Ipv4Config::Off {});
        assert!(serde_json::from_str::<Ipv4Config>(r#"{"mode":"off","address":"10.0.0.1"}"#).is_err());
    }

    #[test]
    fn a_takeover_names_each_interface_once_and_the_files_it_read() {
        let config = |name: &str| {
            NetworkConfig::addresses(
                InterfaceName::parse(name).unwrap(),
                Ipv4Config::Dhcp { request: None },
                Ipv6Config::Automatic,
                NameServers::default(),
            )
        };
        assert!(TakeoverConfigs::new(vec![config("enp2s0"), config("enp3s0")]).is_ok());
        assert!(TakeoverConfigs::new(vec![config("enp2s0"), config("enp2s0")]).is_err());
        let many = (0..33).map(|i| config(&format!("eth{i}"))).collect();
        assert!(TakeoverConfigs::new(many).is_err());

        let digest = "ab".repeat(32);
        assert!(FilesDigest::parse(&digest).is_ok());
        for bad in ["AB".repeat(32), "ab".repeat(31), format!("{}/x", "a".repeat(62)), String::new()] {
            assert!(FilesDigest::parse(&bad).is_err(), "{bad}");
        }
        let change: NetworkChange =
            serde_json::from_str(r#"{"kind":"takeover","interfaces":["enp2s0"]}"#).unwrap();
        assert_eq!(
            change,
            NetworkChange::Takeover { interfaces: vec![InterfaceName::parse("enp2s0").unwrap()] }
        );
    }

    fn name(text: &str) -> InterfaceName {
        InterfaceName::parse(text).unwrap()
    }

    /// A port has no addresses of its own, and nothing is a port of itself
    /// or carries itself as a VLAN.
    #[test]
    fn an_interfaces_file_is_its_addresses_or_a_port() {
        let port = r#"{"interface":"enp1s0","ipv4":{"mode":"off"},"ipv6":"off","dns":[],"port_of":{"kind":"bridge","name":"br0"},"vlans":["enp1s0.10"]}"#;
        let config: NetworkConfig = serde_json::from_str(port).unwrap();
        assert_eq!(config.port_of, Some(PortOf::Bridge { name: name("br0") }));
        assert_eq!(config.vlans.as_slice(), [name("enp1s0.10")]);
        assert!(!config.switched_off(), "a port is up");
        for bad in [
            port.replace(r#"{"mode":"off"}"#, r#"{"mode":"dhcp"}"#),
            port.replace(r#""ipv6":"off""#, r#""ipv6":"automatic""#),
            port.replace(r#""dns":[]"#, r#""dns":["9.9.9.9"]"#),
            port.replace(r#""name":"br0""#, r#""name":"enp1s0""#),
            port.replace("enp1s0.10", "enp1s0"),
            port.replace(r#"["enp1s0.10"]"#, r#"["enp1s0.10","enp1s0.10"]"#),
        ] {
            assert!(serde_json::from_str::<NetworkConfig>(&bad).is_err(), "{bad}");
        }
        // Earlier files (a takeover's record) have none of the new fields.
        let plain: NetworkConfig =
            serde_json::from_str(r#"{"interface":"enp1s0","ipv4":{"mode":"off"},"ipv6":"off","dns":[]}"#)
                .unwrap();
        assert!(plain.switched_off() && plain.port_of.is_none() && !plain.keep_up);
        let kept = NetworkConfig { keep_up: true, ..plain };
        assert!(!kept.switched_off(), "a device made here stays up without addresses");
    }

    #[test]
    fn devices_have_plain_names_macs_and_ids() {
        assert_eq!(MacAddress::parse("52:54:00:AB:cd:01").unwrap().as_str(), "52:54:00:ab:cd:01");
        for bad in [
            "01:00:5e:00:00:01",
            "00:00:00:00:00:00",
            "52:54:00:ab:cd",
            "52-54-00-ab-cd-01",
            "52:54:00:ab:cd:0g",
        ] {
            assert!(MacAddress::parse(bad).is_err(), "{bad}");
        }
        assert!(VlanId::new(1).is_ok() && VlanId::new(4094).is_ok());
        assert!(VlanId::new(0).is_err() && VlanId::new(4095).is_err());
        let bridge = r#"{"name":"br0","kind":{"kind":"bridge","stp":false},"mac":"52:54:00:12:34:56","members":["enp1s0","bond0"]}"#;
        let netdev: Netdev = serde_json::from_str(bridge).unwrap();
        assert_eq!(netdev.members, [name("enp1s0"), name("bond0")]);
        let bond: Netdev = serde_json::from_str(
            r#"{"name":"bond0","kind":{"kind":"bond","mode":"lacp"},"members":["enp2s0"]}"#,
        )
        .unwrap();
        assert_eq!(bond.kind, NetdevKind::Bond { mode: BondMode::Lacp });
        for bad in [
            bridge.replace(r#""name":"br0""#, r#""name":"docker0""#),
            bridge.replace(r#""name":"br0""#, r#""name":"br-0123456789ab""#),
            bridge.replace(r#""name":"br0""#, r#""name":"veth9""#),
            bridge.replace(r#""bond0""#, r#""br0""#),
            bridge.replace(r#""bond0""#, r#""enp1s0""#),
            r#"{"name":"vlan10","kind":{"kind":"vlan","id":10},"members":["enp1s0"]}"#.into(),
            r#"{"name":"vlan10","kind":{"kind":"vlan","id":4095}}"#.into(),
        ] {
            assert!(serde_json::from_str::<Netdev>(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_change_names_each_device_and_interface_once() {
        let change = r#"{"kind":"device","device":"br0","action":"made"}"#;
        let edit = format!(
            r#"{{"change":{change},"netdevs":[{{"name":"br0","kind":{{"kind":"bridge","stp":false}}}}],"links":[{{"interface":"enp1s0","ipv4":{{"mode":"off"}},"ipv6":"off","dns":[],"port_of":{{"kind":"bridge","name":"br0"}}}}]}}"#
        );
        let parsed: NetworkEdit = serde_json::from_str(&edit).unwrap();
        assert_eq!(parsed.change, NetworkChange::Device { device: name("br0"), action: DeviceAction::Made });
        assert_eq!((parsed.netdevs.len(), parsed.removed.len(), parsed.links.len()), (1, 0, 1));
        let nothing = format!(r#"{{"change":{change}}}"#);
        let twice = format!(
            r#"{{"change":{change},"netdevs":[{{"name":"br0","kind":{{"kind":"bridge","stp":false}}}}],"removed":["br0"]}}"#
        );
        let gone = format!(
            r#"{{"change":{change},"removed":["br0"],"links":[{{"interface":"br0","ipv4":{{"mode":"off"}},"ipv6":"off","dns":[]}}]}}"#
        );
        let dockers = format!(r#"{{"change":{change},"removed":["docker0"]}}"#);
        for bad in [nothing, twice, gone, dockers] {
            assert!(serde_json::from_str::<NetworkEdit>(&bad).is_err(), "{bad}");
        }
    }
}
