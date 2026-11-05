//! How an interface should be configured, and the systemd-networkd file
//! that says so. Only where systemd-networkd manages the network; on a
//! Debian with ifupdown or NetworkManager the network is only shown.

use std::net::{IpAddr, Ipv4Addr};

/// How an interface gets its IPv4 address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ipv4Setting {
    Dhcp,
    Static { address: Ipv4Addr, prefix: u8, gateway: Option<Ipv4Addr> },
}

/// IPv6: from router advertisements (and DHCPv6 where they say so), or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6Setting {
    Automatic,
    Off,
}

/// The configuration of one interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceSettings {
    pub interface: String,
    pub ipv4: Ipv4Setting,
    pub ipv6: Ipv6Setting,
    /// Name servers; empty: those DHCP and router advertisements give.
    pub dns: Vec<IpAddr>,
}

/// The most name servers; resolvers use three at most.
pub const MAX_DNS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSettings {
    #[error("not an interface name")]
    Interface,
    #[error("a static address needs a prefix length from 1 to 32")]
    Prefix,
    #[error("{0} cannot be the NAS's address")]
    Address(Ipv4Addr),
    #[error("the gateway {0} is not in the address's network, or is the address itself")]
    Gateway(Ipv4Addr),
    #[error("at most {MAX_DNS} name servers, each once")]
    TooManyDns,
    #[error("{0} cannot be a name server")]
    Dns(IpAddr),
}

/// Interface names as the kernel allows them, and nothing a file could
/// misread.
pub fn is_interface_name(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "lo"
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.starts_with(['-', '.'])
}

fn mask(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix)) }
}

impl InterfaceSettings {
    pub fn checked(self) -> Result<Self, InvalidSettings> {
        if !is_interface_name(&self.interface) {
            return Err(InvalidSettings::Interface);
        }
        if let Ipv4Setting::Static { address, prefix, gateway } = self.ipv4 {
            if !(1..=32).contains(&prefix) {
                return Err(InvalidSettings::Prefix);
            }
            let bits = u32::from(address);
            let host = bits & !mask(prefix);
            let edge = prefix < 31 && (host == 0 || host == !mask(prefix));
            if address.is_unspecified()
                || address.is_loopback()
                || address.is_multicast()
                || address.is_broadcast()
                || address.is_link_local()
                || edge
            {
                return Err(InvalidSettings::Address(address));
            }
            if let Some(gateway) = gateway {
                let same_network = u32::from(gateway) & mask(prefix) == bits & mask(prefix);
                if gateway == address || !same_network || gateway.is_unspecified() || gateway.is_broadcast() {
                    return Err(InvalidSettings::Gateway(gateway));
                }
            }
        }
        if self.dns.len() > MAX_DNS || self.dns.iter().enumerate().any(|(i, ip)| self.dns[..i].contains(ip)) {
            return Err(InvalidSettings::TooManyDns);
        }
        if let Some(bad) = self.dns.iter().find(|ip| ip.is_unspecified() || ip.is_multicast()) {
            return Err(InvalidSettings::Dns(*bad));
        }
        Ok(self)
    }

    /// The `.network` file for systemd-networkd.
    pub fn networkd_file(&self) -> String {
        let mut file = format!(
            "# Written by zendiloOS; changes here are replaced. Change the network in its interface.\n\
             [Match]\nName={}\n\n[Network]\n",
            self.interface
        );
        let dhcp = self.ipv4 == Ipv4Setting::Dhcp;
        file.push_str(if dhcp { "DHCP=ipv4\n" } else { "DHCP=no\n" });
        match self.ipv6 {
            Ipv6Setting::Automatic => file.push_str("IPv6AcceptRA=yes\n"),
            Ipv6Setting::Off => file.push_str("IPv6AcceptRA=no\nLinkLocalAddressing=no\n"),
        }
        if let Ipv4Setting::Static { address, prefix, gateway } = &self.ipv4 {
            file.push_str(&format!("Address={address}/{prefix}\n"));
            if let Some(gateway) = gateway {
                file.push_str(&format!("Gateway={gateway}\n"));
            }
        }
        for server in &self.dns {
            file.push_str(&format!("DNS={server}\n"));
        }
        // Name servers given here replace those DHCP or the router offers.
        if !self.dns.is_empty() {
            if dhcp {
                file.push_str("\n[DHCPv4]\nUseDNS=no\n");
            }
            if self.ipv6 == Ipv6Setting::Automatic {
                file.push_str("\n[IPv6AcceptRA]\nUseDNS=no\n");
            }
        }
        file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(address: &str, prefix: u8, gateway: Option<&str>) -> InterfaceSettings {
        InterfaceSettings {
            interface: "enp1s0".into(),
            ipv4: Ipv4Setting::Static {
                address: address.parse().unwrap(),
                prefix,
                gateway: gateway.map(|g| g.parse().unwrap()),
            },
            ipv6: Ipv6Setting::Automatic,
            dns: vec!["192.168.1.1".parse().unwrap()],
        }
    }

    #[test]
    fn static_addresses_are_checked() {
        assert!(fixed("192.168.1.10", 24, Some("192.168.1.1")).checked().is_ok());
        assert!(fixed("10.0.0.5", 31, None).checked().is_ok(), "a point-to-point /31 has no edges");
        let refused = |settings: InterfaceSettings| settings.checked().unwrap_err();
        assert_eq!(refused(fixed("192.168.1.10", 0, None)), InvalidSettings::Prefix);
        assert_eq!(refused(fixed("192.168.1.10", 33, None)), InvalidSettings::Prefix);
        for bad in ["192.168.1.0", "192.168.1.255", "127.0.0.1", "169.254.1.1", "224.0.0.1", "0.0.0.0"] {
            assert_eq!(
                refused(fixed(bad, 24, None)),
                InvalidSettings::Address(bad.parse().unwrap()),
                "{bad}"
            );
        }
        for gateway in ["192.168.2.1", "192.168.1.10"] {
            let settings = fixed("192.168.1.10", 24, Some(gateway));
            assert_eq!(refused(settings), InvalidSettings::Gateway(gateway.parse().unwrap()), "{gateway}");
        }
        let mut many = fixed("192.168.1.10", 24, None);
        many.dns = ["1.1.1.1", "8.8.8.8", "9.9.9.9", "1.0.0.1"].iter().map(|s| s.parse().unwrap()).collect();
        assert_eq!(refused(many), InvalidSettings::TooManyDns);
        let mut name = fixed("192.168.1.10", 24, None);
        name.interface = "eth0\n[Network]".into();
        assert_eq!(refused(name), InvalidSettings::Interface);
    }

    #[test]
    fn the_networkd_file_says_exactly_that() {
        let file = fixed("192.168.1.10", 24, Some("192.168.1.1")).networkd_file();
        assert!(file.contains("[Match]\nName=enp1s0\n"));
        assert!(file.contains(
            "DHCP=no\nIPv6AcceptRA=yes\nAddress=192.168.1.10/24\nGateway=192.168.1.1\nDNS=192.168.1.1\n"
        ));
        assert!(file.ends_with("\n[IPv6AcceptRA]\nUseDNS=no\n"), "{file}");

        let dhcp = InterfaceSettings {
            interface: "enp1s0".into(),
            ipv4: Ipv4Setting::Dhcp,
            ipv6: Ipv6Setting::Off,
            dns: vec![],
        };
        let file = dhcp.networkd_file();
        assert!(file.contains("DHCP=ipv4\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n"), "{file}");
        assert!(!file.contains("DNS") && !file.contains("Address="), "{file}");
    }
}
