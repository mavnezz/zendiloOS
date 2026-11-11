//! ifupdown's `/etc/network/interfaces` (interfaces(5)), read to take the
//! network over: what it sets up for each wired card, in the terms of
//! [`InterfaceSettings`], and every stanza and line beyond those, named. A
//! takeover happens only when nothing is refused, so nothing ifupdown did
//! is lost on the way.
//!
//! Understood: comments and lines continued with `\`; `auto`,
//! `allow-auto` and `allow-hotplug`; `source` of `interfaces.d/*` or one
//! file in it, and `source-directory` of `interfaces.d`; the loopback;
//! `inet dhcp` and `inet static` (address, netmask, gateway, name servers,
//! and broadcast and network where they say what the address does anyway);
//! `inet6 auto` and `inet6 dhcp`, which router advertisements do. Refused:
//! bonds, bridges, VLANs, wireless, commands run on the way up or down,
//! search domains, mappings, anything else.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use super::{InterfaceKind, InterfaceSettings, Ipv4Setting, Ipv6Setting, MAX_DNS, Network};

/// The main file, and the folder beside it, by their paths below
/// `/etc/network`.
const MAIN: &str = "interfaces";
const DIRECTORY: &str = "interfaces.d";
/// Files sourcing files: further than this, something is odd.
const MAX_DEPTH: usize = 4;

/// One of ifupdown's files, as the host hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfupdownFile {
    /// Below `/etc/network`: `interfaces`, `interfaces.d/<name>`.
    pub path: String,
    pub text: String,
    /// Why it was not read, if it was not.
    pub skipped: Option<String>,
}

/// Something a takeover would not carry over: where it is (a file and
/// line, `interfaces:12`, or an interface) and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub place: String,
    pub reason: String,
}

/// What taking the network over would set up, and what it would not
/// carry over; it happens only when nothing is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeoverPlan {
    /// By name.
    pub interfaces: Vec<InterfaceSettings>,
    pub refused: Vec<Refusal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Inet,
    Inet6,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Inet => "inet",
            Family::Inet6 => "inet6",
        }
    }
}

#[derive(Debug)]
struct Stanza {
    name: String,
    family: Family,
    method: String,
    place: String,
    /// Name (with `_` as `-`), value, place.
    options: Vec<(String, String, String)>,
}

/// The lines of a file as ifupdown reads them: comments and empty lines
/// left out, continued lines joined; each with the number of its first
/// line.
fn lines(text: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut joined: Option<(usize, String)> = None;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        if joined.is_none() && raw.trim_start().starts_with('#') {
            continue;
        }
        let (part, more) = match raw.strip_suffix('\\') {
            Some(part) => (part, true),
            None => (raw, false),
        };
        let line = joined.get_or_insert_with(|| (number, String::new()));
        line.1.push_str(part);
        line.1.push(' ');
        if !more && let Some((first, line)) = joined.take() {
            let line = line.trim().to_owned();
            if !line.is_empty() {
                lines.push((first, line));
            }
        }
    }
    if let Some((first, line)) = joined {
        let line = line.trim().to_owned();
        if !line.is_empty() {
            lines.push((first, line));
        }
    }
    lines
}

/// What run-parts takes from a folder: `source-directory`.
fn run_parts_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Reads the files ifupdown reads, from the main one through what it
/// sources.
struct Reader<'a> {
    files: &'a [IfupdownFile],
    stanzas: Vec<Stanza>,
    /// Interfaces ifupdown brings up by itself: `auto`, `allow-hotplug`.
    brought_up: BTreeSet<String>,
    refused: Vec<Refusal>,
    read: BTreeSet<String>,
}

enum Current {
    None,
    Stanza(usize),
    /// A stanza refused as a whole: its options are not looked at.
    Refused,
}

