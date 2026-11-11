//! Virtual machines (`vms.manage`, administrators): the App Center's plugin
//! with its hardware check and storage, the VMs with their live state,
//! what they are asked to do and how they change, and the image library
//! (`vm_images`). Their consoles have a WebSocket of their own
//! (`vm_console`), their load a topic of the live WebSocket (`live`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_vms::application::{self as vms, VmsError};
use zendiloos_vms::domain::{self as domain, VmAction, VmSettings};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(vm_plugin))
        .routes(routes!(install_vm_plugin))
        .routes(routes!(set_up_vm_storage))
        .routes(routes!(list_vms, create_vm))
        .routes(routes!(get_vm, update_vm))
        .routes(routes!(start_vm))
        .routes(routes!(shut_down_vm))
        .routes(routes!(force_off_vm))
        .routes(routes!(restart_vm))
        .routes(routes!(set_vm_autostart))
        .routes(routes!(delete_vm))
        .merge(super::vm_images::routes())
}

impl From<VmsError> for ApiError {
    fn from(error: VmsError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            VmsError::NotFound | VmsError::PoolNotFound => ApiError::NotFound,
            VmsError::Invalid(e) => ApiError::InvalidInput(e.0),
            VmsError::NoKvm => rejected(StatusCode::CONFLICT, "kvm_missing", None),
            VmsError::NotInstalled => rejected(StatusCode::CONFLICT, "vms_not_installed", None),
            VmsError::Installed => rejected(StatusCode::CONFLICT, "vms_installed", None),
            VmsError::NotSetUp => rejected(StatusCode::CONFLICT, "vms_not_set_up", None),
            VmsError::NameTaken => ApiError::NameTaken,
            VmsError::Creating => rejected(StatusCode::CONFLICT, "vm_creating", None),
            VmsError::ConfirmMismatch => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch", None)
            }
            VmsError::IsoDenied => rejected(StatusCode::UNPROCESSABLE_ENTITY, "iso_denied", None),
            VmsError::NotRunning => rejected(StatusCode::CONFLICT, "vm_not_running", None),
            VmsError::Running => rejected(StatusCode::CONFLICT, "vm_running", None),
            VmsError::ImageInUse(names) => {
                rejected(StatusCode::CONFLICT, "image_in_use", Some(names.join(", ")))
            }
            VmsError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            VmsError::HostFailed(detail) => rejected(StatusCode::BAD_GATEWAY, "agent_failed", Some(detail)),
            VmsError::Port(e) => ApiError::internal(e),
        }
    }
}

/// What the machine's CPU and firmware offer.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmHardware {
    /// `/dev/kvm` is there: the kernel can run VMs with the CPU's help.
    pub kvm: bool,
    /// `vmx` (Intel VT-x) or `svm` (AMD-V); none if the CPU has neither,
    /// or it is off in the firmware.
    pub cpu_virtualization: Option<String>,
    /// An IOMMU is on (VT-d, AMD-Vi): devices could be handed to a VM
    /// later.
    pub iommu: bool,
}

/// A pool zendiloOS made, by its name and volume.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmStoragePlace {
    pub pool: String,
    pub volume: u16,
}

fn place(place: vms::StoragePlace) -> VmStoragePlace {
    VmStoragePlace { pool: place.pool, volume: place.volume }
}

/// The plugin: the hardware check, whether libvirt and QEMU are installed,
/// where the VMs' storage is, and what a VM may have.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmPlugin {
    pub hardware: VmHardware,
    pub installed: bool,
    pub storage: Option<VmStoragePlace>,
    /// Where the storage can go.
    pub pools: Vec<VmStoragePlace>,
    /// Threads and memory of the machine.
    pub cores: u32,
    pub memory_bytes: u64,
    /// Linux bridges of the machine's own, for VMs on the LAN.
    pub bridges: Vec<String>,
    /// The largest image a browser uploads into the library.
    pub image_upload_max_bytes: u64,
}

fn plugin_dto(plugin: vms::VmPlugin) -> VmPlugin {
    let facts = plugin.facts;
    VmPlugin {
        hardware: VmHardware {
            kvm: facts.kvm,
            cpu_virtualization: facts.cpu_virtualization,
            iommu: facts.iommu,
        },
        installed: facts.installed,
        storage: facts.storage.map(place),
        pools: plugin.pools.into_iter().map(place).collect(),
        cores: facts.limits.cores,
        memory_bytes: facts.limits.memory_bytes,
        bridges: facts.limits.bridges,
        image_upload_max_bytes: domain::IMAGE_UPLOAD_MAX,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmOs {
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmFirmware {
    /// OVMF, Secure Boot off.
    Uefi,
    Bios,
}

/// How a disk is attached: virtio (Linux), SATA (systems without virtio
/// drivers).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmDiskBus {
    Virtio,
    Sata,
}

/// The network card a VM sees: virtio, or an e1000e every system knows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmNicModel {
    Virtio,
    E1000e,
}

/// How a network card reaches the network.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmNetwork {
    /// passt: out to the LAN and the internet from the NAS's address;
    /// nothing reaches the VM.
    User,
    /// A bridge of the machine's: the VM is on the LAN as a machine of its own.
    Bridge { bridge: String },
}

