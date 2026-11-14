//! The UPS's use cases: its page, its settings, installing NUT, finding a
//! UPS on USB, and the watch that turns its state into notifications.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use zendiloos_jobs::{Job, JobError, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::domain::{
    Account, Event, Found, Invalid, Settings, ShutdownWhen, SnmpVersion, Source, UPS_NAME, UPS_PORT, UpsState,
    Watch,
};

/// The settings as they are kept, with the password of the NAS's own
/// account at NUT's server, which is made once and never shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub settings: Settings,
    pub local_password: String,
}

#[async_trait]
pub trait SettingsStore: Send + Sync {
    /// The settings; before any were saved, none with a new password.
    async fn load(&self) -> Result<Stored, PortError>;
    async fn save(&self, settings: &Settings, at: Timestamp) -> Result<(), PortError>;
}

/// What of NUT is on the NAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFacts {
    /// NUT's server and monitor are installed.
    pub installed: bool,
    /// Its SNMP driver too.
    pub snmp: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host refused, with its reason.
    #[error("{0}")]
    Refused(String),
    /// A program failed, in its words.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// The NAS's side of NUT: its packages, its files and units.
#[async_trait]
pub trait UpsHost: Send + Sync {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError>;
    /// Installs NUT from Debian; returns when it is done.
    async fn install(&self, context: &RequestContext) -> Result<(), HostError>;
    /// Writes NUT's files for `stored` and starts what they need; without
    /// a UPS, stops everything.
    async fn apply(&self, stored: &Stored, context: &RequestContext) -> Result<(), HostError>;
    /// The UPSes found on USB.
    async fn scan(&self, context: &RequestContext) -> Result<Vec<Found>, HostError>;
}

/// Where a UPS is asked for its state: a NUT server and the UPS's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub ups: String,
}

/// NUT's variables of a UPS, read from its server.
#[async_trait]
pub trait UpsLink: Send + Sync {
    /// The variables, or why there are none.
    async fn read(&self, target: &Target) -> Result<BTreeMap<String, String>, String>;
}

pub struct UpsPorts {
    pub store: Arc<dyn SettingsStore>,
    pub host: Arc<dyn UpsHost>,
    pub link: Arc<dyn UpsLink>,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpsError {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("Network UPS Tools are not installed")]
    NotInstalled,
    #[error("Network UPS Tools are installed already")]
    Installed,
    #[error("NUT's SNMP driver is not installed")]
    SnmpMissing,
    /// No password is kept for this account: it has to be typed.
    #[error("the password is needed")]
    PasswordNeeded,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    HostFailed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for UpsError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => Self::Refused(reason),
            HostError::Failed(reason) => Self::HostFailed(reason),
            HostError::Port(error) => Self::Port(error),
        }
    }
}

/// Where the UPS is, as an administrator sets it: a password left out
/// stays the one that is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    Usb { driver: String, vendor_id: Option<String>, product_id: Option<String>, serial: Option<String> },
    Snmp { address: String, community: String, version: SnmpVersion },
    Server { host: String, port: u16, ups: String, user: String, password: Option<String> },
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInput {
    pub user: String,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsInput {
    pub source: Option<SourceInput>,
    pub shutdown: ShutdownWhen,
    pub cut_power: bool,
    pub serve: Option<AccountInput>,
}

/// The UPS as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub facts: HostFacts,
    pub settings: Settings,
    /// What the UPS says; none without a UPS, and while it does not answer.
    pub state: Option<UpsState>,
}

#[derive(Clone)]
pub struct Ups {
    ports: Arc<UpsPorts>,
    watch: Arc<Mutex<Watch>>,
}

/// Where the UPS of `settings` is asked: NUT's server on the NAS for a UPS
/// of its own, the other device's otherwise.
pub fn target(settings: &Settings) -> Option<Target> {
    match settings.source.as_ref()? {
        Source::Server { host, port, ups, .. } => {
            Some(Target { host: host.clone(), port: *port, ups: ups.clone() })
        }
        _ => Some(Target { host: "127.0.0.1".into(), port: UPS_PORT, ups: UPS_NAME.into() }),
    }
}

/// The settings `input` asks for, with the passwords it left out taken
/// from `kept`: a server's if host and account are the same, the account's
/// for other devices if its name is.
fn merged(input: SettingsInput, kept: &Settings) -> Result<Settings, UpsError> {
    let source = match input.source {
        None => None,
        Some(SourceInput::Usb { driver, vendor_id, product_id, serial }) => {
            Some(Source::Usb { driver, vendor_id, product_id, serial })
        }
        Some(SourceInput::Snmp { address, community, version }) => {
            Some(Source::Snmp { address, community, version })
        }
        Some(SourceInput::Simulated) => Some(Source::Simulated),
        Some(SourceInput::Server { host, port, ups, user, password }) => {
            let password = match password.filter(|password| !password.is_empty()) {
                Some(password) => password,
                None => match &kept.source {
                    Some(Source::Server { host: kept_host, user: kept_user, password, .. })
                        if kept_host.eq_ignore_ascii_case(&host) && *kept_user == user =>
                    {
                        password.clone()
                    }
                    _ => return Err(UpsError::PasswordNeeded),
                },
            };
            Some(Source::Server { host, port, ups, user, password })
        }
    };
    let serve = match input.serve {
        None => None,
        Some(AccountInput { user, password }) => {
            let password = match password.filter(|password| !password.is_empty()) {
                Some(password) => password,
                None => match &kept.serve {
                    Some(account) if account.user == user => account.password.clone(),
                    _ => return Err(UpsError::PasswordNeeded),
                },
            };
            Some(Account { user, password })
        }
    };
    Ok(Settings { source, shutdown: input.shutdown, cut_power: input.cut_power, serve })
}

