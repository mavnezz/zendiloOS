//! How the web interface defends itself: when idle sessions end, when
//! accounts lock after failed logins, and which addresses it refuses.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use zendiloos_kernel::Timestamp;

/// An address or a network of them: `192.168.1.20`, `192.168.1.0/24`,
/// `fd00::/8`. The host bits of a network are cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AddressRange {
    network: IpAddr,
    prefix: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not an IP address or network such as 192.168.1.0/24")]
pub struct InvalidAddressRange(pub String);

fn bits(ip: IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

/// `ip` with all but the first `prefix` bits cleared.
fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
            IpAddr::from((u32::from(v4) & mask).to_be_bytes())
        }
        IpAddr::V6(v6) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
            IpAddr::from((u128::from(v6) & mask).to_be_bytes())
        }
    }
}

impl AddressRange {
    /// Just `ip`.
    pub fn single(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        Self { network: ip, prefix: bits(ip) }
    }

    /// What one client counts as: an IPv4 address, or the /64 network of
    /// an IPv6 one, which a single host is commonly given whole and could
    /// otherwise step through, one address per attempt.
    pub fn client(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        let prefix = if ip.is_ipv4() { 32 } else { 64 };
        Self { network: masked(ip, prefix), prefix }
    }

    pub fn parse(text: &str) -> Result<Self, InvalidAddressRange> {
        let invalid = || InvalidAddressRange(text.to_owned());
        let text = text.trim();
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let ip = address.parse::<IpAddr>().map_err(|_| invalid())?.to_canonical();
        let prefix = match prefix {
            None => bits(ip),
            Some(prefix) if prefix.bytes().all(|b| b.is_ascii_digit()) && !prefix.is_empty() => {
                prefix.parse::<u8>().ok().filter(|p| *p <= bits(ip)).ok_or_else(invalid)?
            }
            Some(_) => return Err(invalid()),
        };
        Ok(Self { network: masked(ip, prefix), prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        ip.is_ipv4() == self.network.is_ipv4() && masked(ip, self.prefix) == self.network
    }

    /// The address, or the network's first one.
    pub fn network(&self) -> IpAddr {
        self.network
    }

    /// One address, not a network.
    pub fn is_single(&self) -> bool {
        self.prefix == bits(self.network)
    }
}

impl fmt::Display for AddressRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_single() {
            write!(f, "{}", self.network)
        } else {
            write!(f, "{}/{}", self.network, self.prefix)
        }
    }
}

/// So many failed logins within so many minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureLimit {
    pub failures: u16,
    pub window_minutes: u16,
}

impl FailureLimit {
    pub fn window(&self) -> Duration {
        Duration::from_secs(u64::from(self.window_minutes) * 60)
    }

    fn checked(self) -> Result<Self, InvalidSecuritySettings> {
        if !(3..=100).contains(&self.failures) {
            return Err(InvalidSecuritySettings::Failures);
        }
        if !(1..=1440).contains(&self.window_minutes) {
            return Err(InvalidSecuritySettings::Window);
        }
        Ok(self)
    }
}

/// Addresses that fail too often are blocked, for so many days or until
/// someone lifts the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoBlock {
    pub limit: FailureLimit,
    /// `None`: until lifted by hand.
    pub release_after_days: Option<u16>,
}

/// At most this many addresses and networks are never blocked.
pub const MAX_ALLOWED: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecuritySettings {
    /// A session ends after this many minutes without requests.
    pub idle_minutes: u16,
    /// Accounts lock for a while after failed logins; `None`: never.
    pub account_lock: Option<FailureLimit>,
    /// `None`: no address is blocked automatically.
    pub auto_block: Option<AutoBlock>,
    /// Never blocked, automatically or by hand.
    pub allowed: Vec<AddressRange>,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            idle_minutes: 30,
            account_lock: Some(FailureLimit { failures: 5, window_minutes: 15 }),
            auto_block: Some(AutoBlock {
                limit: FailureLimit { failures: 10, window_minutes: 5 },
                release_after_days: Some(1),
            }),
            allowed: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSecuritySettings {
    #[error("sessions end after 5 to 1440 idle minutes")]
    IdleMinutes,
    #[error("between 3 and 100 failed attempts")]
    Failures,
    #[error("within 1 to 1440 minutes")]
    Window,
    #[error("blocks are lifted after 1 to 365 days")]
    ReleaseAfter,
    #[error("at most 64 addresses and networks are allowed")]
    TooManyAllowed,
}

impl SecuritySettings {
    /// The settings, if they hold up; addresses allowed twice once.
    pub fn checked(mut self) -> Result<Self, InvalidSecuritySettings> {
        if !(5..=1440).contains(&self.idle_minutes) {
            return Err(InvalidSecuritySettings::IdleMinutes);
        }
        self.account_lock = self.account_lock.map(FailureLimit::checked).transpose()?;
        if let Some(block) = self.auto_block {
            block.limit.checked()?;
            if block.release_after_days.is_some_and(|days| !(1..=365).contains(&days)) {
                return Err(InvalidSecuritySettings::ReleaseAfter);
            }
        }
        let mut seen = Vec::with_capacity(self.allowed.len());
        self.allowed.retain(|range| {
            let new = !seen.contains(range);
            seen.push(*range);
            new
        });
        if self.allowed.len() > MAX_ALLOWED {
            return Err(InvalidSecuritySettings::TooManyAllowed);
        }
        Ok(self)
    }

    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.idle_minutes) * 60)
    }

    /// Never blocked: the allowed ones, and this machine itself.
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        ip.to_canonical().is_loopback() || self.allowed.iter().any(|range| range.contains(ip))
    }
}

