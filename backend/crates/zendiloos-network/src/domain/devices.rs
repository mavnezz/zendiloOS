//! Bridges, bonds and VLANs zendiloOS makes: what its systemd-networkd
//! files say, and what making, changing or removing one does to the
//! interfaces around it, checked before the agent writes anything.
//!
//! zendiloOS's files are the truth: `05-zendiloos-<name>.netdev` for each
//! device it made (a bridge's or bond's members noted in order),
//! `05-zendiloos-<name>.network` for each interface it set up, saying its
//! addresses, or the bridge or bond it is a port of, and the VLANs on it.
//! What an interface without such a file has is taken from what it has
//! now (its addresses, the default route through it).
//!
//! A bridge or bond takes the settings of the first member given that has
//! any, which becomes its first member, and that member's MAC address, so
//! DHCP sees the same machine and hands out the same address. Removed, it
//! gives its settings back to its first member; the other members, and
//! those taken out of one, are switched off (a bond or VLAN made here
//! stays up without addresses instead).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use super::{
    AddressScope, InterfaceKind, InterfaceSettings, Ipv4Setting, Ipv6Setting, MAX_DNS, Network,
    is_docker_bridge, is_interface_name,
};

/// Members of a bridge or bond, at most.
pub const MAX_MEMBERS: usize = 8;
/// The prefix of zendiloOS's files.
const PREFIX: &str = "05-zendiloos-";
/// The note of a bridge's or bond's members in its `.netdev`.
const MEMBERS: &str = "# Members:";

/// How a bond spreads its traffic over its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondMode {
    /// One member carries, another takes over when it fails; works with
    /// any switch.
    ActiveBackup,
    /// IEEE 802.3ad: the switch bundles the ports (LACP); more throughput
    /// and failover, needs a managed switch set up for it.
    Lacp,
    /// Adaptive load balancing: traffic both ways spread over the members
    /// without help from the switch.
    BalanceAlb,
}

impl BondMode {
    /// As systemd-networkd and the API name it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveBackup => "active-backup",
            Self::Lacp => "802.3ad",
            Self::BalanceAlb => "balance-alb",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::ActiveBackup, Self::Lacp, Self::BalanceAlb].into_iter().find(|mode| mode.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    /// STP off by default: a NAS bridges VMs, not switches.
    Bridge {
        stp: bool,
    },
    Bond {
        mode: BondMode,
    },
    /// On `parent`, tagged `id` (1 to 4094).
    Vlan {
        parent: String,
        id: u16,
    },
}

impl DeviceKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bridge { .. } => "bridge",
            Self::Bond { .. } => "bond",
            Self::Vlan { .. } => "VLAN",
        }
    }
}

/// A device zendiloOS made, as its files say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub kind: DeviceKind,
    /// A bridge's or bond's, in order: the first gets the settings back
    /// when it goes.
    pub members: Vec<String>,
    /// The first member's, given at making it; none for a VLAN.
    pub mac: Option<String>,
}

/// What zendiloOS's file for one interface says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFile {
    /// Its addresses; none (IPv4 and IPv6 off) for a port.
    pub settings: InterfaceSettings,
    /// The bridge or bond it is a port of.
    pub port_of: Option<String>,
    /// The VLANs on it.
    pub vlans: Vec<String>,
    /// Up without addresses rather than switched off: a device zendiloOS
    /// made, there to carry others.
    pub keep_up: bool,
}

/// One of zendiloOS's systemd-networkd files, as the host hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkdFile {
    pub name: String,
    pub text: String,
}

/// zendiloOS's network files, read: the devices it made and what each
/// interface it set up has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Configuration {
    /// By name.
    pub devices: Vec<Device>,
    /// By name.
    pub links: Vec<LinkFile>,
}

