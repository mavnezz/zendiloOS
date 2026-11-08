//! The firewall's rules: one ordered list; for a new connection the first
//! rule that matches decides, and when none does, the fallback. A rule
//! names an interface (or any), a service (all ports, one of the NAS's
//! services, a Docker project, or ports), who it is about (anyone, an
//! address, a network, a range) and whether they may in.
//!
//! The services stand for ports the NAS uses now ([`Facts`]): the web
//! interface's, the SSH server's, a project's published ports. Rules are
//! compiled into a [`Ruleset`] of plain matches the host enforces; they act
//! on connections to the NAS itself, and those for all ports, a project or
//! ports also on connections Docker forwards to its containers.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why something given cannot be taken, in words for the person who gave it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

fn invalid(text: impl Into<String>) -> Invalid {
    Invalid(text.into())
}

/// Whether a rule lets a connection in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Allow, Self::Deny].into_iter().find(|action| action.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transport {
    Tcp,
    Udp,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Tcp, Self::Udp].into_iter().find(|transport| transport.as_str() == text)
    }
}

/// What ports given by number are about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Both,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Both => "both",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Tcp, Self::Udp, Self::Both].into_iter().find(|protocol| protocol.as_str() == text)
    }

    pub fn carries(self, transport: Transport) -> bool {
        match self {
            Self::Both => true,
            Self::Tcp => transport == Transport::Tcp,
            Self::Udp => transport == Transport::Udp,
        }
    }
}

/// Ports from `from` to `to`, both included; 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PortRange {
    from: u16,
    to: u16,
}

impl PortRange {
    pub fn new(from: u16, to: u16) -> Result<Self, Invalid> {
        if from == 0 || to == 0 {
            return Err(invalid("port 0 is no port"));
        }
        if from > to {
            return Err(invalid(format!("{from}-{to} runs backwards")));
        }
        Ok(Self { from, to })
    }

    pub fn single(port: u16) -> Result<Self, Invalid> {
        Self::new(port, port)
    }

    pub fn from(self) -> u16 {
        self.from
    }

    pub fn to(self) -> u16 {
        self.to
    }

    pub fn contains(self, port: u16) -> bool {
        (self.from..=self.to).contains(&port)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.from == self.to { write!(f, "{}", self.from) } else { write!(f, "{}-{}", self.from, self.to) }
    }
}

/// Most ports and ranges one rule names.
pub const MAX_PORTS: usize = 32;

/// Ports as people write them: `80, 443, 8000-8100`.
pub fn parse_ports(text: &str) -> Result<Vec<PortRange>, Invalid> {
    let number = |part: &str| {
        part.trim().parse::<u16>().map_err(|_| invalid(format!("{} is no port from 1 to 65535", part.trim())))
    };
    let ports = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('-') {
            Some((from, to)) => PortRange::new(number(from)?, number(to)?),
            None => PortRange::single(number(part)?),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ports.is_empty() {
        return Err(invalid("name at least one port"));
    }
    if ports.len() > MAX_PORTS {
        return Err(invalid(format!("at most {MAX_PORTS} ports or ranges in one rule")));
    }
    Ok(ports)
}

/// Ports as [`parse_ports`] reads them.
pub fn ports_text(ports: &[PortRange]) -> String {
    ports.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

/// Ranges sorted, with those that overlap or touch joined.
pub fn merged(mut ranges: Vec<PortRange>) -> Vec<PortRange> {
    ranges.sort();
    let mut joined: Vec<PortRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match joined.last_mut() {
            Some(last) if u32::from(range.from) <= u32::from(last.to) + 1 => last.to = last.to.max(range.to),
            _ => joined.push(range),
        }
    }
    joined
}

/// A service of the NAS a rule can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KnownService {
    /// Its HTTP and HTTPS ports, also those of a change waiting for
    /// confirmation.
    WebInterface,
    /// Samba: TCP 139 and 445, NetBIOS over UDP 137 and 138.
    Smb,
    /// NFS with rpcbind, mountd, statd and lockd on the ports the package
    /// fixes for them.
    Nfs,
    /// The SSH server's port.
    Ssh,
    /// Being found: WS-Discovery (wsdd2), mDNS (Avahi) and LLMNR.
    Discovery,
    /// ICMP echo requests, IPv4 and IPv6.
    Ping,
}

