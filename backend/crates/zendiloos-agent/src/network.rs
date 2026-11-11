//! Network configuration where systemd-networkd manages the network, and
//! taking it over from ifupdown ([`takeover`]).
//!
//! A change writes `05-zendiloos-<interface>.network` (sorted before what
//! netplan or the image put in `/run` or `/usr/lib`), keeps the file it
//! replaces beside it as `….previous`, and applies it. The agent has a
//! network namespace of its own (`PrivateNetwork`), where neither the
//! host's interfaces nor `networkctl` work; it asks systemd-networkd over
//! D-Bus instead, which names and reconfigures the host's links. Unless it is
//! confirmed in time, the previous file returns: by a timer in the agent,
//! and, should the agent or the machine restart first, when the agent
//! starts. So a setting that cuts the NAS off its network undoes itself.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{
    ConfirmWithin, InterfaceName, Ipv4Config, Ipv6Config, NetworkChange, NetworkConfig, PendingNetwork,
    Response,
};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

mod takeover;

const PREFIX: &str = "05-zendiloos-";
/// The first line of a `.previous` file when there was no file before.
const NO_FILE: &str = "# zendiloos: there was no file\n";
const HEADER: &str =
    "# Written by zendiloOS; changes here are replaced. Change the network in its interface.\n";

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
    paths: NetworkPaths,
    pending: Mutex<Option<Pending>>,
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
/// once that one released it. Its search domains are used, as ifupdown's
/// DHCP clients use them.
pub(crate) fn networkd_file(config: &NetworkConfig) -> String {
    let mut file = format!("{HEADER}[Match]\nName={}\n\n", config.interface.as_str());
    let off = matches!(config.ipv4, Ipv4Config::Off {}) && config.ipv6 == Ipv6Config::Off;
    if off {
        // Switched off: down, and nothing waits for it at boot.
        file.push_str("[Link]\nActivationPolicy=down\nRequiredForOnline=no\n\n");
    }
    file.push_str("[Network]\n");
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

/// What an interface's file becomes for a moment when a takeover goes
/// back: no IPv4 address, so systemd-networkd drops the static ones it
/// set, which outlive it and would stop ifupdown from setting them again.
fn unconfigured(config: &NetworkConfig) -> NetworkConfig {
    NetworkConfig {
        interface: config.interface.clone(),
        ipv4: Ipv4Config::Off {},
        ipv6: config.ipv6,
        dns: zendiloos_agent_proto::NameServers::default(),
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

impl Network {
    pub(crate) fn new(tools: &Tools, paths: NetworkPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            busctl: Program::new(&tools.busctl)?,
            paths,
            pending: Mutex::new(None),
        })
    }

    fn file(&self, interface: &InterfaceName) -> PathBuf {
        self.paths.directory.join(format!("{PREFIX}{}.network", interface.as_str()))
    }

    fn previous(&self, interface: &InterfaceName) -> PathBuf {
        self.paths.directory.join(format!("{PREFIX}{}.network.previous", interface.as_str()))
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
        Ok(Response::NetworkState { managed, pending, network_manager, ifupdown })
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

    /// Has systemd-networkd read the files again and set up `interface`.
    async fn reload(&self, interface: &InterfaceName) -> Result<(), OperationError> {
        let (index, _) = self.link(interface).await.ok_or_else(|| {
            OperationError::Refused(format!("there is no interface {}", interface.as_str()))
        })?;
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["Reload"]].concat()).await?;
        let index = index.to_string();
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["ReconfigureLink", "i", index.as_str()]].concat())
            .await
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
                    NetworkChange::Interface { interface } => {
                        tracing::warn!(
                            interface = interface.as_str(),
                            "network: not confirmed in time, the old settings return"
                        );
                        agent.restore(interface).await;
                    }
                    NetworkChange::Takeover { .. } => {
                        tracing::warn!("network: the takeover was not confirmed in time, ifupdown returns");
                        agent.undo_takeover().await;
                    }
                }
            }
        });
        Pending { change, revert_at: now() + i64::from(seconds), timer }
    }

    pub(crate) async fn apply(
        self: &Arc<Self>,
        config: &NetworkConfig,
        confirm_within: ConfirmWithin,
    ) -> Result<Response, OperationError> {
        if !self.managed().await {
            return Err(OperationError::Refused("systemd-networkd does not manage the network here".into()));
        }
        let interface = &config.interface;
        if self.link(interface).await.is_none() {
            return Err(OperationError::Refused(format!("there is no interface {}", interface.as_str())));
        }
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(OperationError::Conflict);
        }
        let file = self.file(interface);
        let before = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(error) if not_found(&error) => NO_FILE.to_owned(),
            Err(error) => return Err(OperationError::Read(file, error)),
        };
        fs::create_dir_all(&self.paths.directory)
            .map_err(|e| OperationError::Write(self.paths.directory.clone(), e))?;
        crate::durable::replace(&self.previous(interface), before.as_bytes())?;
        crate::durable::replace(&file, networkd_file(config).as_bytes())?;
        if let Err(error) = self.reload(interface).await {
            // Not even applied: back at once.
            self.restore(interface).await;
            return Err(error);
        }
        let seconds = confirm_within.get();
        *pending = Some(self.arm(NetworkChange::Interface { interface: interface.clone() }, seconds));
        tracing::info!(interface = interface.as_str(), seconds, "network: applied, waiting for confirmation");
        Ok(Response::Done)
    }

    pub(crate) async fn confirm(&self) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        let Some(change) = pending.as_ref() else {
            return Err(OperationError::Refused("no change waits for confirmation".into()));
        };
        match &change.change {
            NetworkChange::Interface { interface } => remove(&self.previous(interface))?,
            NetworkChange::Takeover { .. } => self.keep_takeover()?,
        }
        if let Some(change) = pending.take() {
            change.timer.abort();
            tracing::info!(change = ?change.change, "network: confirmed");
        }
        Ok(Response::Done)
    }

    /// Puts the previous file back (or removes ours if there was none) and
    /// applies it. Failures are logged: there is nobody to tell.
    async fn restore(&self, interface: &InterfaceName) {
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
                if let Err(error) = self.reload(interface).await {
                    tracing::error!(%error, interface = interface.as_str(), "network: cannot apply the old settings");
                }
            }
            Err(error) => {
                tracing::error!(%error, interface = interface.as_str(), "network: cannot put the old settings back");
            }
        }
    }

    /// At startup: changes nobody confirmed before the agent stopped go
    /// back, a takeover first.
    pub(crate) async fn restore_unconfirmed(&self) {
        let _pending = self.pending.lock().await;
        self.undo_takeover().await;
        let Ok(entries) = fs::read_dir(&self.paths.directory) else { return };
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
                self.restore(&interface).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{NameServers, Prefix};

    use super::*;

    fn config(ipv4: Ipv4Config, ipv6: Ipv6Config, dns: &[&str]) -> NetworkConfig {
        NetworkConfig {
            interface: InterfaceName::parse("enp1s0").unwrap(),
            ipv4,
            ipv6,
            dns: NameServers::new(dns.iter().map(|d| d.parse().unwrap()).collect()).unwrap(),
        }
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
}
