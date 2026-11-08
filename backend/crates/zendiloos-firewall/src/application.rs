//! Use cases: the settings with what they stand for now; a change applied
//! at once and kept only once confirmed from the browser, which must still
//! get through; the host kept in step as services come and go; services
//! the rules block from everywhere told once.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::domain::{
    Action, Caller, Exposed, Facts, FirewallSettings, Invalid, LocalInterface, Ruleset, Source, admits,
    blocked, compile, interface_of, local_networks,
};

/// The settings and the services told blocked, kept.
#[async_trait]
pub trait FirewallRepository: Send + Sync {
    /// The defaults while nothing was saved.
    async fn settings(&self) -> Result<FirewallSettings, PortError>;
    async fn save(&self, settings: &FirewallSettings) -> Result<(), PortError>;
    /// The services a notification said were blocked, by [`Exposed::key`].
    async fn told_blocked(&self) -> Result<BTreeSet<String>, PortError>;
    async fn save_told_blocked(&self, keys: &BTreeSet<String>) -> Result<(), PortError>;
}

/// What the NAS runs now, from the other contexts.
#[async_trait]
pub trait ServiceFacts: Send + Sync {
    /// What cannot be found out now is as it was last time, or the
    /// defaults: a hiccup never drops a project's ports from the rules.
    async fn facts(&self, context: &RequestContext) -> Facts;
    /// The NAS's interfaces with their addresses; none if they cannot be
    /// read.
    async fn interfaces(&self) -> Vec<LocalInterface>;
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host would not, in its words.
    #[error("{0}")]
    Refused(String),
    /// It tried and failed, in its words.
    #[error("{0}")]
    Failed(String),
    /// Another change waits for confirmation.
    #[error("another change waits for confirmation")]
    Conflict,
    #[error(transparent)]
    Port(#[from] PortError),
}

/// A change waiting for confirmation on the host, if one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostState {
    pub pending_until: Option<Timestamp>,
}

/// nftables on the host, through the system agent.
#[async_trait]
pub trait FirewallHost: Send + Sync {
    async fn state(&self, context: &RequestContext) -> Result<HostState, PortError>;
    /// Puts `ruleset` in force (`None`: no rules at all). With
    /// `confirm_within`, the rules before return unless confirmed in time;
    /// without, a change waiting stays waiting and goes back to the rules
    /// from before it.
    async fn apply(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: Option<u16>,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    async fn confirm(&self, context: &RequestContext) -> Result<(), HostError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FirewallError {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    /// The rules would keep out the browser that asked for them.
    #[error("the rules would keep this browser out")]
    WouldLockOut,
    #[error("another change waits for confirmation")]
    Pending,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    HostFailed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for FirewallError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => Self::Refused(reason),
            HostError::Failed(reason) => Self::HostFailed(reason),
            HostError::Conflict => Self::Pending,
            HostError::Port(error) => Self::Port(error),
        }
    }
}

/// How long a change waits for confirmation before the rules from before
/// return.
pub const CONFIRM_WITHIN_SECONDS: u16 = 90;

/// How a request reached the web interface, as nginx tells it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Connection {
    /// The browser's address.
    pub client: Option<IpAddr>,
    /// The NAS's address it was sent to.
    pub server: Option<IpAddr>,
    /// The port it was sent to.
    pub port: Option<u16>,
}

/// The settings, a change waiting, and what they stand for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallView {
    pub current: FirewallSettings,
    /// Applied, kept once confirmed before the time given.
    pub pending: Option<(FirewallSettings, Timestamp)>,
    pub facts: Facts,
    /// Services on that the settings in force block from everywhere.
    pub blocked: Vec<Exposed>,
    /// Interfaces rules can name.
    pub interfaces: Vec<String>,
    /// The networks the NAS is in.
    pub local_networks: Vec<Source>,
}

pub struct FirewallPorts {
    pub repository: Arc<dyn FirewallRepository>,
    pub facts: Arc<dyn ServiceFacts>,
    pub host: Arc<dyn FirewallHost>,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Default)]
struct State {
    /// Applied and waiting for confirmation; saved once confirmed.
    pending: Option<FirewallSettings>,
    /// What the host was last given; `None` until it was given anything
    /// (or after it went back by itself).
    applied: Option<Option<Ruleset>>,
}

#[derive(Clone)]
pub struct Firewall {
    ports: Arc<FirewallPorts>,
    state: Arc<Mutex<State>>,
    nudge: Arc<Notify>,
}

impl Firewall {
    pub fn new(ports: FirewallPorts) -> Self {
        Self {
            ports: Arc::new(ports),
            state: Arc::new(Mutex::new(State::default())),
            nudge: Arc::new(Notify::new()),
        }
    }

    /// Asks for [`Firewall::refresh`] soon: a service or port changed.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// Resolves once [`Firewall::nudge`] was called.
    pub async fn nudged(&self) {
        self.nudge.notified().await;
    }

