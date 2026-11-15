//! In-memory adapters for the UPS's ports, for tests here and in the API
//! crate.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_kernel::{AuditEntry, AuditLog, Clock, PortError, RequestContext, Timestamp};
use zendiloos_notify::Notifier;
use zendiloos_notify::testing::MemoryNotifications;

use crate::application::{
    HostError, HostFacts, SettingsStore, Stored, Target, Ups, UpsHost, UpsLink, UpsPorts,
};
use crate::domain::{Found, Settings};

/// The password the store makes for the NAS's own account.
pub const LOCAL_PASSWORD: &str = "local-password";

/// The settings, and whether a call for a new battery was told.
#[derive(Default)]
pub struct MemoryStore(pub Mutex<Settings>, pub Mutex<bool>);

#[async_trait]
impl SettingsStore for MemoryStore {
    async fn load(&self) -> Result<Stored, PortError> {
        Ok(Stored { settings: self.0.lock().unwrap().clone(), local_password: LOCAL_PASSWORD.into() })
    }

    async fn save(&self, settings: &Settings, _: Timestamp) -> Result<(), PortError> {
        *self.0.lock().unwrap() = settings.clone();
        Ok(())
    }

    async fn replace_battery_told(&self) -> Result<bool, PortError> {
        Ok(*self.1.lock().unwrap())
    }

    async fn set_replace_battery_told(&self, told: bool) -> Result<(), PortError> {
        *self.1.lock().unwrap() = told;
        Ok(())
    }
}

/// The NAS as NUT's host: what is installed, what was applied, what the
/// scanner finds; it refuses to apply while `refuses` holds a reason.
pub struct FakeHost {
    pub facts: Mutex<HostFacts>,
    pub applied: Mutex<Vec<Stored>>,
    pub found: Mutex<Vec<Found>>,
    pub refuses: Mutex<Option<String>>,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            facts: Mutex::new(HostFacts { installed: true, snmp: true }),
            applied: Mutex::default(),
            found: Mutex::default(),
            refuses: Mutex::default(),
        }
    }
}

#[async_trait]
impl UpsHost for FakeHost {
    async fn facts(&self, _: &RequestContext) -> Result<HostFacts, PortError> {
        Ok(*self.facts.lock().unwrap())
    }

    async fn install(&self, _: &RequestContext) -> Result<(), HostError> {
        *self.facts.lock().unwrap() = HostFacts { installed: true, snmp: true };
        Ok(())
    }

    async fn apply(&self, stored: &Stored, _: &RequestContext) -> Result<(), HostError> {
        if let Some(reason) = self.refuses.lock().unwrap().clone() {
            return Err(HostError::Refused(reason));
        }
        self.applied.lock().unwrap().push(stored.clone());
        Ok(())
    }

    async fn scan(&self, _: &RequestContext) -> Result<Vec<Found>, HostError> {
        Ok(self.found.lock().unwrap().clone())
    }
}

/// A NUT server that answers with `variables`, or not at all while there
/// are none; it remembers whom it was asked for.
#[derive(Default)]
pub struct FakeLink {
    pub variables: Mutex<Option<BTreeMap<String, String>>>,
    pub asked: Mutex<Vec<Target>>,
}

impl FakeLink {
    /// The UPS says `status` (`OL`, `OB LB` …) from now on.
    pub fn says(&self, status: &str) {
        let mut variables = BTreeMap::new();
        variables.insert("ups.status".to_owned(), status.to_owned());
        variables.insert("battery.charge".to_owned(), "80".to_owned());
        variables.insert("battery.runtime".to_owned(), "1200".to_owned());
        *self.variables.lock().unwrap() = Some(variables);
    }

    /// The UPS no longer answers.
    pub fn silent(&self) {
        *self.variables.lock().unwrap() = None;
    }
}

#[async_trait]
impl UpsLink for FakeLink {
    async fn read(&self, target: &Target) -> Result<BTreeMap<String, String>, String> {
        self.asked.lock().unwrap().push(target.clone());
        self.variables.lock().unwrap().clone().ok_or_else(|| "connection refused".to_owned())
    }
}

#[derive(Default)]
pub struct MemoryAudit(pub Mutex<Vec<AuditEntry>>);

impl MemoryAudit {
    pub fn actions(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().iter().map(|entry| entry.action).collect()
    }
}

#[async_trait]
impl AuditLog for MemoryAudit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry);
        Ok(())
    }
}

/// A clock that stands until a test moves it on.
pub struct MovingClock(AtomicI64);

impl Default for MovingClock {
    fn default() -> Self {
        Self(AtomicI64::new(FixedClock.now().unix_seconds()))
    }
}

impl MovingClock {
    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for MovingClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(self.0.load(Ordering::SeqCst))
    }
}

/// The fakes behind a [`Ups`], for a test to look into.
#[derive(Default)]
pub struct Fakes {
    pub store: Arc<MemoryStore>,
    pub host: Arc<FakeHost>,
    pub link: Arc<FakeLink>,
    pub jobs: Arc<MemoryJobs>,
    pub notifications: Arc<MemoryNotifications>,
    pub audit: Arc<MemoryAudit>,
    pub clock: Arc<MovingClock>,
}

/// The use cases over `fakes`.
pub fn ups(fakes: &Fakes) -> Ups {
    let clock = fakes.clock.clone();
    Ups::new(UpsPorts {
        store: fakes.store.clone(),
        host: fakes.host.clone(),
        link: fakes.link.clone(),
        jobs: Jobs::new(fakes.jobs.clone(), clock.clone()),
        notifier: Notifier::new(fakes.notifications.clone(), clock.clone()),
        audit: fakes.audit.clone(),
        clock,
    })
}