impl Reader<'_> {
    fn refuse(&mut self, place: &str, reason: impl Into<String>) {
        self.refused.push(Refusal { place: place.to_owned(), reason: reason.into() });
    }

    fn read(&mut self, path: &str, from: &str, depth: usize) {
        if !self.read.insert(path.to_owned()) {
            return;
        }
        let Some(file) = self.files.iter().find(|file| file.path == path) else {
            self.refuse(from, format!("{path} is not there"));
            return;
        };
        if let Some(why) = &file.skipped {
            self.refuse(path, format!("not read: {why}"));
            return;
        }
        let mut current = Current::None;
        for (number, line) in lines(&file.text) {
            let place = format!("{path}:{number}");
            let words: Vec<&str> = line.split_whitespace().collect();
            let (keyword, rest) = (words[0], &words[1..]);
            match keyword {
                "iface" => {
                    current = Current::Refused;
                    let [name, family, method] = rest else {
                        self.refuse(&place, "an iface line other than `iface <name> <family> <method>`");
                        continue;
                    };
                    let family = match *family {
                        "inet" => Family::Inet,
                        "inet6" => Family::Inet6,
                        other => {
                            self.refuse(&place, format!("{name}: the family {other} is not carried over"));
                            continue;
                        }
                    };
                    self.stanzas.push(Stanza {
                        name: (*name).to_owned(),
                        family,
                        method: (*method).to_owned(),
                        place,
                        options: Vec::new(),
                    });
                    current = Current::Stanza(self.stanzas.len() - 1);
                }
                "auto" | "allow-auto" | "allow-hotplug" => {
                    current = Current::None;
                    self.brought_up.extend(rest.iter().map(|name| (*name).to_owned()));
                }
                "source" | "source-directory" => {
                    current = Current::None;
                    let [target] = rest else {
                        self.refuse(&place, format!("{keyword} with other than one path"));
                        continue;
                    };
                    self.source(keyword, target, path, &place, depth);
                }
                "mapping" | "rename" | "no-auto-down" | "no-scripts" | "template" => {
                    current = Current::Refused;
                    self.refuse(&place, format!("{keyword} is not carried over"));
                }
                other if other.starts_with("allow-") => {
                    current = Current::None;
                    self.refuse(&place, format!("{other} is not carried over"));
                }
                option => match current {
                    Current::Stanza(index) => {
                        let name = option.replace('_', "-");
                        self.stanzas[index].options.push((name, rest.join(" "), place));
                    }
                    Current::Refused => {}
                    Current::None => self.refuse(&place, format!("{option} stands outside a stanza")),
                },
            }
        }
    }

    /// `source interfaces.d/*` and the like: only below `interfaces.d`,
    /// relative paths only from the main file (whose folder is
    /// `/etc/network`).
    fn source(&mut self, keyword: &str, target: &str, from: &str, place: &str, depth: usize) {
        if depth >= MAX_DEPTH {
            self.refuse(place, "sourced too deep");
            return;
        }
        let relative = match target.strip_prefix("/etc/network/") {
            Some(relative) => Some(relative),
            None if from == MAIN && !target.starts_with('/') => Some(target),
            None => None,
        };
        let directory = relative.map(|r| r.trim_end_matches('/'));
        let names: Vec<String> = self
            .files
            .iter()
            .filter_map(|file| file.path.strip_prefix(&format!("{DIRECTORY}/")).map(str::to_owned))
            .collect();
        let too_many = self.files.iter().find(|file| file.path == DIRECTORY).and_then(|f| f.skipped.clone());
        let chosen: Vec<String> = match (keyword, relative, directory) {
            ("source-directory", _, Some(DIRECTORY)) => {
                names.into_iter().filter(|name| run_parts_name(name)).collect()
            }
            ("source", Some(glob), _) if glob == format!("{DIRECTORY}/*") => {
                names.into_iter().filter(|name| !name.starts_with('.')).collect()
            }
            ("source", Some(one), _)
                if one.strip_prefix(&format!("{DIRECTORY}/")).is_some_and(|name| {
                    !name.is_empty() && !name.contains(['/', '*', '?', '[']) && name != "." && name != ".."
                }) =>
            {
                let name = &one[DIRECTORY.len() + 1..];
                vec![name.to_owned()]
            }
            _ => {
                self.refuse(place, format!("{keyword} {target} is not carried over"));
                return;
            }
        };
        if let Some(why) = too_many.filter(|_| chosen.len() != 1) {
            self.refuse(place, format!("{DIRECTORY} not read: {why}"));
            return;
        }
        for name in chosen {
            self.read(&format!("{DIRECTORY}/{name}"), place, depth + 1);
        }
    }
}