/// The sections and their `Key=value` lines of a systemd-networkd file,
/// and its comments.
fn sections(text: &str) -> (Vec<(String, String, String)>, Vec<&str>) {
    let mut section = String::new();
    let mut entries = Vec::new();
    let mut comments = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with(';') {
            comments.push(line);
        } else if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            name.clone_into(&mut section);
        } else if let Some((key, value)) = line.split_once('=') {
            entries.push((section.clone(), key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    (entries, comments)
}

/// The values of `key` in `section`, in order.
fn values<'a>(
    entries: &'a [(String, String, String)],
    section: &'static str,
    key: &'static str,
) -> impl Iterator<Item = &'a str> + 'a {
    entries.iter().filter(move |(s, k, _)| s == section && k == key).map(|(_, _, v)| v.as_str())
}

fn link_file(interface: &str, text: &str) -> LinkFile {
    let (entries, _) = sections(text);
    let get = |section: &'static str, key: &'static str| values(&entries, section, key);
    let dhcp = get("Network", "DHCP").any(|v| matches!(v, "ipv4" | "yes" | "true"));
    let address = get("Network", "Address").find_map(|v| {
        let (ip, prefix) = v.split_once('/')?;
        Some((ip.parse::<Ipv4Addr>().ok()?, prefix.parse::<u8>().ok()?))
    });
    let gateway = get("Network", "Gateway").find_map(|v| v.parse::<Ipv4Addr>().ok());
    let ipv4 = match (address, dhcp) {
        (Some((address, prefix)), _) => Ipv4Setting::Static { address, prefix, gateway },
        (None, true) => {
            Ipv4Setting::Dhcp { keep: get("DHCPv4", "RequestAddress").find_map(|v| v.parse().ok()) }
        }
        (None, false) => Ipv4Setting::Off,
    };
    let ra = get("Network", "IPv6AcceptRA").any(|v| matches!(v, "yes" | "true"));
    let port_of = get("Network", "Bridge").chain(get("Network", "Bond")).next().map(str::to_owned);
    let settings = InterfaceSettings {
        interface: interface.to_owned(),
        ipv4: if port_of.is_some() { Ipv4Setting::Off } else { ipv4 },
        ipv6: if ra && port_of.is_none() { Ipv6Setting::Automatic } else { Ipv6Setting::Off },
        dns: get("Network", "DNS").filter_map(|v| v.parse().ok()).take(MAX_DNS).collect(),
    };
    let vlans = get("Network", "VLAN").map(str::to_owned).collect();
    LinkFile { settings, port_of, vlans, keep_up: false }
}

fn netdev(name: &str, text: &str) -> Option<(Device, Option<u16>)> {
    let (entries, comments) = sections(text);
    let get = |section: &str, key: &str| {
        entries.iter().find(|(s, k, _)| s == section && k == key).map(|(_, _, v)| v.as_str())
    };
    if get("NetDev", "Name") != Some(name) {
        return None;
    }
    let members: Vec<String> = comments
        .iter()
        .find_map(|line| line.strip_prefix(MEMBERS))
        .map(|list| list.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let (kind, id) = match get("NetDev", "Kind")? {
        "bridge" => (DeviceKind::Bridge { stp: matches!(get("Bridge", "STP"), Some("yes" | "true")) }, None),
        "bond" => (DeviceKind::Bond { mode: BondMode::parse(get("Bond", "Mode")?)? }, None),
        "vlan" => {
            let id = get("VLAN", "Id")?.parse().ok()?;
            (DeviceKind::Vlan { parent: String::new(), id }, Some(id))
        }
        _ => return None,
    };
    let mac = get("NetDev", "MACAddress").map(str::to_owned);
    Some((Device { name: name.to_owned(), kind, members, mac }, id))
}

impl Configuration {
    /// Reads zendiloOS's files; what is not one of its own is passed by.
    pub fn read(files: &[NetworkdFile]) -> Self {
        let mut links = BTreeMap::new();
        let mut devices = BTreeMap::new();
        for file in files {
            let Some(rest) = file.name.strip_prefix(PREFIX) else { continue };
            if let Some(interface) = rest.strip_suffix(".network").filter(|name| is_interface_name(name)) {
                links.insert(interface.to_owned(), link_file(interface, &file.text));
            } else if let Some(device) = rest.strip_suffix(".netdev").filter(|name| is_interface_name(name)) {
                devices.extend(netdev(device, &file.text).map(|found| (device.to_owned(), found)));
            }
        }
        let devices = devices
            .into_values()
            .map(|(mut device, _)| {
                match &mut device.kind {
                    DeviceKind::Vlan { parent, .. } => {
                        // On the interface whose file carries it.
                        let carrier =
                            links.values().find(|link: &&LinkFile| link.vlans.contains(&device.name));
                        carrier
                            .map(|link| link.settings.interface.clone())
                            .unwrap_or_default()
                            .clone_into(parent);
                        device.members.clear();
                    }
                    DeviceKind::Bridge { .. } | DeviceKind::Bond { .. } => {
                        // The noted order, for those that are ports of it.
                        let ports: Vec<&String> = links
                            .values()
                            .filter(|link| link.port_of.as_ref() == Some(&device.name))
                            .map(|link| &link.settings.interface)
                            .collect();
                        let mut members: Vec<String> =
                            device.members.iter().filter(|m| ports.contains(m)).cloned().collect();
                        members.extend(
                            ports.into_iter().filter(|p| !members.contains(p)).cloned().collect::<Vec<_>>(),
                        );
                        device.members = members;
                    }
                }
                device
            })
            .collect();
        let mut configuration = Self { devices, links: links.into_values().collect() };
        // A device zendiloOS made stays up without addresses.
        let names: Vec<String> = configuration.devices.iter().map(|d| d.name.clone()).collect();
        for link in &mut configuration.links {
            link.keep_up = names.contains(&link.settings.interface);
        }
        configuration
    }

    pub fn device(&self, name: &str) -> Option<&Device> {
        self.devices.iter().find(|device| device.name == name)
    }

    /// The internal networks for VMs: bridges zendiloOS made without
    /// members whose own file gives the NAS no address on them (neither
    /// IPv4 nor IPv6), by name. The agent keeps the NAS off these (its
    /// `bridge` table drops what goes between it and them), so VMs on one
    /// reach each other only.
    pub fn internal_networks(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|device| matches!(device.kind, DeviceKind::Bridge { .. }) && device.members.is_empty())
            .filter(|device| {
                self.link(&device.name).is_some_and(|link| {
                    link.port_of.is_none()
                        && link.settings.ipv4 == Ipv4Setting::Off
                        && link.settings.ipv6 == Ipv6Setting::Off
                })
            })
            .map(|device| device.name.clone())
            .collect()
    }

    pub fn link(&self, interface: &str) -> Option<&LinkFile> {
        self.links.iter().find(|link| link.settings.interface == interface)
    }

    /// The VLANs on `interface`, as its file says.
    fn vlans_on(&self, interface: &str) -> Vec<String> {
        self.link(interface).map(|link| link.vlans.clone()).unwrap_or_default()
    }

    /// The bridge or bond `interface` is a port of, by zendiloOS's files.
    fn port_of(&self, interface: &str) -> Option<&str> {
        self.link(interface).and_then(|link| link.port_of.as_deref())
    }
}

/// A rule a request broke, in words for the person who sent it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidDevice(pub String);

fn invalid<T>(text: impl Into<String>) -> Result<T, InvalidDevice> {
    Err(InvalidDevice(text.into()))
}

/// What a change was, while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    Made,
    Changed,
    Removed,
}

