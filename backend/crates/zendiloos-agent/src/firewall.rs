//! The firewall: one nftables table of zendiloOS's own, `inet
//! zendiloos_firewall`, which nothing else touches and which touches
//! nothing else (Docker's tables stay as Docker made them).
//!
//! The rules go to `/etc/zendiloos/firewall/zendiloos.nft`, which replaces
//! the table in one transaction; `zendiloos-firewall.service` loads that
//! file at boot, before the network comes up, and when the agent restarts
//! it. A file is first checked with `nft -c`. Both run outside this
//! sandbox, which has no network of its own to filter (`PrivateNetwork`):
//! the check in a transient unit, the loading by the unit.
//!
//! A change that must be confirmed keeps the file it replaces beside it
//! (`zendiloos.nft.previous`); unless it is confirmed in time, that file
//! returns: by a timer in the agent and, should the agent or the machine
//! restart first, when the agent starts. So rules that shut the browser out
//! undo themselves.
//!
//! The table has two chains. `input` filters new connections to the NAS:
//! answers, the loopback, Docker's bridges, what IPv6 and DHCP need, then
//! the rules, then the fallback. `forward` filters new connections Docker
//! forwards to its containers' published ports (`ct status dnat`), matched
//! by the port they were sent to; the rest passes on to Docker's own
//! chains. An accepting rule here never overrides another table's drop.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{
    ConfirmWithin, FirewallConfig, FirewallPorts, FirewallRule, FirewallSource, FirewallTraffic,
    FirewallVerdict, PendingFirewall, Response,
};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::config::Tools;
use crate::operations::OperationError;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
const HEADER: &str =
    "# Written by zendiloOS; changes made here are replaced. Change it in the web interface.\n";
/// Creating the table first makes deleting it work whether or not it is
/// there; all in one transaction with what follows.
const REPLACE: &str = "table inet zendiloos_firewall\ndelete table inet zendiloos_firewall\n";
const FILE: &str = "zendiloos.nft";
const UNIT: &str = "zendiloos-firewall.service";

/// Where the rules are written; other than the real place in tests.
#[derive(Debug, Clone)]
pub struct FirewallPaths {
    /// `/etc/zendiloos/firewall`.
    pub directory: PathBuf,
}

impl Default for FirewallPaths {
    fn default() -> Self {
        Self { directory: PathBuf::from("/etc/zendiloos/firewall") }
    }
}

fn verdict(verdict: FirewallVerdict) -> &'static str {
    match verdict {
        FirewallVerdict::Accept => "accept",
        FirewallVerdict::Drop => "drop",
    }
}

fn ports(ports: &FirewallPorts) -> String {
    let each: Vec<String> = ports
        .ranges()
        .iter()
        .map(|r| if r.from() == r.to() { r.from().to_string() } else { format!("{}-{}", r.from(), r.to()) })
        .collect();
    format!("{{ {} }}", each.join(", "))
}

/// `ip saddr …` or `ip6 saddr …`; nothing for anyone.
fn source(source: FirewallSource) -> String {
    let family = |ipv4: bool| if ipv4 { "ip" } else { "ip6" };
    match source {
        FirewallSource::Any => String::new(),
        FirewallSource::Network { address, prefix } if prefix == if address.is_ipv4() { 32 } else { 128 } => {
            format!("{} saddr {address} ", family(address.is_ipv4()))
        }
        FirewallSource::Network { address, prefix } => {
            format!("{} saddr {address}/{prefix} ", family(address.is_ipv4()))
        }
        FirewallSource::Range { from, to } => format!("{} saddr {from}-{to} ", family(from.is_ipv4())),
    }
}

