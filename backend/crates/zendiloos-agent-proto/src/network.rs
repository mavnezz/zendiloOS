//! Network settings the agent writes as a systemd-networkd file. Every
//! value is checked while it is read, so the file can only say what these
//! types allow.

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
    Dhcp,
    Static { address: Ipv4Addr, prefix: Prefix, gateway: Option<Ipv4Addr> },
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

/// A change that waits to be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingNetwork {
    pub interface: InterfaceName,
    /// When the old configuration returns, in Unix seconds.
    pub revert_at: i64,
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
    }
}