impl KnownService {
    pub const ALL: [Self; 6] =
        [Self::WebInterface, Self::Smb, Self::Nfs, Self::Ssh, Self::Discovery, Self::Ping];

    pub fn key(self) -> &'static str {
        match self {
            Self::WebInterface => "web_interface",
            Self::Smb => "smb",
            Self::Nfs => "nfs",
            Self::Ssh => "ssh",
            Self::Discovery => "discovery",
            Self::Ping => "ping",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|service| service.key() == key)
    }
}

/// Samba's ports.
pub const SMB_TCP: [u16; 2] = [139, 445];
pub const SMB_UDP: [u16; 2] = [137, 138];
/// NFS (2049), rpcbind (111), and mountd, statd and lockd on the ports the
/// package sets in `/etc/nfs.conf.d/zendiloos.conf`: TCP and UDP each.
/// All below Linux's range for outgoing connections (32768 to 60999), so
/// no program's own connection takes one of them first.
pub const NFS_PORTS: [u16; 5] = [111, 2049, 20048, 32765, 32767];
/// LLMNR (5355) and WS-Discovery's HTTP (5357); WS-Discovery (3702), mDNS
/// (5353) and LLMNR over UDP.
pub const DISCOVERY_TCP: [u16; 2] = [5355, 5357];
pub const DISCOVERY_UDP: [u16; 3] = [3702, 5353, 5355];

/// What a rule is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    /// Every port and protocol.
    All,
    Known(KnownService),
    /// The ports a Docker Compose project's containers publish.
    Project(String),
    Ports {
        protocol: Protocol,
        ports: Vec<PortRange>,
    },
}

impl Service {
    /// The ports it stands for now.
    pub fn reach(&self, facts: &Facts) -> Reach {
        match self {
            Self::All => Reach { all: true, ..Reach::default() },
            Self::Known(KnownService::WebInterface) => Reach::ports(&facts.web_ports, &[]),
            Self::Known(KnownService::Smb) => Reach::ports(&SMB_TCP, &SMB_UDP),
            Self::Known(KnownService::Nfs) => Reach::ports(&NFS_PORTS, &NFS_PORTS),
            Self::Known(KnownService::Ssh) => Reach::ports(&[facts.ssh_port], &[]),
            Self::Known(KnownService::Discovery) => Reach::ports(&DISCOVERY_TCP, &DISCOVERY_UDP),
            Self::Known(KnownService::Ping) => Reach { ping: true, ..Reach::default() },
            Self::Project(name) => {
                let ports = facts.projects.iter().filter(|p| p.name == *name).flat_map(|p| p.ports.iter());
                let (tcp, udp): (Vec<_>, Vec<_>) =
                    ports.partition(|(transport, _)| *transport == Transport::Tcp);
                let numbers = |list: Vec<&(Transport, u16)>| {
                    list.into_iter().map(|(_, port)| *port).collect::<Vec<_>>()
                };
                Reach::ports(&numbers(tcp), &numbers(udp))
            }
            Self::Ports { protocol, ports } => {
                let of =
                    |transport| if protocol.carries(transport) { merged(ports.clone()) } else { Vec::new() };
                Reach { tcp: of(Transport::Tcp), udp: of(Transport::Udp), ..Reach::default() }
            }
        }
    }

    /// Whether it also acts on connections Docker forwards to containers:
    /// those for all ports, a project or ports; the NAS's own services
    /// listen on the NAS itself.
    pub fn forwards(&self) -> bool {
        matches!(self, Self::All | Self::Project(_) | Self::Ports { .. })
    }
}

