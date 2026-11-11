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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub interface: InterfaceName,
    pub ipv4: Ipv4Config,
    pub ipv6: Ipv6Config,
    pub dns: NameServers,
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
        let config = |name: &str| NetworkConfig {
            interface: InterfaceName::parse(name).unwrap(),
            ipv4: Ipv4Config::Dhcp { request: None },
            ipv6: Ipv6Config::Automatic,
            dns: NameServers::default(),
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
}
