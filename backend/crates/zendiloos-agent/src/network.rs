//! Network configuration where systemd-networkd manages the network.
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{
    ConfirmWithin, InterfaceName, Ipv4Config, Ipv6Config, NetworkConfig, PendingNetwork, Response,
};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

const PREFIX: &str = "05-zendiloos-";
/// The first line of a `.previous` file when there was no file before.
const NO_FILE: &str = "# zendiloos: there was no file\n";

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };

/// `busctl call` to systemd-networkd's manager, before the method.
const MANAGER_CALL: &[&str] =
    &["call", "org.freedesktop.network1", "/org/freedesktop/network1", "org.freedesktop.network1.Manager"];

/// Where the network is configured; other than the real place in tests.
#[derive(Debug, Clone)]
pub struct NetworkPaths {
    /// `/etc/systemd/network`.
    pub directory: PathBuf,
}

impl Default for NetworkPaths {
    fn default() -> Self {
        Self { directory: PathBuf::from("/etc/systemd/network") }
    }
}

#[derive(Debug)]
struct Pending {
    interface: InterfaceName,
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
pub(crate) fn networkd_file(config: &NetworkConfig) -> String {
    let mut file = format!(
        "# Written by zendiloOS; changes here are replaced. Change the network in its interface.\n\
         [Match]\nName={}\n\n[Network]\n",
        config.interface.as_str()
    );
    let dhcp = config.ipv4 == Ipv4Config::Dhcp;
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
    for server in config.dns.as_slice() {
        file.push_str(&format!("DNS={server}\n"));
    }
    // Name servers given here replace those DHCP or the router offer.
    if !config.dns.as_slice().is_empty() {
        if dhcp {
            file.push_str("\n[DHCPv4]\nUseDNS=no\n");
        }
        if config.ipv6 == Ipv6Config::Automatic {
            file.push_str("\n[IPv6AcceptRA]\nUseDNS=no\n");
        }
    }
    file
}

fn write_atomically(path: &Path, content: &str) -> Result<(), OperationError> {
    let temporary = path.with_extension("zendiloos-new");
    fs::write(&temporary, content).map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))
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

    async fn managed(&self) -> bool {
        let active =
            zendiloos_exec::run(&self.systemctl, &["is-active", "--quiet", "systemd-networkd"], LIMITS).await;
        active.is_ok_and(|output| output.status.success())
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let pending = self.pending.lock().await;
        let pending = pending
            .as_ref()
            .map(|p| PendingNetwork { interface: p.interface.clone(), revert_at: p.revert_at });
        Ok(Response::NetworkState { managed: self.managed().await, pending })
    }

    /// systemd-networkd's index of `interface`; `None` if it knows none.
    async fn link(&self, interface: &InterfaceName) -> Option<i32> {
        let asked = [MANAGER_CALL, &["GetLinkByName", "s", interface.as_str()]].concat();
        let output = zendiloos_exec::run(&self.busctl, &asked, LIMITS).await.ok()?;
        if !output.status.success() {
            return None;
        }
        // `io 2 "/org/freedesktop/network1/link/_32"`
        String::from_utf8(output.stdout).ok()?.split_whitespace().nth(1)?.parse().ok()
    }

    /// Has systemd-networkd read the files again and set up `interface`.
    async fn reload(&self, interface: &InterfaceName) -> Result<(), OperationError> {
        let link = self.link(interface).await.ok_or_else(|| {
            OperationError::Refused(format!("there is no interface {}", interface.as_str()))
        })?;
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["Reload"]].concat()).await?;
        let index = link.to_string();
        crate::pools::run(&self.busctl, &[MANAGER_CALL, &["ReconfigureLink", "i", index.as_str()]].concat())
            .await
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => NO_FILE.to_owned(),
            Err(error) => return Err(OperationError::Read(file, error)),
        };
        fs::create_dir_all(&self.paths.directory)
            .map_err(|e| OperationError::Write(self.paths.directory.clone(), e))?;
        write_atomically(&self.previous(interface), &before)?;
        write_atomically(&file, &networkd_file(config))?;
        if let Err(error) = self.reload(interface).await {
            // Not even applied: back at once.
            self.restore(interface).await;
            return Err(error);
        }
        let seconds = confirm_within.get();
        let agent = Arc::clone(self);
        let waiting_for = interface.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(seconds))).await;
            let mut pending = agent.pending.lock().await;
            if pending.as_ref().is_some_and(|p| p.interface == waiting_for) {
                *pending = None;
                drop(pending);
                tracing::warn!(
                    interface = waiting_for.as_str(),
                    "network: not confirmed in time, the old settings return"
                );
                agent.restore(&waiting_for).await;
            }
        });
        let revert_at = now() + i64::from(seconds);
        *pending = Some(Pending { interface: interface.clone(), revert_at, timer });
        tracing::info!(interface = interface.as_str(), seconds, "network: applied, waiting for confirmation");
        Ok(Response::Done)
    }

    pub(crate) async fn confirm(&self) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        let Some(change) = pending.take() else {
            return Err(OperationError::Refused("no change waits for confirmation".into()));
        };
        change.timer.abort();
        let previous = self.previous(&change.interface);
        fs::remove_file(&previous).map_err(|e| OperationError::Write(previous, e))?;
        tracing::info!(interface = change.interface.as_str(), "network: confirmed");
        Ok(Response::Done)
    }

    /// Puts the previous file back (or removes ours if there was none) and
    /// applies it. Failures are logged: there is nobody to tell.
    async fn restore(&self, interface: &InterfaceName) {
        let (file, previous) = (self.file(interface), self.previous(interface));
        let restored = match fs::read_to_string(&previous) {
            Ok(content) if content == NO_FILE => fs::remove_file(&file)
                .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
            Ok(content) => fs::write(&file, content),
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
    /// back.
    pub(crate) async fn restore_unconfirmed(&self) {
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

    #[test]
    fn the_file_says_exactly_the_settings() {
        let fixed = Ipv4Config::Static {
            address: "192.168.1.10".parse().unwrap(),
            prefix: Prefix::new(24).unwrap(),
            gateway: Some("192.168.1.1".parse().unwrap()),
        };
        let file = networkd_file(&config(fixed, Ipv6Config::Automatic, &["192.168.1.1"]));
        assert!(file.contains("[Match]\nName=enp1s0\n"));
        assert!(file.contains(
            "DHCP=no\nIPv6AcceptRA=yes\nAddress=192.168.1.10/24\nGateway=192.168.1.1\nDNS=192.168.1.1\n"
        ));
        assert!(file.ends_with("\n[IPv6AcceptRA]\nUseDNS=no\n"), "{file}");

        let file = networkd_file(&config(Ipv4Config::Dhcp, Ipv6Config::Off, &[]));
        assert!(file.contains("DHCP=ipv4\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n"), "{file}");
        assert!(!file.contains("DNS") && !file.contains("Address="), "{file}");
    }
}
