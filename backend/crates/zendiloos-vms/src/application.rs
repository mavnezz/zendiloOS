//! Use cases: the plugin installed and its storage set up in one or more
//! locations, VMs made (in a job), run, changed and removed, the image
//! library filled from shares and browsers, the VMs' load watched, and
//! their consoles opened. All for administrators (`vms.manage`); the
//! caller checks that.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobError, Jobs, Progress};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::domain::{
    BootDevice, Disk, Drive, Firmware, HostLimits, Invalid, IsoChoice, Location, Nic, PowerState, Vm,
    VmAction, VmCounters, VmImage, VmSettings, check_name, is_vm_id,
};

mod images;
mod locations;
mod stats;

pub use images::{ImageUpload, UploadTicket, VmImageView};
pub use locations::{DatasetUsage, LocationView, VmOnLocation, ZvolUsage};
pub use stats::{STATS_EVERY, VmStatsSubscription};

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
    /// Replaces its settings, devices and all. `false`: its name is taken.
    async fn update(&self, vm: &Vm) -> Result<bool, PortError>;
    async fn delete(&self, id: &str) -> Result<bool, PortError>;
}

/// The image library as zendiloOS keeps it.
#[async_trait]
pub trait VmImageRepository: Send + Sync {
    /// Oldest first.
    async fn list(&self) -> Result<Vec<VmImage>, PortError>;
    async fn find(&self, id: &str) -> Result<Option<VmImage>, PortError>;
    /// `false`: its name or id is taken.
    async fn create(&self, image: &VmImage) -> Result<bool, PortError>;
    /// In the library, of `bytes`.
    async fn set_ready(&self, id: &str, bytes: u64) -> Result<(), PortError>;
    async fn set_bytes(&self, id: &str, bytes: u64) -> Result<(), PortError>;
    /// `false`: another image has that name.
    async fn rename(&self, id: &str, name: &str) -> Result<bool, PortError>;
    async fn delete(&self, id: &str) -> Result<bool, PortError>;
}

/// The locations of the VMs' storage, in the order they were added: the
/// first holds the image library.
#[async_trait]
pub trait VmLocationRepository: Send + Sync {
    /// The first first.
    async fn list(&self) -> Result<Vec<Location>, PortError>;
    /// Last; one there already stays where it is.
    async fn add(&self, location: &Location) -> Result<(), PortError>;
    async fn remove(&self, pool: &str) -> Result<bool, PortError>;
}

/// A pool zendiloOS made, where a location can go: its health and space as
/// files see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmPool {
    pub location: Location,
    /// ZFS's word: `online`, `degraded`, `faulted`, …
    pub health: String,
    pub used_bytes: u64,
    pub available_bytes: u64,
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
    /// The locations of the VMs' storage the host has, by volume.
    pub locations: Vec<Location>,
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