/// What ifupdown sets up for one interface.
#[derive(Debug, Default)]
struct Wanted {
    place: String,
    ipv4: Option<Ipv4Setting>,
    ipv6: Option<Ipv6Setting>,
    dns: Vec<IpAddr>,
    /// `inet static` without name servers of its own: those in force.
    dns_in_force: bool,
}

/// Why an option makes its stanza one a takeover does not carry over;
/// none if it is one it may.
fn not_carried(option: &str, family: Family, method: &str) -> Option<String> {
    let kind = match option {
        "pre-up" | "up" | "post-up" | "down" | "pre-down" | "post-down" => Some("runs commands"),
        o if o.starts_with("bridge-") => Some("a bridge"),
        o if o.starts_with("bond-") || o == "slaves" => Some("a bond"),
        o if o.starts_with("vlan-") => Some("a VLAN"),
        o if o.starts_with("wpa-") || o.starts_with("wireless-") => Some("wireless"),
        "dns-search" | "dns-domain" => Some("search domains are not carried over"),
        _ => None,
    };
    if let Some(kind) = kind {
        return Some(format!("{option}: {kind}"));
    }
    let allowed: &[&str] = match (family, method) {
        (Family::Inet, "dhcp") => &["dns-nameservers"],
        (Family::Inet, "static") => {
            &["address", "netmask", "gateway", "dns-nameservers", "broadcast", "network"]
        }
        (Family::Inet6, "auto") => &["dhcp", "accept-ra", "privext", "dns-nameservers"],
        (Family::Inet6, "dhcp") => &["accept-ra", "dns-nameservers"],
        _ => &[],
    };
    (!allowed.contains(&option)).then(|| format!("{option} is not carried over"))
}

fn mask(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix)) }
}

/// A netmask as `255.255.255.0` or `24`.
fn prefix_of(netmask: &str) -> Option<u8> {
    if let Ok(prefix) = netmask.parse::<u8>() {
        return (1..=32).contains(&prefix).then_some(prefix);
    }
    let bits = u32::from(netmask.parse::<Ipv4Addr>().ok()?);
    let prefix = u8::try_from(bits.leading_ones()).ok()?;
    (prefix > 0 && bits == mask(prefix)).then_some(prefix)
}