fn notification(event: Event) -> NewNotification {
    match event {
        Event::OnBattery => NewNotification::new(Level::Warning, "ups_on_battery"),
        Event::OnMains => NewNotification::new(Level::Success, "ups_on_mains"),
        Event::BatteryLow => NewNotification::new(Level::Error, "ups_battery_low"),
        Event::Unreachable => NewNotification::new(Level::Warning, "ups_unreachable"),
        Event::ReplaceBattery => NewNotification::new(Level::Warning, "ups_replace_battery"),
    }
}

/// A use case's error as its job tells it.
fn job_error(error: &UpsError) -> JobError {
    let code = match error {
        UpsError::Refused(_) | UpsError::Invalid(_) => "refused",
        _ => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

impl Ups {
    pub fn new(ports: UpsPorts) -> Self {
        Self { ports: Arc::new(ports), watch: Arc::default() }
    }

    async fn state(&self, settings: &Settings) -> Option<UpsState> {
        let target = target(settings)?;
        match self.ports.link.read(&target).await {
            Ok(variables) => UpsState::from_variables(&variables),
            Err(reason) => {
                tracing::debug!(%reason, host = %target.host, "ups: no answer");
                None
            }
        }
    }

    pub async fn overview(&self, context: &RequestContext) -> Result<Overview, UpsError> {
        let facts = self.ports.host.facts(context).await?;
        let settings = self.ports.store.load().await?.settings;
        let state = if facts.installed { self.state(&settings).await } else { None };
        Ok(Overview { facts, settings, state })
    }

    /// Whether other devices may follow the UPS through the NAS: NUT's
    /// server then listens on the network.
    pub async fn serving(&self) -> Result<bool, PortError> {
        let settings = self.ports.store.load().await?.settings;
        Ok(settings.source.is_some() && settings.serve.is_some())
    }

    /// Puts the settings in force, then keeps them: what NUT refused is
    /// not kept.
    pub async fn set(
        &self,
        input: SettingsInput,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Overview, UpsError> {
        let done = self.change(input, context).await;
        let target = match &done {
            Ok(settings) => settings.source.as_ref().map_or("none", Source::kind),
            Err(_) => "",
        };
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("ups.update", outcome, target, actor, context).await;
        done?;
        // The driver needs a moment before it has spoken to the UPS; the
        // page asks again by itself.
        self.overview(context).await
    }

    async fn change(&self, input: SettingsInput, context: &RequestContext) -> Result<Settings, UpsError> {
        let kept = self.ports.store.load().await?;
        let settings = merged(input, &kept.settings)?.checked()?;
        let facts = self.ports.host.facts(context).await?;
        if settings.source.is_some() {
            if !facts.installed {
                return Err(UpsError::NotInstalled);
            }
            if matches!(settings.source, Some(Source::Snmp { .. })) && !facts.snmp {
                return Err(UpsError::SnmpMissing);
            }
        }
        if facts.installed {
            let stored = Stored { settings: settings.clone(), local_password: kept.local_password };
            self.ports.host.apply(&stored, context).await?;
        }
        self.ports.store.save(&settings, self.ports.clock.now()).await?;
        self.watch.lock().await.forget();
        Ok(settings)
    }

    /// Installs NUT from Debian, in a job; settings that are there already
    /// (from a configuration that was restored) are put in force after it.
    pub async fn install(&self, actor: &Actor, context: &RequestContext) -> Result<Job, UpsError> {
        let facts = self.ports.host.facts(context).await?;
        if facts.installed && facts.snmp {
            return Err(UpsError::Installed);
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("ups.install", Some("nut".into()), Some(actor), move |progress| async move {
                progress.step("packages", Some(0.05)).await;
                let done = this.installed(&context).await;
                let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
                this.audit("ups.install", outcome, "nut", &who, &context).await;
                done.map_err(|error| job_error(&error))
            })
            .await?;
        Ok(job)
    }

    async fn installed(&self, context: &RequestContext) -> Result<(), UpsError> {
        self.ports.host.install(context).await?;
        let stored = self.ports.store.load().await?;
        if stored.settings.source.is_some() {
            self.ports.host.apply(&stored, context).await?;
        }
        Ok(())
    }

    pub async fn scan(&self, context: &RequestContext) -> Result<Vec<Found>, UpsError> {
        if !self.ports.host.facts(context).await?.installed {
            return Err(UpsError::NotInstalled);
        }
        Ok(self.ports.host.scan(context).await?)
    }

    /// One look at the UPS, and a notification for what changed since the
    /// last. Called every few seconds.
    pub async fn watch(&self) {
        let settings = match self.ports.store.load().await {
            Ok(stored) => stored.settings,
            Err(error) => {
                tracing::warn!(%error, "ups: cannot read the settings");
                return;
            }
        };
        let mut watch = self.watch.lock().await;
        if settings.source.is_none() {
            watch.forget();
            return;
        }
        let state = self.state(&settings).await;
        let events = watch.observe(state.as_ref());
        drop(watch);
        for event in events {
            self.ports.notifier.notify(notification(event)).await;
        }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let mut entry = AuditEntry::new(self.ports.clock.now(), action, outcome, context).by(actor);
        if !target.is_empty() {
            entry = entry.target(target.to_owned());
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "ups: cannot write the audit log");
        }
    }
}

#[cfg(test)]
mod tests;
