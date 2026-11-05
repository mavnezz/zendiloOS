use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use zendiloos_identity::domain::Permission;
use zendiloos_storage::application::StorageError;
use zendiloos_storage::domain as storage;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::Client;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(disks))
        .routes(routes!(smart))
        .routes(routes!(arrays))
        .routes(routes!(volumes))
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::DiskNotFound => ApiError::NotFound,
            StorageError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Sata,
    Sas,
    Nvme,
    Usb,
    Virtio,
    Other,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Media {
    Rotational,
    SolidState,
}

/// What a disk is used for.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiskUsage {
    /// Holds the operating system.
    System,
    /// Belongs to the RAID array named in `array`.
    ArrayMember,
    /// Carries a data volume directly.
    Volume,
    /// Has partitions or data zendiloOS does not manage.
    Foreign,
    Unused,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Disk {
    /// Kernel name, e.g. `sda`; identifies the disk in this API.
    pub name: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    pub size_bytes: u64,
    pub transport: Transport,
    pub media: Media,
    pub removable: bool,
    pub read_only: bool,
    pub usage: DiskUsage,
    /// The array, if `usage` is `array_member`.
    pub array: Option<String>,
    pub partitions: Vec<Partition>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Partition {
    pub name: String,
    pub size_bytes: u64,
    pub filesystem: Option<String>,
    pub label: Option<String>,
    pub mount_points: Vec<String>,
}

impl From<storage::Disk> for Disk {
    fn from(disk: storage::Disk) -> Self {
        let (usage, array) = match disk.usage {
            storage::DiskUsage::System => (DiskUsage::System, None),
            storage::DiskUsage::ArrayMember { array } => (DiskUsage::ArrayMember, Some(array)),
            storage::DiskUsage::Volume => (DiskUsage::Volume, None),
            storage::DiskUsage::Foreign => (DiskUsage::Foreign, None),
            storage::DiskUsage::Unused => (DiskUsage::Unused, None),
        };
        Self {
            name: disk.name,
            model: disk.model,
            serial: disk.serial,
            wwn: disk.wwn,
            size_bytes: disk.size_bytes,
            transport: match disk.transport {
                storage::Transport::Sata => Transport::Sata,
                storage::Transport::Sas => Transport::Sas,
                storage::Transport::Nvme => Transport::Nvme,
                storage::Transport::Usb => Transport::Usb,
                storage::Transport::Virtio => Transport::Virtio,
                storage::Transport::Other => Transport::Other,
            },
            media: match disk.media {
                storage::Media::Rotational => Media::Rotational,
                storage::Media::SolidState => Media::SolidState,
            },
            removable: disk.removable,
            read_only: disk.read_only,
            usage,
            array,
            partitions: disk
                .partitions
                .into_iter()
                .map(|p| Partition {
                    name: p.name,
                    size_bytes: p.size_bytes,
                    filesystem: p.filesystem,
                    label: p.label,
                    mount_points: p.mount_points,
                })
                .collect(),
        }
    }
}

/// All disks of the host. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/disks",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Disk>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn disks(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<Disk>>, ApiError> {
    auth.require(Permission::StorageView)?;
    let disks = state.storage.list_disks.execute(&client).await?;
    Ok(Json(disks.into_iter().map(Disk::from).collect()))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmartStatus {
    /// The report below is complete.
    Available,
    /// The disk is spun down and was not woken to read it.
    Standby,
    /// The device has no SMART; `reason` says why.
    Unsupported,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmartProtocol {
    Ata,
    Nvme,
    Scsi,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiskHealth {
    Good,
    /// Signs of wear or past errors.
    Warning,
    /// The disk should be replaced.
    Failing,
}

impl From<storage::Health> for DiskHealth {
    fn from(health: storage::Health) -> Self {
        match health {
            storage::Health::Good => DiskHealth::Good,
            storage::Health::Warning => DiskHealth::Warning,
            storage::Health::Failing => DiskHealth::Failing,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingCode {
    OverallFailed,
    AttributeFailing,
    AttributeFailedInPast,
    ReallocatedSectors,
    PendingSectors,
    UncorrectableSectors,
    ErrorLogEntries,
    SelfTestFailed,
    NvmeCriticalWarning,
    NvmeSpareLow,
    NvmeWornOut,
    NvmeMediaErrors,
}

/// A reason behind the health verdict.
#[derive(Debug, Serialize, ToSchema)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: DiskHealth,
    /// The ATA attribute concerned, for `attribute_*`.
    pub attribute_id: Option<u8>,
    pub attribute_name: Option<String>,
    /// The count or percentage the finding is about.
    pub value: Option<u64>,
    /// The limit `value` is compared with, for `nvme_spare_low`.
    pub threshold: Option<u64>,
}

impl From<storage::Finding> for Finding {
    fn from(finding: storage::Finding) -> Self {
        use storage::Finding as F;
        let severity = finding.health().into();
        let (code, attribute, value, threshold) = match finding {
            F::OverallFailed => (FindingCode::OverallFailed, None, None, None),
            F::AttributeFailing { id, name } => (FindingCode::AttributeFailing, Some((id, name)), None, None),
            F::AttributeFailedInPast { id, name } => {
                (FindingCode::AttributeFailedInPast, Some((id, name)), None, None)
            }
            F::ReallocatedSectors(count) => (FindingCode::ReallocatedSectors, None, Some(count), None),
            F::PendingSectors(count) => (FindingCode::PendingSectors, None, Some(count), None),
            F::UncorrectableSectors(count) => (FindingCode::UncorrectableSectors, None, Some(count), None),
            F::ErrorLogEntries(count) => (FindingCode::ErrorLogEntries, None, Some(count), None),
            F::SelfTestFailed => (FindingCode::SelfTestFailed, None, None, None),
            F::NvmeCriticalWarning(bits) => (FindingCode::NvmeCriticalWarning, None, Some(bits.into()), None),
            F::NvmeSpareLow { available, threshold } => {
                (FindingCode::NvmeSpareLow, None, Some(available.into()), Some(threshold.into()))
            }
            F::NvmeWornOut(percent) => (FindingCode::NvmeWornOut, None, Some(percent.into()), None),
            F::NvmeMediaErrors(count) => (FindingCode::NvmeMediaErrors, None, Some(count), None),
        };
        let (attribute_id, attribute_name) = attribute.unzip();
        Self { code, severity, attribute_id, attribute_name, value, threshold }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u16,
    pub worst: u16,
    pub threshold: u16,
    pub raw: u64,
    /// The raw value as smartctl formats it.
    pub raw_text: String,
    pub prefailure: bool,
    /// `now`, `past` or absent.
    pub failed: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    pub available_spare_percent: u8,
    pub available_spare_threshold_percent: u8,
    pub percentage_used: u8,
    pub media_errors: u64,
    pub unsafe_shutdowns: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SelfTest {
    pub passed: bool,
    pub description: String,
    pub power_on_hours: Option<u64>,
}

/// SMART data of one disk with zendiloOS's assessment.
#[derive(Debug, Serialize, ToSchema)]
pub struct SmartReport {
    pub status: SmartStatus,
    /// Why there is no data, for `unsupported`.
    pub reason: Option<String>,
    /// Absent unless `status` is `available`.
    pub health: Option<DiskHealth>,
    pub findings: Vec<Finding>,
    pub protocol: Option<SmartProtocol>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    /// The drive's own verdict.
    pub passed: Option<bool>,
    pub temperature_celsius: Option<i64>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    /// ATA only.
    pub attributes: Vec<SmartAttribute>,
    /// NVMe only.
    pub nvme: Option<NvmeHealth>,
    pub error_log_count: Option<u64>,
    pub last_self_test: Option<SelfTest>,
}

impl SmartReport {
    fn without_data(status: SmartStatus, reason: Option<String>) -> Self {
        Self {
            status,
            reason,
            health: None,
            findings: Vec::new(),
            protocol: None,
            model: None,
            serial: None,
            firmware: None,
            passed: None,
            temperature_celsius: None,
            power_on_hours: None,
            power_cycles: None,
            attributes: Vec::new(),
            nvme: None,
            error_log_count: None,
            last_self_test: None,
        }
    }
}

/// NVMe data units are 1000 blocks of 512 bytes.
const NVME_DATA_UNIT: u64 = 512_000;

impl From<storage::SmartReport> for SmartReport {
    fn from(report: storage::SmartReport) -> Self {
        let data = match report {
            storage::SmartReport::Standby => return Self::without_data(SmartStatus::Standby, None),
            storage::SmartReport::Unsupported { reason } => {
                return Self::without_data(SmartStatus::Unsupported, Some(reason));
            }
            storage::SmartReport::Available(data) => *data,
        };
        let assessment = data.assess();
        Self {
            status: SmartStatus::Available,
            reason: None,
            health: Some(assessment.health.into()),
            findings: assessment.findings.into_iter().map(Finding::from).collect(),
            protocol: Some(match data.protocol {
                storage::SmartProtocol::Ata => SmartProtocol::Ata,
                storage::SmartProtocol::Nvme => SmartProtocol::Nvme,
                storage::SmartProtocol::Scsi => SmartProtocol::Scsi,
            }),
            model: data.model,
            serial: data.serial,
            firmware: data.firmware,
            passed: data.passed,
            temperature_celsius: data.temperature_celsius,
            power_on_hours: data.power_on_hours,
            power_cycles: data.power_cycles,
            attributes: data
                .attributes
                .into_iter()
                .map(|a| SmartAttribute {
                    id: a.id,
                    name: a.name,
                    value: a.value,
                    worst: a.worst,
                    threshold: a.threshold,
                    raw: a.raw,
                    raw_text: a.raw_text,
                    prefailure: a.prefailure,
                    failed: match a.failure {
                        storage::AttributeFailure::None => None,
                        storage::AttributeFailure::Now => Some("now".to_owned()),
                        storage::AttributeFailure::Past => Some("past".to_owned()),
                    },
                })
                .collect(),
            nvme: data.nvme.map(|n| NvmeHealth {
                critical_warning: n.critical_warning,
                available_spare_percent: n.available_spare,
                available_spare_threshold_percent: n.available_spare_threshold,
                percentage_used: n.percentage_used,
                media_errors: n.media_errors,
                unsafe_shutdowns: n.unsafe_shutdowns,
                bytes_read: n.data_units_read.saturating_mul(NVME_DATA_UNIT),
                bytes_written: n.data_units_written.saturating_mul(NVME_DATA_UNIT),
            }),
            error_log_count: data.error_log_count,
            last_self_test: data.last_self_test.map(|t| SelfTest {
                passed: t.passed,
                description: t.description,
                power_on_hours: t.power_on_hours,
            }),
        }
    }
}

/// SMART data of one disk. Sleeping disks are not woken: their report has
/// status `standby`. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/disks/{name}/smart",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path, description = "Kernel name of the disk, e.g. `sda`")),
    responses(
        (status = OK, body = SmartReport),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such disk"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn smart(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<Json<SmartReport>, ApiError> {
    auth.require(Permission::StorageView)?;
    let report = state.storage.get_smart_report.execute(&name, &client).await?;
    Ok(Json(report.into()))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrayHealth {
    Healthy,
    /// Redundancy is reduced and nothing is rebuilding it.
    Degraded,
    /// Redundancy is reduced and being rebuilt.
    Rebuilding,
    /// Not running; the data is unavailable.
    Inactive,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    Active,
    Spare,
    Faulty,
    Replacement,
    Journal,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArrayMember {
    /// Kernel name of the disk or partition.
    pub device: String,
    pub index: u32,
    pub state: MemberState,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperation {
    Resync,
    Recovery,
    Check,
    Repair,
    Reshape,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SyncProgress {
    pub operation: SyncOperation,
    /// 0 to 100; absent while waiting for another array.
    pub percent: Option<f32>,
    pub remaining_seconds: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
}

/// A software RAID array.
#[derive(Debug, Serialize, ToSchema)]
pub struct RaidArray {
    pub name: String,
    /// `raid1`, `raid5`, …; unknown while inactive.
    pub level: Option<String>,
    pub health: ArrayHealth,
    pub read_only: bool,
    pub size_bytes: Option<u64>,
    /// Devices the array is made for; absent without redundancy.
    pub devices_total: Option<u32>,
    pub devices_working: Option<u32>,
    pub members: Vec<ArrayMember>,
    pub sync: Option<SyncProgress>,
}

impl From<storage::Array> for RaidArray {
    fn from(array: storage::Array) -> Self {
        let health = match array.health() {
            storage::ArrayHealth::Healthy => ArrayHealth::Healthy,
            storage::ArrayHealth::Degraded => ArrayHealth::Degraded,
            storage::ArrayHealth::Rebuilding => ArrayHealth::Rebuilding,
            storage::ArrayHealth::Inactive => ArrayHealth::Inactive,
        };
        Self {
            name: array.name,
            level: array.level,
            health,
            read_only: array.read_only,
            size_bytes: array.size_bytes,
            devices_total: array.slots.map(|s| s.total),
            devices_working: array.slots.map(|s| s.working),
            members: array
                .members
                .into_iter()
                .map(|m| ArrayMember {
                    device: m.device,
                    index: m.index,
                    state: match m.state {
                        storage::MemberState::Active => MemberState::Active,
                        storage::MemberState::Spare => MemberState::Spare,
                        storage::MemberState::Faulty => MemberState::Faulty,
                        storage::MemberState::Replacement => MemberState::Replacement,
                        storage::MemberState::Journal => MemberState::Journal,
                    },
                })
                .collect(),
            sync: array.sync.map(|s| SyncProgress {
                operation: match s.operation {
                    storage::SyncOperation::Resync => SyncOperation::Resync,
                    storage::SyncOperation::Recovery => SyncOperation::Recovery,
                    storage::SyncOperation::Check => SyncOperation::Check,
                    storage::SyncOperation::Repair => SyncOperation::Repair,
                    storage::SyncOperation::Reshape => SyncOperation::Reshape,
                },
                percent: s.percent,
                remaining_seconds: s.remaining_seconds,
                speed_bytes_per_second: s.speed_bytes_per_second,
            }),
        }
    }
}

/// The software RAID arrays. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/arrays",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<RaidArray>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn arrays(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<RaidArray>>, ApiError> {
    auth.require(Permission::StorageView)?;
    let arrays = state.storage.list_arrays.execute(&client).await?;
    Ok(Json(arrays.into_iter().map(RaidArray::from).collect()))
}

/// A data filesystem.
#[derive(Debug, Serialize, ToSchema)]
pub struct Volume {
    /// Filesystem UUID, or the device name if there is none.
    pub id: String,
    pub label: Option<String>,
    /// `btrfs`, `ext4` or `xfs`.
    pub filesystem: String,
    /// Block devices carrying it.
    pub devices: Vec<String>,
    /// The RAID array it lives on, if any.
    pub array: Option<String>,
    /// The disks underneath.
    pub disks: Vec<String>,
    pub mount_points: Vec<String>,
    /// Only known while mounted.
    pub size_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

impl From<storage::Volume> for Volume {
    fn from(volume: storage::Volume) -> Self {
        Self {
            id: volume.id,
            label: volume.label,
            filesystem: volume.filesystem,
            devices: volume.devices,
            array: volume.array,
            disks: volume.disks,
            mount_points: volume.mount_points,
            size_bytes: volume.space.map(|s| s.size_bytes),
            used_bytes: volume.space.map(|s| s.used_bytes),
            available_bytes: volume.space.map(|s| s.available_bytes),
        }
    }
}

/// The data volumes. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/volumes",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Volume>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn volumes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<Volume>>, ApiError> {
    auth.require(Permission::StorageView)?;
    let volumes = state.storage.list_volumes.execute(&client).await?;
    Ok(Json(volumes.into_iter().map(Volume::from).collect()))
}
