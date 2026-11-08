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
    Action, Caller, Exposed, Facts, FirewallSettings, Invalid, LocalInterface, ProjectPorts, Ruleset, Source,
    admits, blocked, compile, interface_of, local_networks,
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
    /// The ports Docker's projects published when Docker last answered:
    /// after a restart they stand for the projects until it answers again,
    /// so a rule about a project holds from the start.
    async fn known_projects(&self) -> Result<Vec<ProjectPorts>, PortError>;
    async fn save_known_projects(&self, projects: &[ProjectPorts]) -> Result<(), PortError>;
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

/// What the host is to enforce outside of a change beginning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRules {
    /// The settings kept, with the services as they are now (`None`: no
    /// rules at all).
    pub kept: Option<Ruleset>,
    /// While the change begun here waits: its rules as they are now (the
    /// inner `None`: no rules at all), in force until it is confirmed or
    /// goes back to `kept`.
    pub waiting: Option<Option<Ruleset>>,
}

/// nftables on the host, through the system agent.
#[async_trait]
pub trait FirewallHost: Send + Sync {
    async fn state(&self, context: &RequestContext) -> Result<HostState, PortError>;
    /// Begins a change: `ruleset` is in force (`None`: no rules at all),
    /// and the rules from before return unless it is confirmed within
    /// `confirm_within` seconds. One change at a time. While it waits,
    /// connections opened before it meet the rules too, so a confirmation
    /// sent on one proves the browser still gets in.
    async fn begin(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: u16,
        context: &RequestContext,
    ) -> Result<HostState, HostError>;
    /// Puts `rules` in force, deciding on the host, at once with its timer:
    /// the change [`HostRules::waiting`] names, if it still waits, with
    /// what returns brought up to date; otherwise the rules kept, and a
    /// change it does not name ends (begun before a restart of zendiloosd,
    /// nobody can confirm it). Answers whether a change waits now.
    async fn keep(&self, rules: &HostRules, context: &RequestContext) -> Result<HostState, HostError>;
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
    /// Applied and waiting for confirmation; saved once confirmed. Kept in
    /// memory only: after a restart nobody can confirm a change begun
    /// before, and the host ends it at the first refresh.
    pending: Option<FirewallSettings>,
    /// What the host was last given; `None` until it was given anything,
    /// or when what it has is not known (it went back by itself, a change
    /// begun before a restart waits there).
    applied: Option<HostRules>,
}

#[derive(Clone)]
pub struct Firewall {
    ports: Arc<FirewallPorts>,
    state: Arc<Mutex<State>>,
    nudge: Arc<Notify>,
    /// The projects' ports as saved last, so they are saved only when they
    /// change; `None` until known.
    saved_projects: Arc<Mutex<Option<Vec<ProjectPorts>>>>,
}

impl Firewall {
    pub fn new(ports: FirewallPorts) -> Self {
        Self {
            ports: Arc::new(ports),
            state: Arc::new(Mutex::new(State::default())),
            nudge: Arc::new(Notify::new()),
            saved_projects: Arc::new(Mutex::new(None)),
        }
    }

    /// What the services stand for now. Until Docker answered once, the
    /// projects' ports are those saved when it last did, so a rule about a
    /// project holds from the start; once it did, they are saved.
    async fn facts(&self, context: &RequestContext) -> Facts {
        let mut facts = self.ports.facts.facts(context).await;
        let mut saved = self.saved_projects.lock().await;
        if facts.projects_unknown {
            if saved.is_none() {
                match self.ports.repository.known_projects().await {
                    Ok(projects) => *saved = Some(projects),
                    Err(error) => tracing::warn!(%error, "firewall: cannot read the projects' ports saved"),
                }
            }
            facts.projects = saved.clone().unwrap_or_default();
        } else if saved.as_ref() != Some(&facts.projects) {
            match self.ports.repository.save_known_projects(&facts.projects).await {
                Ok(()) => *saved = Some(facts.projects.clone()),
                Err(error) => tracing::warn!(%error, "firewall: cannot save the projects' ports"),
            }
        }
        facts
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
    /// in time, and the rules from before are back. A change waiting there
    /// that was not begun here is to be ended by [`Firewall::keep`].
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
        if until.is_some() && state.pending.is_none() {
            state.applied = None;
        }
        Ok(until)
    }

