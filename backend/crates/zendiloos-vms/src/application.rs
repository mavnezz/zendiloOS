//! Use cases: the plugin installed and its storage set up, VMs made (an
//! ISO copied, the domain defined, in a job), run, changed and removed,
//! and their consoles opened. All for administrators (`vms.manage`); the
//! caller checks that.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobError, Jobs, Progress};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::domain::{
    HostLimits, Invalid, IsoChoice, IsoOrigin, PowerState, Vm, VmAction, VmSettings, check_name, is_vm_id,
};

/// The VMs as zendiloOS keeps them.
#[async_trait]
pub trait VmRepository: Send + Sync {
    /// Oldest first.
    async fn list(&self) -> Result<Vec<Vm>, PortError>;
    async fn find(&self, id: &str) -> Result<Option<Vm>, PortError>;
    /// `false`: its name or id is taken.
    async fn create(&self, vm: &Vm) -> Result<bool, PortError>;
    async fn set_ready(&self, id: &str) -> Result<(), PortError>;
    async fn set_autostart(&self, id: &str, autostart: bool) -> Result<bool, PortError>;
    async fn delete(&self, id: &str) -> Result<bool, PortError>;
}

/// A pool zendiloOS made, by its name and volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePlace {
    pub pool: String,
    pub volume: u16,
}

/// What the machine offers VMs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostFacts {
    /// `/dev/kvm` is there.
    pub kvm: bool,
    /// `vmx` (Intel VT-x) or `svm` (AMD-V).
    pub cpu_virtualization: Option<String>,
    pub iommu: bool,
    /// libvirt and QEMU are installed.
    pub installed: bool,
    pub limits: HostLimits,
    /// Where the VMs' storage is, once set up.
    pub storage: Option<StoragePlace>,
}

/// An ISO the administrator may read, where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoSource {
    pub share: String,
    pub volume: u16,
    /// In the share.
    pub path: String,
}

/// A VM's console, connected: what its VNC server sends comes on `output`,
/// which closes when the console does; what goes to `input` reaches it.
/// Dropping `input` closes the connection.
#[derive(Debug)]
pub struct ConsoleSession {
    pub output: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub input: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Where the steps of a long piece of work go.
pub trait Steps: Send + Sync {
    fn step(&self, step: &str, percent: Option<u8>);
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host would not, in its words.
    #[error("{0}")]
    Refused(String),
    /// It tried and failed, in its words.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// libvirt, QEMU and the VMs' storage on the host, through the system agent.
#[async_trait]
pub trait VmHost: Send + Sync {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError>;
    /// The pools zendiloOS made, where the storage can go.
    async fn pools(&self, context: &RequestContext) -> Result<Vec<StoragePlace>, PortError>;
    /// Installs libvirt and QEMU from Debian, waiting until apt is done.
    async fn install(&self, context: &RequestContext) -> Result<(), HostError>;
    /// Puts the VMs' storage on the pool; libvirt follows it.
    async fn set_up(&self, place: &StoragePlace, context: &RequestContext) -> Result<(), HostError>;
    /// Copies the ISO for the VM `id` beside its disk.
    async fn copy_iso(
        &self,
        id: &str,
        place: &StoragePlace,
        iso: &IsoSource,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    /// Makes its disk and defines it; with the copy of its ISO if it has one.
    async fn define(&self, vm: &Vm, context: &RequestContext) -> Result<(), HostError>;
    async fn set_autostart(
        &self,
        id: &str,
        autostart: bool,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    async fn control(&self, id: &str, action: VmAction, context: &RequestContext) -> Result<(), HostError>;
    /// Removes the VM, its disk and its ISO's copy; what is gone already
    /// is no failure.
    async fn remove(&self, vm: &Vm, context: &RequestContext) -> Result<(), HostError>;
    /// What libvirt says each VM zendiloOS made does, by id.
    async fn states(&self, context: &RequestContext) -> Result<BTreeMap<String, PowerState>, PortError>;
    async fn open_console(&self, id: &str, context: &RequestContext) -> Result<ConsoleSession, HostError>;
}

#[derive(Debug, thiserror::Error)]
pub enum IsoError {
    /// No share of that id, or the administrator may not read the file.
    #[error("the ISO cannot be read")]
    Denied,
    #[error("there is no such ISO")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Files in shares, as the signed-in administrator may read them.
#[async_trait]
pub trait IsoFiles: Send + Sync {
    async fn locate(
        &self,
        actor: &Actor,
        iso: &IsoChoice,
        context: &RequestContext,
    ) -> Result<IsoSource, IsoError>;
}

/// New ids for VMs: twelve lowercase hexadecimal digits, at random.
pub trait VmIds: Send + Sync {
    fn new_id(&self) -> String;
}

pub struct VmsPorts {
    pub repository: Arc<dyn VmRepository>,
    pub host: Arc<dyn VmHost>,
    pub isos: Arc<dyn IsoFiles>,
    pub ids: Arc<dyn VmIds>,
    pub jobs: Jobs,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, thiserror::Error)]
pub enum VmsError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Invalid(#[from] Invalid),
    /// No `/dev/kvm`: the CPU cannot virtualise, or it is off in the firmware.
    #[error("this machine cannot run virtual machines: there is no /dev/kvm")]
    NoKvm,
    #[error("libvirt and QEMU are not installed")]
    NotInstalled,
    #[error("libvirt and QEMU are installed already")]
    Installed,
    #[error("the virtual machines' storage is not set up")]
    NotSetUp,
    #[error("there is no such pool")]
    PoolNotFound,
    #[error("another virtual machine has that name")]
    NameTaken,
    #[error("the virtual machine is still being created")]
    Creating,
    #[error("the name typed is not the virtual machine's")]
    ConfirmMismatch,
    #[error("the ISO cannot be read")]
    IsoDenied,
    #[error("the virtual machine is not running")]
    NotRunning,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    HostFailed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for VmsError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => Self::Refused(reason),
            HostError::Failed(reason) => Self::HostFailed(reason),
            HostError::Port(error) => Self::Port(error),
        }
    }
}

impl From<IsoError> for VmsError {
    fn from(error: IsoError) -> Self {
        match error {
            IsoError::Denied => Self::IsoDenied,
            IsoError::NotFound => Self::NotFound,
            IsoError::Invalid(reason) => Self::Invalid(Invalid(reason)),
            IsoError::Port(error) => Self::Port(error),
        }
    }
}

fn job_error(error: HostError) -> JobError {
    match error {
        HostError::Refused(reason) => JobError::new("refused", Some(reason)),
        HostError::Failed(reason) => JobError::new("agent_failed", Some(reason)),
        HostError::Port(error) => JobError::new("agent_failed", Some(error.to_string())),
    }
}

/// Consoles open at once per account.
pub const CONSOLES_PER_ACCOUNT: usize = 4;

/// The plugin as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmPlugin {
    pub facts: HostFacts,
    /// Where the storage can go.
    pub pools: Vec<StoragePlace>,
}

/// A VM and what it does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmView {
    pub vm: Vm,
    pub state: PowerState,
}

