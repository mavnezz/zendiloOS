use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use zendiloos_identity::domain::Permission;
use zendiloos_storage::application::StorageError;
use zendiloos_storage::domain as storage;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(overview))
        .routes(routes!(disks))
        .routes(routes!(smart))
        .routes(routes!(arrays))
        .routes(routes!(pools, create_pool))
        .routes(routes!(plan_pool))
        .routes(routes!(destroy_pool))
        .routes(routes!(plan_replace))
        .routes(routes!(replace_disk))
        .routes(routes!(scrub_pool))
        .routes(routes!(stop_scrub))
        .routes(routes!(volumes))
}

fn rejected(status: StatusCode, code: &'static str, detail: Option<String>) -> ApiError {
    ApiError::Rejected { status, code, detail }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        use storage::PlanError as P;
        let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
        match error {
            StorageError::DiskNotFound | StorageError::PoolNotFound | StorageError::Plan(P::PoolNotFound) => {
                ApiError::NotFound
            }
            StorageError::Plan(P::InvalidName(e)) => ApiError::InvalidInput(e.to_string()),
            StorageError::Plan(P::NameTaken) | StorageError::Conflict => {
                rejected(StatusCode::CONFLICT, "name_taken", None)
            }
            StorageError::Plan(e @ P::DiskCount { .. }) => {
                rejected(unprocessable, "disk_count", Some(e.to_string()))
            }
            StorageError::Plan(P::DiskNotFound(disk)) => {
                rejected(unprocessable, "disk_not_found", Some(disk))
            }
            StorageError::Plan(P::DiskUnusable { disk, reason }) => {
                let reason = match reason {
                    storage::Unusable::System => "system",
                    storage::Unusable::ArrayMember => "array_member",
                    storage::Unusable::PoolMember => "pool_member",
                    storage::Unusable::NoStableId => "no_stable_id",
                    storage::Unusable::ReadOnly => "read_only",
                };
                rejected(unprocessable, "disk_unusable", Some(format!("{disk}: {reason}")))
            }
            StorageError::Plan(P::DuplicateDisk(disk)) => {
                rejected(unprocessable, "duplicate_disk", Some(disk))
            }
            StorageError::Plan(P::NoVolumeNumber) => rejected(StatusCode::CONFLICT, "no_volume_number", None),
            StorageError::Plan(P::UnsupportedLayout) => rejected(unprocessable, "unsupported_layout", None),
            StorageError::Plan(P::PoolNotManaged) => rejected(StatusCode::CONFLICT, "pool_not_managed", None),
            StorageError::Plan(P::DeviceNotFound) => rejected(unprocessable, "device_not_found", None),
            StorageError::Plan(P::DiskTooSmall) => rejected(unprocessable, "disk_too_small", None),
            StorageError::PlanExpired => rejected(StatusCode::CONFLICT, "plan_expired", None),
            StorageError::ConfirmationMismatch => rejected(unprocessable, "confirmation_mismatch", None),
            StorageError::PlanChanged => rejected(StatusCode::CONFLICT, "plan_changed", None),
            StorageError::Refused(reason) => rejected(StatusCode::CONFLICT, "refused", Some(reason)),
            StorageError::NotScrubbing => rejected(StatusCode::CONFLICT, "not_scrubbing", None),
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
    /// Holds a device of the ZFS pool named in `pool`.
    PoolMember,
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
    /// The pool, if `usage` is `pool_member`.
    pub pool: Option<String>,
    /// Name of the disk's `/dev/disk/by-id` link; disks without one cannot
    /// be used for pools.
    pub id_link: Option<String>,
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
        let (usage, array, pool) = match disk.usage {
            storage::DiskUsage::System => (DiskUsage::System, None, None),
            storage::DiskUsage::ArrayMember { array } => (DiskUsage::ArrayMember, Some(array), None),
            storage::DiskUsage::PoolMember { pool } => (DiskUsage::PoolMember, None, Some(pool)),
            storage::DiskUsage::Volume => (DiskUsage::Volume, None, None),
            storage::DiskUsage::Foreign => (DiskUsage::Foreign, None, None),
            storage::DiskUsage::Unused => (DiskUsage::Unused, None, None),
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
            pool,
            id_link: disk.id_link,
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
    /// `zfs`, `btrfs`, `ext4` or `xfs`.
    pub filesystem: String,
    /// Block devices carrying it.
    pub devices: Vec<String>,
    /// The RAID array it lives on, if any.
    pub array: Option<String>,
    /// The ZFS pool it is, if any.
    pub pool: Option<String>,
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
            pool: volume.pool,
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

/// State of a pool or one of its devices.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VdevState {
    Online,
    /// Working with less redundancy than intended.
    Degraded,
    Faulted,
    Offline,
    /// Gone from the system.
    Removed,
    Unavailable,
    /// All I/O stopped after failures.
    Suspended,
    Unknown,
}

impl From<storage::VdevState> for VdevState {
    fn from(state: storage::VdevState) -> Self {
        match state {
            storage::VdevState::Online => VdevState::Online,
            storage::VdevState::Degraded => VdevState::Degraded,
            storage::VdevState::Faulted => VdevState::Faulted,
            storage::VdevState::Offline => VdevState::Offline,
            storage::VdevState::Removed => VdevState::Removed,
            storage::VdevState::Unavailable => VdevState::Unavailable,
            storage::VdevState::Suspended => VdevState::Suspended,
            storage::VdevState::Unknown => VdevState::Unknown,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolLayout {
    Single,
    Mirror,
    Raidz,
    /// Not a layout zendiloOS creates; see `layout_name`.
    Other,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplacingRole {
    /// Being replaced.
    Old,
    /// Resilvering in as the replacement.
    New,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PoolDevice {
    /// As ZFS shows it, e.g. `ata-WDC_WD40EFRX_WD-1234-part1`.
    pub name: String,
    /// Identifies the device even after it disappeared.
    pub guid: String,
    /// Kernel name of its disk, if present.
    pub disk: Option<String>,
    pub state: VdevState,
    pub read_errors: u64,
    pub write_errors: u64,
    pub checksum_errors: u64,
    pub replacing: Option<ReplacingRole>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    /// Verifies all data, repairing from redundancy.
    Scrub,
    /// Rebuilds redundancy onto a new device.
    Resilver,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Running,
    Finished,
    Canceled,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PoolScan {
    pub kind: ScanKind,
    pub state: ScanState,
    /// 0 to 100.
    pub percent: f32,
    /// Unix seconds.
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub errors: u64,
    /// While it runs: how fast the current pass goes, as `zpool status`
    /// estimates it.
    pub bytes_per_second: Option<u64>,
    /// While it runs: the estimated time until it is done.
    pub seconds_left: Option<u64>,
    /// A scrub that was paused.
    pub paused: bool,
}

/// A ZFS pool; for pools zendiloOS created, also a volume.
#[derive(Debug, Serialize, ToSchema)]
pub struct Pool {
    pub name: String,
    pub guid: String,
    pub state: VdevState,
    pub layout: PoolLayout,
    /// For `raidz`.
    pub parity: Option<u8>,
    /// For `other`, e.g. `stripe`.
    pub layout_name: Option<String>,
    /// Disks that may fail without losing data.
    pub redundancy: usize,
    /// Raw capacity and allocation, including redundancy.
    pub size_bytes: u64,
    pub allocated_bytes: u64,
    /// Space as files see it.
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    /// The `N` of `/volume<N>`; only for pools zendiloOS created.
    pub volume: Option<u16>,
    /// Where it is mounted, if it is.
    pub mount_point: Option<String>,
    pub devices: Vec<PoolDevice>,
    pub scan: Option<PoolScan>,
    /// Files with permanent errors.
    pub data_errors: u64,
}

impl Pool {
    /// `pool` as read at `read_at`, which running scans are estimated from.
    fn new(pool: storage::Pool, read_at: zendiloos_kernel::Timestamp) -> Self {
        let disks = pool.devices.iter().filter(|d| d.replacing != Some(storage::Replacing::New)).count();
        let redundancy = pool.layout.redundancy(disks);
        let (layout, parity, layout_name) = match pool.layout {
            storage::PoolLayout::Single => (PoolLayout::Single, None, None),
            storage::PoolLayout::Mirror => (PoolLayout::Mirror, None, None),
            storage::PoolLayout::Raidz { parity } => (PoolLayout::Raidz, Some(parity), None),
            storage::PoolLayout::Other(name) => (PoolLayout::Other, None, Some(name)),
        };
        let mounted = pool.space.is_some_and(|space| space.mounted);
        Self {
            name: pool.name,
            guid: pool.guid,
            state: pool.state.into(),
            layout,
            parity,
            layout_name,
            redundancy,
            size_bytes: pool.size_bytes,
            allocated_bytes: pool.allocated_bytes,
            used_bytes: pool.space.map(|s| s.used_bytes),
            available_bytes: pool.space.map(|s| s.available_bytes),
            volume: pool.volume.map(storage::VolumeNumber::get),
            mount_point: pool.volume.filter(|_| mounted).map(storage::VolumeNumber::mount_point),
            devices: pool
                .devices
                .into_iter()
                .map(|d| PoolDevice {
                    name: d.name,
                    guid: d.guid,
                    disk: d.disk,
                    state: d.state.into(),
                    read_errors: d.read_errors,
                    write_errors: d.write_errors,
                    checksum_errors: d.checksum_errors,
                    replacing: d.replacing.map(|role| match role {
                        storage::Replacing::Old => ReplacingRole::Old,
                        storage::Replacing::New => ReplacingRole::New,
                    }),
                })
                .collect(),
            scan: pool.scan.map(|scan| PoolScan {
                bytes_per_second: scan.estimate(read_at).map(|e| e.bytes_per_second),
                seconds_left: scan.estimate(read_at).map(|e| e.seconds_left),
                paused: scan.pass.is_some_and(|pass| pass.paused),
                percent: scan.percent(),
                kind: match scan.kind {
                    storage::ScanKind::Scrub => ScanKind::Scrub,
                    storage::ScanKind::Resilver => ScanKind::Resilver,
                },
                state: match scan.state {
                    storage::ScanState::Running => ScanState::Running,
                    storage::ScanState::Finished => ScanState::Finished,
                    storage::ScanState::Canceled => ScanState::Canceled,
                },
                started_at: scan.started_at.unix_seconds(),
                finished_at: scan.finished_at.map(|t| t.unix_seconds()),
                errors: scan.errors,
            }),
            data_errors: pool.data_errors,
        }
    }
}

/// The ZFS pools. Fails if ZFS is unavailable, e.g. when its kernel module
/// could not be built for a new kernel. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/pools",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Pool>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn pools(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<Pool>>, ApiError> {
    auth.require(Permission::StorageView)?;
    let pools = state.storage.list_pools.execute(&client).await?;
    Ok(Json(pools.pools.into_iter().map(|pool| Pool::new(pool, pools.read_at)).collect()))
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewPoolLayout {
    Single,
    Mirror,
    Raidz1,
    Raidz2,
    Raidz3,
}

impl From<NewPoolLayout> for storage::PoolLayout {
    fn from(layout: NewPoolLayout) -> Self {
        match layout {
            NewPoolLayout::Single => storage::PoolLayout::Single,
            NewPoolLayout::Mirror => storage::PoolLayout::Mirror,
            NewPoolLayout::Raidz1 => storage::PoolLayout::Raidz { parity: 1 },
            NewPoolLayout::Raidz2 => storage::PoolLayout::Raidz { parity: 2 },
            NewPoolLayout::Raidz3 => storage::PoolLayout::Raidz { parity: 3 },
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolPlanRequest {
    pub name: String,
    pub layout: NewPoolLayout,
    /// Kernel names, e.g. `sdb`.
    pub disks: Vec<String>,
}

/// A disk a plan will erase.
#[derive(Debug, Serialize, ToSchema)]
pub struct PlannedDisk {
    pub name: String,
    pub id_link: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub size_bytes: u64,
    /// What is on it now and will be lost, e.g. `ntfs "Backup"`.
    pub erases: Vec<String>,
}

impl From<storage::PlannedDisk> for PlannedDisk {
    fn from(disk: storage::PlannedDisk) -> Self {
        Self {
            name: disk.name,
            id_link: disk.id_link,
            model: disk.model,
            serial: disk.serial,
            size_bytes: disk.size_bytes,
            erases: disk.erases,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanWarningCode {
    /// Disks of different sizes: each is used only up to the smallest.
    MixedSizes,
    /// One disk failure loses all data.
    NoRedundancy,
    /// At least one disk has data on it.
    ErasesData,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlanWarning {
    pub code: PlanWarningCode,
    /// For `mixed_sizes`.
    pub unused_bytes: Option<u64>,
}

fn warnings(warnings: Vec<storage::PlanWarning>) -> Vec<PlanWarning> {
    warnings
        .into_iter()
        .map(|warning| match warning {
            storage::PlanWarning::MixedSizes { unused_bytes } => {
                PlanWarning { code: PlanWarningCode::MixedSizes, unused_bytes: Some(unused_bytes) }
            }
            storage::PlanWarning::NoRedundancy => {
                PlanWarning { code: PlanWarningCode::NoRedundancy, unused_bytes: None }
            }
            storage::PlanWarning::ErasesData => {
                PlanWarning { code: PlanWarningCode::ErasesData, unused_bytes: None }
            }
        })
        .collect()
}

/// What creating the pool would do. Confirm it with `token` within the
/// time given.
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolPlan {
    pub token: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub name: String,
    pub layout: PoolLayout,
    pub parity: Option<u8>,
    pub volume: u16,
    pub mount_point: String,
    /// Usable bytes before ZFS overhead.
    pub capacity_bytes: u64,
    pub redundancy: usize,
    /// All of them will be erased.
    pub disks: Vec<PlannedDisk>,
    pub warnings: Vec<PlanWarning>,
}

/// Plans a new pool: which disks would be erased and what the pool would
/// offer. Erases nothing. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/plan",
    tag = "storage",
    security(("session" = [])),
    request_body = PoolPlanRequest,
    responses(
        (status = OK, body = PoolPlan),
        (status = CONFLICT, body = Problem, description = "`name_taken`, `no_volume_number`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `disk_count`, `disk_not_found`, `disk_unusable`, `duplicate_disk`, `unsupported_layout`"),
    )
)]
async fn plan_pool(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<PoolPlanRequest>,
) -> Result<Json<PoolPlan>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let actor = auth.0.actor();
    let planned = state
        .storage
        .plan_pool
        .execute(&request.name, request.layout.into(), request.disks, &actor, &client)
        .await?;
    let plan = planned.plan;
    let (layout, parity) = match plan.layout {
        storage::PoolLayout::Raidz { parity } => (PoolLayout::Raidz, Some(parity)),
        storage::PoolLayout::Mirror => (PoolLayout::Mirror, None),
        _ => (PoolLayout::Single, None),
    };
    Ok(Json(PoolPlan {
        token: planned.token,
        expires_at: planned.expires_at.unix_seconds(),
        name: plan.name.to_string(),
        layout,
        parity,
        volume: plan.volume.get(),
        mount_point: plan.volume.mount_point(),
        capacity_bytes: plan.capacity_bytes,
        redundancy: plan.redundancy,
        disks: plan.disks.into_iter().map(PlannedDisk::from).collect(),
        warnings: warnings(plan.warnings),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePoolRequest {
    /// From the plan.
    pub token: String,
    /// The pool's name, typed again.
    pub confirm: String,
}

/// Creates the pool of a plan, erasing its disks. The disks are checked
/// again first: if anything changed since the plan, nothing happens.
/// Answers with the job. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools",
    tag = "storage",
    security(("session" = [])),
    request_body = CreatePoolRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`plan_expired`, `plan_changed`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn create_pool(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<CreatePoolRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::StorageManage)?;
    let job =
        state.storage.create_pool.execute(&request.token, &request.confirm, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DestroyPoolRequest {
    /// The pool's name, typed again.
    pub confirm: String,
}

/// Destroys a pool zendiloOS created and erases its disks. All its data is
/// lost. Answers with the job. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/{name}/destroy",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    request_body = DestroyPoolRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`pool_not_managed`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn destroy_pool(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(request): ApiJson<DestroyPoolRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::StorageManage)?;
    let job = state.storage.destroy_pool.execute(&name, &request.confirm, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplacePlanRequest {
    /// GUID of the device to replace.
    pub device: String,
    /// Kernel name of the new disk.
    pub disk: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReplacePlan {
    pub token: String,
    pub expires_at: i64,
    pub pool: String,
    pub device: String,
    pub device_name: String,
    /// Will be erased.
    pub disk: PlannedDisk,
    pub warnings: Vec<PlanWarning>,
}

/// Plans replacing a device of a pool with another disk. Erases nothing.
/// Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/{name}/replace/plan",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    request_body = ReplacePlanRequest,
    responses(
        (status = OK, body = ReplacePlan),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`device_not_found`, `disk_unusable`, `disk_too_small`"),
    )
)]
async fn plan_replace(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(request): ApiJson<ReplacePlanRequest>,
) -> Result<Json<ReplacePlan>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let planned = state
        .storage
        .plan_replace
        .execute(&name, &request.device, &request.disk, &auth.0.actor(), &client)
        .await?;
    let plan = planned.plan;
    Ok(Json(ReplacePlan {
        token: planned.token,
        expires_at: planned.expires_at.unix_seconds(),
        pool: plan.pool.to_string(),
        device: plan.device,
        device_name: plan.device_name,
        disk: plan.disk.into(),
        warnings: warnings(plan.warnings),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceDiskRequest {
    pub token: String,
}

/// Carries out a planned replacement; ZFS then resilvers onto the new disk.
/// Answers with the job. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/{name}/replace",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    request_body = ReplaceDiskRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`plan_expired`, `plan_changed`"),
    )
)]
async fn replace_disk(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(_name): Path<String>,
    ApiJson(request): ApiJson<ReplaceDiskRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::StorageManage)?;
    let job = state.storage.replace_disk.execute(&request.token, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Starts checking all data of the pool; progress shows in the pool.
/// Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/{name}/scrub",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Started."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn scrub_pool(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::StorageManage)?;
    state.storage.scrub_pool.execute(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// How bad a concern is; the worst one is the verdict.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Notice,
    Warning,
    Critical,
}

impl From<storage::Severity> for Severity {
    fn from(severity: storage::Severity) -> Self {
        match severity {
            storage::Severity::Notice => Severity::Notice,
            storage::Severity::Warning => Severity::Warning,
            storage::Severity::Critical => Severity::Critical,
        }
    }
}

/// Something wrong with a pool or disk, as a code with parameters for the
/// interface to word, e.g. `pool_degraded` with `pool` and `state`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Concern {
    pub severity: Severity,
    /// `pool_unavailable`, `pool_degraded` (`pool`, `state`);
    /// `pool_data_errors` (`pool`, `files`); `device_errors` (`pool`,
    /// `device`, `errors`); `volume_filling` (`pool`, `percent`);
    /// `disk_failing`, `disk_worn` (`disk`, `model`); `no_redundancy`
    /// (`pool`); `scrub_overdue` (`pool`, `days`).
    pub code: String,
    pub params: std::collections::BTreeMap<String, String>,
}

fn state_name(state: storage::VdevState) -> &'static str {
    match state {
        storage::VdevState::Online => "online",
        storage::VdevState::Degraded => "degraded",
        storage::VdevState::Faulted => "faulted",
        storage::VdevState::Offline => "offline",
        storage::VdevState::Removed => "removed",
        storage::VdevState::Unavailable => "unavailable",
        storage::VdevState::Suspended => "suspended",
        storage::VdevState::Unknown => "unknown",
    }
}

impl From<storage::Concern> for Concern {
    fn from(concern: storage::Concern) -> Self {
        use storage::Concern as C;
        let severity = concern.severity().into();
        let (code, params): (&str, Vec<(&str, String)>) = match concern {
            C::PoolUnavailable { pool, state } => {
                ("pool_unavailable", vec![("pool", pool), ("state", state_name(state).into())])
            }
            C::PoolDegraded { pool, state } => {
                ("pool_degraded", vec![("pool", pool), ("state", state_name(state).into())])
            }
            C::PoolDataErrors { pool, files } => {
                ("pool_data_errors", vec![("pool", pool), ("files", files.to_string())])
            }
            C::DeviceErrors { pool, device, errors } => {
                ("device_errors", vec![("pool", pool), ("device", device), ("errors", errors.to_string())])
            }
            C::VolumeFilling { pool, percent } => {
                ("volume_filling", vec![("pool", pool), ("percent", percent.to_string())])
            }
            C::DiskFailing { disk, model } => {
                ("disk_failing", vec![("disk", disk), ("model", model.unwrap_or_default())])
            }
            C::DiskWorn { disk, model } => {
                ("disk_worn", vec![("disk", disk), ("model", model.unwrap_or_default())])
            }
            C::NoRedundancy { pool } => ("no_redundancy", vec![("pool", pool)]),
            C::ScrubOverdue { pool, days } => {
                ("scrub_overdue", vec![("pool", pool), ("days", days.to_string())])
            }
        };
        Self {
            severity,
            code: code.to_owned(),
            params: params.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
        }
    }
}

/// What the storage is and what is wrong with it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StorageOverview {
    /// The worst concern's severity; `null` when all is well.
    pub verdict: Option<Severity>,
    /// The worst first.
    pub concerns: Vec<Concern>,
    pub pools: usize,
    /// Disks other than the system's own.
    pub disks: usize,
    /// Across all volumes, as files see it.
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// Every concern about pools and disks in one place, the worst first.
/// SMART verdicts are the monitor's last ones (read every six hours), so
/// this reads no disk. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/overview",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = StorageOverview),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn overview(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<StorageOverview>, ApiError> {
    auth.require(Permission::StorageView)?;
    let summary = state.storage.overview.execute(&client).await?;
    let spaces = summary.pools.iter().filter_map(|pool| pool.space);
    let (used_bytes, available_bytes) =
        spaces.fold((0, 0), |(used, free), space| (used + space.used_bytes, free + space.available_bytes));
    Ok(Json(StorageOverview {
        verdict: summary.overview.verdict().map(Severity::from),
        concerns: summary.overview.concerns.into_iter().map(Concern::from).collect(),
        pools: summary.pools.len(),
        disks: summary.disks.iter().filter(|disk| disk.usage != storage::DiskUsage::System).count(),
        used_bytes,
        available_bytes,
    }))
}

/// Stops the pool's scrub; what it checked so far stays checked. A resilver
/// cannot be stopped. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/pools/{name}/scrub/stop",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Stopped."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`not_scrubbing`"),
    )
)]
async fn stop_scrub(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::StorageManage)?;
    state.storage.stop_scrub.execute(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
