//! Use cases of the Network context and the ports they need.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobError, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::domain::{
    Configuration, Device, DeviceAction, DeviceUpdate, Difference, DifferenceKind, Edit, IfupdownFile,
    InterfaceSettings, InvalidDevice, InvalidSettings, Ipv4Setting, Network, NetworkdFile, NewDevice, Part,
    Planning, Refusal, Subject, TakeoverPlan, differences, restoring, takeover_plan,
};

/// How the machine is connected, read from the host.
#[async_trait]
pub trait NetworkSource: Send + Sync {
    async fn network(&self) -> Result<Network, PortError>;
}

/// What manages the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    /// systemd-networkd: zendiloOS changes it.
    Networkd,
    /// ifupdown (`/etc/network/interfaces`): zendiloOS can take it over.
    Ifupdown,
    /// NetworkManager: only shown.
    NetworkManager,
    /// Something else, or nothing that can be told: only shown.
    Unknown,
}

/// What a change waiting for confirmation changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// One interface's settings.
    Interface(String),
    /// The network, taken over from ifupdown: the interfaces set up.
    Takeover(Vec<String>),
    /// A bridge, bond or VLAN made, changed or removed.
    Device(String, DeviceAction),
    /// zendiloOS's settings written back over files changed or lost
    /// outside it.
    Restore,
}

impl Change {
    /// What a change of `subject` is, while it waits.
    pub fn of(subject: &Subject) -> Self {
        match subject {
            Subject::Interface(interface) => Self::Interface(interface.clone()),
            Subject::Device(device, action) => Self::Device(device.clone(), *action),
            Subject::Restore => Self::Restore,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    pub change: Change,
    /// When the old settings return unless the change is confirmed.
    pub revert_at: Timestamp,
}

/// ifupdown on the host: its files as read, and what a takeover needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfupdownHost {
    pub files: Vec<IfupdownFile>,
    /// Of the files as read; a takeover names it, and the host refuses
    /// one if they changed since.
    pub digest: String,
    /// `networking.service` runs: ifupdown brought the network up.
    pub active: bool,
    pub resolved_installed: bool,
}

/// How the host manages its network, as it tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostNetwork {
    /// systemd-networkd manages it.
    pub managed: bool,
    pub network_manager: bool,
    pub pending: Option<PendingChange>,
    /// Not managed: ifupdown's files, where it is enabled.
    pub ifupdown: Option<IfupdownHost>,
    /// zendiloOS's systemd-networkd files as they are.
    pub files: Vec<NetworkdFile>,
}

/// Whether the network can be changed here, and a change that waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSetup {
    /// systemd-networkd manages the network: zendiloOS changes it.
    pub managed: bool,
    pub manager: Manager,
    pub pending: Option<PendingChange>,
    /// Where ifupdown manages the network: what taking it over would do.
    pub takeover: Option<Takeover>,
    /// The bridges, bonds and VLANs zendiloOS made, by name.
    pub devices: Vec<Device>,
    /// Of its bridges, the internal networks for VMs: without members or
    /// an address of the NAS's, which the NAS keeps itself off.
    pub internal: Vec<String>,
    /// zendiloOS's files differ from its settings: changed or lost outside
    /// it. Told while no change waits.
    pub drift: Option<Drift>,
}

/// How zendiloOS's files differ from its settings as last confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub differences: Vec<Difference>,
    /// When the settings were confirmed (or taken from the files).
    pub kept_at: Timestamp,
}

/// zendiloOS's network settings as last confirmed: its systemd-networkd
/// files as they were then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptSettings {
    pub files: Vec<NetworkdFile>,
    pub kept_at: Timestamp,
    /// The differences last told as a notification, so each is told once.
    pub said: Option<String>,
}