/// The ports a service stands for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    pub all: bool,
    pub ping: bool,
    pub tcp: Vec<PortRange>,
    pub udp: Vec<PortRange>,
}

impl Reach {
    fn ports(tcp: &[u16], udp: &[u16]) -> Self {
        let ranges =
            |ports: &[u16]| merged(ports.iter().filter_map(|port| PortRange::single(*port).ok()).collect());
        Self { tcp: ranges(tcp), udp: ranges(udp), ..Self::default() }
    }

    fn ranges(&self, transport: Transport) -> &[PortRange] {
        match transport {
            Transport::Tcp => &self.tcp,
            Transport::Udp => &self.udp,
        }
    }

    pub fn has(&self, transport: Transport, port: u16) -> bool {
        self.all || self.ranges(transport).iter().any(|range| range.contains(port))
    }

    /// Every port it stands for, one by one.
    fn endpoints(&self) -> Vec<(Transport, u16)> {
        [Transport::Tcp, Transport::Udp]
            .into_iter()
            .flat_map(|t| self.ranges(t).iter().flat_map(move |r| (r.from..=r.to).map(move |p| (t, p))))
            .collect()
    }
}

/// A Docker Compose project's name: lowercase letters, digits, `-` and
/// `_`, beginning with a letter or digit; at most 63 characters.
pub fn is_project_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A network interface's name as the kernel takes it: 1 to 15 characters
/// of letters, digits, `-`, `_` and `.`, not beginning with `-` or `.`;
/// not the loopback.
pub fn is_interface_name(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "lo"
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.starts_with(['-', '.'])
}

/// Who a rule is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Any,
    Address(IpAddr),
    /// The network's address, its host bits zero, and its prefix length.
    Network {
        address: IpAddr,
        prefix: u8,
    },
    /// Both ends included, of one family.
    Range {
        from: IpAddr,
        to: IpAddr,
    },
}

/// An IPv4 address an IPv6 socket saw (`::ffff:192.0.2.1`) as IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

fn full(ip: IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

/// `ip` with all but the first `prefix` bits zero.
fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix.min(32))) };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(prefix.min(128))) };
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

fn address(text: &str) -> Result<IpAddr, Invalid> {
    text.trim()
        .parse::<IpAddr>()
        .map(canonical)
        .map_err(|_| invalid(format!("{} is no IP address", text.trim())))
}

impl Source {
    /// `any` (or nothing), an address, a network as `192.168.1.0/24` (host
    /// bits are dropped), or a range as `192.168.1.10-192.168.1.20`.
    pub fn parse(text: &str) -> Result<Self, Invalid> {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("any") {
            return Ok(Self::Any);
        }
        if let Some((ip, prefix)) = text.split_once('/') {
            let ip = address(ip)?;
            let prefix = prefix
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= full(ip))
                .ok_or_else(|| invalid(format!("/{} is no prefix length for {ip}", prefix.trim())))?;
            return Self::network(ip, prefix);
        }
        if let Some((from, to)) = text.split_once('-') {
            return Self::range(address(from)?, address(to)?);
        }
        Ok(Self::Address(address(text)?))
    }

    /// A network; one of a single address is that address.
    pub fn network(ip: IpAddr, prefix: u8) -> Result<Self, Invalid> {
        let ip = canonical(ip);
        if prefix > full(ip) {
            return Err(invalid(format!("/{prefix} is no prefix length for {ip}")));
        }
        if prefix == full(ip) {
            return Ok(Self::Address(ip));
        }
        Ok(Self::Network { address: masked(ip, prefix), prefix })
    }

    pub fn range(from: IpAddr, to: IpAddr) -> Result<Self, Invalid> {
        let (from, to) = (canonical(from), canonical(to));
        if from.is_ipv4() != to.is_ipv4() {
            return Err(invalid("a range runs between two addresses of one kind, IPv4 or IPv6"));
        }
        if from > to {
            return Err(invalid(format!("{from}-{to} runs backwards")));
        }
        if from == to {
            return Ok(Self::Address(from));
        }
        Ok(Self::Range { from, to })
    }

    /// The same, checked: what [`Source::parse`] would make of it.
    fn checked(self) -> Result<Self, Invalid> {
        match self {
            Self::Any => Ok(Self::Any),
            Self::Address(ip) => Ok(Self::Address(canonical(ip))),
            Self::Network { address, prefix } => Self::network(address, prefix),
            Self::Range { from, to } => Self::range(from, to),
        }
    }

    pub fn is_any(self) -> bool {
        self == Self::Any
    }

    pub fn contains(self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        match self {
            Self::Any => true,
            Self::Address(address) => address == ip,
            Self::Network { address, prefix } => {
                address.is_ipv4() == ip.is_ipv4() && masked(ip, prefix) == address
            }
            Self::Range { from, to } => from.is_ipv4() == ip.is_ipv4() && from <= ip && ip <= to,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::Address(ip) => write!(f, "{ip}"),
            Self::Network { address, prefix } => write!(f, "{address}/{prefix}"),
            Self::Range { from, to } => write!(f, "{from}-{to}"),
        }
    }
}