/// The settings of one stanza, or why not.
fn interpret(stanza: &Stanza, wanted: &mut Wanted) -> Result<(), String> {
    if let Some(why) =
        stanza.options.iter().find_map(|(option, _, _)| not_carried(option, stanza.family, &stanza.method))
    {
        return Err(why);
    }
    let value = |name: &str| {
        stanza.options.iter().find(|(option, _, _)| option == name).map(|(_, value, _)| value.as_str())
    };
    if stanza
        .options
        .iter()
        .enumerate()
        .any(|(i, (o, _, _))| stanza.options[..i].iter().any(|(p, _, _)| p == o))
    {
        return Err("an option given twice".into());
    }
    if let Some(servers) = value("dns-nameservers") {
        for server in servers.split_whitespace() {
            let server: IpAddr =
                server.parse().map_err(|_| format!("{server} is not a name server's address"))?;
            if !wanted.dns.contains(&server) {
                wanted.dns.push(server);
            }
        }
    }
    let flag = |name: &str, allowed: &[&str]| match value(name) {
        Some(set) if !allowed.contains(&set) => Err(format!("{name} {set} is not carried over")),
        _ => Ok(()),
    };
    match (stanza.family, stanza.method.as_str()) {
        (Family::Inet, "dhcp") => wanted.ipv4 = Some(Ipv4Setting::Dhcp { keep: None }),
        (Family::Inet, "static") => {
            let address = value("address").ok_or("a static address without its address")?;
            let (address, given) = match address.split_once('/') {
                Some((address, prefix)) => (address, Some(prefix)),
                None => (address, None),
            };
            let address: Ipv4Addr =
                address.parse().map_err(|_| format!("{address} is not an IPv4 address"))?;
            let from_address =
                given.map(|p| prefix_of(p).ok_or(format!("/{p} is not a prefix length"))).transpose()?;
            let from_netmask =
                value("netmask").map(|m| prefix_of(m).ok_or(format!("{m} is not a netmask"))).transpose()?;
            let prefix = match (from_address, from_netmask) {
                (Some(a), Some(m)) if a != m => {
                    return Err("the address's prefix and the netmask differ".into());
                }
                (Some(prefix), _) | (None, Some(prefix)) => prefix,
                (None, None) => return Err("a static address without its netmask".into()),
            };
            let bits = u32::from(address);
            let (network, broadcast) = (bits & mask(prefix), bits | !mask(prefix));
            match value("network") {
                Some(n) if n.parse::<Ipv4Addr>().ok().map(u32::from) != Some(network) => {
                    return Err(format!("network {n} is not the address's network"));
                }
                _ => {}
            }
            match value("broadcast") {
                Some("+") | None => {}
                Some(b) if b.parse::<Ipv4Addr>().ok().map(u32::from) == Some(broadcast) => {}
                Some(b) => return Err(format!("broadcast {b} is not the network's own")),
            }
            let gateway = value("gateway")
                .map(|g| g.parse::<Ipv4Addr>().map_err(|_| format!("{g} is not an IPv4 address")))
                .transpose()?;
            wanted.ipv4 = Some(Ipv4Setting::Static { address, prefix, gateway });
            wanted.dns_in_force |= value("dns-nameservers").is_none();
        }
        (Family::Inet6, "auto") => {
            flag("dhcp", &["0", "1"])?;
            flag("accept-ra", &["1", "2"])?;
            flag("privext", &["0"])?;
            wanted.ipv6 = Some(Ipv6Setting::Automatic);
        }
        (Family::Inet6, "dhcp") => {
            flag("accept-ra", &["1", "2"])?;
            wanted.ipv6 = Some(Ipv6Setting::Automatic);
        }
        (family, method) => return Err(format!("{} {method} is not carried over", family.name())),
    }
    Ok(())
}

fn kind_name(kind: &InterfaceKind) -> &str {
    match kind {
        InterfaceKind::Ethernet => "wired card",
        InterfaceKind::Loopback => "loopback",
        InterfaceKind::Bridge => "bridge",
        InterfaceKind::Bond => "bond",
        InterfaceKind::Vlan => "VLAN",
        InterfaceKind::Wireless => "wireless card",
        InterfaceKind::Other(other) => other,
    }
}

