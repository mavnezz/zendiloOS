//! Settings kept in memory, facts set by hand and a host that pretends to
//! run nftables, for tests here and in the API crate (feature `testing`).

use std::collections::BTreeSet;
use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::{FirewallHost, FirewallRepository, HostError, HostState, ServiceFacts};
use crate::domain::{Facts, FirewallSettings, LocalInterface, Ruleset};

#[derive(Default)]
pub struct MemoryFirewall {
    settings: Mutex<Option<FirewallSettings>>,
    told: Mutex<BTreeSet<String>>,
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

#[async_trait]
impl FirewallHost for FakeFirewallHost {
    async fn state(&self, _context: &RequestContext) -> Result<HostState, PortError> {
        Ok(HostState { pending_until: self.inside.lock().unwrap().before.as_ref().map(|(_, until)| *until) })
    }

    async fn apply(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: Option<u16>,
        _context: &RequestContext,
    ) -> Result<(), HostError> {
        let mut inside = self.inside.lock().unwrap();
        if let Some(reason) = inside.fail.clone() {
            return Err(HostError::Failed(reason));
        }
        if confirm_within.is_some() && inside.before.is_some() {
            return Err(HostError::Conflict);
        }
        let rules = ruleset.map_or(0, |r| r.rules.len());
        inside.calls.push(format!("apply {rules} rules, confirm {confirm_within:?}"));
        if let Some(seconds) = confirm_within {
            let until = Timestamp::from_unix_seconds(inside.now + i64::from(seconds));
            inside.before = Some((inside.rules.clone(), until));
        }
        inside.rules = ruleset.cloned();
        Ok(())
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