/// One rule of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// A rule switched off is kept but does nothing.
    pub enabled: bool,
    /// `None`: on every interface.
    pub interface: Option<String>,
    pub service: Service,
    pub source: Source,
    pub action: Action,
}

/// Most rules in the list.
pub const MAX_RULES: usize = 200;

/// The firewall's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallSettings {
    pub enabled: bool,
    /// What happens to a connection no rule matches.
    pub fallback: Action,
    /// The first that matches decides.
    pub rules: Vec<Rule>,
}

impl Default for FirewallSettings {
    /// Off; once on, everything stays allowed until rules say otherwise.
    fn default() -> Self {
        Self { enabled: false, fallback: Action::Allow, rules: Vec::new() }
    }
}

impl FirewallSettings {
    pub fn check(self) -> Result<Self, Invalid> {
        if self.rules.len() > MAX_RULES {
            return Err(invalid(format!("at most {MAX_RULES} rules")));
        }
        let rules = self
            .rules
            .into_iter()
            .enumerate()
            .map(|(index, rule)| rule.checked().map_err(|e| invalid(format!("rule {}: {}", index + 1, e.0))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { rules, ..self })
    }
}

impl Rule {
    fn checked(self) -> Result<Self, Invalid> {
        if let Some(name) = &self.interface
            && !is_interface_name(name)
        {
            return Err(invalid(format!("{name} is no interface name")));
        }
        match &self.service {
            Service::Project(name) if !is_project_name(name) => {
                return Err(invalid(format!("{name} is no Docker project name")));
            }
            Service::Ports { ports, .. } if ports.is_empty() => {
                return Err(invalid("name at least one port"));
            }
            Service::Ports { ports, .. } if ports.len() > MAX_PORTS => {
                return Err(invalid(format!("at most {MAX_PORTS} ports or ranges in one rule")));
            }
            _ => {}
        }
        Ok(Self { source: self.source.checked()?, ..self })
    }
}

/// A Docker Compose project and the host ports its running containers
/// publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPorts {
    pub name: String,
    pub ports: Vec<(Transport, u16)>,
}

/// What the NAS runs now, which the services stand for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The web interface's ports: plain HTTP (if served) and HTTPS, also
    /// those of a change waiting for confirmation.
    pub web_ports: Vec<u16>,
    /// The HTTPS port in use, which a browser reaches the interface on.
    pub https_port: u16,
    pub ssh_port: u16,
    pub ssh_on: bool,
    pub smb_on: bool,
    /// The NFS server runs and exports at least one folder.
    pub nfs_on: bool,
    /// WS-Discovery or Bonjour is on.
    pub discovery_on: bool,
    pub projects: Vec<ProjectPorts>,
    /// Docker has not answered since zendiloosd started: `projects` says
    /// nothing, the ports saved when it last answered stand for them.
    pub projects_unknown: bool,
    /// Docker's bridges besides `docker0` (`br-` and 12 hex digits), by
    /// name: containers on them reach the NAS, and only these.
    pub bridges: Vec<String>,
}