/// The lines of one rule in a chain: one per match, ping twice for
/// anyone (ICMP and ICMPv6). Forwarded connections match by the port they
/// were sent to; ping is never forwarded.
fn lines(rule: &FirewallRule, forwarded: bool) -> Vec<String> {
    let interface =
        rule.interface.as_ref().map(|i| format!("iifname \"{}\" ", i.as_str())).unwrap_or_default();
    let from = source(rule.source);
    let matches: Vec<String> = match (&rule.traffic, forwarded) {
        (FirewallTraffic::All {}, _) => vec![String::new()],
        (FirewallTraffic::Ping {}, true) => vec![],
        (FirewallTraffic::Ping {}, false) => match rule.source.is_ipv4() {
            Some(true) => vec!["icmp type echo-request ".into()],
            Some(false) => vec!["icmpv6 type echo-request ".into()],
            None => vec![
                "meta l4proto icmp icmp type echo-request ".into(),
                "meta l4proto ipv6-icmp icmpv6 type echo-request ".into(),
            ],
        },
        (FirewallTraffic::Tcp { ports: p }, false) => vec![format!("tcp dport {} ", ports(p))],
        (FirewallTraffic::Udp { ports: p }, false) => vec![format!("udp dport {} ", ports(p))],
        (FirewallTraffic::Tcp { ports: p }, true) => {
            vec![format!("meta l4proto tcp ct original proto-dst {} ", ports(p))]
        }
        (FirewallTraffic::Udp { ports: p }, true) => {
            vec![format!("meta l4proto udp ct original proto-dst {} ", ports(p))]
        }
    };
    matches.into_iter().map(|m| format!("\t\t{interface}{from}{m}{}\n", verdict(rule.verdict))).collect()
}

/// The rules file: the table replaced by the rules, or removed.
pub(crate) fn ruleset(config: Option<&FirewallConfig>) -> String {
    let mut text = format!("{HEADER}{REPLACE}");
    let Some(config) = config else { return text };
    let fallback = verdict(config.fallback);
    text.push_str(
        "table inet zendiloos_firewall {\n\
         \tchain input {\n\
         \t\ttype filter hook input priority filter - 5; policy accept;\n\
         \t\tct state established,related accept\n\
         \t\tiif \"lo\" accept\n\
         \t\t# Containers reach the NAS.\n\
         \t\tiifname \"docker0\" accept\n\
         \t\tiifname \"br-*\" accept\n\
         \t\t# What IPv6 needs to work, and answers to DHCP.\n\
         \t\tmeta l4proto ipv6-icmp icmpv6 type { destination-unreachable, packet-too-big, time-exceeded, \
         parameter-problem, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert, mld-listener-query } accept\n\
         \t\tmeta l4proto icmp icmp type { destination-unreachable, time-exceeded, parameter-problem } accept\n\
         \t\tudp sport 67 udp dport 68 accept\n\
         \t\tip6 saddr fe80::/10 udp sport 547 udp dport 546 accept\n",
    );
    for rule in config.rules.iter() {
        text.extend(lines(rule, false));
    }
    let _ = write!(
        text,
        "\t\t{fallback}\n\
         \t}}\n\
         \tchain forward {{\n\
         \t\ttype filter hook forward priority filter - 5; policy accept;\n\
         \t\tct state established,related accept\n\
         \t\t# Containers reach each other and the world.\n\
         \t\tiifname \"docker0\" accept\n\
         \t\tiifname \"br-*\" accept\n\
         \t\tiifname \"veth*\" accept\n\
         \t\t# What Docker forwards to a published port, from outside.\n\
         \t\tct status dnat jump published\n\
         \t}}\n\
         \tchain published {{\n"
    );
    for rule in config.rules.iter().filter(|rule| rule.forward) {
        text.extend(lines(rule, true));
    }
    let _ = write!(text, "\t\t{fallback}\n\t}}\n}}\n");
    text
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Debug)]
struct Pending {
    revert_at: i64,
    timer: JoinHandle<()>,
}

#[derive(Debug)]
pub(crate) struct Firewall {
    systemctl: Program,
    systemd_run: Program,
    nft: PathBuf,
    paths: FirewallPaths,
    pending: Mutex<Option<Pending>>,
}

