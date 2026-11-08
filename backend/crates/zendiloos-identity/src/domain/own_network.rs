//! Whether a client is on the NAS's own network: where an open setup may be
//! completed from (a machine installed from zendiloOS's ISO has no setup
//! code; whoever comes first from here becomes its administrator).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Loopback, the private ranges of IPv4 (RFC 1918), link-local addresses,
/// the shared range of carrier-grade NAT (100.64.0.0/10, which VPNs such as
/// Tailscale use too), and IPv6's unique local (fc00::/7) and link-local
/// (fe80::/10) addresses; IPv4 addresses written as IPv6 count as IPv4.
/// Nothing reachable from the internet as it is.
pub fn is_own_network(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => own_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => own_v4(v4),
            None => own_v6(v6),
        },
    }
}

fn own_v4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        // 100.64.0.0/10
        || (a == 100 && (64..128).contains(&b))
}

fn own_v6(address: Ipv6Addr) -> bool {
    let first = address.segments()[0];
    address.is_loopback()
        // fc00::/7
        || first & 0xfe00 == 0xfc00
        // fe80::/10
        || first & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_of_the_own_network_count() {
        let own = [
            "127.0.0.1",
            "127.8.9.10",
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.20",
            "169.254.12.34",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "fd12:3456:789a::1",
            "fc00::1",
            "fe80::1",
            "febf::1",
            "::ffff:192.168.1.20",
            "::ffff:10.0.2.2",
        ];
        let elsewhere = [
            "8.8.8.8",
            "172.15.255.255",
            "172.32.0.1",
            "192.169.0.1",
            "192.0.2.1",
            "100.63.255.255",
            "100.128.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "2001:db8::1",
            "2a01:4f8::1",
            "fec0::1",
            "ff02::1",
            "::",
            "::ffff:8.8.8.8",
        ];
        for address in own {
            assert!(is_own_network(address.parse().unwrap()), "{address} is on the own network");
        }
        for address in elsewhere {
            assert!(!is_own_network(address.parse().unwrap()), "{address} is not");
        }
    }
}