impl Default for Facts {
    fn default() -> Self {
        Self {
            web_ports: vec![80, 443],
            https_port: 443,
            ssh_port: 22,
            ssh_on: false,
            smb_on: false,
            nfs_on: false,
            discovery_on: false,
            projects: Vec::new(),
            projects_unknown: false,
            bridges: Vec::new(),
        }
    }
}

/// What a compiled rule matches beyond interface and source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Traffic {
    All,
    Ping,
    Tcp(Vec<PortRange>),
    Udp(Vec<PortRange>),
}

/// A rule as the host enforces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    pub interface: Option<String>,
    pub source: Source,
    pub traffic: Traffic,
    /// Also on connections Docker forwards to containers (matched by the
    /// published port), not only on those to the NAS.
    pub forward: bool,
    pub action: Action,
}

/// The rules as the host enforces them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruleset {
    pub rules: Vec<Compiled>,
    pub fallback: Action,
    /// Docker's bridges besides `docker0`, from [`Facts::bridges`].
    pub bridges: Vec<String>,
}

/// The rules switched on, with their services resolved to the ports they
/// stand for now; `None` while the firewall is off. A rule whose service
/// stands for nothing (a project without published ports) is left out.
pub fn compile(settings: &FirewallSettings, facts: &Facts) -> Option<Ruleset> {
    if !settings.enabled {
        return None;
    }
    let mut rules = Vec::new();
    for rule in settings.rules.iter().filter(|rule| rule.enabled) {
        let reach = rule.service.reach(facts);
        let mut traffic = Vec::new();
        if reach.all {
            traffic.push(Traffic::All);
        } else {
            if reach.ping {
                traffic.push(Traffic::Ping);
            }
            if !reach.tcp.is_empty() {
                traffic.push(Traffic::Tcp(reach.tcp));
            }
            if !reach.udp.is_empty() {
                traffic.push(Traffic::Udp(reach.udp));
            }
        }
        rules.extend(traffic.into_iter().map(|traffic| Compiled {
            interface: rule.interface.clone(),
            source: rule.source,
            traffic,
            forward: rule.service.forwards(),
            action: rule.action,
        }));
    }
    let mut bridges = facts.bridges.clone();
    bridges.sort_unstable();
    bridges.dedup();
    Some(Ruleset { rules, fallback: settings.fallback, bridges })
}

/// How a request reached the web interface, as far as known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caller {
    pub address: Option<IpAddr>,
    /// The interface it came in on.
    pub interface: Option<String>,
    /// The port it was sent to.
    pub port: Option<u16>,
}

/// Whether `caller` would still reach the web interface under `settings`:
/// the first rule that matches decides, or the fallback. A rule for one
/// interface counts when it is not known which one the caller came in on
/// only if it denies: so this says no where it cannot be sure.
pub fn admits(settings: &FirewallSettings, facts: &Facts, caller: &Caller) -> bool {
    let Some(address) = caller.address.map(canonical) else { return true };
    if !settings.enabled || address.is_loopback() {
        return true;
    }
    let port = caller.port.unwrap_or(facts.https_port);
    for rule in settings.rules.iter().filter(|rule| rule.enabled) {
        let on_interface = match (&rule.interface, &caller.interface) {
            (None, _) => true,
            (Some(rule), Some(came)) => rule == came,
            (Some(_), None) => rule.action == Action::Deny,
        };
        if on_interface
            && rule.source.contains(address)
            && rule.service.reach(facts).has(Transport::Tcp, port)
        {
            return rule.action == Action::Allow;
        }
    }
    settings.fallback == Action::Allow
}