    /// Gives the host the rules kept and those of the change waiting, with
    /// the services as they are now, when they differ from what it was
    /// given last. Answers the settings in force and the facts.
    async fn keep(
        &self,
        state: &mut State,
        context: &RequestContext,
    ) -> Result<(FirewallSettings, Facts), FirewallError> {
        let kept = self.ports.repository.settings().await?;
        let facts = self.facts(context).await;
        let wanted = HostRules {
            kept: compile(&kept, &facts),
            waiting: state.pending.as_ref().map(|settings| compile(settings, &facts)),
        };
        if state.applied.as_ref() != Some(&wanted) {
            let host = match self.ports.host.keep(&wanted, context).await {
                Ok(host) => host,
                Err(error) => {
                    state.applied = None;
                    return Err(error.into());
                }
            };
            let rules = |rules: &Option<Ruleset>| rules.as_ref().map_or(0, |r| r.rules.len());
            tracing::info!(
                rules = rules(&wanted.kept),
                waiting = wanted.waiting.as_ref().map(rules),
                "firewall: rules brought up to date"
            );
            if host.pending_until.is_none() && state.pending.take().is_some() {
                tracing::warn!("firewall: the change was not confirmed in time; the rules kept are in force");
                state.applied = Some(HostRules { kept: wanted.kept, waiting: None });
            } else {
                state.applied = Some(wanted);
            }
        }
        Ok((state.pending.clone().unwrap_or(kept), facts))
    }

    pub async fn view(&self, context: &RequestContext) -> Result<FirewallView, FirewallError> {
        let current = self.ports.repository.settings().await?;
        let pending = {
            let mut state = self.state.lock().await;
            let until = self.settle(&mut state, context).await?;
            state.pending.clone().zip(until)
        };
        let facts = self.facts(context).await;
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
        let until = self.settle(&mut state, context).await?;
        if state.pending.is_some() {
            return Err(FirewallError::Pending);
        }
        if until.is_some() {
            // Begun before zendiloosd restarted: it ends first.
            self.keep(&mut state, context).await?;
        }
        let facts = self.facts(context).await;
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
        let begun = self.ports.host.begin(ruleset.as_ref(), CONFIRM_WITHIN_SECONDS, context).await;
        let outcome = if begun.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.record("firewall.apply", outcome, describe(&settings), actor, context).await;
        if begun.is_err() {
            // Whatever the host has now, it is given the rules again.
            state.applied = None;
        }
        begun?;
        state.pending = Some(settings);
        // What returns is what the host had: the rules kept, as last given.
        state.applied =
            state.applied.take().map(|given| HostRules { kept: given.kept, waiting: Some(ruleset) });
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
        // The host keeps the change's rules as it was given them last.
        state.applied = state
            .applied
            .take()
            .and_then(|given| given.waiting.map(|rules| HostRules { kept: rules, waiting: None }));
        drop(state);
        let facts = self.facts(context).await;
        self.tell_blocked(&settings, &facts).await;
        Ok(())
    }

    /// Puts the rules in force with the services' ports as they are now:
    /// at start, when something changed, and now and then. The host is
    /// asked only when the rules it enforces would change. A change
    /// waiting for confirmation stays waiting; one begun before a restart
    /// ends, as nobody can confirm it any more.
    pub async fn refresh(&self, context: &RequestContext) -> Result<(), FirewallError> {
        let mut state = self.state.lock().await;
        self.settle(&mut state, context).await?;
        let (settings, facts) = self.keep(&mut state, context).await?;
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