/// What a change is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// One interface's settings.
    Interface(String),
    Device(String, DeviceAction),
    /// zendiloOS's settings written back, where its files were changed or
    /// lost outside it ([`super::kept`]).
    Restore,
}

/// What becomes of an interface a change touches, for people to see
/// before it is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Becomes {
    /// A member of `device`; `moved`: its settings went to the device,
    /// `dropped`: those it had go.
    Member { device: String, moved: bool, dropped: bool },
    /// It has these settings: a new VLAN's, a device's taking its first
    /// member's, or given back to that member.
    Settings(InterfaceSettings),
    /// Leaves the device it was a member of, switched off.
    SwitchedOff,
    /// Keeps its settings, and carries `vlan` from now on (or no longer).
    Parent { vlan: String, carries: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub interface: String,
    pub becomes: Becomes,
}

/// What a change writes: devices made or changed, devices removed,
/// interfaces' files; with what it does, for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub subject: Subject,
    pub devices: Vec<Device>,
    pub removed: Vec<String>,
    pub links: Vec<LinkFile>,
    /// Interfaces whose file of zendiloOS's goes (only a restore takes one
    /// away: written outside zendiloOS).
    pub unset: Vec<String>,
    pub effects: Vec<Effect>,
}

/// A bridge, bond or VLAN to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    pub name: String,
    pub kind: DeviceKind,
    /// Of a bridge (none: for VMs only) or bond (at least one), in order.
    pub members: Vec<String>,
    /// A VLAN's own; a bridge or bond takes its first member's.
    pub settings: Option<InterfaceSettings>,
}