/// A disk: a sparse zvol in its slot, 0 to 7.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmDisk {
    pub slot: u8,
    pub size_gib: u32,
    pub bus: VmDiskBus,
}

/// A network card as asked for, in its slot, 0 to 3.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmNicChoice {
    pub slot: u8,
    pub model: VmNicModel,
    pub network: VmNetwork,
}

/// A network card, with the MAC address zendiloOS gave it.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmNic {
    pub slot: u8,
    pub model: VmNicModel,
    pub network: VmNetwork,
    pub mac: String,
}

/// A CD drive in its slot, 0 to 2, with an image of the library or empty.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmDrive {
    pub slot: u8,
    /// An image's id; none: empty.
    pub image: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmBootKind {
    Disk,
    Drive,
}

/// A device a VM boots from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmBootDevice {
    pub kind: VmBootKind,
    pub slot: u8,
}

/// What libvirt says a VM does.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmPowerState {
    Running,
    Paused,
    ShuttingDown,
    ShutOff,
    Crashed,
    Suspended,
    /// Being made: its disks made, its domain defined.
    Creating,
    Unknown,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VirtualMachine {
    /// Twelve hexadecimal digits.
    pub id: String,
    pub name: String,
    pub os: VmOs,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: VmFirmware,
    pub disks: Vec<VmDisk>,
    pub nics: Vec<VmNic>,
    pub drives: Vec<VmDrive>,
    /// The devices it boots from, first first.
    pub boot: Vec<VmBootDevice>,
    pub autostart: bool,
    pub pool: String,
    pub volume: u16,
    pub created_at: i64,
    pub state: VmPowerState,
}

fn os_dto(os: domain::OsPreset) -> VmOs {
    match os {
        domain::OsPreset::Linux => VmOs::Linux,
        domain::OsPreset::Windows => VmOs::Windows,
        domain::OsPreset::Other => VmOs::Other,
    }
}

fn state_dto(state: domain::PowerState) -> VmPowerState {
    use domain::PowerState as State;
    match state {
        State::Running => VmPowerState::Running,
        State::Paused => VmPowerState::Paused,
        State::ShuttingDown => VmPowerState::ShuttingDown,
        State::ShutOff => VmPowerState::ShutOff,
        State::Crashed => VmPowerState::Crashed,
        State::Suspended => VmPowerState::Suspended,
        State::Creating => VmPowerState::Creating,
        State::Unknown => VmPowerState::Unknown,
    }
}

fn network_dto(network: domain::Network) -> VmNetwork {
    match network {
        domain::Network::User => VmNetwork::User,
        domain::Network::Bridge(bridge) => VmNetwork::Bridge { bridge },
    }
}

fn vm_dto(view: vms::VmView) -> VirtualMachine {
    let vm = view.vm;
    let settings = vm.settings;
    VirtualMachine {
        id: vm.id,
        name: settings.name,
        os: os_dto(settings.os),
        cpus: settings.cpus,
        memory_mib: settings.memory_mib,
        firmware: match settings.firmware {
            domain::Firmware::Uefi => VmFirmware::Uefi,
            domain::Firmware::Bios => VmFirmware::Bios,
        },
        disks: settings
            .disks
            .into_iter()
            .map(|disk| VmDisk {
                slot: disk.slot,
                size_gib: disk.size_gib,
                bus: match disk.bus {
                    domain::DiskBus::Virtio => VmDiskBus::Virtio,
                    domain::DiskBus::Sata => VmDiskBus::Sata,
                },
            })
            .collect(),
        nics: settings
            .nics
            .into_iter()
            .map(|nic| VmNic {
                slot: nic.slot,
                model: match nic.model {
                    domain::NicModel::Virtio => VmNicModel::Virtio,
                    domain::NicModel::E1000e => VmNicModel::E1000e,
                },
                network: network_dto(nic.network),
                mac: nic.mac,
            })
            .collect(),
        drives: settings
            .drives
            .into_iter()
            .map(|drive| VmDrive { slot: drive.slot, image: drive.image })
            .collect(),
        boot: settings
            .boot
            .into_iter()
            .map(|device| match device {
                domain::BootDevice::Disk(slot) => VmBootDevice { kind: VmBootKind::Disk, slot },
                domain::BootDevice::Drive(slot) => VmBootDevice { kind: VmBootKind::Drive, slot },
            })
            .collect(),
        autostart: settings.autostart,
        pool: vm.pool,
        volume: vm.volume,
        created_at: vm.created_at.unix_seconds(),
        state: state_dto(view.state),
    }
}