/// libvirt, QEMU, the VMs' storage and the image library on the host,
/// through the system agent.
#[async_trait]
pub trait VmHost: Send + Sync {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError>;
    /// The pools zendiloOS made, where a location can go.
    async fn pools(&self, context: &RequestContext) -> Result<Vec<VmPool>, PortError>;
    /// Installs libvirt and QEMU from Debian, waiting until apt is done.
    async fn install(&self, context: &RequestContext) -> Result<(), HostError>;
    /// Puts a location of the VMs' storage on the pool; libvirt follows it.
    async fn add_location(&self, place: &Location, context: &RequestContext) -> Result<(), HostError>;
    /// Removes an empty location: its dataset destroyed.
    async fn remove_location(&self, place: &Location, context: &RequestContext) -> Result<(), HostError>;
    /// What each location's dataset and its zvols hold.
    async fn storage_usage(&self, context: &RequestContext) -> Result<Vec<DatasetUsage>, PortError>;
    /// Copies the ISO into the library as `image`; the bytes copied.
    async fn copy_image(
        &self,
        image: &str,
        place: &Location,
        iso: &IsoSource,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<u64, HostError>;
    /// The library's files: their size by image.
    async fn image_files(
        &self,
        place: &Location,
        context: &RequestContext,
    ) -> Result<BTreeMap<String, u64>, PortError>;
    /// Removes an image's file; one that is gone is no failure.
    async fn delete_image(&self, image: &str, volume: u16, context: &RequestContext)
    -> Result<(), HostError>;
    /// Where an upload of `bytes` as `image` is written: closed once all is
    /// written, then [`VmHost::finish_upload`] says what came of it.
    async fn open_upload(
        &self,
        image: &str,
        volume: u16,
        bytes: u64,
        context: &RequestContext,
    ) -> Result<tokio::fs::File, HostError>;
    /// The bytes an upload stored, once its writer is closed.
    async fn finish_upload(&self, image: &str, context: &RequestContext) -> Result<u64, HostError>;
    /// Makes a new VM's disks and defines it; the images in its drives are
    /// in the library on `library`'s volume.
    async fn define(&self, vm: &Vm, library: u16, context: &RequestContext) -> Result<(), HostError>;
    /// Changes a VM that is off: disks made or grown, the domain defined
    /// anew, the disks in `removed` destroyed.
    async fn update(
        &self,
        vm: &Vm,
        library: u16,
        removed: &[u8],
        context: &RequestContext,
    ) -> Result<(), HostError>;
    /// Puts the image the VM's drive `drive` has into it, or ejects it:
    /// at once while it runs.
    async fn change_media(
        &self,
        vm: &Vm,
        library: u16,
        drive: u8,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    async fn set_autostart(
        &self,
        id: &str,
        autostart: bool,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    async fn control(&self, id: &str, action: VmAction, context: &RequestContext) -> Result<(), HostError>;
    /// Removes the VM and its disks; what is gone already is no failure.
    async fn remove(&self, vm: &Vm, context: &RequestContext) -> Result<(), HostError>;
    /// What libvirt says each VM zendiloOS made does, by id.
    async fn states(&self, context: &RequestContext) -> Result<BTreeMap<String, PowerState>, PortError>;
    /// What libvirt counts of each running VM zendiloOS made, by id: one
    /// look for all.
    async fn counters(&self, context: &RequestContext) -> Result<BTreeMap<String, VmCounters>, PortError>;
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

/// New ids for VMs and images: twelve lowercase hexadecimal digits, at
/// random; and MAC addresses for their network cards.
pub trait VmIds: Send + Sync {
    fn new_id(&self) -> String;
    /// `52:54:00:xx:xx:xx`, QEMU's prefix, the rest at random.
    fn new_mac(&self) -> String;
}

pub struct VmsPorts {
    pub repository: Arc<dyn VmRepository>,
    pub images: Arc<dyn VmImageRepository>,
    pub locations: Arc<dyn VmLocationRepository>,
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
    /// What is on a location that would go: VMs by name, the library.
    #[error("the location holds {}", .0.join(", "))]
    LocationInUse(Vec<String>),
    #[error("another virtual machine or image has that name")]
    NameTaken,
    #[error("the virtual machine is still being created")]
    Creating,
    #[error("the name typed is not the virtual machine's")]
    ConfirmMismatch,
    #[error("the ISO cannot be read")]
    IsoDenied,
    #[error("the virtual machine is not running")]
    NotRunning,
    /// Its hardware changes only while it is off.
    #[error("the virtual machine is not shut off: shut it down before its hardware changes")]
    Running,
    /// The image is in the drives of these VMs.
    #[error("the image is in use by {}", .0.join(", "))]
    ImageInUse(Vec<String>),
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

fn outcome<T, E>(done: &Result<T, E>) -> Outcome {
    if done.is_ok() { Outcome::Success } else { Outcome::Failure }
}

/// Consoles open at once per account.
pub const CONSOLES_PER_ACCOUNT: usize = 4;

/// The plugin as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmPlugin {
    pub facts: HostFacts,
    /// The locations of the VMs' storage, the first (the library's) first.
    pub locations: Vec<Location>,
    /// Where a location can go.
    pub pools: Vec<VmPool>,
}

impl VmPlugin {
    /// The location of the library, once set up.
    pub fn library(&self) -> Option<&Location> {
        self.locations.first()
    }
}

/// A VM and what it does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmView {
    pub vm: Vm,
    pub state: PowerState,
}

/// What an administrator asks for: the settings of a VM to make. Its
/// cards' MAC addresses are given here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVm {
    pub settings: VmSettings,
}

/// What an administrator changes of a VM: all but what it runs. A card in
/// a slot it had keeps its MAC address, a new one gets one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmChange {
    pub name: String,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: Firmware,
    pub disks: Vec<Disk>,
    pub nics: Vec<Nic>,
    pub drives: Vec<Drive>,
    pub boot: Vec<BootDevice>,
    pub autostart: bool,
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

/// A VM being deleted or changed: no second change of it meanwhile.
struct Busy {
    busy: Arc<Mutex<BTreeSet<String>>>,
    id: String,
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.busy.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.id);
    }
}

#[derive(Clone)]
pub struct Vms {
    ports: Arc<VmsPorts>,
    consoles: Arc<OpenConsoles>,
    /// VMs being deleted or changed.
    busy: Arc<Mutex<BTreeSet<String>>>,
    /// The library and the drives that hold its images change one at a
    /// time: an image is never deleted while a VM is given it.
    library: Arc<tokio::sync::Mutex<()>>,
    stats: Arc<stats::Hub>,
}