/// A bridge's or bond's members, and what it is, changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUpdate {
    pub name: String,
    pub members: Vec<String>,
    /// A bond's; none keeps it.
    pub mode: Option<BondMode>,
    /// A bridge's; none keeps it.
    pub stp: Option<bool>,
}

/// What the machine has, as the planning sees it: what zendiloOS's files
/// say and what the interfaces have now.
pub struct Planning<'a> {
    pub network: &'a Network,
    pub configuration: &'a Configuration,
}

/// The name of a new device, checked: an interface name as the kernel takes
/// it, none Docker would use or the firewall takes for a container's, and
/// none in use.
fn check_name(planning: &Planning<'_>, name: &str) -> Result<(), InvalidDevice> {
    if !is_interface_name(name) {
        return invalid(format!(
            "{name:?} is no interface name: 1 to 15 letters, digits, '-', '_' or '.', not starting with '-' or '.'"
        ));
    }
    if is_docker_bridge(name) || name.starts_with("veth") {
        return invalid(format!("{name} is a name Docker uses"));
    }
    let taken = planning.network.interfaces.iter().any(|i| i.name == name)
        || planning.network.docker_bridges.iter().any(|b| b == name)
        || planning.configuration.device(name).is_some();
    if taken {
        return invalid(format!("there is an interface {name} already"));
    }
    Ok(())
}