/// A service that is on, which the rules could block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Exposed {
    Known(KnownService),
    Project(String),
}

impl Exposed {
    /// `smb`, `project:<name>`.
    pub fn key(&self) -> String {
        match self {
            Self::Known(service) => service.key().to_owned(),
            Self::Project(name) => format!("project:{name}"),
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        match key.strip_prefix("project:") {
            Some(name) if is_project_name(name) => Some(Self::Project(name.to_owned())),
            Some(_) => None,
            None => KnownService::parse(key).filter(|s| *s != KnownService::Ping).map(Self::Known),
        }
    }

    pub fn service(&self) -> Service {
        match self {
            Self::Known(service) => Service::Known(*service),
            Self::Project(name) => Service::Project(name.clone()),
        }
    }
}

/// The services on now: the web interface always, the others when they
/// run, projects that publish ports.
pub fn exposed(facts: &Facts) -> Vec<Exposed> {
    let mut on = vec![Exposed::Known(KnownService::WebInterface)];
    for (running, service) in [
        (facts.smb_on, KnownService::Smb),
        (facts.nfs_on, KnownService::Nfs),
        (facts.ssh_on, KnownService::Ssh),
        (facts.discovery_on, KnownService::Discovery),
    ] {
        if running {
            on.push(Exposed::Known(service));
        }
    }
    on.extend(
        facts.projects.iter().filter(|p| !p.ports.is_empty()).map(|p| Exposed::Project(p.name.clone())),
    );
    on
}

/// The services on now that nobody reaches under `settings`: going down
/// the rules, one that allows any of their ports first means someone gets
/// in; one that denies all of them to anyone on every interface first, or
/// the fallback denying, means nobody does. Rules naming only some
/// addresses or one interface do not decide it.
pub fn blocked(settings: &FirewallSettings, facts: &Facts) -> Vec<Exposed> {
    if !settings.enabled {
        return Vec::new();
    }
    exposed(facts)
        .into_iter()
        .filter(|service| {
            let endpoints = service.service().reach(facts).endpoints();
            let forwarded = matches!(service, Exposed::Project(_));
            for rule in settings.rules.iter().filter(|rule| rule.enabled) {
                if forwarded && !rule.service.forwards() {
                    continue;
                }
                let reach = rule.service.reach(facts);
                if !endpoints.iter().any(|(t, p)| reach.has(*t, *p)) {
                    continue;
                }
                if rule.action == Action::Allow {
                    return false;
                }
                if rule.source.is_any()
                    && rule.interface.is_none()
                    && endpoints.iter().all(|(t, p)| reach.has(*t, *p))
                {
                    return true;
                }
            }
            settings.fallback == Action::Deny
        })
        .collect()
}

/// An interface of the NAS and its addresses with their prefix lengths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInterface {
    pub name: String,
    pub addresses: Vec<(IpAddr, u8)>,
}

/// The networks the NAS is in, for rules about "the local network": its
/// addresses' networks, but loopback and link-local ones.
pub fn local_networks(interfaces: &[LocalInterface]) -> Vec<Source> {
    let mut networks = Vec::new();
    for (ip, prefix) in interfaces.iter().flat_map(|i| i.addresses.iter()) {
        let link_local = match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        };
        if ip.is_loopback() || link_local {
            continue;
        }
        if let Ok(network) = Source::network(*ip, *prefix)
            && !networks.contains(&network)
        {
            networks.push(network);
        }
    }
    networks
}

/// The interface that has `ip`.
pub fn interface_of(interfaces: &[LocalInterface], ip: IpAddr) -> Option<String> {
    let ip = canonical(ip);
    interfaces.iter().find(|i| i.addresses.iter().any(|(a, _)| canonical(*a) == ip)).map(|i| i.name.clone())
}

#[cfg(test)]
mod tests;