impl Vms {
    pub fn new(ports: VmsPorts) -> Self {
        Self {
            ports: Arc::new(ports),
            consoles: Arc::default(),
            busy: Arc::default(),
            library: Arc::default(),
            stats: Arc::default(),
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
        let entry = AuditEntry::new(self.ports.clock.now(), action, outcome, context)
            .by(actor)
            .target(target.to_owned());
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "vms: cannot write the audit log");
        }
    }

    /// A new id no VM and no image has.
    async fn new_id(&self) -> Result<String, VmsError> {
        let id = self.ports.ids.new_id();
        let taken =
            self.ports.repository.find(&id).await?.is_some() || self.ports.images.find(&id).await?.is_some();
        if !is_vm_id(&id) || taken {
            return Err(VmsError::Port(PortError::new(
                "virtual machine ids",
                format!("{id:?} is no new id"),
            )));
        }
        Ok(id)
    }

    fn take_busy(&self, id: &str, what: &str) -> Result<Busy, VmsError> {
        if !self.busy.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned()) {
            return Err(VmsError::Refused(format!("the virtual machine is being {what}")));
        }
        Ok(Busy { busy: Arc::clone(&self.busy), id: id.to_owned() })
    }

    // The plugin.

    pub async fn plugin(&self, context: &RequestContext) -> Result<VmPlugin, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        let locations = self.ordered_locations(&facts).await?;
        let pools = self.ports.host.pools(context).await?;
        Ok(VmPlugin { facts, locations, pools })
    }

    /// The locations in the order they were added, those the host has; one
    /// the host has and the records not (set up before they kept them, or
    /// by an earlier zendiloOS) is taken in, after those known.
    async fn ordered_locations(&self, facts: &HostFacts) -> Result<Vec<Location>, VmsError> {
        let mut known = self.ports.locations.list().await?;
        for location in &facts.locations {
            if !known.iter().any(|other| other.pool == location.pool) {
                self.ports.locations.add(location).await?;
                known.push(location.clone());
            }
        }
        Ok(known.into_iter().filter(|location| facts.locations.contains(location)).collect())
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
                this.audit("vms.install", outcome(&done), "libvirt", &who, &context).await;
                done.map_err(job_error)
            })
            .await?;
        Ok(job)
    }

    /// Puts the VMs' storage on a pool zendiloOS made: its first location,
    /// which holds the image library, or one more.
    pub async fn set_up(
        &self,
        pool: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmPlugin, VmsError> {
        self.place_location(pool, "vms.storage", actor, context).await
    }

    /// The VMs' storage, which VMs and images need: the facts with the
    /// locations there are in their order, and the library's location.
    async fn storage(&self, context: &RequestContext) -> Result<(HostFacts, Location), VmsError> {
        let mut facts = self.ports.host.facts(context).await?;
        if !facts.installed {
            return Err(VmsError::NotInstalled);
        }
        let locations = self.ordered_locations(&facts).await?;
        let library = locations.first().cloned().ok_or(VmsError::NotSetUp)?;
        facts.limits.locations = locations;
        Ok((facts, library))
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

    /// The ids of the library's images a VM may be given.
    async fn ready_images(&self) -> Result<Vec<String>, VmsError> {
        Ok(self
            .ports
            .images
            .list()
            .await?
            .into_iter()
            .filter(|image| image.ready)
            .map(|image| image.id)
            .collect())
    }

    /// Records the VM and makes it in a job: its disks made and the domain
    /// defined. Should that fail, what was made goes again, the record too.
    pub async fn create(
        &self,
        new: NewVm,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(VmView, Job), VmsError> {
        let (facts, library) = self.storage(context).await?;
        let _library = self.library.lock().await;
        let existing = self.ports.repository.list().await?;
        let taken: Vec<&str> = existing.iter().map(|vm| vm.settings.name.as_str()).collect();
        let macs: BTreeSet<String> =
            existing.iter().flat_map(|vm| vm.settings.nics.iter().map(|nic| nic.mac.clone())).collect();
        let mut settings = new.settings;
        for nic in &mut settings.nics {
            nic.mac.clear();
        }
        settings.sort_devices();
        settings.place_disks(None, &facts.limits.locations)?;
        settings.give_macs(&[], &macs, || self.ports.ids.new_mac());
        let images = self.ready_images().await?;
        let images: Vec<&str> = images.iter().map(String::as_str).collect();
        let name = settings.check(&facts.limits, &taken, &images, None)?;
        let vm = Vm {
            id: self.new_id().await?,
            settings: VmSettings { name, ..settings },
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
                let done = this.make(&made, library.volume, &progress, &context_owned).await;
                match &done {
                    Ok(()) => {
                        if let Err(error) = this.ports.repository.set_ready(&made.id).await {
                            tracing::error!(%error, id = made.id, "vms: made, but cannot record it");
                        }
                    }
                    Err(_) => this.undo(&made, &context_owned).await,
                }
                this.audit("vms.create", outcome(&done), &made.settings.name, &who, &context_owned).await;
                done.map_err(job_error)
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
        library: u16,
        progress: &Progress,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        progress.step("defining", Some(0.1)).await;
        self.ports.host.define(vm, library, context).await
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

    /// Changes a VM. Its name, autostart and the images in its drives
    /// change whenever; everything else, its hardware, only while it is
    /// shut off. Removing a disk destroys it: `confirm` must then be its
    /// name, typed again. Its domain is defined anew from the settings.
    pub async fn update(
        &self,
        id: &str,
        change: VmChange,
        confirm: Option<&str>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmView, VmsError> {
        let vm = self.ready(id).await?;
        let _busy = self.take_busy(&vm.id, "changed")?;
        let (facts, library) = self.storage(context).await?;
        let _library = self.library.lock().await;
        let others: Vec<Vm> =
            self.ports.repository.list().await?.into_iter().filter(|other| other.id != vm.id).collect();
        let taken: Vec<&str> = others.iter().map(|other| other.settings.name.as_str()).collect();
        let macs: BTreeSet<String> =
            others.iter().flat_map(|other| other.settings.nics.iter().map(|nic| nic.mac.clone())).collect();
        let VmChange { name, cpus, memory_mib, firmware, disks, mut nics, drives, boot, autostart } = change;
        for nic in &mut nics {
            nic.mac.clear();
        }
        let mut settings = VmSettings {
            name,
            os: vm.settings.os,
            cpus,
            memory_mib,
            firmware,
            disks,
            nics,
            drives,
            boot,
            autostart,
        };
        settings.sort_devices();
        settings.place_disks(Some(&vm.settings), &facts.limits.locations)?;
        settings.give_macs(&vm.settings.nics, &macs, || self.ports.ids.new_mac());
        let images = self.ready_images().await?;
        let images: Vec<&str> = images.iter().map(String::as_str).collect();
        settings.name = settings.check(&facts.limits, &taken, &images, Some(&vm.settings))?;

        let hardware = settings.hardware_differs(&vm.settings);
        let removed = settings.removed_disks(&vm.settings);
        let media = settings.changed_media(&vm.settings);
        if hardware {
            let states = self.ports.host.states(context).await?;
            if states.get(&vm.id) != Some(&PowerState::ShutOff) {
                return Err(VmsError::Running);
            }
        }
        if !removed.is_empty()
            && check_name(confirm.unwrap_or_default()).ok().as_deref() != Some(vm.settings.name.as_str())
        {
            return Err(VmsError::ConfirmMismatch);
        }
        let changed = Vm { settings, ..vm.clone() };
        let done = async {
            if hardware {
                self.ports.host.update(&changed, library.volume, &removed, context).await?;
            } else {
                for drive in &media {
                    self.ports.host.change_media(&changed, library.volume, *drive, context).await?;
                }
                if changed.settings.autostart != vm.settings.autostart {
                    self.ports.host.set_autostart(&vm.id, changed.settings.autostart, context).await?;
                }
            }
            Ok::<_, HostError>(())
        }
        .await;
        self.audit("vms.change", outcome(&done), &changed.settings.name, actor, context).await;
        done?;
        if !self.ports.repository.update(&changed).await? {
            return Err(VmsError::NameTaken);
        }
        drop(_library);
        self.get(&vm.id, context).await
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
        self.audit(action.audit(), outcome(&done), &vm.settings.name, actor, context).await;
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
        let action = if autostart { "vms.autostart_on" } else { "vms.autostart_off" };
        self.audit(action, outcome(&done), &vm.settings.name, actor, context).await;
        done?;
        self.ports.repository.set_autostart(&vm.id, autostart).await?;
        self.get(id, context).await
    }

    /// Removes the VM with its disks; `confirm` must be its name, typed
    /// again. The images it had stay in the library.
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
        let _busy = self.take_busy(&vm.id, "deleted")?;
        let removed = self.ports.host.remove(&vm, context).await;
        self.audit("vms.delete", outcome(&removed), &vm.settings.name, actor, context).await;
        removed?;
        self.ports.repository.delete(&vm.id).await?;
        Ok(())
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
        self.audit("vms.console", outcome(&opened), &view.vm.settings.name, actor, context).await;
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