/// What taking the network over from ifupdown would set up, from its
/// files and the network as it is: each interface it brings up, if a
/// wired card here.
pub fn takeover_plan(files: &[IfupdownFile], network: &Network) -> TakeoverPlan {
    let mut reader = Reader {
        files,
        stanzas: Vec::new(),
        brought_up: BTreeSet::new(),
        refused: Vec::new(),
        read: BTreeSet::new(),
    };
    reader.read(MAIN, MAIN, 0);
    let mut refused = std::mem::take(&mut reader.refused);
    let mut wanted: BTreeMap<String, Wanted> = BTreeMap::new();
    let mut families: BTreeSet<(String, &'static str)> = BTreeSet::new();
    // An interface with a stanza refused is not set up at all.
    let mut not_carried: BTreeSet<String> = BTreeSet::new();
    for stanza in &reader.stanzas {
        let refuse = |reason: String| Refusal { place: stanza.place.clone(), reason };
        let name = stanza.name.as_str();
        if name == "lo" {
            if stanza.method != "loopback" || !stanza.options.is_empty() {
                refused.push(refuse("lo: the loopback with more than `loopback`".into()));
            }
            continue;
        }
        if name.contains('.') {
            refused.push(refuse(format!("{name}: a VLAN")));
            continue;
        }
        if !super::is_interface_name(name) {
            refused.push(refuse(format!("{name} is not the name of an interface zendiloOS sets up")));
            continue;
        }
        if !families.insert((name.to_owned(), stanza.family.name())) {
            refused.push(refuse(format!("{name}: a second {} stanza", stanza.family.name())));
            not_carried.insert(name.to_owned());
            continue;
        }
        let entry = wanted
            .entry(name.to_owned())
            .or_insert_with(|| Wanted { place: stanza.place.clone(), ..Wanted::default() });
        if let Err(why) = interpret(stanza, entry) {
            refused.push(refuse(format!("{name}: {why}")));
            not_carried.insert(name.to_owned());
        }
    }
    let in_force: Vec<IpAddr> = network
        .name_servers
        .iter()
        .filter(|ip| **ip != IpAddr::from([127, 0, 0, 53]))
        .take(MAX_DNS)
        .copied()
        .collect();
    let mut interfaces = Vec::new();
    for (name, wanted) in wanted.into_iter().filter(|(name, _)| !not_carried.contains(name)) {
        let refuse =
            |reason: String| Refusal { place: wanted.place.clone(), reason: format!("{name}: {reason}") };
        let Some(live) = network.interfaces.iter().find(|interface| interface.name == name) else {
            refused.push(refuse("there is no such interface here".into()));
            continue;
        };
        if live.kind != InterfaceKind::Ethernet {
            refused.push(refuse(format!("a {}, not a wired card", kind_name(&live.kind))));
            continue;
        }
        if let Some(bridge) = &live.member_of {
            refused.push(refuse(format!("part of {bridge}")));
            continue;
        }
        if !reader.brought_up.contains(&name) {
            refused.push(refuse("ifupdown brings it up only by hand (no auto or allow-hotplug line)".into()));
            continue;
        }
        let ipv4 = match wanted.ipv4 {
            Some(Ipv4Setting::Dhcp { .. }) => Ipv4Setting::Dhcp { keep: network.dynamic_address(&name) },
            Some(other) => other,
            None => Ipv4Setting::Off,
        };
        // ifupdown named none: those in force came from /etc/resolv.conf,
        // and systemd-resolved needs to be told them.
        let dns = if wanted.dns.is_empty() && wanted.dns_in_force { in_force.clone() } else { wanted.dns };
        let settings = InterfaceSettings {
            interface: name.clone(),
            ipv4,
            ipv6: wanted.ipv6.unwrap_or(Ipv6Setting::Automatic),
            dns,
        };
        match settings.checked() {
            Ok(settings) => interfaces.push(settings),
            Err(error) => refused.push(refuse(error.to_string())),
        }
    }
    if interfaces.is_empty() && refused.is_empty() {
        refused.push(Refusal {
            place: MAIN.into(),
            reason: "ifupdown sets up no interface here but the loopback; something else does".into(),
        });
    }
    TakeoverPlan { interfaces, refused }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Address, AddressScope, Interface, LinkState};

    fn file(path: &str, text: &str) -> IfupdownFile {
        IfupdownFile { path: path.into(), text: text.into(), skipped: None }
    }

    fn card(name: &str, kind: InterfaceKind, address: Option<(&str, bool)>) -> Interface {
        Interface {
            name: name.into(),
            kind,
            state: LinkState::Up,
            mac: None,
            mtu: 1500,
            member_of: None,
            addresses: address
                .map(|(ip, dynamic)| Address {
                    ip: ip.parse().unwrap(),
                    prefix: 24,
                    scope: AddressScope::Global,
                    dynamic,
                })
                .into_iter()
                .collect(),
        }
    }

    /// The owner's NAS: two Realtek ports, the first with DHCP, the second
    /// down; its router is the name server.
    fn nas() -> Network {
        Network::new(
            vec![
                card("enp2s0", InterfaceKind::Ethernet, Some(("192.168.1.127", true))),
                card("enp3s0", InterfaceKind::Ethernet, None),
                card("wlan0", InterfaceKind::Wireless, None),
                card("br0", InterfaceKind::Bridge, None),
            ],
            vec![],
            vec!["192.168.1.1".parse().unwrap()],
        )
    }

    fn plan(files: &[IfupdownFile]) -> TakeoverPlan {
        takeover_plan(files, &nas())
    }

    fn reasons(plan: &TakeoverPlan) -> Vec<String> {
        plan.refused.iter().map(|r| format!("{}: {}", r.place, r.reason)).collect()
    }

    /// What the Debian installer writes for DHCP, with IPv6 from the router.
    const INSTALLER: &str = "# This file describes the network interfaces available on your system\n\
        # and how to activate them. For more information, see interfaces(5).\n\
        \n\
        source /etc/network/interfaces.d/*\n\
        \n\
        # The loopback network interface\n\
        auto lo\n\
        iface lo inet loopback\n\
        \n\
        # The primary network interface\n\
        allow-hotplug enp2s0\n\
        iface enp2s0 inet dhcp\n\
        # This is an autoconfigured IPv6 interface\n\
        iface enp2s0 inet6 auto\n";

    #[test]
    fn dhcp_keeps_the_address_the_card_has() {
        let plan = plan(&[file("interfaces", INSTALLER)]);
        assert_eq!(reasons(&plan), Vec::<String>::new());
        assert_eq!(
            plan.interfaces,
            [InterfaceSettings {
                interface: "enp2s0".into(),
                ipv4: Ipv4Setting::Dhcp { keep: Some("192.168.1.127".parse().unwrap()) },
                ipv6: Ipv6Setting::Automatic,
                dns: vec![],
            }]
        );
    }

    #[test]
    fn a_static_address_with_its_netmask_gateway_and_name_servers() {
        let text = "auto enp2s0\n\
            iface enp2s0 inet static\n\
            \taddress 192.168.1.10\n\
            \tnetmask 255.255.255.0\n\
            \tnetwork 192.168.1.0\n\
            \tbroadcast 192.168.1.255\n\
            \tgateway 192.168.1.1\n\
            \t# dns-* options are implemented by the resolvconf package, if installed\n\
            \tdns-nameservers 192.168.1.1 \\\n\
            \t  9.9.9.9\n";
        let fixed = plan(&[file("interfaces", text)]);
        assert_eq!(reasons(&fixed), Vec::<String>::new());
        let settings = &fixed.interfaces[0];
        assert_eq!(
            settings.ipv4,
            Ipv4Setting::Static {
                address: "192.168.1.10".parse().unwrap(),
                prefix: 24,
                gateway: Some("192.168.1.1".parse().unwrap())
            }
        );
        assert_eq!(settings.dns, ["192.168.1.1".parse::<IpAddr>().unwrap(), "9.9.9.9".parse().unwrap()]);
        // No inet6 stanza: the kernel took router advertisements, as
        // systemd-networkd will.
        assert_eq!(settings.ipv6, Ipv6Setting::Automatic);

        // The installer's own form, the prefix with the address; without
        // name servers, those in force (from resolv.conf) are kept.
        let text = "allow-hotplug enp2s0\niface enp2s0 inet static\n  address 10.0.0.5/8\n";
        let installer = plan(&[file("interfaces", text)]);
        assert!(matches!(installer.interfaces[0].ipv4, Ipv4Setting::Static { prefix: 8, gateway: None, .. }));
        assert_eq!(installer.interfaces[0].dns, ["192.168.1.1".parse::<IpAddr>().unwrap()]);
    }

    #[test]
    fn sourced_files_count_as_ifupdown_reads_them() {
        let main = "source interfaces.d/*\nsource-directory /etc/network/interfaces.d\nauto lo\niface lo inet loopback\n";
        let files = [
            file("interfaces", main),
            file("interfaces.d/enp2s0", "auto enp2s0\niface enp2s0 inet dhcp\n"),
            // Sourced by neither: a leading dot keeps it from the glob and
            // from run-parts.
            file("interfaces.d/.old", "iface br0 inet manual\n  bridge_ports enp2s0\n"),
            // The glob takes this one, run-parts would not.
            file("interfaces.d/enp3s0.disabled", "auto enp3s0\niface enp3s0 inet dhcp\n"),
        ];
        let sourced = plan(&files);
        let names: Vec<&str> = sourced.interfaces.iter().map(|s| s.interface.as_str()).collect();
        assert_eq!(names, ["enp2s0", "enp3s0"], "{:?}", reasons(&sourced));
        assert!(sourced.refused.is_empty(), "each file read once: {:?}", reasons(&sourced));

        // A file of its own, and one there is not.
        let one = [file("interfaces", "source /etc/network/interfaces.d/enp2s0\n"), files[1].clone()];
        assert_eq!(takeover_plan(&one, &nas()).interfaces.len(), 1);
        let missing = plan_reasons(&[file("interfaces", "source interfaces.d/gone\n")]);
        assert_eq!(missing, ["interfaces:1: interfaces.d/gone is not there"]);
        // Anywhere else, links and globs of other kinds are refused.
        let elsewhere = plan_reasons(&[file("interfaces", "source /etc/other/*\nsource interfaces.d/e*\n")]);
        assert_eq!(
            elsewhere,
            [
                "interfaces:1: source /etc/other/* is not carried over",
                "interfaces:2: source interfaces.d/e* is not carried over"
            ]
        );
        let linked = [
            file("interfaces", "source /etc/network/interfaces.d/*\n"),
            IfupdownFile { skipped: Some("a link".into()), ..file("interfaces.d/x", "") },
        ];
        assert_eq!(plan_reasons(&linked), ["interfaces.d/x: not read: a link"]);
    }

    fn plan_reasons(files: &[IfupdownFile]) -> Vec<String> {
        reasons(&plan(files))
    }

    #[test]
    fn what_is_beyond_a_wired_card_is_named_and_refused() {
        let text = "auto enp2s0 br0 bond0 enp2s0.10 wlan0 enp3s0\n\
            iface enp2s0 inet dhcp\n  post-up /usr/local/bin/hello\n\
            iface br0 inet dhcp\n  bridge_ports enp3s0\n\
            iface bond0 inet manual\n  bond-slaves enp3s0\n\
            iface enp2s0.10 inet dhcp\n\
            iface wlan0 inet dhcp\n  wpa-ssid home\n\
            iface enp3s0 inet static\n  address 192.168.2.5/24\n  dns-search home.arpa\n\
            mapping eth*\n  script /usr/local/bin/map\n\
            allow-ovs br1\n";
        assert_eq!(
            plan_reasons(&[file("interfaces", text)]),
            [
                "interfaces:14: mapping is not carried over",
                "interfaces:16: allow-ovs is not carried over",
                "interfaces:2: enp2s0: post-up: runs commands",
                "interfaces:4: br0: bridge-ports: a bridge",
                "interfaces:6: bond0: bond-slaves: a bond",
                "interfaces:8: enp2s0.10: a VLAN",
                "interfaces:9: wlan0: wpa-ssid: wireless",
                "interfaces:11: enp3s0: dns-search: search domains are not carried over",
            ]
        );
    }

    #[test]
    fn the_cards_must_be_there_wired_and_brought_up() {
        let text = "auto eth9 wlan0 br0\niface eth9 inet dhcp\niface wlan0 inet dhcp\niface br0 inet dhcp\n\
            iface enp3s0 inet dhcp\nauto enp2s0\niface enp2s0 inet manual\niface enp2s0 inet6 static\n";
        assert_eq!(
            plan_reasons(&[file("interfaces", text)]),
            [
                "interfaces:7: enp2s0: inet manual is not carried over",
                "interfaces:8: enp2s0: inet6 static is not carried over",
                "interfaces:4: br0: a bridge, not a wired card",
                "interfaces:5: enp3s0: ifupdown brings it up only by hand (no auto or allow-hotplug line)",
                "interfaces:2: eth9: there is no such interface here",
                "interfaces:3: wlan0: a wireless card, not a wired card",
            ]
        );
    }

    #[test]
    fn values_must_say_what_systemd_networkd_can() {
        let static_with = |lines: &str| {
            plan_reasons(&[file("interfaces", &format!("auto enp2s0\niface enp2s0 inet static\n{lines}"))])
        };
        assert_eq!(
            static_with("  netmask 24\n"),
            ["interfaces:2: enp2s0: a static address without its address"]
        );
        assert_eq!(
            static_with("  address 10.0.0.5\n"),
            ["interfaces:2: enp2s0: a static address without its netmask"]
        );
        assert_eq!(
            static_with("  address 10.0.0.5/8\n  netmask 255.255.0.0\n"),
            ["interfaces:2: enp2s0: the address's prefix and the netmask differ"]
        );
        assert_eq!(static_with("  address 10.0.0.5/8\n  broadcast 10.0.0.255\n").len(), 1);
        assert_eq!(static_with("  address 10.0.0.5\n  netmask 255.0.255.0\n").len(), 1);
        assert_eq!(
            static_with("  address 10.0.0.5/8\n  gateway 192.168.1.1\n").len(),
            1,
            "a gateway elsewhere"
        );
        assert_eq!(
            static_with("  address 10.0.0.5/8\n  mtu 9000\n"),
            ["interfaces:2: enp2s0: mtu is not carried over"]
        );
        assert_eq!(static_with("  address 10.0.0.5/8\n  address 10.0.0.6/8\n").len(), 1);
        let ipv6 = |options: &str| {
            plan_reasons(&[file(
                "interfaces",
                &format!("auto enp2s0\niface enp2s0 inet dhcp\niface enp2s0 inet6 auto\n{options}"),
            )])
        };
        assert!(ipv6("  dhcp 1\n  accept_ra 2\n  privext 0\n").is_empty());
        assert_eq!(ipv6("  privext 2\n"), ["interfaces:3: enp2s0: privext 2 is not carried over"]);
        let twice = plan_reasons(&[file(
            "interfaces",
            "auto enp2s0\niface enp2s0 inet dhcp\niface enp2s0 inet dhcp\n",
        )]);
        assert_eq!(twice, ["interfaces:3: enp2s0: a second inet stanza"]);
        let lo = plan_reasons(&[file(
            "interfaces",
            "auto lo enp2s0\niface lo inet loopback\n  up ip addr add 10.1.1.1/32 dev lo\niface enp2s0 inet dhcp\n",
        )]);
        assert_eq!(lo, ["interfaces:2: lo: the loopback with more than `loopback`"]);
        let stray =
            plan_reasons(&[file("interfaces", "  address 10.0.0.1\nauto enp2s0\niface enp2s0 inet dhcp\n")]);
        assert_eq!(stray, ["interfaces:1: address stands outside a stanza"]);
        let only_lo = plan_reasons(&[file("interfaces", "auto lo\niface lo inet loopback\n")]);
        assert_eq!(
            only_lo,
            ["interfaces: ifupdown sets up no interface here but the loopback; something else does"]
        );
    }

    #[test]
    fn lines_are_read_as_ifupdown_reads_them() {
        let read = lines(
            "# a comment \\\nauto lo\n\n  # indented\niface eth0 inet static \\\n  address 1.2.3.4/24\\\n\n  gateway 1.2.3.1\n",
        );
        assert_eq!(
            read,
            [
                (2, "auto lo".to_owned()),
                (5, "iface eth0 inet static    address 1.2.3.4/24".to_owned()),
                (8, "gateway 1.2.3.1".to_owned()),
            ]
        );
        assert_eq!(prefix_of("255.255.254.0"), Some(23));
        assert_eq!(prefix_of("0.0.0.0"), None);
        assert_eq!(prefix_of("33"), None);
    }
}
