//! Network configuration where systemd-networkd manages the network, and
//! taking it over from ifupdown ([`takeover`]).
//!
//! A change writes zendiloOS's files, `05-zendiloos-<name>.network` for an
//! interface and `05-zendiloos-<name>.netdev` for a bridge, bond or VLAN it
//! makes (sorted before what netplan or the image put in `/run` or
//! `/usr/lib`), and applies them. One change may write several at once: a
//! bridge's `.netdev`, its own `.network` and its members'. First it writes
//! a record of every file it touches as it was, and of the devices it makes
//! (`zendiloos-change.json` beside them, root's alone), so a change undoes
//! itself as one, from wherever it got to.
//!
//! The agent has a network namespace of its own (`PrivateNetwork`), where
//! neither the host's interfaces nor `networkctl` work; it asks
//! systemd-networkd over D-Bus instead, which names and reconfigures the
//! host's links. systemd-networkd makes the devices of new `.netdev` files
//! when it reloads, but never removes one, nor changes one it made: a
//! device that goes, or whose kind of bond or bridge changes, is deleted
//! (`ip link delete`) in a transient unit, which runs in the host's
//! namespace, and made anew from its file when systemd-networkd reloads.
//!
//! Unless a change is confirmed in time, the files return as they were and
//! the devices it made go: by a timer in the agent, and, should the agent
//! or the machine restart first, when the agent starts. So a setting that
//! cuts the NAS off its network undoes itself. After each change and its
//! undoing, `zendiloos-docker-bridges.service` brings Docker's FORWARD
//! chain up to date with the bridges zendiloOS made ([`crate::docker_bridges`]).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{
    BondMode, ConfirmWithin, InterfaceName, Ipv4Config, Ipv6Config, Netdev, NetdevKind, NetworkChange,
    NetworkConfig, NetworkEdit, NetworkdFile, PendingNetwork, PortOf, Response,
};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::config::Tools;
use crate::operations::OperationError;

mod takeover;

pub(crate) const PREFIX: &str = "05-zendiloos-";
/// The first line of a `.previous` file when there was no file before: what
/// an agent before changes of several files kept, put back at the start.
const NO_FILE: &str = "# zendiloos: there was no file\n";
const HEADER: &str =
    "# Written by zendiloOS; changes here are replaced. Change the network in its interface.\n";
/// The note in a bridge's or bond's file of its members in order: the
/// first gets its settings back when it goes.
pub(crate) const MEMBERS: &str = "# Members: ";
/// The record of a change waiting to be confirmed, in the networkd
/// directory (which systemd-networkd ignores: no `.network` or `.netdev`).
const RECORD: &str = "zendiloos-change.json";
/// Brings Docker's FORWARD chain up to date with the bridges zendiloOS made.
const DOCKER_BRIDGES: &str = "zendiloos-docker-bridges.service";
/// The most files of zendiloOS's handed to zendiloosd, and their size.
const MAX_FILES: usize = 128;
const MAX_FILE: u64 = 64 * 1024;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };

/// `busctl call` to systemd-networkd's manager, before the method.
const MANAGER_CALL: &[&str] =
    &["call", "org.freedesktop.network1", "/org/freedesktop/network1", "org.freedesktop.network1.Manager"];

/// Where the network is configured; other than the real places in tests.
#[derive(Debug, Clone)]
pub struct NetworkPaths {
    /// `/etc/systemd/network`.
    pub directory: PathBuf,
    /// `/etc/network`: ifupdown's files, and the record of a takeover.
    pub ifupdown: PathBuf,
    /// `/etc/resolv.conf`.
    pub resolv_conf: PathBuf,
}

impl Default for NetworkPaths {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("/etc/systemd/network"),
            ifupdown: PathBuf::from("/etc/network"),
            resolv_conf: PathBuf::from("/etc/resolv.conf"),
        }
    }
}

#[derive(Debug)]
struct Pending {
    change: NetworkChange,
    revert_at: i64,
    timer: JoinHandle<()>,
}

#[derive(Debug)]
pub(crate) struct Network {
    systemctl: Program,
    busctl: Program,
    systemd_run: Program,
    /// Run in a transient unit, in the host's namespace: deletes a link.
    ip: PathBuf,
    paths: NetworkPaths,
    pending: Mutex<Option<Pending>>,
}

/// A file a change writes or removes, as it was before; none: there was
/// none.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    name: String,
    before: Option<String>,
}

/// Everything a change does, as it was before: what undoing it needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    change: NetworkChange,
    files: Vec<Saved>,
    /// Devices it made, or made anew: their links go when it is undone.
    made: Vec<InterfaceName>,
    /// Links set up again once the files are back.
    links: Vec<InterfaceName>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The systemd-networkd file for `config`.
