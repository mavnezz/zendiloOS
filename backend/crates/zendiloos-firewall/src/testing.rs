//! Settings kept in memory, facts set by hand and a host that pretends to
//! run nftables, for tests here and in the API crate (feature `testing`).

use std::collections::BTreeSet;
use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::{
    FirewallHost, FirewallRepository, HostError, HostRules, HostState, RestoredFirewall, ServiceFacts,
};
use crate::domain::{Facts, FirewallSettings, LocalInterface, ProjectPorts, Ruleset};

#[derive(Default)]
pub struct MemoryFirewall {
    settings: Mutex<Option<FirewallSettings>>,
    told: Mutex<BTreeSet<String>>,
    projects: Mutex<Vec<ProjectPorts>>,
    /// The settings a restored backup brought.
    pub restored: Mutex<Option<RestoredFirewall>>,
}

#[async_trait]
impl FirewallRepository for MemoryFirewall {
    async fn settings(&self) -> Result<FirewallSettings, PortError> {
        Ok(self.settings.lock().unwrap().clone().unwrap_or_default())
    }

    async fn save(&self, settings: &FirewallSettings) -> Result<(), PortError> {
        *self.settings.lock().unwrap() = Some(settings.clone());
        Ok(())
    }

    async fn told_blocked(&self) -> Result<BTreeSet<String>, PortError> {
        Ok(self.told.lock().unwrap().clone())
    }

    async fn save_told_blocked(&self, keys: &BTreeSet<String>) -> Result<(), PortError> {
        keys.clone_into(&mut self.told.lock().unwrap());
        Ok(())
    }

    async fn known_projects(&self) -> Result<Vec<ProjectPorts>, PortError> {
        Ok(self.projects.lock().unwrap().clone())
    }

    async fn save_known_projects(&self, projects: &[ProjectPorts]) -> Result<(), PortError> {
        projects.clone_into(&mut self.projects.lock().unwrap());
        Ok(())
    }

    async fn restored(&self) -> Result<Option<RestoredFirewall>, PortError> {
        Ok(self.restored.lock().unwrap().clone())
    }

    async fn forget_restored(&self) -> Result<(), PortError> {
        *self.restored.lock().unwrap() = None;
        Ok(())
    }
}

/// Facts and interfaces as a test sets them.
pub struct FixedFacts {
    pub facts: Mutex<Facts>,
    pub interfaces: Mutex<Vec<LocalInterface>>,
}

impl Default for FixedFacts {
    /// The defaults, SMB on, on one interface in 192.168.1.0/24.
    fn default() -> Self {
        Self {
            facts: Mutex::new(Facts { smb_on: true, ..Facts::default() }),
            interfaces: Mutex::new(vec![LocalInterface {
                name: "enp1s0".into(),
                addresses: vec![("192.168.1.10".parse().unwrap(), 24)],
            }]),
        }
    }
}

impl FixedFacts {
    pub fn set(&self, change: impl FnOnce(&mut Facts)) {
        change(&mut self.facts.lock().unwrap());
    }
}

#[async_trait]
impl ServiceFacts for FixedFacts {
    async fn facts(&self, _context: &RequestContext) -> Facts {
        self.facts.lock().unwrap().clone()
    }

    async fn interfaces(&self) -> Vec<LocalInterface> {
        self.interfaces.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct HostInside {
    /// What is in force now; `None`: no rules.
    rules: Option<Ruleset>,
    /// What returns unless confirmed, and when.
    before: Option<(Option<Ruleset>, Timestamp)>,
    calls: Vec<String>,
    fail: Option<String>,
    now: i64,
}

/// nftables pretended: what is in force, what returns unconfirmed.
#[derive(Default)]
pub struct FakeFirewallHost {
    inside: Mutex<HostInside>,
}

impl FakeFirewallHost {
    /// The rules in force.
    pub fn rules(&self) -> Option<Ruleset> {
        self.inside.lock().unwrap().rules.clone()
    }

    /// What it was asked, one line each.
    pub fn calls(&self) -> Vec<String> {
        self.inside.lock().unwrap().calls.clone()
    }

    /// Applying fails with `reason` from now on; `None`: works again.
    pub fn fail(&self, reason: Option<&str>) {
        self.inside.lock().unwrap().fail = reason.map(str::to_owned);
    }

    pub fn set_now(&self, now: Timestamp) {
        self.inside.lock().unwrap().now = now.unix_seconds();
    }

    /// The time runs out: an unconfirmed change goes back.
    pub fn time_up(&self) {
        let mut inside = self.inside.lock().unwrap();
        if let Some((before, _)) = inside.before.take() {
            inside.rules = before;
            inside.calls.push("went back".into());
        }
    }
}

fn count(ruleset: Option<&Ruleset>) -> usize {
    ruleset.map_or(0, |r| r.rules.len())
}

#[async_trait]
impl FirewallHost for FakeFirewallHost {
    async fn state(&self, _context: &RequestContext) -> Result<HostState, PortError> {
        Ok(HostState { pending_until: self.inside.lock().unwrap().before.as_ref().map(|(_, until)| *until) })
    }

    async fn begin(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: u16,
        _context: &RequestContext,
    ) -> Result<HostState, HostError> {
        let mut inside = self.inside.lock().unwrap();
        if let Some(reason) = inside.fail.clone() {
            return Err(HostError::Failed(reason));
        }
        if inside.before.is_some() {
            return Err(HostError::Conflict);
        }
        inside.calls.push(format!("apply {} rules, confirm within {confirm_within}", count(ruleset)));
        let until = Timestamp::from_unix_seconds(inside.now + i64::from(confirm_within));
        inside.before = Some((inside.rules.clone(), until));
        inside.rules = ruleset.cloned();
        Ok(HostState { pending_until: Some(until) })
    }

    async fn keep(&self, rules: &HostRules, _context: &RequestContext) -> Result<HostState, HostError> {
        let mut inside = self.inside.lock().unwrap();
        if let Some(reason) = inside.fail.clone() {
            return Err(HostError::Failed(reason));
        }
        let waiting = rules.waiting.as_ref().map(|w| format!(", {} waiting", count(w.as_ref())));
        inside.calls.push(format!(
            "keep {} rules{}",
            count(rules.kept.as_ref()),
            waiting.unwrap_or_default()
        ));
        let before = inside.before.take();
        match (before, &rules.waiting) {
            (Some((_, until)), Some(waiting)) => {
                inside.before = Some((rules.kept.clone(), until));
                inside.rules.clone_from(waiting);
            }
            // Nothing waits, or nobody can confirm what does.
            _ => inside.rules.clone_from(&rules.kept),
        }
        Ok(HostState { pending_until: inside.before.as_ref().map(|(_, until)| *until) })
    }

    async fn confirm(&self, _context: &RequestContext) -> Result<(), HostError> {
        let mut inside = self.inside.lock().unwrap();
        if inside.before.take().is_none() {
            return Err(HostError::Refused("no change waits for confirmation".into()));
        }
        inside.calls.push("confirm".into());
        Ok(())
    }
}