impl Firewall {
    pub(crate) fn new(tools: &Tools, paths: FirewallPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            nft: tools.nft.clone(),
            paths,
            pending: Mutex::new(None),
        })
    }

    fn file(&self) -> PathBuf {
        self.paths.directory.join(FILE)
    }

    fn previous(&self) -> PathBuf {
        self.paths.directory.join(format!("{FILE}.previous"))
    }

    fn candidate(&self) -> PathBuf {
        self.paths.directory.join(format!("{FILE}.check"))
    }

    pub(crate) async fn state(&self) -> Response {
        let pending = self.pending.lock().await;
        Response::FirewallState {
            pending: pending.as_ref().map(|p| PendingFirewall { revert_at: p.revert_at }),
        }
    }

    /// `nft -c -f` on `text`, in a unit of its own; nft's complaint if it
    /// fails.
    async fn check(&self, text: &str) -> Result<(), OperationError> {
        let candidate = self.candidate();
        crate::durable::replace(&candidate, text.as_bytes())?;
        let nft = self.nft.display().to_string();
        let file = candidate.display().to_string();
        let args = [
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "--service-type=exec",
            "--description=zendiloOS: checking the firewall's rules",
            nft.as_str(),
            "-c",
            "-f",
            file.as_str(),
        ];
        let output =
            zendiloos_exec::run_combined(&self.systemd_run, args, &Surroundings::default(), LIMITS).await;
        let _ = fs::remove_file(&candidate);
        let output = output?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().map(str::trim).find(|l| l.contains("Error")).or(text.lines().next());
        Err(OperationError::Failed(line.unwrap_or("nft -c failed").trim().to_owned()))
    }

    /// The unit loads the file.
    async fn load(&self) -> Result<(), OperationError> {
        zendiloos_exec::run(&self.systemctl, &["restart", UNIT], LIMITS).await?.success(&self.systemctl)?;
        Ok(())
    }

    fn read(&self) -> Result<Option<String>, OperationError> {
        match fs::read_to_string(self.file()) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(OperationError::Read(self.file(), error)),
        }
    }

    pub(crate) async fn apply(
        self: &Arc<Self>,
        config: Option<&FirewallConfig>,
        confirm_within: Option<ConfirmWithin>,
    ) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        if confirm_within.is_some() && pending.is_some() {
            return Err(OperationError::Conflict);
        }
        let before = self.read()?;
        // Never on, nothing waiting, and to stay off: nothing to do.
        if config.is_none() && before.is_none() && confirm_within.is_none() {
            return Ok(Response::Done);
        }
        let directory = &self.paths.directory;
        fs::create_dir_all(directory).map_err(|e| OperationError::Write(directory.clone(), e))?;
        let text = ruleset(config);
        self.check(&text).await?;
        if confirm_within.is_some() {
            let kept = before.unwrap_or_else(|| ruleset(None));
            crate::durable::replace(&self.previous(), kept.as_bytes())?;
        }
        let applied = match crate::durable::replace(&self.file(), text.as_bytes()) {
            Ok(()) => self.load().await,
            Err(error) => Err(error),
        };
        if let Err(error) = applied {
            if confirm_within.is_some() {
                // Not even in force: back at once.
                self.restore().await;
            }
            return Err(error);
        }
        let Some(within) = confirm_within else {
            tracing::info!(on = config.is_some(), "firewall: rules in force");
            return Ok(Response::Done);
        };
        let seconds = within.get();
        let agent = Arc::clone(self);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(seconds))).await;
            let mut pending = agent.pending.lock().await;
            if pending.take().is_some() {
                drop(pending);
                tracing::warn!("firewall: not confirmed in time, the rules from before return");
                agent.restore().await;
            }
        });
        *pending = Some(Pending { revert_at: now() + i64::from(seconds), timer });
        tracing::info!(on = config.is_some(), seconds, "firewall: rules in force, waiting for confirmation");
        Ok(Response::Done)
    }

    pub(crate) async fn confirm(&self) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        let Some(change) = pending.take() else {
            return Err(OperationError::Refused("no change waits for confirmation".into()));
        };
        change.timer.abort();
        let previous = self.previous();
        fs::remove_file(&previous).map_err(|e| OperationError::Write(previous, e))?;
        tracing::info!("firewall: confirmed");
        Ok(Response::Done)
    }

    /// Puts the previous rules back and loads them. Failures are logged:
    /// there is nobody to tell.
    async fn restore(&self) {
        let previous = self.previous();
        let restored = match fs::read_to_string(&previous) {
            Ok(text) => crate::durable::replace(&self.file(), text.as_bytes()).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => Err(error),
        };
        if let Err(error) = restored.and_then(|()| fs::remove_file(&previous)) {
            tracing::error!(%error, "firewall: cannot put the rules from before back");
            return;
        }
        if let Err(error) = self.load().await {
            tracing::error!(%error, "firewall: cannot load the rules from before");
        }
    }

    /// At startup: a change nobody confirmed before the agent stopped goes
    /// back.
    pub(crate) async fn restore_unconfirmed(&self) {
        if self.previous().exists() {
            tracing::warn!("firewall: a change was not confirmed, the rules from before return");
            self.restore().await;
        }
    }
}

#[cfg(test)]
mod tests;