///
/// DHCP identifies the NAS by its card's MAC address (`ClientIdentifier=mac`)
/// rather than by systemd's DUID, which comes from the machine ID and so
/// changes with every installation; and asks for the address the interface
/// has (`RequestAddress=`), so a server that keyed the lease by another
/// client's identifier, such as ifupdown's DHCP client, hands it out again
/// once that one released it. A bridge or bond zendiloOS makes has the MAC
/// address of the card whose settings it took, so its lease stays. Its
/// search domains are used, as ifupdown's DHCP clients use them.
///
/// A port of a bridge or bond has no addresses of its own; neither has an
/// interface without IPv4 and IPv6 that carries VLANs or is a device made to
/// carry others (`keep_up`): those stay up, while any other without
/// addresses is switched off.
pub(crate) fn networkd_file(config: &NetworkConfig) -> String {
    let mut file = format!("{HEADER}[Match]\nName={}\n\n", config.interface.as_str());
    let off = config.switched_off();
    let addressless = matches!(config.ipv4, Ipv4Config::Off {}) && config.ipv6 == Ipv6Config::Off;
    if off {
        // Switched off: down, and nothing waits for it at boot.
        file.push_str("[Link]\nActivationPolicy=down\nRequiredForOnline=no\n\n");
    } else if addressless && config.port_of.is_none() {
        // Up to carry others; nothing waits for an address it never gets.
        file.push_str("[Link]\nRequiredForOnline=no\n\n");
    }
    file.push_str("[Network]\n");
    match &config.port_of {
        Some(PortOf::Bridge { name }) => file.push_str(&format!("Bridge={}\n", name.as_str())),
        Some(PortOf::Bond { name }) => file.push_str(&format!("Bond={}\n", name.as_str())),
        None => {}
    }
    let dhcp = matches!(config.ipv4, Ipv4Config::Dhcp { .. });
    file.push_str(if dhcp { "DHCP=ipv4\n" } else { "DHCP=no\n" });
    match config.ipv6 {
        Ipv6Config::Automatic => file.push_str("IPv6AcceptRA=yes\n"),
        Ipv6Config::Off => file.push_str("IPv6AcceptRA=no\nLinkLocalAddressing=no\n"),
    }
    if let Ipv4Config::Static { address, prefix, gateway } = &config.ipv4 {
        file.push_str(&format!("Address={address}/{}\n", prefix.get()));
        if let Some(gateway) = gateway {
            file.push_str(&format!("Gateway={gateway}\n"));
        }
    }
    let dns = if off { &[][..] } else { config.dns.as_slice() };
    for server in dns {
        file.push_str(&format!("DNS={server}\n"));
    }
    for vlan in config.vlans.as_slice() {
        file.push_str(&format!("VLAN={}\n", vlan.as_str()));
    }
    if let Ipv4Config::Dhcp { request } = &config.ipv4 {
        file.push_str("\n[DHCPv4]\nClientIdentifier=mac\nUseDomains=yes\n");
        if let Some(address) = request {
            file.push_str(&format!("RequestAddress={address}\n"));
        }
        // Name servers given here replace those DHCP offers.
        if !dns.is_empty() {
            file.push_str("UseDNS=no\n");
        }
    }
    if !dns.is_empty() && config.ipv6 == Ipv6Config::Automatic {
        file.push_str("\n[IPv6AcceptRA]\nUseDNS=no\n");
    }
    file
}

/// systemd-networkd's name for a bond's mode.
fn bond_mode(mode: BondMode) -> &'static str {
    match mode {
        BondMode::ActiveBackup => "active-backup",
        BondMode::Lacp => "802.3ad",
        BondMode::BalanceAlb => "balance-alb",
    }
}

/// The `.netdev` file of a device zendiloOS makes. Its members are noted
/// for zendiloosd, in order: each member's own file says what it is a port
/// of. Bonds watch their members' links every 100 ms, which active-backup
/// and balance-alb need to notice a member failing; with LACP the switch
/// is asked to tell every second and traffic spreads by address and port.
pub(crate) fn netdev_file(netdev: &Netdev) -> String {
    let mut file = HEADER.to_owned();
    if !netdev.members.is_empty() {
        let members: Vec<&str> = netdev.members.iter().map(InterfaceName::as_str).collect();
        file.push_str(&format!("{MEMBERS}{}\n", members.join(" ")));
    }
    let kind = match netdev.kind {
        NetdevKind::Bridge { .. } => "bridge",
        NetdevKind::Bond { .. } => "bond",
        NetdevKind::Vlan { .. } => "vlan",
    };
    file.push_str(&format!("[NetDev]\nName={}\nKind={kind}\n", netdev.name.as_str()));
    if let Some(mac) = &netdev.mac {
        file.push_str(&format!("MACAddress={}\n", mac.as_str()));
    }
    match netdev.kind {
        NetdevKind::Bridge { stp } => {
            file.push_str(&format!("\n[Bridge]\nSTP={}\n", if stp { "yes" } else { "no" }))
        }
        NetdevKind::Bond { mode } => {
            file.push_str(&format!("\n[Bond]\nMode={}\nMIIMonitorSec=100ms\n", bond_mode(mode)));
            if mode == BondMode::Lacp {
                file.push_str("LACPTransmitRate=fast\nTransmitHashPolicy=layer3+4\n");
            }
        }
        NetdevKind::Vlan { id } => file.push_str(&format!("\n[VLAN]\nId={}\n", id.get())),
    }
    file
}

