//! Virtual machines (`vms.manage`, administrators): the App Center's plugin
//! with its hardware check and storage, the VMs with their live state, and
//! what they are asked to do. Their consoles have a WebSocket of their own
//! (`vm_console`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_vms::application::{self as vms, VmsError};
use zendiloos_vms::domain::{self as domain, IsoChoice, VmAction, VmSettings};

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
        .routes(routes!(get_vm))
        .routes(routes!(start_vm))
        .routes(routes!(shut_down_vm))
        .routes(routes!(force_off_vm))
        .routes(routes!(restart_vm))
        .routes(routes!(set_vm_autostart))
        .routes(routes!(delete_vm))
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

/// How a VM reaches the network.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmNetwork {
    /// passt: out to the LAN and the internet from the NAS's address;
    /// nothing reaches the VM.
    User,
    /// A bridge of the machine's: the VM is on the LAN as a machine of its own.
    Bridge { bridge: String },
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
    /// Being made: its ISO copied, its disk made, its domain defined.
    Creating,
    Unknown,
}

/// Where a VM's ISO was copied from.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmIsoOrigin {
    pub share: String,
    pub path: String,
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
    pub disk_gib: u32,
    pub network: VmNetwork,
    pub autostart: bool,
    pub iso: Option<VmIsoOrigin>,
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
        disk_gib: settings.disk_gib,
        network: match settings.network {
            domain::Network::User => VmNetwork::User,
            domain::Network::Bridge(bridge) => VmNetwork::Bridge { bridge },
        },
        autostart: settings.autostart,
        iso: vm.iso.map(|iso| VmIsoOrigin { share: iso.share, path: iso.path }),
        pool: vm.pool,
        volume: vm.volume,
        created_at: vm.created_at.unix_seconds(),
        state: state_dto(view.state),
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

/// An ISO in a share, picked as the VM's installation medium.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmIsoChoice {
    pub share_id: i64,
    /// In the share: `isos/debian-13.1.0-amd64-netinst.iso`.
    pub path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateVmRequest {
    pub name: String,
    pub os: VmOs,
    pub cpus: u16,
    pub memory_mib: u32,
    pub firmware: VmFirmware,
    pub disk_gib: u32,
    pub network: VmNetwork,
    #[serde(default)]
    pub autostart: bool,
    /// Copied to the VMs' storage and attached read-only; none: it boots
    /// from its disk, or into the firmware.
    #[serde(default)]
    pub iso: Option<VmIsoChoice>,
}

/// A VM recorded, and the job that makes it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedVm {
    pub vm: VirtualMachine,
    pub job: Job,
}

/// Makes a VM, as a job: the ISO copied from its share (which the
/// administrator must be able to read), a sparse disk, the domain defined.
/// Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms",
    tag = "vms",
    security(("session" = [])),
    request_body = CreateVmRequest,
    responses(
        (status = ACCEPTED, body = CreatedVm),
        (status = CONFLICT, body = Problem, description = "`vms_not_installed`, `vms_not_set_up`, `name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: more than the machine has, an unknown bridge, no .iso; `iso_denied`"),
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
        firmware: match request.firmware {
            VmFirmware::Uefi => domain::Firmware::Uefi,
            VmFirmware::Bios => domain::Firmware::Bios,
        },
        disk_gib: request.disk_gib,
        network: match request.network {
            VmNetwork::User => domain::Network::User,
            VmNetwork::Bridge { bridge } => domain::Network::Bridge(bridge),
        },
        autostart: request.autostart,
    };
    let iso = request.iso.map(|iso| IsoChoice { share_id: iso.share_id, path: iso.path });
    let (view, job) = state.vms.create(vms::NewVm { settings, iso }, &auth.0.actor(), &client).await?;
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

/// Removes the VM: powered off, undefined, its disk and its ISO's copy
/// destroyed. Needs `vms.manage`.
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