    async fn record(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: String,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let entry =
            AuditEntry::new(self.ports.clock.now(), action, outcome, context).by(actor).target(target);
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "firewall: cannot write the audit log");
        }
    }

    /// Forgets a change the host no longer waits for: it was not confirmed
    /// in time, and the rules from before are back.
    async fn settle(
        &self,
        state: &mut State,
        context: &RequestContext,
    ) -> Result<Option<Timestamp>, FirewallError> {
        let until = self.ports.host.state(context).await?.pending_until;
        if until.is_none() && state.pending.take().is_some() {
            tracing::warn!("firewall: the change was not confirmed in time; the rules from before are back");
            state.applied = None;
        }
        Ok(until)
    }

    pub async fn view(&self, context: &RequestContext) -> Result<FirewallView, FirewallError> {
        let current = self.ports.repository.settings().await?;
        let pending = {
            let mut state = self.state.lock().await;
            let until = self.settle(&mut state, context).await?;
            state.pending.clone().zip(until)
        };
        let facts = self.ports.facts.facts(context).await;
        let interfaces = self.ports.facts.interfaces().await;
        let in_force = pending.as_ref().map_or(&current, |(settings, _)| settings);
        Ok(FirewallView {
            blocked: blocked(in_force, &facts),
            pending,
            current,
            facts,
            local_networks: local_networks(&interfaces),
            interfaces: interfaces.into_iter().map(|i| i.name).collect(),
        })
    }

    /// Applies new settings; the rules from before return by themselves
    /// unless confirmed within [`CONFIRM_WITHIN_SECONDS`]. Settings that
    /// would keep out the browser asking for them are refused, unless
    /// `lock_out` says that is meant.
    pub async fn apply(
        &self,
        settings: FirewallSettings,
        connection: Connection,
        lock_out: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), FirewallError> {
        let settings = settings.check()?;
        let mut state = self.state.lock().await;
        self.settle(&mut state, context).await?;
        if state.pending.is_some() {
            return Err(FirewallError::Pending);
        }
        let facts = self.ports.facts.facts(context).await;
        if !lock_out {
            let interfaces = self.ports.facts.interfaces().await;
            let caller = Caller {
                address: connection.client,
                interface: connection.server.and_then(|ip| interface_of(&interfaces, ip)),
                port: connection.port,
            };
            if !admits(&settings, &facts, &caller) {
                return Err(FirewallError::WouldLockOut);
            }
        }
        let ruleset = compile(&settings, &facts);
        let applied = self.ports.host.apply(ruleset.as_ref(), Some(CONFIRM_WITHIN_SECONDS), context).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.record("firewall.apply", outcome, describe(&settings), actor, context).await;
        if applied.is_err() {
            // Whatever the host has now, it is given the rules again.
            state.applied = None;
        }
        applied?;
        state.pending = Some(settings);
        state.applied = Some(ruleset);
        // Looked at again once the time is up, so rules that went back
        // are brought up to date.
        let nudge = Arc::clone(&self.nudge);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(CONFIRM_WITHIN_SECONDS) + 5)).await;
            nudge.notify_one();
        });
        Ok(())
    }

    /// Keeps the change applied: the browser still gets through.
    pub async fn confirm(&self, actor: &Actor, context: &RequestContext) -> Result<(), FirewallError> {
        let mut state = self.state.lock().await;
        self.settle(&mut state, context).await?;
        let Some(settings) = state.pending.clone() else {
            return Err(FirewallError::Refused("no change waits for confirmation".into()));
        };
        let confirmed = self.ports.host.confirm(context).await;
        let outcome = if confirmed.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.record("firewall.confirm", outcome, describe(&settings), actor, context).await;
        confirmed?;
        self.ports.repository.save(&settings).await?;
        state.pending = None;
        drop(state);
        let facts = self.ports.facts.facts(context).await;
        self.tell_blocked(&settings, &facts).await;
        Ok(())
    }

    /// Puts the rules in force with the services' ports as they are now:
    /// at start, when something changed, and now and then. The host is
    /// asked only when the rules it enforces would change. A change
    /// waiting for confirmation stays waiting.
    pub async fn refresh(&self, context: &RequestContext) -> Result<(), FirewallError> {
        let mut state = self.state.lock().await;
        self.settle(&mut state, context).await?;
        let settings = match &state.pending {
            Some(settings) => settings.clone(),
            None => self.ports.repository.settings().await?,
        };
        let facts = self.ports.facts.facts(context).await;
        let ruleset = compile(&settings, &facts);
        if state.applied.as_ref() != Some(&ruleset) {
            if let Err(error) = self.ports.host.apply(ruleset.as_ref(), None, context).await {
                state.applied = None;
                return Err(error.into());
            }
            tracing::info!(
                rules = ruleset.as_ref().map_or(0, |r| r.rules.len()),
                "firewall: rules brought up to date"
            );
            state.applied = Some(ruleset);
        }
        let waiting = state.pending.is_some();
        drop(state);
        if !waiting {
            self.tell_blocked(&settings, &facts).await;
        }
        Ok(())
    }

    /// Tells of services blocked from everywhere once, when they become
    /// so; one blocked again after it was not is told again.
    async fn tell_blocked(&self, settings: &FirewallSettings, facts: &Facts) {
        let now: Vec<Exposed> = blocked(settings, facts);
        let keys: BTreeSet<String> = now.iter().map(Exposed::key).collect();
        let before = match self.ports.repository.told_blocked().await {
            Ok(before) => before,
            Err(error) => {
                tracing::warn!(%error, "firewall: cannot read which blocked services were told");
                return;
            }
        };
        for service in now.iter().filter(|service| !before.contains(&service.key())) {
            let mut told =
                NewNotification::new(Level::Warning, "firewall_blocked").with("service", service.key());
            if let Exposed::Project(name) = service {
                told = told.with("project", name.clone());
            }
            self.ports.notifier.notify(told).await;
        }
        if keys != before
            && let Err(error) = self.ports.repository.save_told_blocked(&keys).await
        {
            tracing::warn!(%error, "firewall: cannot keep which blocked services were told");
        }
    }
}

/// The settings in a few words, for the audit log.
fn describe(settings: &FirewallSettings) -> String {
    if !settings.enabled {
        return "off".to_owned();
    }
    let fallback = match settings.fallback {
        Action::Allow => "allow",
        Action::Deny => "deny",
    };
    format!("on, {} rules, otherwise {fallback}", settings.rules.len())
}

#[cfg(test)]
mod tests;