/// What a `.netdev` file makes, without the notes: when it changes, the
/// device is made anew.
fn definition(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).collect()
}

/// The kind a `.netdev` of zendiloOS's names (`bridge`, `bond`, `vlan`).
fn netdev_kind(text: &str) -> Option<&str> {
    text.lines().find_map(|line| line.trim().strip_prefix("Kind=")).map(str::trim)
}

/// What an interface's file becomes for a moment when a takeover goes
/// back: no IPv4 address, so systemd-networkd drops the static ones it
/// set, which outlive it and would stop ifupdown from setting them again.
fn unconfigured(config: &NetworkConfig) -> NetworkConfig {
    NetworkConfig {
        ipv4: Ipv4Config::Off {},
        dns: zendiloos_agent_proto::NameServers::default(),
        ..config.clone()
    }
}

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// Removes `path`; already gone is fine.
fn remove(path: &std::path::Path) -> Result<(), OperationError> {
    match fs::remove_file(path) {
        Err(error) if !not_found(&error) => Err(OperationError::Write(path.to_owned(), error)),
        _ => Ok(()),
    }
}

/// The interface a file of zendiloOS's is for, and whether it is a
/// `.netdev`; none for any other name.
fn ours(name: &str) -> Option<(InterfaceName, bool)> {
    let rest = name.strip_prefix(PREFIX)?;
    let (interface, netdev) = match rest.strip_suffix(".network") {
        Some(interface) => (interface, false),
        None => (rest.strip_suffix(".netdev")?, true),
    };
    Some((InterfaceName::parse(interface).ok()?, netdev))
}