/// What an administrator asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVm {
    pub settings: VmSettings,
    pub iso: Option<IsoChoice>,
}

/// The consoles each account has open.
#[derive(Debug, Default)]
struct OpenConsoles(Mutex<BTreeMap<i64, usize>>);

/// A place among an account's consoles, given back when dropped.
#[derive(Debug)]
struct Seat {
    open: Arc<OpenConsoles>,
    account: i64,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let mut open = self.open.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.account) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.account);
            }
        }
    }
}

/// A VM's console, open for one account.
#[derive(Debug)]
pub struct Console {
    pub session: ConsoleSession,
    /// The VM's name, for the window.
    pub name: String,
    _seat: Seat,
}

/// The steps of a job's work, on their way to the job.
struct JobSteps(tokio::sync::mpsc::UnboundedSender<(String, Option<u8>)>);

impl Steps for JobSteps {
    fn step(&self, step: &str, percent: Option<u8>) {
        let _ = self.0.send((step.to_owned(), percent));
    }
}

#[derive(Clone)]
pub struct Vms {
    ports: Arc<VmsPorts>,
    consoles: Arc<OpenConsoles>,
    /// VMs whose deletion runs: not deleted twice at once.
    deleting: Arc<Mutex<BTreeSet<String>>>,
}

impl Vms {
    pub fn new(ports: VmsPorts) -> Self {
        Self { ports: Arc::new(ports), consoles: Arc::default(), deleting: Arc::default() }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.ports.clock.now(), action, outcome, context)
            .by(actor)
            .target(target.to_owned());
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "vms: cannot write the audit log");
        }
    }

    // The plugin.

    pub async fn plugin(&self, context: &RequestContext) -> Result<VmPlugin, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        let pools = self.ports.host.pools(context).await?;
        Ok(VmPlugin { facts, pools })
    }

    /// Installs libvirt and QEMU from Debian, in a job; refused on a machine
    /// without KVM, which could install them but never run a VM.
    pub async fn install(&self, actor: &Actor, context: &RequestContext) -> Result<Job, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        if !facts.kvm {
            return Err(VmsError::NoKvm);
        }
        if facts.installed {
            return Err(VmsError::Installed);
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("vms.install", Some("libvirt".into()), Some(actor), move |progress| async move {
                progress.step("packages", Some(0.05)).await;
                let done = this.ports.host.install(&context).await;
                let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
                this.audit("vms.install", outcome, "libvirt", &who, &context).await;
                done.map_err(job_error)
            })
            .await?;
        Ok(job)
    }

    /// Puts the VMs' storage on a pool zendiloOS made.
    pub async fn set_up(
        &self,
        pool: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmPlugin, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        if !facts.installed {
            return Err(VmsError::NotInstalled);
        }
        let place = self
            .ports
            .host
            .pools(context)
            .await?
            .into_iter()
            .find(|place| place.pool == pool)
            .ok_or(VmsError::PoolNotFound)?;
        let done = self.ports.host.set_up(&place, context).await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("vms.storage", outcome, pool, actor, context).await;
        done?;
        self.plugin(context).await
    }

    // The VMs.

    pub async fn list(&self, context: &RequestContext) -> Result<Vec<VmView>, VmsError> {
        let vms = self.ports.repository.list().await?;
        let states = self.states(&vms, context).await;
        Ok(vms
            .into_iter()
            .map(|vm| {
                let state = Self::state_of(&vm, &states);
                VmView { vm, state }
            })
            .collect())
    }

    pub async fn get(&self, id: &str, context: &RequestContext) -> Result<VmView, VmsError> {
        let vm = self.find(id).await?;
        let states = self.states(std::slice::from_ref(&vm), context).await;
        let state = Self::state_of(&vm, &states);
        Ok(VmView { vm, state })
    }

    async fn find(&self, id: &str) -> Result<Vm, VmsError> {
        if !is_vm_id(id) {
            return Err(VmsError::NotFound);
        }
        self.ports.repository.find(id).await?.ok_or(VmsError::NotFound)
    }

    /// libvirt's states; none while it cannot be asked, when every VM shows
    /// as unknown rather than the list failing.
    async fn states(&self, vms: &[Vm], context: &RequestContext) -> BTreeMap<String, PowerState> {
        if !vms.iter().any(|vm| vm.ready) {
            return BTreeMap::new();
        }
        self.ports.host.states(context).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "vms: libvirt cannot say what the virtual machines do");
            BTreeMap::new()
        })
    }

    fn state_of(vm: &Vm, states: &BTreeMap<String, PowerState>) -> PowerState {
        if !vm.ready {
            return PowerState::Creating;
        }
        states.get(&vm.id).copied().unwrap_or(PowerState::Unknown)
    }

    /// Records the VM and makes it in a job: the ISO copied (if one was
    /// picked), the disk made and the domain defined. Should that fail,
    /// what was made goes again, the record too.
    pub async fn create(
        &self,
        new: NewVm,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(VmView, Job), VmsError> {
        let facts = self.ports.host.facts(context).await?;
        if !facts.installed {
            return Err(VmsError::NotInstalled);
        }
        let place = facts.storage.clone().ok_or(VmsError::NotSetUp)?;
        let existing = self.ports.repository.list().await?;
        let taken: Vec<&str> = existing.iter().map(|vm| vm.settings.name.as_str()).collect();
        let name = new.settings.check(&facts.limits, &taken)?;
        let iso = match &new.iso {
            Some(choice) => {
                choice.check()?;
                Some(self.ports.isos.locate(actor, choice, context).await?)
            }
            None => None,
        };
        let id = self.ports.ids.new_id();
        if !is_vm_id(&id) || existing.iter().any(|vm| vm.id == id) {
            return Err(VmsError::Port(PortError::new(
                "virtual machine ids",
                format!("{id:?} is no new id"),
            )));
        }
        let vm = Vm {
            id,
            settings: VmSettings { name, ..new.settings },
            iso: iso
                .as_ref()
                .map(|source| IsoOrigin { share: source.share.clone(), path: source.path.clone() }),
            pool: place.pool.clone(),
            volume: place.volume,
            created_at: self.ports.clock.now(),
            ready: false,
        };
        if !self.ports.repository.create(&vm).await? {
            return Err(VmsError::NameTaken);
        }
        let this = self.clone();
        let (who, context_owned, made) = (actor.clone(), context.clone(), vm.clone());
        let started = self
            .ports
            .jobs
            .start("vms.create", Some(vm.settings.name.clone()), Some(actor), move |progress| async move {
                let outcome = this.make(&made, &place, iso.as_ref(), &progress, &context_owned).await;
                match &outcome {
                    Ok(()) => {
                        if let Err(error) = this.ports.repository.set_ready(&made.id).await {
                            tracing::error!(%error, id = made.id, "vms: made, but cannot record it");
                        }
                    }
                    Err(_) => this.undo(&made, &context_owned).await,
                }
                let result = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
                this.audit("vms.create", result, &made.settings.name, &who, &context_owned).await;
                outcome.map_err(job_error)
            })
            .await;
        match started {
            Ok(job) => Ok((VmView { vm, state: PowerState::Creating }, job)),
            Err(error) => {
                let _ = self.ports.repository.delete(&vm.id).await;
                Err(error.into())
            }
        }
    }

    async fn make(
        &self,
        vm: &Vm,
        place: &StoragePlace,
        iso: Option<&IsoSource>,
        progress: &Progress,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        if let Some(iso) = iso {
            progress.step("copying_iso", Some(0.0)).await;
            // Reported as the agent's answer comes, kept in order.
            let (sender, mut reported) = tokio::sync::mpsc::unbounded_channel::<(String, Option<u8>)>();
            let forwarded = {
                let progress = progress.clone();
                tokio::spawn(async move {
                    while let Some((step, percent)) = reported.recv().await {
                        let share = percent.map(|p| f32::from(p.min(100)) / 100.0 * 0.9);
                        progress.step(&step, share).await;
                    }
                })
            };
            let copied = self.ports.host.copy_iso(&vm.id, place, iso, &JobSteps(sender), context).await;
            let _ = forwarded.await;
            copied?;
        }
        progress.step("defining", Some(0.95)).await;
        self.ports.host.define(vm, context).await
    }

    /// What a failed creation left goes: on the host and in the records.
    async fn undo(&self, vm: &Vm, context: &RequestContext) {
        if let Err(error) = self.ports.host.remove(vm, context).await {
            tracing::warn!(%error, id = vm.id, "vms: cannot remove what a failed creation left");
        }
        if let Err(error) = self.ports.repository.delete(&vm.id).await {
            tracing::error!(%error, id = vm.id, "vms: cannot forget a virtual machine that was not made");
        }
    }

    async fn ready(&self, id: &str) -> Result<Vm, VmsError> {
        let vm = self.find(id).await?;
        if vm.ready { Ok(vm) } else { Err(VmsError::Creating) }
    }

    pub async fn control(
        &self,
        id: &str,
        action: VmAction,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmView, VmsError> {
        let vm = self.ready(id).await?;
        let done = self.ports.host.control(&vm.id, action, context).await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit(action.audit(), outcome, &vm.settings.name, actor, context).await;
        done?;
        self.get(id, context).await
    }

    pub async fn set_autostart(
        &self,
        id: &str,
        autostart: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmView, VmsError> {
        let vm = self.ready(id).await?;
        let done = self.ports.host.set_autostart(&vm.id, autostart, context).await;
        let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
        let action = if autostart { "vms.autostart_on" } else { "vms.autostart_off" };
        self.audit(action, outcome, &vm.settings.name, actor, context).await;
        done?;
        self.ports.repository.set_autostart(&vm.id, autostart).await?;
        self.get(id, context).await
    }

    /// Removes the VM with its disk; `confirm` must be its name, typed
    /// again.
    pub async fn delete(
        &self,
        id: &str,
        confirm: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        let vm = self.ready(id).await?;
        if check_name(confirm).ok().as_deref() != Some(vm.settings.name.as_str()) {
            return Err(VmsError::ConfirmMismatch);
        }
        if !self.deleting.lock().unwrap_or_else(PoisonError::into_inner).insert(vm.id.clone()) {
            return Err(VmsError::Refused("the virtual machine is being deleted".into()));
        }
        let removed = self.ports.host.remove(&vm, context).await;
        let outcome = if removed.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("vms.delete", outcome, &vm.settings.name, actor, context).await;
        let forgotten = match removed {
            Ok(()) => self.ports.repository.delete(&vm.id).await.map(|_| ()).map_err(VmsError::from),
            Err(error) => Err(error.into()),
        };
        self.deleting.lock().unwrap_or_else(PoisonError::into_inner).remove(&vm.id);
        forgotten
    }

    /// Opens a running VM's console: at most [`CONSOLES_PER_ACCOUNT`] per
    /// account, each recorded in the audit log.
    pub async fn open_console(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Console, VmsError> {
        let view = self.get(id, context).await?;
        if view.state == PowerState::Creating {
            return Err(VmsError::Creating);
        }
        if !view.state.is_on() {
            return Err(VmsError::NotRunning);
        }
        let seat = self.take_seat(actor.account_id)?;
        let opened = self.ports.host.open_console(&view.vm.id, context).await;
        let outcome = if opened.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("vms.console", outcome, &view.vm.settings.name, actor, context).await;
        Ok(Console { session: opened?, name: view.vm.settings.name, _seat: seat })
    }

    fn take_seat(&self, account: i64) -> Result<Seat, VmsError> {
        let mut open = self.consoles.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = open.entry(account).or_insert(0);
        if *count >= CONSOLES_PER_ACCOUNT {
            return Err(VmsError::Refused(format!(
                "{CONSOLES_PER_ACCOUNT} consoles are open already; close one first"
            )));
        }
        *count += 1;
        Ok(Seat { open: Arc::clone(&self.consoles), account })
    }
}

#[cfg(test)]
mod tests;