/// An address or network the web interface refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedAddress {
    pub range: AddressRange,
    pub blocked_at: Timestamp,
    /// `None`: until lifted by hand.
    pub until: Option<Timestamp>,
    /// Blocked after failed logins, not by hand.
    pub automatic: bool,
}

impl BlockedAddress {
    pub fn is_over(&self, now: Timestamp) -> bool {
        self.until.is_some_and(|until| now >= until)
    }
}

/// Whether requests from `ip` are refused.
pub fn is_blocked(
    ip: IpAddr,
    blocked: &[BlockedAddress],
    settings: &SecuritySettings,
    now: Timestamp,
) -> bool {
    !settings.is_allowed(ip) && blocked.iter().any(|block| !block.is_over(now) && block.range.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(text: &str) -> AddressRange {
        AddressRange::parse(text).unwrap()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_client_is_an_ipv4_address_or_an_ipv6_network() {
        let v4: IpAddr = "192.0.2.7".parse().unwrap();
        assert_eq!(AddressRange::client(v4), AddressRange::single(v4));
        let v6: IpAddr = "2001:db8:0:1:aaaa:bbbb:cccc:dddd".parse().unwrap();
        assert_eq!(AddressRange::client(v6).to_string(), "2001:db8:0:1::/64");
        assert!(AddressRange::client(v6).contains("2001:db8:0:1::99".parse().unwrap()));
        assert!(!AddressRange::client(v6).contains("2001:db8:0:2::1".parse().unwrap()));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(AddressRange::client(mapped), AddressRange::single(v4), "a v4 address seen over v6");
    }
    #[test]
    fn ranges_are_addresses_or_networks() {
        assert_eq!(range(" 192.168.1.20 ").to_string(), "192.168.1.20");
        assert_eq!(range("192.168.1.20/24").to_string(), "192.168.1.0/24", "host bits cleared");
        assert_eq!(range("fd00::1/8").to_string(), "fd00::/8");
        assert_eq!(range("::ffff:10.0.0.1").to_string(), "10.0.0.1", "mapped IPv4 is IPv4");
        assert_eq!(range("0.0.0.0/0").to_string(), "0.0.0.0/0");
        for bad in ["", "nas", "192.168.1.300", "10.0.0.0/33", "10.0.0.0/", "10.0.0.0/+8", "fd00::/129"] {
            assert!(AddressRange::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn networks_contain_their_addresses() {
        assert!(range("192.168.1.0/24").contains(ip("192.168.1.77")));
        assert!(!range("192.168.1.0/24").contains(ip("192.168.2.1")));
        assert!(range("192.168.1.0/24").contains(ip("::ffff:192.168.1.5")));
        assert!(range("fd00::/8").contains(ip("fd12::5")));
        assert!(!range("fd00::/8").contains(ip("192.168.1.5")));
        assert!(range("10.0.0.0/0").contains(ip("8.8.8.8")));
        assert!(range("10.1.2.3").contains(ip("10.1.2.3")) && !range("10.1.2.3").contains(ip("10.1.2.4")));
    }

    #[test]
    fn settings_are_checked() {
        let fine = SecuritySettings::default();
        assert_eq!(fine.clone().checked(), Ok(fine.clone()));
        let idle = SecuritySettings { idle_minutes: 4, ..fine.clone() };
        assert_eq!(idle.checked(), Err(InvalidSecuritySettings::IdleMinutes));
        let lock = SecuritySettings {
            account_lock: Some(FailureLimit { failures: 2, window_minutes: 5 }),
            ..fine.clone()
        };
        assert_eq!(lock.checked(), Err(InvalidSecuritySettings::Failures));
        let block = SecuritySettings {
            auto_block: Some(AutoBlock {
                limit: FailureLimit { failures: 10, window_minutes: 5 },
                release_after_days: Some(0),
            }),
            ..fine.clone()
        };
        assert_eq!(block.checked(), Err(InvalidSecuritySettings::ReleaseAfter));
        let twice = SecuritySettings { allowed: vec![range("10.0.0.0/8"), range("10.0.0.0/8")], ..fine };
        assert_eq!(twice.checked().unwrap().allowed, [range("10.0.0.0/8")]);
    }

    #[test]
    fn allowed_addresses_and_this_machine_are_never_blocked() {
        let now = Timestamp::from_unix_seconds(100);
        let block = |text: &str, until: Option<i64>| BlockedAddress {
            range: range(text),
            blocked_at: Timestamp::from_unix_seconds(0),
            until: until.map(Timestamp::from_unix_seconds),
            automatic: false,
        };
        let blocked = [block("192.168.1.0/24", None), block("10.0.0.9", Some(100)), block("127.0.0.1", None)];
        let settings =
            SecuritySettings { allowed: vec![range("192.168.1.5")], ..SecuritySettings::default() };

        assert!(is_blocked(ip("192.168.1.6"), &blocked, &settings, now));
        assert!(!is_blocked(ip("192.168.1.5"), &blocked, &settings, now), "allowed wins");
        assert!(!is_blocked(ip("10.0.0.9"), &blocked, &settings, now), "the block is over");
        assert!(!is_blocked(ip("127.0.0.1"), &blocked, &settings, now), "this machine");
        assert!(!is_blocked(ip("172.16.0.1"), &blocked, &settings, now));
    }
}