/// Where zendiloOS keeps its network settings: its database.
#[async_trait]
pub trait SettingsStore: Send + Sync {
    /// The settings as last confirmed; none before zendiloOS kept any.
    async fn kept(&self) -> Result<Option<KeptSettings>, PortError>;
    /// Keeps `files` as the settings from `at` on, with nothing said of
    /// them.
    async fn keep(&self, files: &[NetworkdFile], at: Timestamp) -> Result<(), PortError>;
    /// Notes what was told of the differences.
    async fn note_said(&self, said: Option<&str>) -> Result<(), PortError>;
}

/// What looking at the files at the start found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looked {
    /// systemd-networkd does not manage the network: there is nothing of
    /// zendiloOS's to compare.
    NotManaged,
    /// A change waits: the files are not settled; look again later.
    Pending,
    /// The first look: the files as they are became the settings.
    Kept,
    /// The files say the settings.
    Same,
    /// They differ in so many ways.
    Differs(usize),
}

/// Taking the network over from ifupdown, as it would go now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Takeover {
    pub plan: TakeoverPlan,
    /// systemd-resolved is installed from Debian first.
    pub installs_resolved: bool,
    /// `networking.service` runs; ifupdown has nothing to give otherwise.
    pub ifupdown_running: bool,
    /// Of ifupdown's files the plan was made from.
    pub digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigureError {
    #[error("refused by the host: {0}")]
    Refused(String),
    /// Another change waits for confirmation.
    #[error("another change waits for confirmation")]
    Pending,
    #[error(transparent)]
    Invalid(#[from] InvalidSettings),
    #[error(transparent)]
    InvalidDevice(#[from] InvalidDevice),
    /// What uses a bridge that would go: VMs, by name.
    #[error("in use by {}", .0.join(", "))]
    InUse(Vec<String>),
    /// ifupdown sets up what a takeover would not carry over.
    #[error("not carried over: {}", refusals(.0))]
    NotCarriedOver(Vec<Refusal>),
    /// zendiloOS's files differ from its settings: restored or kept first.
    #[error("zendiloOS's network files differ from its settings: restore them or keep the files first")]
    Drifted,
    #[error(transparent)]
    Port(#[from] PortError),
}

/// What differs, as a key: told once until it changes.
fn said(found: &[Difference]) -> String {
    found
        .iter()
        .map(|difference| {
            let part = match difference.part {
                Part::Device => "device",
                Part::Interface => "interface",
            };
            let kind = match difference.kind {
                DifferenceKind::Missing => "missing",
                DifferenceKind::Changed => "changed",
                DifferenceKind::Added => "added",
            };
            format!("{part} {} {kind}", difference.name)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `interfaces:12: br0: a bridge; …`
pub fn refusals(refused: &[Refusal]) -> String {
    refused.iter().map(|r| format!("{}: {}", r.place, r.reason)).collect::<Vec<_>>().join("; ")
}

/// Changes the network on the host; unconfirmed changes go back by
/// themselves.
#[async_trait]
pub trait NetworkConfigurator: Send + Sync {
    async fn state(&self, context: &RequestContext) -> Result<HostNetwork, PortError>;

    /// Writes what `edit` says as one change: devices made, changed or
    /// removed, interfaces' files.
    async fn change(
        &self,
        edit: &Edit,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError>;

    /// Installs systemd-resolved from Debian, waiting until apt is done;
    /// the network stays as it is.
    async fn install_resolved(&self, context: &RequestContext) -> Result<(), ConfigureError>;

    /// Takes the network over from ifupdown with `interfaces`, if its files
    /// are still those of `digest`; ifupdown returns unless confirmed.
    async fn take_over(
        &self,
        interfaces: &[InterfaceSettings],
        digest: &str,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError>;

    async fn confirm(&self, context: &RequestContext) -> Result<(), ConfigureError>;
}

/// What uses a device and would lose its network were it removed or taken
/// into a bridge or bond, asked of the contexts that put things on the
/// host's links (the VMs: on a bridge, or directly on a card or bond).
#[async_trait]
pub trait DeviceUsers: Send + Sync {
    /// The names of what uses `device`; none: it may go.
    async fn users(&self, device: &str) -> Result<Vec<String>, PortError>;
}

/// A bridge, bond or VLAN to make, change or remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRequest {
    Make(NewDevice),
    Change(DeviceUpdate),
    Remove(String),
}

/// Query: interfaces, addresses, default routes and name servers.
#[derive(Clone)]
pub struct GetNetwork {
    source: Arc<dyn NetworkSource>,
}

impl GetNetwork {
    pub fn new(source: Arc<dyn NetworkSource>) -> Self {
        Self { source }
    }

    pub async fn execute(&self) -> Result<Network, PortError> {
        self.source.network().await
    }
}

/// How long a change waits for confirmation from the new address.
pub const CONFIRM_WITHIN_SECONDS: u16 = 120;

/// What the network's use cases need.
#[derive(Clone)]
pub struct NetworkPorts {
    pub source: Arc<dyn NetworkSource>,
    pub configurator: Arc<dyn NetworkConfigurator>,
    pub users: Arc<dyn DeviceUsers>,
    pub store: Arc<dyn SettingsStore>,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

/// Changing an interface's settings, and taking the network over from
/// ifupdown: applied at once, kept only if confirmed in time, which needs
/// the NAS reachable afterwards.
#[derive(Clone)]
pub struct ConfigureNetwork {
    ports: NetworkPorts,
}

fn job_error(error: ConfigureError) -> JobError {
    let code = match &error {
        ConfigureError::Refused(_) => "refused",
        ConfigureError::Pending => "change_pending",
        ConfigureError::Invalid(_) | ConfigureError::InvalidDevice(_) => "invalid_input",
        ConfigureError::InUse(_) => "device_in_use",
        ConfigureError::NotCarriedOver(_) => "not_carried_over",
        ConfigureError::Drifted => "network_drift",
        ConfigureError::Port(_) => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

impl ConfigureNetwork {
    pub fn new(ports: NetworkPorts) -> Self {
        Self { ports }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.ports.clock.now(), action, outcome, request)
            .by(actor)
            .target(target.to_owned());
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    /// What manages the network, a change that waits, and where ifupdown
    /// manages it, what taking it over would do.
    pub async fn setup(&self, request: &RequestContext) -> Result<NetworkSetup, PortError> {
        let host = self.ports.configurator.state(request).await?;
        let manager = match (&host.ifupdown, host.managed, host.network_manager) {
            (_, true, _) => Manager::Networkd,
            (_, false, true) => Manager::NetworkManager,
            (Some(_), false, false) => Manager::Ifupdown,
            (None, false, false) => Manager::Unknown,
        };
        let drift = self.drift(&host).await?;
        let takeover = match host.ifupdown {
            Some(ifupdown) if manager == Manager::Ifupdown => {
                let network = self.ports.source.network().await?;
                Some(Takeover {
                    plan: takeover_plan(&ifupdown.files, &network),
                    installs_resolved: !ifupdown.resolved_installed,
                    ifupdown_running: ifupdown.active,
                    digest: ifupdown.digest,
                })
            }
            _ => None,
        };
        let configuration = Configuration::read(&host.files);
        let internal = configuration.internal_networks();
        Ok(NetworkSetup {
            managed: host.managed,
            manager,
            pending: host.pending,
            takeover,
            devices: configuration.devices,
            internal,
            drift,
        })
    }

    /// How the files differ from the settings kept, while systemd-networkd
    /// manages the network and no change waits (one that waits is in the
    /// files until it is confirmed or goes back).
    async fn drift(&self, host: &HostNetwork) -> Result<Option<Drift>, PortError> {
        if !host.managed || host.pending.is_some() {
            return Ok(None);
        }
        let Some(kept) = self.ports.store.kept().await? else { return Ok(None) };
        let found = differences(&Configuration::read(&kept.files), &Configuration::read(&host.files));
        Ok((!found.is_empty()).then_some(Drift { differences: found, kept_at: kept.kept_at }))
    }

    /// Refuses a change while the files differ from the settings: it would
    /// be planned on files zendiloOS did not write, and confirming it would
    /// keep them.
    async fn refuse_drift(&self, host: &HostNetwork) -> Result<(), ConfigureError> {
        match self.drift(host).await? {
            Some(_) => Err(ConfigureError::Drifted),
            None => Ok(()),
        }
    }

    /// Keeps the files as they are as zendiloOS's settings.
    async fn keep_files(&self, request: &RequestContext) -> Result<(), PortError> {
        let host = self.ports.configurator.state(request).await?;
        self.ports.store.keep(&host.files, self.ports.clock.now()).await
    }

    /// At the start, before anything changes the network: compares the
    /// files with the settings kept. A first look keeps the files as the
    /// settings; files that differ are told as a notification (each
    /// difference once, also across restarts) and on the Network page,
    /// never rewritten by themselves: written back at the start, a network
    /// that worked could be cut off with nobody there to confirm it, and
    /// one that did not would go back to it unconfirmed.
    pub async fn look_at_files(&self, request: &RequestContext) -> Result<Looked, PortError> {
        let host = self.ports.configurator.state(request).await?;
        if !host.managed {
            return Ok(Looked::NotManaged);
        }
        if host.pending.is_some() {
            return Ok(Looked::Pending);
        }
        let Some(kept) = self.ports.store.kept().await? else {
            self.ports.store.keep(&host.files, self.ports.clock.now()).await?;
            tracing::info!(files = host.files.len(), "network: zendiloOS's settings kept from its files");
            return Ok(Looked::Kept);
        };
        let found = differences(&Configuration::read(&kept.files), &Configuration::read(&host.files));
        if found.is_empty() {
            if kept.said.is_some() {
                self.ports.store.note_said(None).await?;
            }
            return Ok(Looked::Same);
        }
        let said = said(&found);
        tracing::warn!(differences = said, "network: zendiloOS's files differ from its settings");
        if kept.said.as_deref() != Some(said.as_str()) {
            let mut names: Vec<&str> = Vec::new();
            for difference in &found {
                if !names.contains(&difference.name.as_str()) {
                    names.push(&difference.name);
                }
            }
            self.ports
                .notifier
                .notify(
                    NewNotification::new(Level::Warning, "network_drift")
                        .with("count", found.len().to_string())
                        .with("names", names.join(", ")),
                )
                .await;
            self.ports.store.note_said(Some(&said)).await?;
        }
        Ok(Looked::Differs(found.len()))
    }

    /// Writes zendiloOS's settings back over the files, as one change that
    /// goes back unless confirmed within two minutes; confirmed, the files
    /// say the settings again.
    pub async fn restore(
        &self,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<NetworkSetup, ConfigureError> {
        let done = async {
            let host = self.ports.configurator.state(request).await?;
            if host.pending.is_some() {
                return Err(ConfigureError::Pending);
            }
            if !host.managed {
                return Err(ConfigureError::Refused(
                    "systemd-networkd does not manage the network here".into(),
                ));
            }
            let kept = self.ports.store.kept().await?;
            let kept = kept
                .ok_or_else(|| ConfigureError::Refused("zendiloOS keeps no network settings yet".into()))?;
            let edit = restoring(&Configuration::read(&kept.files), &Configuration::read(&host.files))
                .ok_or_else(|| {
                    ConfigureError::Refused("the files say zendiloOS's settings already".into())
                })?;
            self.ports.configurator.change(&edit, CONFIRM_WITHIN_SECONDS, request).await
        }
        .await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("network.restore", outcome, "", actor, request).await;
        done?;
        Ok(self.setup(request).await?)
    }

    /// Keeps the files as they are as zendiloOS's settings: changed by hand
    /// on purpose, say.
    pub async fn keep(
        &self,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<NetworkSetup, ConfigureError> {
        let done = async {
            let host = self.ports.configurator.state(request).await?;
            if host.pending.is_some() {
                return Err(ConfigureError::Pending);
            }
            if !host.managed {
                return Err(ConfigureError::Refused(
                    "systemd-networkd does not manage the network here".into(),
                ));
            }
            Ok(self.ports.store.keep(&host.files, self.ports.clock.now()).await?)
        }
        .await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("network.keep_files", outcome, "", actor, request).await;
        done?;
        Ok(self.setup(request).await?)
    }

    pub async fn apply(
        &self,
        settings: InterfaceSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<NetworkSetup, ConfigureError> {
        let mut settings = settings.checked()?;
        let network = self.ports.source.network().await;
        if let Ipv4Setting::Dhcp { keep } = &mut settings.ipv4 {
            // The address DHCP gave it, asked for again: it stays where the
            // server agrees, also once the client's identity changed.
            *keep = network.as_ref().ok().and_then(|network| network.dynamic_address(&settings.interface));
        }
        let interface = settings.interface.clone();
        let applied = async {
            // The VLANs on it stay; a member's settings are its device's.
            let host = self.ports.configurator.state(request).await?;
            self.refuse_drift(&host).await?;
            let configuration = Configuration::read(&host.files);
            let network = network.unwrap_or_default();
            let edit = Planning { network: &network, configuration: &configuration }.settings(settings)?;
            self.ports.configurator.change(&edit, CONFIRM_WITHIN_SECONDS, request).await
        }
        .await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("network.apply", outcome, &interface, actor, request).await;
        applied?;
        Ok(self.setup(request).await?)
    }

    /// What a bridge, bond or VLAN made, changed or removed would do,
    /// checked, without doing it: the page shows what becomes of each
    /// interface before it is asked for.
    pub async fn plan(
        &self,
        device: DeviceRequest,
        request: &RequestContext,
    ) -> Result<Edit, ConfigureError> {
        let host = self.ports.configurator.state(request).await?;
        self.refuse_drift(&host).await?;
        let configuration = Configuration::read(&host.files);
        let network = self.ports.source.network().await?;
        let planning = Planning { network: &network, configuration: &configuration };
        // A member taken into a bridge or bond carries nothing of its own
        // any more: a VM directly on it would lose its network.
        let joining: Vec<String> = match &device {
            DeviceRequest::Make(new) => new.members.clone(),
            DeviceRequest::Change(update) => {
                let had = configuration.device(&update.name).map(|d| d.members.clone()).unwrap_or_default();
                update.members.iter().filter(|member| !had.contains(member)).cloned().collect()
            }
            DeviceRequest::Remove(_) => Vec::new(),
        };
        for member in &joining {
            let users = self.ports.users.users(member).await?;
            if !users.is_empty() {
                return Err(ConfigureError::InUse(users));
            }
        }
        Ok(match device {
            DeviceRequest::Make(new) => planning.make(new)?,
            DeviceRequest::Change(update) => planning.change(update)?,
            DeviceRequest::Remove(name) => {
                let users = self.ports.users.users(&name).await?;
                if !users.is_empty() {
                    return Err(ConfigureError::InUse(users));
                }
                planning.remove(&name, &users)?
            }
        })
    }

    /// Makes, changes or removes a bridge, bond or VLAN as [`Self::plan`]
    /// says, with what that does to its members and parent, as one change
    /// that goes back unless confirmed within two minutes.
    pub async fn device(
        &self,
        device: DeviceRequest,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<NetworkSetup, ConfigureError> {
        let (action, name) = match &device {
            DeviceRequest::Make(new) => ("network.device_make", new.name.clone()),
            DeviceRequest::Change(update) => ("network.device_change", update.name.clone()),
            DeviceRequest::Remove(name) => ("network.device_remove", name.clone()),
        };
        let done = async {
            if self.ports.configurator.state(request).await?.pending.is_some() {
                return Err(ConfigureError::Pending);
            }
            let edit = self.plan(device, request).await?;
            self.ports.configurator.change(&edit, CONFIRM_WITHIN_SECONDS, request).await
        }
        .await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit(action, outcome, &name, actor, request).await;
        done?;
        Ok(self.setup(request).await?)
    }

    /// Takes the network over from ifupdown, in a job: systemd-resolved
    /// installed first if it is missing, then the interfaces set up as the
    /// plan says. Refused while anything would not be carried over.
    pub async fn take_over(&self, actor: &Actor, request: &RequestContext) -> Result<Job, ConfigureError> {
        let setup = self.setup(request).await?;
        if setup.pending.is_some() {
            return Err(ConfigureError::Pending);
        }
        let Some(takeover) = setup.takeover else {
            return Err(ConfigureError::Refused(
                match setup.manager {
                    Manager::Networkd => "zendiloOS manages the network already",
                    Manager::NetworkManager => "NetworkManager manages the network here",
                    Manager::Ifupdown | Manager::Unknown => "ifupdown does not manage the network here",
                }
                .into(),
            ));
        };
        if !takeover.plan.refused.is_empty() {
            return Err(ConfigureError::NotCarriedOver(takeover.plan.refused));
        }
        if !takeover.ifupdown_running {
            return Err(ConfigureError::Refused(
                "networking.service is not running: ifupdown did not bring the network up".into(),
            ));
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), request.clone());
        let job = self
            .ports
            .jobs
            .start("network.take_over", Some("ifupdown".into()), Some(actor), move |progress| async move {
                let done = async {
                    if takeover.installs_resolved {
                        progress.step("packages", Some(0.1)).await;
                        this.ports.configurator.install_resolved(&context).await?;
                    }
                    progress.step("taking_over", Some(0.8)).await;
                    let interfaces = &takeover.plan.interfaces;
                    this.ports
                        .configurator
                        .take_over(interfaces, &takeover.digest, CONFIRM_WITHIN_SECONDS, &context)
                        .await
                }
                .await;
                let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
                this.audit("network.take_over", outcome, "ifupdown", &who, &context).await;
                done.map_err(job_error)
            })
            .await?;
        Ok(job)
    }

    /// Keeps the change that waits; its files become zendiloOS's settings.
    /// Changes are refused while the files differ from the settings, so
    /// what is kept is the settings with this change: a restore's are the
    /// settings themselves.
    pub async fn confirm(&self, actor: &Actor, request: &RequestContext) -> Result<(), ConfigureError> {
        let pending = self.ports.configurator.state(request).await?.pending;
        let confirmed = self.ports.configurator.confirm(request).await;
        let outcome = if confirmed.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = match pending.map(|p| p.change) {
            Some(Change::Interface(interface) | Change::Device(interface, _)) => interface,
            Some(Change::Takeover(_)) => "ifupdown".into(),
            Some(Change::Restore) | None => String::new(),
        };
        self.audit("network.confirm", outcome, &target, actor, request).await;
        confirmed?;
        // The network is as confirmed; the settings follow, and if they
        // cannot be kept now, the next start tells the difference.
        if let Err(error) = self.keep_files(request).await {
            tracing::error!(%error, "network: confirmed, but the settings cannot be kept");
        }
        Ok(())
    }
}

/// All use cases of this context.
#[derive(Clone)]
pub struct NetworkUseCases {
    pub get_network: GetNetwork,
    pub configure: ConfigureNetwork,
}

impl NetworkUseCases {
    pub fn new(ports: NetworkPorts) -> Self {
        Self { get_network: GetNetwork::new(ports.source.clone()), configure: ConfigureNetwork::new(ports) }
    }
}

#[cfg(test)]
mod tests;