impl Network {
    pub(crate) fn new(tools: &Tools, paths: NetworkPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            busctl: Program::new(&tools.busctl)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            ip: tools.ip.clone(),
            paths,
            pending: Mutex::new(None),
        })
    }

    fn file(&self, interface: &InterfaceName) -> PathBuf {
        self.paths.directory.join(network_name(interface))
    }

    fn netdev(&self, device: &InterfaceName) -> PathBuf {
        self.paths.directory.join(netdev_name(device))
    }

    fn previous(&self, interface: &InterfaceName) -> PathBuf {
        self.paths.directory.join(format!("{PREFIX}{}.network.previous", interface.as_str()))
    }

    fn change_record(&self) -> PathBuf {
        self.paths.directory.join(RECORD)
    }

    /// Whether `unit` is active.
    async fn active(&self, unit: &str) -> bool {
        let active = zendiloos_exec::run(&self.systemctl, &["is-active", "--quiet", unit], LIMITS).await;
        active.is_ok_and(|output| output.status.success())
    }

    async fn managed(&self) -> bool {
        self.active("systemd-networkd.service").await
    }

    /// `systemctl` with fixed arguments, waiting up to two minutes: stopping
    /// ifupdown releases DHCP leases.
    async fn systemctl(&self, args: &[&str]) -> Result<(), OperationError> {
        let limits = Limits { timeout: Duration::from_secs(120), ..LIMITS };
        zendiloos_exec::run(&self.systemctl, args, limits).await?.success(&self.systemctl)?;
        Ok(())
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let pending = self.pending.lock().await;
        let pending =
            pending.as_ref().map(|p| PendingNetwork { change: p.change.clone(), revert_at: p.revert_at });
        let managed = self.managed().await;
        let (network_manager, ifupdown) = if managed {
            (false, None)
        } else {
            (self.active("NetworkManager.service").await, self.ifupdown().await?)
        };
        Ok(Response::NetworkState { managed, pending, network_manager, ifupdown, files: self.files() })
    }

    /// zendiloOS's `.network` and `.netdev` files as they are, by name;
    /// what is no plain file or too large is left out.
    fn files(&self) -> Vec<NetworkdFile> {
        let Ok(entries) = fs::read_dir(&self.paths.directory) else { return Vec::new() };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter(|entry| entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| ours(name).is_some())
            .collect();
        names.sort();
        names.truncate(MAX_FILES);
        names
            .into_iter()
            .filter_map(|name| {
                let text = fs::read_to_string(self.paths.directory.join(&name)).ok()?;
                Some(NetworkdFile { name, text })
            })
            .collect()
    }

    /// systemd-networkd's index of `interface` and its object on the bus;
    /// `None` if it knows none.
    async fn link(&self, interface: &InterfaceName) -> Option<(i32, String)> {
        let asked = [MANAGER_CALL, &["GetLinkByName", "s", interface.as_str()]].concat();
        let output = zendiloos_exec::run(&self.busctl, &asked, LIMITS).await.ok()?;
        if !output.status.success() {
            return None;
        }
        // `io 2 "/org/freedesktop/network1/link/_32"`
        let text = String::from_utf8(output.stdout).ok()?;
        let mut words = text.split_whitespace();
        let index = words.nth(1)?.parse().ok()?;
        let object = words.next()?.trim_matches('"').to_owned();
        let plain = object.strip_prefix("/org/freedesktop/network1/link/").is_some_and(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
        Some((index, if plain { object } else { String::new() }))
    }

    async fn networkd_reload(&self) -> Result<(), OperationError> {
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["Reload"]].concat()).await
    }

    /// Has systemd-networkd set up `interface` again with its file, if it
    /// knows the link (a device it is about to make it sets up by itself).
    async fn reconfigure(&self, interface: &InterfaceName) -> Result<(), OperationError> {
        let Some((index, _)) = self.link(interface).await else { return Ok(()) };
        let index = index.to_string();
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["ReconfigureLink", "i", index.as_str()]].concat())
            .await
    }

    /// Deletes the link of a device zendiloOS made, in a transient unit:
    /// this sandbox has no network of the host's to delete it in. Already
    /// gone is fine.
    async fn delete_link(&self, device: &InterfaceName) -> Result<(), OperationError> {
        if self.link(device).await.is_none() {
            return Ok(());
        }
        let ip = self.ip.display().to_string();
        let args = [
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "--service-type=exec",
            "--description=zendiloOS: removing a network device",
            // Deleting a link needs this capability, and nothing else.
            "--property=CapabilityBoundingSet=CAP_NET_ADMIN",
            "--property=NoNewPrivileges=yes",
            "--property=ProtectSystem=strict",
            "--property=ProtectHome=yes",
            "--property=PrivateTmp=yes",
            ip.as_str(),
            "link",
            "delete",
            "dev",
            device.as_str(),
        ];
        let output =
            zendiloos_exec::run_combined(&self.systemd_run, args, &Surroundings::default(), LIMITS).await?;
        if output.status.success() {
            tracing::info!(device = device.as_str(), "network: device deleted");
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let why =
            text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("ip link delete failed");
        Err(OperationError::Failed(format!("{} was not deleted: {why}", device.as_str())))
    }

    /// Brings Docker's FORWARD chain up to date with the bridges zendiloOS
    /// made. Failures are logged: Docker may not be there.
    async fn sync_docker(&self) {
        if let Err(error) = self.systemctl(&["start", DOCKER_BRIDGES]).await {
            tracing::warn!(%error, "network: Docker's chain was not brought up to date with the bridges");
        }
    }

    /// Arms the timer that undoes `change` unless it is confirmed within
    /// `seconds`.
    fn arm(self: &Arc<Self>, change: NetworkChange, seconds: u16) -> Pending {
        let agent = Arc::clone(self);
        let waiting_for = change.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(seconds))).await;
            // Held while the old settings return: nothing else changes the
            // network meanwhile.
            let mut pending = agent.pending.lock().await;
            if pending.as_ref().is_some_and(|p| p.change == waiting_for) {
                *pending = None;
                match &waiting_for {
                    NetworkChange::Takeover { .. } => {
                        tracing::warn!("network: the takeover was not confirmed in time, ifupdown returns");
                        agent.undo_takeover().await;
                    }
                    change => {
                        tracing::warn!(?change, "network: not confirmed in time, the old settings return");
                        agent.undo_change().await;
                    }
                }
            }
        });
        Pending { change, revert_at: now() + i64::from(seconds), timer }
    }

    /// See [`zendiloos_agent_proto::Request::ChangeNetwork`].
    pub(crate) async fn change(
        self: &Arc<Self>,
        edit: &NetworkEdit,
        confirm_within: ConfirmWithin,
    ) -> Result<Response, OperationError> {
        if !self.managed().await {
            return Err(OperationError::Refused("systemd-networkd does not manage the network here".into()));
        }
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(OperationError::Conflict);
        }
        if fs::symlink_metadata(self.change_record()).is_ok() {
            return Err(OperationError::Refused("an earlier change is still being undone".into()));
        }
        let plan = self.check(edit).await?;
        let record = Record {
            change: edit.change.clone(),
            files: plan
                .files
                .iter()
                .map(|(name, _, before)| Saved { name: name.clone(), before: before.clone() })
                .collect(),
            made: plan.made.clone(),
            links: edit.links.iter().map(|link| link.interface.clone()).collect(),
        };
        let text =
            serde_json::to_string_pretty(&record).map_err(|e| OperationError::Failed(e.to_string()))?;
        fs::create_dir_all(&self.paths.directory)
            .map_err(|e| OperationError::Write(self.paths.directory.clone(), e))?;
        crate::durable::replace_mode(&self.change_record(), text.as_bytes(), 0o600)?;
        if let Err(error) = self.carry_out(edit, &plan).await {
            // Not even applied: back at once.
            tracing::error!(%error, "network: the change failed, the old settings return");
            self.undo_change().await;
            return Err(error);
        }
        self.sync_docker().await;
        let seconds = confirm_within.get();
        *pending = Some(self.arm(edit.change.clone(), seconds));
        tracing::info!(change = ?edit.change, seconds, "network: applied, waiting for confirmation");
        Ok(Response::Done)
    }

    /// What `edit` writes and deletes, checked against the host: devices
    /// removed are zendiloOS's, new ones take no name in use, every file is
    /// for an interface there is (or one made here), ports are of bridges
    /// and bonds zendiloOS has, VLANs are its own.
    async fn check(&self, edit: &NetworkEdit) -> Result<Plan, OperationError> {
        let refused = |why: String| Err(OperationError::Refused(why));
        let read = |path: &std::path::Path| match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if not_found(&error) => Ok(None),
            Err(error) => Err(OperationError::Read(path.to_owned(), error)),
        };
        // The kinds of the devices zendiloOS has once this is done.
        let mut kinds: Vec<(InterfaceName, String)> = Vec::new();
        for file in self.files() {
            let Some((device, true)) = ours(&file.name) else { continue };
            if edit.removed.contains(&device) || edit.netdevs.iter().any(|n| n.name == device) {
                continue;
            }
            kinds.extend(netdev_kind(&file.text).map(|kind| (device, kind.to_owned())));
        }
        let mut plan = Plan::default();
        for gone in &edit.removed {
            let Some(before) = read(&self.netdev(gone))? else {
                return refused(format!("{} was not made by zendiloOS", gone.as_str()));
            };
            plan.files.push((netdev_name(gone), None, Some(before)));
            plan.files.push((network_name(gone), None, read(&self.file(gone))?));
            plan.delete.push(gone.clone());
        }
        for netdev in &edit.netdevs {
            let before = read(&self.netdev(&netdev.name))?;
            let text = netdev_file(netdev);
            match &before {
                None if self.link(&netdev.name).await.is_some() => {
                    return refused(format!("there is an interface {} already", netdev.name.as_str()));
                }
                None => plan.made.push(netdev.name.clone()),
                Some(before) if definition(before) != definition(&text) => {
                    // systemd-networkd changes no device it made: made anew.
                    plan.made.push(netdev.name.clone());
                    plan.delete.push(netdev.name.clone());
                }
                Some(_) => {}
            }
            let kind = match netdev.kind {
                NetdevKind::Bridge { .. } => "bridge",
                NetdevKind::Bond { .. } => "bond",
                NetdevKind::Vlan { .. } => "vlan",
            };
            kinds.push((netdev.name.clone(), kind.to_owned()));
            plan.files.push((netdev_name(&netdev.name), Some(text), before));
        }
        let kind_of =
            |name: &InterfaceName| kinds.iter().find(|(device, _)| device == name).map(|(_, k)| k.as_str());
        for link in &edit.links {
            let made_here = edit.netdevs.iter().any(|netdev| netdev.name == link.interface);
            if !made_here && self.link(&link.interface).await.is_none() {
                return refused(format!("there is no interface {}", link.interface.as_str()));
            }
            match &link.port_of {
                Some(PortOf::Bridge { name }) if kind_of(name) != Some("bridge") => {
                    return refused(format!("{} is no bridge zendiloOS made", name.as_str()));
                }
                Some(PortOf::Bond { name }) if kind_of(name) != Some("bond") => {
                    return refused(format!("{} is no bond zendiloOS made", name.as_str()));
                }
                _ => {}
            }
            if let Some(vlan) = link.vlans.as_slice().iter().find(|vlan| kind_of(vlan) != Some("vlan")) {
                return refused(format!("{} is no VLAN zendiloOS made", vlan.as_str()));
            }
            plan.files.push((
                network_name(&link.interface),
                Some(networkd_file(link)),
                read(&self.file(&link.interface))?,
            ));
        }
        Ok(plan)
    }

    /// Writes the files, deletes the links of devices that go or are made
    /// anew, and has systemd-networkd read the files again (making the new
    /// devices) and set up each interface whose file changed.
    async fn carry_out(&self, edit: &NetworkEdit, plan: &Plan) -> Result<(), OperationError> {
        for (name, text, _) in &plan.files {
            let path = self.paths.directory.join(name);
            match text {
                Some(text) => crate::durable::replace(&path, text.as_bytes())?,
                None => remove(&path)?,
            }
        }
        for device in &plan.delete {
            self.delete_link(device).await?;
        }
        self.networkd_reload().await?;
        for link in &edit.links {
            self.reconfigure(&link.interface).await?;
        }
        Ok(())
    }

    pub(crate) async fn confirm(&self) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        let Some(change) = pending.as_ref() else {
            return Err(OperationError::Refused("no change waits for confirmation".into()));
        };
        match &change.change {
            NetworkChange::Takeover { .. } => self.keep_takeover()?,
            _ => remove(&self.change_record())?,
        }
        if let Some(change) = pending.take() {
            change.timer.abort();
            tracing::info!(change = ?change.change, "network: confirmed");
        }
        Ok(Response::Done)
    }

    /// Undoes the change its record names, from wherever it got to: every
    /// file as it was, the devices it made deleted, systemd-networkd's
    /// files read again (which makes the devices it removed anew) and the
    /// interfaces set up again. The record goes last, so an undoing cut off
    /// is done again at the next start. Failures are logged: there is
    /// nobody to tell.
    async fn undo_change(&self) {
        let path = self.change_record();
        let record = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if not_found(&error) => return,
            Err(error) => {
                tracing::error!(%error, "network: cannot read the change's record");
                return;
            }
        };
        let record: Record = match serde_json::from_str(&record) {
            Ok(record) => record,
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "network: the change's record cannot be read; going back by hand");
                return;
            }
        };
        let mut restored = true;
        for saved in &record.files {
            if ours(&saved.name).is_none() {
                tracing::error!(
                    name = saved.name,
                    "network: the record names a file that is not zendiloOS's"
                );
                restored = false;
                continue;
            }
            let file = self.paths.directory.join(&saved.name);
            let back = match &saved.before {
                Some(text) => crate::durable::replace(&file, text.as_bytes()),
                None => remove(&file),
            };
            if let Err(error) = back {
                tracing::error!(%error, "network: cannot put a file back");
                restored = false;
            }
        }
        for device in &record.made {
            if let Err(error) = self.delete_link(device).await {
                tracing::error!(%error, "network: going back, a device stays");
            }
        }
        if let Err(error) = self.networkd_reload().await {
            tracing::error!(%error, "network: going back, systemd-networkd did not reload");
        }
        for link in &record.links {
            if let Err(error) = self.reconfigure(link).await {
                tracing::error!(%error, interface = link.as_str(), "network: cannot apply the old settings");
            }
        }
        self.sync_docker().await;
        if !restored {
            tracing::error!("network: not all files are back; trying again at the next start");
            return;
        }
        if let Err(error) = remove(&path) {
            tracing::error!(%error, "network: cannot remove the change's record");
        }
    }

    /// Puts an agent's earlier `.previous` file back (or removes the file
    /// if there was none) and applies it.
    async fn restore_previous(&self, interface: &InterfaceName) {
        let (file, previous) = (self.file(interface), self.previous(interface));
        let restored = match fs::read_to_string(&previous) {
            Ok(content) if content == NO_FILE => {
                fs::remove_file(&file).or_else(|e| if not_found(&e) { Ok(()) } else { Err(e) })
            }
            Ok(content) => crate::durable::replace(&file, content.as_bytes()).map_err(std::io::Error::other),
            Err(error) => Err(error),
        };
        match restored.and_then(|()| fs::remove_file(&previous)) {
            Ok(()) => {
                let applied = match self.networkd_reload().await {
                    Ok(()) => self.reconfigure(interface).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = applied {
                    tracing::error!(%error, interface = interface.as_str(), "network: cannot apply the old settings");
                }
            }
            Err(error) => {
                tracing::error!(%error, interface = interface.as_str(), "network: cannot put the old settings back");
            }
        }
    }

    /// At startup: changes nobody confirmed before the agent stopped go
    /// back, a takeover first; Docker's chain follows the bridges there are.
    pub(crate) async fn restore_unconfirmed(&self) {
        let _pending = self.pending.lock().await;
        self.undo_takeover().await;
        if fs::symlink_metadata(self.change_record()).is_ok() {
            tracing::warn!("network: a change was not confirmed, the old settings return");
            self.undo_change().await;
        }
        if let Ok(entries) = fs::read_dir(&self.paths.directory) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let interface = name
                    .strip_prefix(PREFIX)
                    .and_then(|rest| rest.strip_suffix(".network.previous"))
                    .and_then(|interface| InterfaceName::parse(interface).ok());
                if let Some(interface) = interface {
                    tracing::warn!(
                        interface = interface.as_str(),
                        "network: a change was not confirmed, the old settings return"
                    );
                    self.restore_previous(&interface).await;
                }
            }
        }
        if self.managed().await {
            self.sync_docker().await;
        }
    }
}