fn disks(disks: Vec<VmDisk>) -> Vec<domain::Disk> {
    disks
        .into_iter()
        .map(|disk| domain::Disk {
            slot: disk.slot,
            size_gib: disk.size_gib,
            bus: match disk.bus {
                VmDiskBus::Virtio => domain::DiskBus::Virtio,
                VmDiskBus::Sata => domain::DiskBus::Sata,
            },
        })
        .collect()
}

/// The cards asked for; their MAC addresses are zendiloOS's to give.
fn nics(nics: Vec<VmNicChoice>) -> Vec<domain::Nic> {
    nics.into_iter()
        .map(|nic| domain::Nic {
            slot: nic.slot,
            model: match nic.model {
                VmNicModel::Virtio => domain::NicModel::Virtio,
                VmNicModel::E1000e => domain::NicModel::E1000e,
            },
            network: match nic.network {
                VmNetwork::User => domain::Network::User,
                VmNetwork::Bridge { bridge } => domain::Network::Bridge(bridge),
            },
            mac: String::new(),
        })
        .collect()
}

fn drives(drives: Vec<VmDrive>) -> Vec<domain::Drive> {
    drives.into_iter().map(|drive| domain::Drive { slot: drive.slot, image: drive.image }).collect()
}

fn boot(boot: Vec<VmBootDevice>) -> Vec<domain::BootDevice> {
    boot.into_iter()
        .map(|device| match device.kind {
            VmBootKind::Disk => domain::BootDevice::Disk(device.slot),
            VmBootKind::Drive => domain::BootDevice::Drive(device.slot),
        })
        .collect()
}

fn firmware(firmware: VmFirmware) -> domain::Firmware {
    match firmware {
        VmFirmware::Uefi => domain::Firmware::Uefi,
        VmFirmware::Bios => domain::Firmware::Bios,
    }
}

/// The plugin: hardware, installation and storage. Needs `vms.manage`.
#[utoipa::path(get, path = "/vms/plugin", tag = "vms", security(("session" = [])), responses((status = OK, body = VmPlugin)))]
async fn vm_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<VmPlugin>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(plugin_dto(state.vms.plugin(&client).await?)))
}

/// Installs libvirt and QEMU from Debian (without libvirt's own network),
/// as a job. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/plugin/install",
    tag = "vms",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`kvm_missing`: the machine cannot run VMs; `vms_installed`"),
    )
)]
async fn install_vm_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let job = state.vms.install(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmStorageRequest {
    /// A pool zendiloOS made.
    pub pool: String,
}

/// Puts the VMs' storage on a pool: `<pool>/zendiloos.vms`, mounted at
/// `/volume<N>/@vms`. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/plugin/storage",
    tag = "vms",
    security(("session" = [])),
    request_body = VmStorageRequest,
    responses(
        (status = OK, body = VmPlugin),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such pool"),
        (status = CONFLICT, body = Problem, description = "`vms_not_installed`; `refused`: the storage is on another pool"),
    )
)]
async fn set_up_vm_storage(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<VmStorageRequest>,
) -> Result<Json<VmPlugin>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(plugin_dto(state.vms.set_up(&request.pool, &auth.0.actor(), &client).await?)))
}

/// Every VM with what libvirt says it does. Needs `vms.manage`.
#[utoipa::path(get, path = "/vms", tag = "vms", security(("session" = [])), responses((status = OK, body = [VirtualMachine])))]
async fn list_vms(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<VirtualMachine>>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(state.vms.list(&client).await?.into_iter().map(vm_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateVmRequest {
    pub name: String,
    pub os: VmOs,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: VmFirmware,
    /// 1 to 8.
    pub disks: Vec<VmDisk>,
    /// Up to 4.
    pub nics: Vec<VmNicChoice>,
    /// Up to 3, with images of the library or empty.
    pub drives: Vec<VmDrive>,
    /// The devices it boots from, first first.
    pub boot: Vec<VmBootDevice>,
    #[serde(default)]
    pub autostart: bool,
}

/// A VM recorded, and the job that makes it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedVm {
    pub vm: VirtualMachine,
    pub job: Job,
}

/// Makes a VM, as a job: its sparse disks made, the domain defined, the
/// library's images in its drives. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms",
    tag = "vms",
    security(("session" = [])),
    request_body = CreateVmRequest,
    responses(
        (status = ACCEPTED, body = CreatedVm),
        (status = CONFLICT, body = Problem, description = "`vms_not_installed`, `vms_not_set_up`, `name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: more than the machine has, an unknown bridge or image, devices that break the rules"),
    )
)]
async fn create_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<CreateVmRequest>,
) -> Result<(StatusCode, Json<CreatedVm>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let settings = VmSettings {
        name: request.name,
        os: match request.os {
            VmOs::Linux => domain::OsPreset::Linux,
            VmOs::Windows => domain::OsPreset::Windows,
            VmOs::Other => domain::OsPreset::Other,
        },
        cpus: request.cpus,
        memory_mib: request.memory_mib,
        firmware: firmware(request.firmware),
        disks: disks(request.disks),
        nics: nics(request.nics),
        drives: drives(request.drives),
        boot: boot(request.boot),
        autostart: request.autostart,
    };
    let (view, job) = state.vms.create(vms::NewVm { settings }, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(CreatedVm { vm: vm_dto(view), job: job.into() })))
}