impl Planning<'_> {
    /// What `interface` has now: zendiloOS's file says, or else its
    /// addresses do. None for an interface there is not.
    pub fn settings_of(&self, interface: &str) -> Option<InterfaceSettings> {
        let shown = self.network.interfaces.iter().find(|i| i.name == interface);
        if let Some(link) = self.configuration.link(interface).filter(|link| link.port_of.is_none()) {
            let mut settings = link.settings.clone();
            if let Ipv4Setting::Dhcp { keep } = &mut settings.ipv4 {
                *keep = self.network.dynamic_address(interface).or(*keep);
            }
            return Some(settings);
        }
        let shown = shown?;
        let v4 = shown.addresses.iter().find_map(|a| match a.ip {
            IpAddr::V4(ip) if a.scope == AddressScope::Global => Some((ip, a.prefix, a.dynamic)),
            _ => None,
        });
        let gateway = self.network.default_routes.iter().find_map(|route| match route.gateway {
            IpAddr::V4(gateway) if route.interface == interface => Some(gateway),
            _ => None,
        });
        let ipv4 = match v4 {
            Some((ip, _, true)) => Ipv4Setting::Dhcp { keep: Some(ip) },
            Some((address, prefix, false)) => Ipv4Setting::Static { address, prefix, gateway },
            None => Ipv4Setting::Off,
        };
        let ipv6 = if shown.addresses.iter().any(|a| a.ip.is_ipv6()) {
            Ipv6Setting::Automatic
        } else {
            Ipv6Setting::Off
        };
        let dns = if matches!(ipv4, Ipv4Setting::Static { .. }) {
            let stub: IpAddr = [127, 0, 0, 53].into();
            self.network.name_servers.iter().copied().filter(|ip| *ip != stub).take(MAX_DNS).collect()
        } else {
            Vec::new()
        };
        Some(InterfaceSettings { interface: interface.to_owned(), ipv4, ipv6, dns })
    }

    /// Whether `interface` has settings of its own to give.
    fn has_settings(&self, interface: &str) -> bool {
        self.settings_of(interface).is_some_and(|settings| !settings.switched_off())
    }

    /// The bridge or bond `interface` is in: by zendiloOS's files or as the
    /// kernel says.
    pub fn master_of(&self, interface: &str) -> Option<String> {
        self.configuration.port_of(interface).map(str::to_owned).or_else(|| {
            self.network.interfaces.iter().find(|i| i.name == interface).and_then(|i| i.member_of.clone())
        })
    }

    /// The file `interface` has once it keeps its settings: as now, with
    /// the VLANs zendiloOS put on it.
    fn link_of(&self, interface: &str) -> Option<LinkFile> {
        let settings = self.settings_of(interface)?;
        let keep_up = self.configuration.device(interface).is_some();
        Some(LinkFile { settings, port_of: None, vlans: self.configuration.vlans_on(interface), keep_up })
    }

    /// What a member leaving its bridge or bond has: `settings` of its own.
    /// A bond or VLAN made here stays up without addresses, there to carry
    /// others; a card without settings is switched off.
    fn leaving(&self, settings: InterfaceSettings) -> (LinkFile, Becomes) {
        let made = self.configuration.device(&settings.interface).is_some();
        let becomes = if made || !settings.switched_off() {
            Becomes::Settings(settings.clone())
        } else {
            Becomes::SwitchedOff
        };
        (LinkFile { settings, port_of: None, vlans: Vec::new(), keep_up: made }, becomes)
    }

    /// Checks `members` of `device` (a bridge, or with `cards_only` a bond):
    /// each once, there, of a kind that can be in one, and free (in no
    /// other bridge or bond, carrying no VLANs), but those it has already.
    fn check_members(&self, device: &str, members: &[String], cards_only: bool) -> Result<(), InvalidDevice> {
        if members.len() > MAX_MEMBERS {
            return invalid(format!("a bridge or bond has at most {MAX_MEMBERS} members"));
        }
        for (index, member) in members.iter().enumerate() {
            if members[..index].contains(member) {
                return invalid(format!("{member} is named twice"));
            }
            if member == device {
                return invalid(format!("{device} cannot be a member of itself"));
            }
            let Some(interface) = self.network.interfaces.iter().find(|i| &i.name == member) else {
                return invalid(format!("there is no interface {member}"));
            };
            let ours = self.configuration.device(member);
            let fits = match (&interface.kind, ours.map(|d| &d.kind)) {
                (InterfaceKind::Ethernet, _) => true,
                (_, Some(DeviceKind::Bond { .. } | DeviceKind::Vlan { .. })) => !cards_only,
                _ => false,
            };
            if !fits {
                return invalid(if cards_only {
                    format!("{member} is no wired card: a bond is made of wired cards")
                } else {
                    format!("{member} cannot be in a bridge: only wired cards, and bonds and VLANs made here")
                });
            }
            if let Some(master) = self.master_of(member).filter(|master| master != device) {
                return invalid(format!("{member} is a member of {master} already"));
            }
            if let Some(vlan) = self.configuration.vlans_on(member).first() {
                return invalid(format!("{member} carries the VLAN {vlan}"));
            }
            let observed =
                self.network.interfaces.iter().find(|i| i.parent.as_deref() == Some(member.as_str()));
            if let Some(vlan) = observed {
                return invalid(format!("{member} carries the VLAN {}", vlan.name));
            }
        }
        Ok(())
    }

    /// No device ends up on itself: a VLAN on a bridge it is a member of,
    /// through any number of steps.
    fn check_no_cycle(&self, changed: &Device) -> Result<(), InvalidDevice> {
        let lower = |device: &Device| -> Vec<String> {
            match &device.kind {
                DeviceKind::Vlan { parent, .. } => vec![parent.clone()],
                _ => device.members.clone(),
            }
        };
        let device = |name: &str| -> Option<Device> {
            if name == changed.name {
                Some(changed.clone())
            } else {
                self.configuration.device(name).cloned()
            }
        };
        let mut seen: Vec<String> = Vec::new();
        let mut next = lower(changed);
        while let Some(name) = next.pop() {
            if name == changed.name {
                return invalid(format!("{} would end up on itself", changed.name));
            }
            if seen.contains(&name) {
                continue;
            }
            if let Some(found) = device(&name) {
                next.extend(lower(&found));
            }
            seen.push(name);
        }
        Ok(())
    }

    fn mac_of(&self, interface: &str) -> Option<String> {
        let device = self.configuration.device(interface).and_then(|d| d.mac.clone());
        device.or_else(|| {
            self.network.interfaces.iter().find(|i| i.name == interface).and_then(|i| i.mac.clone())
        })
    }

    /// Makes a bridge, bond or VLAN.
    pub fn make(&self, new: NewDevice) -> Result<Edit, InvalidDevice> {
        check_name(self, &new.name)?;
        let subject = Subject::Device(new.name.clone(), DeviceAction::Made);
        if let DeviceKind::Vlan { parent, id } = &new.kind {
            return self.make_vlan(&new, parent, *id, subject);
        }
        let bond = matches!(new.kind, DeviceKind::Bond { .. });
        if bond && new.members.is_empty() {
            return invalid("a bond needs at least one member");
        }
        if new.settings.is_some() {
            return invalid("a bridge or bond takes the settings of its first member");
        }
        self.check_members(&new.name, &new.members, bond)?;
        // The first member with settings first: the device takes them, and
        // gives them back to it.
        let mut members = new.members.clone();
        let carrier = members.iter().position(|member| self.has_settings(member));
        if let Some(index) = carrier {
            let first = members.remove(index);
            members.insert(0, first);
        }
        let settings = match carrier {
            Some(_) => {
                let mut settings = self.settings_of(&members[0]).unwrap_or_else(|| off(&new.name));
                settings.interface.clone_from(&new.name);
                settings
            }
            None => off(&new.name),
        };
        let device = Device {
            name: new.name.clone(),
            kind: new.kind.clone(),
            mac: members.first().and_then(|first| self.mac_of(first)),
            members: members.clone(),
        };
        self.check_no_cycle(&device)?;
        let mut links =
            vec![LinkFile { settings: settings.clone(), port_of: None, vlans: Vec::new(), keep_up: true }];
        let mut effects = vec![Effect { interface: new.name.clone(), becomes: Becomes::Settings(settings) }];
        for (index, member) in members.iter().enumerate() {
            let moved = index == 0 && carrier.is_some();
            let dropped = !moved && self.has_settings(member);
            links.push(port(member, &new.name));
            effects.push(Effect {
                interface: member.clone(),
                becomes: Becomes::Member { device: new.name.clone(), moved, dropped },
            });
        }
        Ok(Edit { subject, devices: vec![device], removed: Vec::new(), links, unset: Vec::new(), effects })
    }

    fn make_vlan(
        &self,
        new: &NewDevice,
        parent: &str,
        id: u16,
        subject: Subject,
    ) -> Result<Edit, InvalidDevice> {
        if !(1..=4094).contains(&id) {
            return invalid("a VLAN id is from 1 to 4094");
        }
        if !new.members.is_empty() {
            return invalid("a VLAN has a parent, no members");
        }
        let Some(interface) = self.network.interfaces.iter().find(|i| i.name == parent) else {
            return invalid(format!("there is no interface {parent}"));
        };
        if !matches!(interface.kind, InterfaceKind::Ethernet | InterfaceKind::Bond | InterfaceKind::Bridge) {
            return invalid(format!("a VLAN goes on a wired card, a bond or a bridge, not on {parent}"));
        }
        if let Some(master) = self.master_of(parent) {
            return invalid(format!("{parent} is a member of {master}: put the VLAN on {master}"));
        }
        let twice = self.configuration.devices.iter().any(
            |device| matches!(&device.kind, DeviceKind::Vlan { parent: p, id: i } if p == parent && *i == id),
        );
        if twice {
            return invalid(format!("{parent} carries the VLAN {id} already"));
        }
        let mut settings = new.settings.clone().unwrap_or_else(|| off(&new.name));
        settings.interface.clone_from(&new.name);
        let settings = settings.checked().map_err(|error| InvalidDevice(error.to_string()))?;
        let device = Device {
            name: new.name.clone(),
            kind: DeviceKind::Vlan { parent: parent.to_owned(), id },
            members: Vec::new(),
            mac: None,
        };
        self.check_no_cycle(&device)?;
        let Some(mut carrier) = self.link_of(parent) else {
            return invalid(format!("there is no interface {parent}"));
        };
        carrier.vlans.push(new.name.clone());
        let links = vec![
            carrier,
            LinkFile { settings: settings.clone(), port_of: None, vlans: Vec::new(), keep_up: true },
        ];
        let effects = vec![
            Effect { interface: new.name.clone(), becomes: Becomes::Settings(settings) },
            Effect {
                interface: parent.to_owned(),
                becomes: Becomes::Parent { vlan: new.name.clone(), carries: true },
            },
        ];
        Ok(Edit { subject, devices: vec![device], removed: Vec::new(), links, unset: Vec::new(), effects })
    }

    fn ours(&self, name: &str) -> Result<&Device, InvalidDevice> {
        self.configuration
            .device(name)
            .ok_or_else(|| InvalidDevice(format!("{name} is no bridge, bond or VLAN zendiloOS made")))
    }

    /// Changes a bridge's or bond's members, a bond's mode or a bridge's STP.
    pub fn change(&self, update: DeviceUpdate) -> Result<Edit, InvalidDevice> {
        let current = self.ours(&update.name)?.clone();
        let kind = match (&current.kind, update.mode, update.stp) {
            (DeviceKind::Vlan { .. }, ..) => {
                return invalid("a VLAN's parent and id stay as made: remove it and make another");
            }
            (DeviceKind::Bridge { .. }, Some(_), _) => return invalid("a bridge has no mode"),
            (DeviceKind::Bond { .. }, _, Some(_)) => return invalid("a bond has no STP"),
            (DeviceKind::Bridge { stp }, None, new) => DeviceKind::Bridge { stp: new.unwrap_or(*stp) },
            (DeviceKind::Bond { mode }, new, None) => DeviceKind::Bond { mode: new.unwrap_or(*mode) },
        };
        let bond = matches!(kind, DeviceKind::Bond { .. });
        if bond && update.members.is_empty() {
            return invalid("a bond needs at least one member");
        }
        self.check_members(&update.name, &update.members, bond)?;
        let device = Device { members: update.members.clone(), kind, ..current.clone() };
        if device == current {
            return invalid(format!("{} stays as it is", update.name));
        }
        self.check_no_cycle(&device)?;
        let mut links = Vec::new();
        let mut effects = Vec::new();
        for member in &update.members {
            if !current.members.contains(member) {
                links.push(port(member, &update.name));
                let dropped = self.has_settings(member);
                effects.push(Effect {
                    interface: member.clone(),
                    becomes: Becomes::Member { device: update.name.clone(), moved: false, dropped },
                });
            }
        }
        for member in current.members.iter().filter(|member| !update.members.contains(member)) {
            let (file, becomes) = self.leaving(off(member));
            links.push(file);
            effects.push(Effect { interface: member.clone(), becomes });
        }
        let subject = Subject::Device(update.name.clone(), DeviceAction::Changed);
        Ok(Edit { subject, devices: vec![device], removed: Vec::new(), links, unset: Vec::new(), effects })
    }

    /// Removes a bridge, bond or VLAN zendiloOS made. `users` is what uses
    /// it (VMs on a bridge): while there is any, it stays.
    pub fn remove(&self, name: &str, users: &[String]) -> Result<Edit, InvalidDevice> {
        let device = self.ours(name)?.clone();
        if !users.is_empty() {
            return invalid(format!("{name} is in use by {}", users.join(", ")));
        }
        if let Some(master) = self.master_of(name) {
            return invalid(format!("{name} is a member of {master}: take it out first"));
        }
        if let Some(vlan) = self.configuration.vlans_on(name).first() {
            return invalid(format!("{name} carries the VLAN {vlan}: remove it first"));
        }
        let mut links = Vec::new();
        let mut effects = Vec::new();
        match &device.kind {
            DeviceKind::Vlan { parent, .. } => {
                if let Some(mut carrier) = self.link_of(parent) {
                    carrier.vlans.retain(|vlan| vlan != name);
                    links.push(carrier);
                    effects.push(Effect {
                        interface: parent.clone(),
                        becomes: Becomes::Parent { vlan: name.to_owned(), carries: false },
                    });
                }
            }
            DeviceKind::Bridge { .. } | DeviceKind::Bond { .. } => {
                let settings = self.settings_of(name).unwrap_or_else(|| off(name));
                for (index, member) in device.members.iter().enumerate() {
                    let settings = if index == 0 {
                        InterfaceSettings { interface: member.clone(), ..settings.clone() }
                    } else {
                        off(member)
                    };
                    let (file, becomes) = self.leaving(settings);
                    links.push(file);
                    effects.push(Effect { interface: member.clone(), becomes });
                }
            }
        }
        let subject = Subject::Device(name.to_owned(), DeviceAction::Removed);
        Ok(Edit {
            subject,
            devices: Vec::new(),
            removed: vec![name.to_owned()],
            links,
            unset: Vec::new(),
            effects,
        })
    }

    /// An interface's settings, as before: a member's are its bridge's or
    /// bond's, and the VLANs on it stay.
    pub fn settings(&self, settings: InterfaceSettings) -> Result<Edit, InvalidDevice> {
        let interface = settings.interface.clone();
        if let Some(master) = self.master_of(&interface) {
            return invalid(format!("{interface} is a member of {master}: its settings are {master}'s"));
        }
        let link = LinkFile {
            port_of: None,
            vlans: self.configuration.vlans_on(&interface),
            keep_up: self.configuration.device(&interface).is_some(),
            settings: settings.clone(),
        };
        Ok(Edit {
            subject: Subject::Interface(interface.clone()),
            devices: Vec::new(),
            removed: Vec::new(),
            links: vec![link],
            unset: Vec::new(),
            effects: vec![Effect { interface, becomes: Becomes::Settings(settings) }],
        })
    }
}

/// No addresses: up if it carries others, else switched off.
fn off(interface: &str) -> InterfaceSettings {
    InterfaceSettings {
        interface: interface.to_owned(),
        ipv4: Ipv4Setting::Off,
        ipv6: Ipv6Setting::Off,
        dns: Vec::new(),
    }
}

fn port(member: &str, device: &str) -> LinkFile {
    LinkFile { settings: off(member), port_of: Some(device.to_owned()), vlans: Vec::new(), keep_up: false }
}

#[cfg(test)]
mod tests;
