//! The firewall's rules as the agent writes them for nftables. Every value
//! is checked while it is read: an interface name, an address or a port
//! reaches the rules file only in a form these types allow.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::InterfaceName;

/// What happens to a new connection a rule (or no rule) matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallVerdict {
    Accept,
    Drop,
}

/// Ports from `from` to `to`, both included; 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PortRangeFields", into = "PortRangeFields")]
pub struct FirewallPortRange {
    from: u16,
    to: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortRangeFields {
    from: u16,
    to: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ports run from 1 to 65535, the first not after the last")]
pub struct InvalidFirewallPorts;

impl FirewallPortRange {
    pub fn new(from: u16, to: u16) -> Result<Self, InvalidFirewallPorts> {
        if from == 0 || from > to { Err(InvalidFirewallPorts) } else { Ok(Self { from, to }) }
    }

    pub fn from(self) -> u16 {
        self.from
    }

    pub fn to(self) -> u16 {
        self.to
    }
}

impl TryFrom<PortRangeFields> for FirewallPortRange {
    type Error = InvalidFirewallPorts;

    fn try_from(fields: PortRangeFields) -> Result<Self, Self::Error> {
        Self::new(fields.from, fields.to)
    }
}

impl From<FirewallPortRange> for PortRangeFields {
    fn from(range: FirewallPortRange) -> Self {
        Self { from: range.from, to: range.to }
    }
}

/// The ports of one rule: 1 to [`FirewallPorts::MAX`] ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<FirewallPortRange>", into = "Vec<FirewallPortRange>")]
pub struct FirewallPorts(Vec<FirewallPortRange>);

impl FirewallPorts {
    /// Enough for a project publishing many ports one by one.
    pub const MAX: usize = 512;

    pub fn new(ranges: Vec<FirewallPortRange>) -> Result<Self, InvalidFirewallPorts> {
        if (1..=Self::MAX).contains(&ranges.len()) { Ok(Self(ranges)) } else { Err(InvalidFirewallPorts) }
    }

    pub fn ranges(&self) -> &[FirewallPortRange] {
        &self.0
    }
}

impl TryFrom<Vec<FirewallPortRange>> for FirewallPorts {
    type Error = InvalidFirewallPorts;

    fn try_from(ranges: Vec<FirewallPortRange>) -> Result<Self, Self::Error> {
        Self::new(ranges)
    }
}

impl From<FirewallPorts> for Vec<FirewallPortRange> {
    fn from(ports: FirewallPorts) -> Self {
        ports.0
    }
}

/// What a rule matches beyond interface and source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FirewallTraffic {
    All {},
    /// ICMP and ICMPv6 echo requests.
    Ping {},
    Tcp {
        ports: FirewallPorts,
    },
    Udp {
        ports: FirewallPorts,
    },
}

/// Who a rule is about: anyone, a network (a single address has the full
/// prefix length), or a range of one family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SourceFields", into = "SourceFields")]
pub enum FirewallSource {
    Any,
    Network { address: IpAddr, prefix: u8 },
    Range { from: IpAddr, to: IpAddr },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceFields {
    Any {},
    Network { address: IpAddr, prefix: u8 },
    Range { from: IpAddr, to: IpAddr },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an address, network or range of addresses")]
pub struct InvalidFirewallSource;

impl FirewallSource {
    /// A network: `prefix` at most the address's length, its host bits
    /// zero.
    pub fn network(address: IpAddr, prefix: u8) -> Result<Self, InvalidFirewallSource> {
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let host_bits_zero = match address {
            IpAddr::V4(v4) => prefix >= 32 || u32::from(v4).checked_shl(u32::from(prefix)).unwrap_or(0) == 0,
            IpAddr::V6(v6) => {
                prefix >= 128 || u128::from(v6).checked_shl(u32::from(prefix)).unwrap_or(0) == 0
            }
        };
        if prefix <= bits && host_bits_zero {
            Ok(Self::Network { address, prefix })
        } else {
            Err(InvalidFirewallSource)
        }
    }

    /// A range of one family, `from` not after `to`.
    pub fn range(from: IpAddr, to: IpAddr) -> Result<Self, InvalidFirewallSource> {
        if from.is_ipv4() == to.is_ipv4() && from <= to {
            Ok(Self::Range { from, to })
        } else {
            Err(InvalidFirewallSource)
        }
    }

    /// The family it is about: `None` for anyone.
    pub fn is_ipv4(self) -> Option<bool> {
        match self {
            Self::Any => None,
            Self::Network { address, .. } => Some(address.is_ipv4()),
            Self::Range { from, .. } => Some(from.is_ipv4()),
        }
    }
}

impl TryFrom<SourceFields> for FirewallSource {
    type Error = InvalidFirewallSource;

    fn try_from(fields: SourceFields) -> Result<Self, Self::Error> {
        match fields {
            SourceFields::Any {} => Ok(Self::Any),
            SourceFields::Network { address, prefix } => Self::network(address, prefix),
            SourceFields::Range { from, to } => Self::range(from, to),
        }
    }
}

impl From<FirewallSource> for SourceFields {
    fn from(source: FirewallSource) -> Self {
        match source {
            FirewallSource::Any => Self::Any {},
            FirewallSource::Network { address, prefix } => Self::Network { address, prefix },
            FirewallSource::Range { from, to } => Self::Range { from, to },
        }
    }
}

/// One rule as nftables enforces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirewallRule {
    /// None: every interface.
    pub interface: Option<InterfaceName>,
    pub source: FirewallSource,
    pub traffic: FirewallTraffic,
    /// Also on connections Docker forwards to containers, matched by the
    /// port they were sent to (before Docker's address translation).
    pub forward: bool,
    pub verdict: FirewallVerdict,
}

/// The rules of one ruleset: at most [`FirewallRules::MAX`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<FirewallRule>", into = "Vec<FirewallRule>")]
pub struct FirewallRules(Vec<FirewallRule>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most 1024 rules")]
pub struct TooManyFirewallRules;

impl FirewallRules {
    pub const MAX: usize = 1024;

    pub fn new(rules: Vec<FirewallRule>) -> Result<Self, TooManyFirewallRules> {
        if rules.len() <= Self::MAX { Ok(Self(rules)) } else { Err(TooManyFirewallRules) }
    }

    pub fn iter(&self) -> impl Iterator<Item = &FirewallRule> {
        self.0.iter()
    }
}

impl TryFrom<Vec<FirewallRule>> for FirewallRules {
    type Error = TooManyFirewallRules;

    fn try_from(rules: Vec<FirewallRule>) -> Result<Self, Self::Error> {
        Self::new(rules)
    }
}

impl From<FirewallRules> for Vec<FirewallRule> {
    fn from(rules: FirewallRules) -> Self {
        rules.0
    }
}

/// One of Docker's bridges: `docker0`, or `br-` and the 12 hex digits of
/// the network's id. Containers on them reach the NAS; a bridge of the
/// machine's own (`br-lan`) never passes for one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DockerBridge(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not one of Docker's bridges")]
pub struct NotADockerBridge;

impl DockerBridge {
    pub fn parse(name: &str) -> Result<Self, NotADockerBridge> {
        let docker = name == "docker0"
            || name
                .strip_prefix("br-")
                .is_some_and(|id| id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        if docker { Ok(Self(name.to_owned())) } else { Err(NotADockerBridge) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DockerBridge {
    type Error = NotADockerBridge;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::parse(&name)
    }
}

impl From<DockerBridge> for String {
    fn from(bridge: DockerBridge) -> Self {
        bridge.0
    }
}

/// The firewall's rules in order, and what happens when none matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirewallConfig {
    pub rules: FirewallRules,
    pub fallback: FirewallVerdict,
    /// Docker's bridges besides `docker0`, which always is one; at most
    /// [`FirewallConfig::MAX_BRIDGES`].
    #[serde(default, deserialize_with = "bridges")]
    pub bridges: Vec<DockerBridge>,
}

impl FirewallConfig {
    pub const MAX_BRIDGES: usize = 1024;
}

fn bridges<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<DockerBridge>, D::Error> {
    let bridges = Vec::<DockerBridge>::deserialize(deserializer)?;
    if bridges.len() > FirewallConfig::MAX_BRIDGES {
        return Err(serde::de::Error::custom("too many bridges"));
    }
    Ok(bridges)
}

/// The rules of the change waiting for confirmation, as they stand now
/// (none: no rules at all).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingFirewall {
    pub config: Option<FirewallConfig>,
}

/// A change of the firewall that waits to be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingFirewall {
    /// When the rules from before return, in Unix seconds.
    pub revert_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn values_are_checked_while_they_are_read() {
        assert!(FirewallPortRange::new(80, 80).is_ok());
        for (from, to) in [(0, 1), (9, 8)] {
            assert!(FirewallPortRange::new(from, to).is_err());
        }
        assert!(serde_json::from_str::<FirewallPortRange>(r#"{"from":0,"to":80}"#).is_err());
        assert!(serde_json::from_str::<FirewallPortRange>(r#"{"from":1,"to":2,"x":3}"#).is_err());
        assert!(serde_json::from_str::<FirewallPorts>("[]").is_err());
        let ports: FirewallPorts =
            serde_json::from_str(r#"[{"from":80,"to":80},{"from":8000,"to":8100}]"#).unwrap();
        assert_eq!((ports.ranges()[1].from(), ports.ranges()[1].to()), (8000, 8100));
        assert!(
            FirewallPorts::new(vec![FirewallPortRange::new(1, 1).unwrap(); FirewallPorts::MAX + 1]).is_err()
        );

        assert!(FirewallSource::network(ip("192.168.1.0"), 24).is_ok());
        assert!(FirewallSource::network(ip("192.168.1.5"), 32).is_ok());
        assert!(FirewallSource::network(ip("0.0.0.0"), 0).is_ok());
        assert!(FirewallSource::network(ip("fd00:1::"), 64).is_ok());
        for (address, prefix) in [("192.168.1.5", 24), ("10.0.0.0", 33), ("fd00::1", 64), ("::", 129)] {
            assert!(FirewallSource::network(ip(address), prefix).is_err(), "{address}/{prefix}");
        }
        assert!(FirewallSource::range(ip("10.0.0.1"), ip("10.0.0.9")).is_ok());
        assert!(FirewallSource::range(ip("10.0.0.9"), ip("10.0.0.1")).is_err());
        assert!(FirewallSource::range(ip("10.0.0.1"), ip("fd00::1")).is_err());
        let json = r#"{"kind":"network","address":"192.168.1.7","prefix":24}"#;
        assert!(serde_json::from_str::<FirewallSource>(json).is_err());
        assert_eq!(FirewallSource::Any.is_ipv4(), None);
        assert_eq!(FirewallSource::network(ip("fd00::"), 8).unwrap().is_ipv4(), Some(false));
        assert_eq!(FirewallSource::range(ip("1.1.1.1"), ip("1.1.1.2")).unwrap().is_ipv4(), Some(true));

        let rule = FirewallRule {
            interface: Some(InterfaceName::parse("enp1s0").unwrap()),
            source: FirewallSource::network(ip("192.168.1.0"), 24).unwrap(),
            traffic: FirewallTraffic::Tcp { ports },
            forward: false,
            verdict: FirewallVerdict::Accept,
        };
        let config = FirewallConfig {
            rules: FirewallRules::new(vec![rule.clone()]).unwrap(),
            fallback: FirewallVerdict::Drop,
            bridges: vec![DockerBridge::parse("br-0123456789ab").unwrap()],
        };
        let text = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<FirewallConfig>(&text).unwrap(), config);
        assert!(
            text.contains(r#""source":{"kind":"network","address":"192.168.1.0","prefix":24}"#),
            "{text}"
        );
        let sneaky = text.replace("enp1s0", "eth0\\\"; flush ruleset");
        assert!(serde_json::from_str::<FirewallConfig>(&sneaky).is_err());
        assert!(FirewallRules::new(vec![rule; FirewallRules::MAX + 1]).is_err());
        assert_eq!(config.rules.iter().count(), 1);
    }

    #[test]
    fn only_dockers_bridges_pass_for_dockers() {
        assert_eq!(DockerBridge::parse("docker0").unwrap().as_str(), "docker0");
        assert!(DockerBridge::parse("br-0123456789ab").is_ok());
        for odd in ["br-lan", "br-0123456789a", "br-0123456789abc", "br-0123456789ag", "eth0", "docker1", ""]
        {
            assert!(DockerBridge::parse(odd).is_err(), "{odd}");
        }
        let text = r#"{"rules":[],"fallback":"drop","bridges":["br-0123456789ab"]}"#;
        let config: FirewallConfig = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&config).unwrap(), text);
        assert!(serde_json::from_str::<FirewallConfig>(&text.replace("br-0123456789ab", "br-lan")).is_err());
        let without: FirewallConfig = serde_json::from_str(r#"{"rules":[],"fallback":"drop"}"#).unwrap();
        assert!(without.bridges.is_empty());
        let many = vec![r#""docker0""#; FirewallConfig::MAX_BRIDGES + 1].join(",");
        let many = format!(r#"{{"rules":[],"fallback":"drop","bridges":[{many}]}}"#);
        assert!(serde_json::from_str::<FirewallConfig>(&many).is_err());
    }
}