/// One VM. Needs `vms.manage`.
#[utoipa::path(
    get,
    path = "/vms/{id}",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses((status = OK, body = VirtualMachine), (status = NOT_FOUND, body = Problem, description = "`not_found`"))
)]
async fn get_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VirtualMachine>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(vm_dto(state.vms.get(&id, &client).await?)))
}

/// A VM's settings as they are to be: all of them, its system aside.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateVmRequest {
    pub name: String,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: VmFirmware,
    /// A disk of a slot it had keeps its data and only grows; one left out
    /// is destroyed.
    pub disks: Vec<VmDisk>,
    /// A card of a slot it had keeps its MAC address.
    pub nics: Vec<VmNicChoice>,
    pub drives: Vec<VmDrive>,
    pub boot: Vec<VmBootDevice>,
    pub autostart: bool,
    /// The VM's name, typed again: needed when a disk is removed.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// Changes a VM, its domain defined anew from its settings. Its name,
/// autostart and the images in its drives change while it runs (a medium
/// at once); anything else only while it is shut off. Needs `vms.manage`.
#[utoipa::path(
    put,
    path = "/vms/{id}",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = UpdateVmRequest,
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_running`: hardware changes while it is not shut off; `vm_creating`; `name_taken`; `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a disk shrunk, more than the machine has; `confirmation_mismatch`: a disk removed without the name"),
    )
)]
async fn update_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<UpdateVmRequest>,
) -> Result<Json<VirtualMachine>, ApiError> {
    auth.require(Permission::VmsManage)?;
    let change = vms::VmChange {
        name: request.name,
        cpus: request.cpus,
        memory_mib: request.memory_mib,
        firmware: firmware(request.firmware),
        disks: disks(request.disks),
        nics: nics(request.nics),
        drives: drives(request.drives),
        boot: boot(request.boot),
        autostart: request.autostart,
    };
    let view = state.vms.update(&id, change, request.confirm.as_deref(), &auth.0.actor(), &client).await?;
    Ok(Json(vm_dto(view)))
}

async fn control(
    auth: Auth,
    state: ApiState,
    client: zendiloos_kernel::RequestContext,
    id: String,
    action: VmAction,
) -> Result<Json<VirtualMachine>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(vm_dto(state.vms.control(&id, action, &auth.0.actor(), &client).await?)))
}

/// Starts the VM. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/start",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`: libvirt's reason, such as it runs already"),
    )
)]
async fn start_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VirtualMachine>, ApiError> {
    control(auth, state, client, id, VmAction::Start).await
}

/// Asks the guest to shut down (ACPI). Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/shutdown",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`: libvirt's reason"),
    )
)]
async fn shut_down_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VirtualMachine>, ApiError> {
    control(auth, state, client, id, VmAction::Shutdown).await
}

/// Powers the VM off at once. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/force-off",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`: libvirt's reason"),
    )
)]
async fn force_off_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VirtualMachine>, ApiError> {
    control(auth, state, client, id, VmAction::ForceOff).await
}

/// Asks the guest to restart (ACPI). Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/restart",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`: libvirt's reason"),
    )
)]
async fn restart_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VirtualMachine>, ApiError> {
    control(auth, state, client, id, VmAction::Restart).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmAutostartRequest {
    pub autostart: bool,
}

/// Whether the VM starts with the machine. Needs `vms.manage`.
#[utoipa::path(
    put,
    path = "/vms/{id}/autostart",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = VmAutostartRequest,
    responses(
        (status = OK, body = VirtualMachine),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`"),
    )
)]
async fn set_vm_autostart(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<VmAutostartRequest>,
) -> Result<Json<VirtualMachine>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(vm_dto(state.vms.set_autostart(&id, request.autostart, &auth.0.actor(), &client).await?)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteVmRequest {
    /// The VM's name, typed again.
    pub confirm: String,
}

/// Removes the VM: powered off, undefined, its disks destroyed; the images
/// it had stay in the library. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/delete",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = DeleteVmRequest,
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn delete_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<DeleteVmRequest>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.delete(&id, &request.confirm, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