/// What a change writes (`Some`) or removes (`None`), by name, with what
/// was there; the devices it makes and the links it deletes first.
#[derive(Debug, Default)]
struct Plan {
    files: Vec<(String, Option<String>, Option<String>)>,
    made: Vec<InterfaceName>,
    delete: Vec<InterfaceName>,
}

fn network_name(interface: &InterfaceName) -> String {
    format!("{PREFIX}{}.network", interface.as_str())
}

fn netdev_name(device: &InterfaceName) -> String {
    format!("{PREFIX}{}.netdev", device.as_str())
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{MacAddress, NameServers, Prefix, VlanId, Vlans};

    use super::*;

    fn name(text: &str) -> InterfaceName {
        InterfaceName::parse(text).unwrap()
    }

    fn config(ipv4: Ipv4Config, ipv6: Ipv6Config, dns: &[&str]) -> NetworkConfig {
        NetworkConfig::addresses(
            name("enp1s0"),
            ipv4,
            ipv6,
            NameServers::new(dns.iter().map(|d| d.parse().unwrap()).collect()).unwrap(),
        )
    }

    const DHCP: Ipv4Config = Ipv4Config::Dhcp { request: None };

    #[test]
    fn the_file_says_exactly_the_settings() {
        let fixed = Ipv4Config::Static {
            address: "192.168.1.10".parse().unwrap(),
            prefix: Prefix::new(24).unwrap(),
            gateway: Some("192.168.1.1".parse().unwrap()),
        };
        let file = networkd_file(&config(fixed, Ipv6Config::Automatic, &["192.168.1.1"]));
        assert!(file.starts_with(HEADER), "{file}");
        assert!(file.contains("[Match]\nName=enp1s0\n\n[Network]\n"), "{file}");
        assert!(file.contains(
            "DHCP=no\nIPv6AcceptRA=yes\nAddress=192.168.1.10/24\nGateway=192.168.1.1\nDNS=192.168.1.1\n"
        ));
        assert!(file.ends_with("\n[IPv6AcceptRA]\nUseDNS=no\n"), "{file}");
        assert!(!file.contains("[DHCPv4]") && !file.contains("[Link]"), "{file}");

        let file = networkd_file(&config(DHCP, Ipv6Config::Off, &[]));
        assert!(file.contains("DHCP=ipv4\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n"), "{file}");
        assert!(!file.contains("DNS=") && !file.contains("Address="), "{file}");

        // Name servers of its own replace those DHCP offers.
        let file = networkd_file(&config(DHCP, Ipv6Config::Off, &["9.9.9.9"]));
        assert!(
            file.ends_with("DNS=9.9.9.9\n\n[DHCPv4]\nClientIdentifier=mac\nUseDomains=yes\nUseDNS=no\n"),
            "{file}"
        );
    }

    /// DHCP keeps the address: the MAC as the client's identity, the
    /// address the interface has asked for.
    #[test]
    fn dhcp_names_the_card_and_asks_for_the_address_it_has() {
        let asking = Ipv4Config::Dhcp { request: Some("192.168.1.127".parse().unwrap()) };
        let file = networkd_file(&config(asking, Ipv6Config::Automatic, &[]));
        assert!(
            file.ends_with(
                "[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n\n\
                 [DHCPv4]\nClientIdentifier=mac\nUseDomains=yes\nRequestAddress=192.168.1.127\n"
            ),
            "{file}"
        );
    }

    #[test]
    fn an_interface_without_addresses_is_switched_off() {
        let file = networkd_file(&config(Ipv4Config::Off {}, Ipv6Config::Off, &["9.9.9.9"]));
        assert_eq!(
            file,
            format!(
                "{HEADER}[Match]\nName=enp1s0\n\n[Link]\nActivationPolicy=down\nRequiredForOnline=no\n\n\
                 [Network]\nDHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n"
            )
        );
        // IPv6 alone keeps it on.
        let file = networkd_file(&config(Ipv4Config::Off {}, Ipv6Config::Automatic, &[]));
        assert!(file.ends_with("[Network]\nDHCP=no\nIPv6AcceptRA=yes\n"), "{file}");
        let back = unconfigured(&config(DHCP, Ipv6Config::Automatic, &["1.1.1.1"]));
        assert_eq!(back.ipv4, Ipv4Config::Off {});
        assert_eq!((back.ipv6, back.dns.as_slice().len()), (Ipv6Config::Automatic, 0));
    }

    /// A port has no addresses and says what it is a port of; a link
    /// carrying VLANs names them; a device made to carry others stays up
    /// without addresses.
    #[test]
    fn ports_vlans_and_devices_without_addresses() {
        let port = NetworkConfig {
            port_of: Some(PortOf::Bridge { name: name("br0") }),
            vlans: Vlans::new(vec![name("enp1s0.10")]).unwrap(),
            ..config(Ipv4Config::Off {}, Ipv6Config::Off, &[])
        };
        assert_eq!(
            networkd_file(&port),
            format!(
                "{HEADER}[Match]\nName=enp1s0\n\n[Network]\nBridge=br0\nDHCP=no\nIPv6AcceptRA=no\n\
                 LinkLocalAddressing=no\nVLAN=enp1s0.10\n"
            )
        );
        let bonded = NetworkConfig { port_of: Some(PortOf::Bond { name: name("bond0") }), ..port.clone() };
        assert!(networkd_file(&bonded).contains("[Network]\nBond=bond0\n"));

        let bridge = NetworkConfig {
            interface: name("br0"),
            keep_up: true,
            ..config(Ipv4Config::Off {}, Ipv6Config::Off, &[])
        };
        let file = networkd_file(&bridge);
        assert!(file.contains("[Link]\nRequiredForOnline=no\n\n[Network]\nDHCP=no\n"), "{file}");
        assert!(!file.contains("ActivationPolicy"), "{file}");

        // A bridge with the card's DHCP settings: its MAC keeps the lease.
        let dhcp = NetworkConfig {
            interface: name("br0"),
            keep_up: true,
            ..config(
                Ipv4Config::Dhcp { request: Some("10.0.2.15".parse().unwrap()) },
                Ipv6Config::Automatic,
                &[],
            )
        };
        let file = networkd_file(&dhcp);
        assert!(!file.contains("[Link]"), "{file}");
        assert!(file.ends_with("ClientIdentifier=mac\nUseDomains=yes\nRequestAddress=10.0.2.15\n"), "{file}");
    }

    #[test]
    fn devices_are_written_with_their_members_noted() {
        let bridge = Netdev {
            name: name("br0"),
            kind: NetdevKind::Bridge { stp: false },
            mac: Some(MacAddress::parse("52:54:00:12:34:56").unwrap()),
            members: vec![name("enp1s0"), name("bond0")],
        };
        assert_eq!(
            netdev_file(&bridge),
            format!(
                "{HEADER}# Members: enp1s0 bond0\n[NetDev]\nName=br0\nKind=bridge\nMACAddress=52:54:00:12:34:56\n\n\
                 [Bridge]\nSTP=no\n"
            )
        );
        let lacp = Netdev {
            name: name("bond0"),
            kind: NetdevKind::Bond { mode: BondMode::Lacp },
            mac: None,
            members: vec![name("enp2s0"), name("enp3s0")],
        };
        let file = netdev_file(&lacp);
        assert!(
            file.ends_with("[Bond]\nMode=802.3ad\nMIIMonitorSec=100ms\nLACPTransmitRate=fast\nTransmitHashPolicy=layer3+4\n"),
            "{file}"
        );
        let backup = Netdev { kind: NetdevKind::Bond { mode: BondMode::ActiveBackup }, ..lacp.clone() };
        assert!(netdev_file(&backup).ends_with("[Bond]\nMode=active-backup\nMIIMonitorSec=100ms\n"));
        let alb = Netdev { kind: NetdevKind::Bond { mode: BondMode::BalanceAlb }, ..lacp.clone() };
        assert!(netdev_file(&alb).contains("Mode=balance-alb\n"));
        let vlan = Netdev {
            name: name("enp1s0.10"),
            kind: NetdevKind::Vlan { id: VlanId::new(10).unwrap() },
            mac: None,
            members: vec![],
        };
        assert_eq!(
            netdev_file(&vlan),
            format!("{HEADER}[NetDev]\nName=enp1s0.10\nKind=vlan\n\n[VLAN]\nId=10\n")
        );
        assert_eq!(netdev_kind(&netdev_file(&vlan)), Some("vlan"));

        // Other members are noted, the device stays; another mode makes it anew.
        let fewer = Netdev { members: vec![name("enp2s0")], ..lacp.clone() };
        assert_eq!(definition(&netdev_file(&fewer)), definition(&netdev_file(&lacp)));
        assert_ne!(definition(&netdev_file(&backup)), definition(&netdev_file(&lacp)));
    }

    #[test]
    fn only_zendiloos_files_are_its_own() {
        assert_eq!(ours("05-zendiloos-br0.netdev"), Some((name("br0"), true)));
        assert_eq!(ours("05-zendiloos-enp1s0.10.network"), Some((name("enp1s0.10"), false)));
        for other in [
            "05-zendiloos-enp1s0.network.previous",
            "10-netplan-enp1s0.network",
            "05-zendiloos-.network",
            "05-zendiloos-a b.netdev",
            "zendiloos-change.json",
        ] {
            assert_eq!(ours(other), None, "{other}");
        }
    }
}
