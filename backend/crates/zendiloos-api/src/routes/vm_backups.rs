//! What the VMs are given of the host, and how they are kept safe
//! (`vms.manage`): the host's USB and PCI devices with the IOMMU; snapshots
//! of a VM's disks, by hand and on schedules, rolled back and deleted; and
//! backups into a share or onto a removable drive, as jobs that can be
//! canceled, restored as a new VM or over the one they came from.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_vms::application::VmBackupView;
use zendiloos_vms::domain::{self as domain, BackupPart, BackupPlace};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(vm_host_devices))
        .routes(routes!(list_vm_snapshots, take_vm_snapshot))
        .routes(routes!(rollback_vm_snapshot))
        .routes(routes!(delete_vm_snapshot))
        .routes(routes!(set_vm_snapshot_schedules))
        .routes(routes!(list_vm_backups))
        .routes(routes!(back_up_vm))
        .routes(routes!(cancel_vm_backup))
        .routes(routes!(restore_vm_backup))
        .routes(routes!(delete_vm_backup))
}

/// The IOMMU, and what helps to switch it on.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmIommu {
    /// `/sys/kernel/iommu_groups` lists groups: PCI devices can be handed
    /// to VMs.
    pub on: bool,
    /// What the kernel's command line says of it (`intel_iommu=on`, …).
    pub cmdline: Vec<String>,
    /// `intel`, `amd` or none: which hint switches it on.
    pub cpu_vendor: Option<String>,
}

/// A USB device plugged into the host.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmHostUsb {
    /// `vendor:product` in hex, `046d:c31c`: how a VM is given it.
    pub id: String,
    /// Where it is plugged in: `1-2.3`.
    pub port: String,
    pub manufacturer: Option<String>,
    pub name: Option<String>,
}

/// Why the host keeps a PCI device.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmHostNeeds {
    /// A disk on it is in use: in a pool, mounted, the system's.
    Disk { disk: String },
    /// The console's graphics.
    Console,
    /// The host's network is on it; which card, if known.
    Network { interface: Option<String> },
    /// A bridge of the PCI bus.
    Bridge,
}

/// A PCI function of the host's.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmHostPci {
    /// `0000:01:00.0`.
    pub address: String,
    /// In hex: `8086`.
    pub vendor: String,
    pub device: String,
    /// Class, subclass and interface in hex: `020000` is an Ethernet card.
    pub class: String,
    pub vendor_name: Option<String>,
    pub device_name: Option<String>,
    /// The devices of one group go to a VM together.
    pub iommu_group: Option<u32>,
    pub driver: Option<String>,
    /// Why the host keeps it; such a device, and every one of its group, is
    /// not handed over.
    pub needed: Option<VmHostNeeds>,
}

/// The host's devices a VM can be given.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmHostDevices {
    pub iommu: VmIommu,
    pub usb: Vec<VmHostUsb>,
    pub pci: Vec<VmHostPci>,
}

/// The host's USB and PCI devices with the PCI devices' IOMMU groups, why
/// the host keeps those it needs, and the IOMMU. Needs `vms.manage`.
#[utoipa::path(get, path = "/vms/devices", tag = "vms", security(("session" = [])), responses((status = OK, body = VmHostDevices)))]
async fn vm_host_devices(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<VmHostDevices>, ApiError> {
    auth.require(Permission::VmsManage)?;
    let devices = state.vms.host_devices(&client).await?;
    let vendor = devices.iommu.cpu_vendor.as_deref().and_then(|vendor| match vendor {
        "GenuineIntel" => Some("intel".to_owned()),
        "AuthenticAMD" => Some("amd".to_owned()),
        _ => None,
    });
    Ok(Json(VmHostDevices {
        iommu: VmIommu { on: devices.iommu.on, cmdline: devices.iommu.cmdline, cpu_vendor: vendor },
        usb: devices
            .usb
            .into_iter()
            .map(|usb| VmHostUsb {
                id: usb.device.id(),
                port: usb.port,
                manufacturer: usb.manufacturer,
                name: usb.name,
            })
            .collect(),
        pci: devices
            .pci
            .into_iter()
            .map(|pci| VmHostPci {
                address: pci.address,
                vendor: format!("{:04x}", pci.vendor),
                device: format!("{:04x}", pci.device),
                class: format!("{:06x}", pci.class),
                vendor_name: pci.vendor_name,
                device_name: pci.device_name,
                iommu_group: pci.iommu_group,
                driver: pci.driver,
                needed: pci.needed.map(|needed| match needed {
                    domain::HostNeeds::Disk(disk) => VmHostNeeds::Disk { disk },
                    domain::HostNeeds::Console => VmHostNeeds::Console,
                    domain::HostNeeds::Network(interface) => VmHostNeeds::Network { interface },
                    domain::HostNeeds::Bridge => VmHostNeeds::Bridge,
                }),
            })
            .collect(),
    }))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmSnapshotFrequency {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

fn frequency_dto(frequency: domain::Frequency) -> VmSnapshotFrequency {
    match frequency {
        domain::Frequency::Hourly => VmSnapshotFrequency::Hourly,
        domain::Frequency::Daily => VmSnapshotFrequency::Daily,
        domain::Frequency::Weekly => VmSnapshotFrequency::Weekly,
        domain::Frequency::Monthly => VmSnapshotFrequency::Monthly,
    }
}

/// How consistent a snapshot or backup is.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmConsistency {
    /// Taken while the VM was off.
    Off,
    /// While it ran, its file systems frozen by the QEMU guest agent.
    Frozen,
    /// While it ran without the guest agent: as after a power cut.
    Crash,
}

fn consistency_dto(consistency: domain::Consistency) -> VmConsistency {
    match consistency {
        domain::Consistency::Off => VmConsistency::Off,
        domain::Consistency::Frozen => VmConsistency::Frozen,
        domain::Consistency::Crash => VmConsistency::Crash,
    }
}

/// A snapshot of all of a VM's disks at one moment.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmSnapshot {
    /// `zdl-YYYY-MM-DD_HH.MM.SS`, UTC: how it is named in ZFS.
    pub label: String,
    pub taken_at: i64,
    pub name: Option<String>,
    /// The schedules it was taken for; none: by hand.
    pub schedules: Vec<VmSnapshotFrequency>,
    pub consistency: VmConsistency,
    /// The slots of the disks it has; a disk added later is not in it.
    pub disks: Vec<u8>,
    /// What it costs: data changed since.
    pub used_bytes: u64,
    /// What the disks held then.
    pub referenced_bytes: u64,
}

fn snapshot_dto(snapshot: domain::VmSnapshot) -> VmSnapshot {
    VmSnapshot {
        label: snapshot.label,
        taken_at: snapshot.taken_at.unix_seconds(),
        name: snapshot.name,
        schedules: snapshot.schedules.into_iter().map(frequency_dto).collect(),
        consistency: consistency_dto(snapshot.consistency),
        disks: snapshot.disks,
        used_bytes: snapshot.used_bytes,
        referenced_bytes: snapshot.referenced_bytes,
    }
}

/// A schedule: how often, and how many of its snapshots to keep.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmSnapshotSchedule {
    pub frequency: VmSnapshotFrequency,
    /// 1 to 100.
    pub keep: u32,
}

fn schedule_dto(schedule: domain::SnapshotSchedule) -> VmSnapshotSchedule {
    VmSnapshotSchedule { frequency: frequency_dto(schedule.frequency), keep: schedule.keep }
}

/// A VM's snapshots, newest first, and its schedules.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmSnapshots {
    pub snapshots: Vec<VmSnapshot>,
    pub schedules: Vec<VmSnapshotSchedule>,
}

/// The VM's snapshots and schedules. Needs `vms.manage`.
#[utoipa::path(
    get,
    path = "/vms/{id}/snapshots",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses((status = OK, body = VmSnapshots), (status = NOT_FOUND, body = Problem, description = "`not_found`"))
)]
async fn list_vm_snapshots(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<VmSnapshots>, ApiError> {
    auth.require(Permission::VmsManage)?;
    let found = state.vms.snapshots(&id, &client).await?;
    Ok(Json(VmSnapshots {
        snapshots: found.snapshots.into_iter().map(snapshot_dto).collect(),
        schedules: found.schedules.into_iter().map(schedule_dto).collect(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TakeVmSnapshotRequest {
    /// Up to 64 characters; none: shown by its time.
    #[serde(default)]
    pub name: String,
}

/// Takes a snapshot of all the VM's disks at once. While it runs, the
/// snapshot is crash-consistent unless the QEMU guest agent froze its file
/// systems. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/snapshots",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = TakeVmSnapshotRequest,
    responses(
        (status = CREATED, body = VmSnapshot),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`"),
    )
)]
async fn take_vm_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<TakeVmSnapshotRequest>,
) -> Result<(StatusCode, Json<VmSnapshot>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let snapshot = state.vms.take_snapshot(&id, &request.name, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(snapshot_dto(snapshot))))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RollbackVmSnapshotRequest {
    /// The snapshots taken after it go with it (`zfs rollback -r`): only if
    /// true, else a rollback to any but the newest is refused.
    #[serde(default)]
    pub newer: bool,
}

/// What a rollback destroyed.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmRollback {
    /// The labels of the snapshots after it, gone.
    pub removed: Vec<String>,
}

/// Returns the VM's disks to the snapshot; the VM must be shut off. A disk
/// keeps its size, and one added since stays as it is. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/snapshots/{label}/rollback",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path), ("label" = String, Path)),
    request_body = RollbackVmSnapshotRequest,
    responses(
        (status = OK, body = VmRollback),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`newer_snapshots`: the detail names the snapshots that would go; `vm_running`; `refused`"),
    )
)]
async fn rollback_vm_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, label)): Path<(String, String)>,
    ApiJson(request): ApiJson<RollbackVmSnapshotRequest>,
) -> Result<Json<VmRollback>, ApiError> {
    auth.require(Permission::VmsManage)?;
    let removed = state.vms.rollback_snapshot(&id, &label, request.newer, &auth.0.actor(), &client).await?;
    Ok(Json(VmRollback { removed }))
}

/// Deletes the snapshot of every disk of the VM. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/snapshots/{label}/delete",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path), ("label" = String, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = Problem, description = "`not_found`"))
)]
async fn delete_vm_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, label)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.delete_snapshot(&id, &label, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VmSnapshotSchedulesRequest {
    /// One per frequency.
    pub schedules: Vec<VmSnapshotSchedule>,
}

/// Replaces how often the VM is snapshotted and how many of each to keep;
/// looked at every five minutes. Snapshots of a schedule that goes stay.
/// Needs `vms.manage`.
#[utoipa::path(
    put,
    path = "/vms/{id}/snapshots/schedules",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = VmSnapshotSchedulesRequest,
    responses(
        (status = OK, body = [VmSnapshotSchedule]),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: 1 to 100 kept, one per frequency"),
    )
)]
async fn set_vm_snapshot_schedules(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<VmSnapshotSchedulesRequest>,
) -> Result<Json<Vec<VmSnapshotSchedule>>, ApiError> {
    auth.require(Permission::VmsManage)?;
    let schedules = request
        .schedules
        .into_iter()
        .map(|schedule| domain::SnapshotSchedule {
            frequency: match schedule.frequency {
                VmSnapshotFrequency::Hourly => domain::Frequency::Hourly,
                VmSnapshotFrequency::Daily => domain::Frequency::Daily,
                VmSnapshotFrequency::Weekly => domain::Frequency::Weekly,
                VmSnapshotFrequency::Monthly => domain::Frequency::Monthly,
            },
            keep: schedule.keep,
        })
        .collect();
    let set = state.vms.set_snapshot_schedules(&id, schedules, &auth.0.actor(), &client).await?;
    Ok(Json(set.into_iter().map(schedule_dto).collect()))
}

/// Where a backup goes or is.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VmBackupTarget {
    Share {
        share_id: i64,
    },
    /// A removable drive, mounted, by its id.
    Drive {
        drive: String,
    },
}

/// What a file of a backup holds.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmBackupFilePart {
    /// A disk's raw image, compressed with zstd: `disk<N>.raw.zst`.
    Disk { slot: u8, volsize: u64 },
    /// The UEFI variables: `nvram.fd`.
    Nvram,
    /// The TPM's state: `tpm2-00.permall`.
    Tpm,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VmBackupFile {
    pub part: VmBackupFilePart,
    pub bytes: u64,
    pub sha256: String,
}

/// A backup of a VM.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmBackup {
    pub id: i64,
    /// The VM it came from, and whether it is still there.
    pub vm: String,
    pub vm_exists: bool,
    pub vm_name: String,
    pub target: VmBackupTarget,
    /// The share's name, or the drive's id.
    pub place: String,
    /// Its folder beneath the share's or drive's root, with `manifest.json`.
    pub folder: String,
    pub taken_at: i64,
    pub consistency: VmConsistency,
    /// What its files hold in all.
    pub bytes: u64,
    pub files: Vec<VmBackupFile>,
    /// The VM's settings then: its system, vCPUs, memory and disks.
    pub os: super::vms::VmOs,
    pub cpus: u16,
    pub memory_mib: u32,
    pub disks_gib: Vec<u32>,
    pub tpm: bool,
    pub secure_boot: bool,
}

fn backup_dto(view: VmBackupView) -> VmBackup {
    let backup = view.backup;
    let bytes = backup.bytes();
    let target = match &backup.place {
        BackupPlace::Share { share_id, .. } => VmBackupTarget::Share { share_id: *share_id },
        BackupPlace::Drive { drive } => VmBackupTarget::Drive { drive: drive.clone() },
    };
    VmBackup {
        id: backup.id,
        vm: backup.vm,
        vm_exists: view.vm_exists,
        vm_name: backup.vm_name,
        place: backup.place.shown().to_owned(),
        target,
        folder: backup.folder,
        taken_at: backup.taken_at.unix_seconds(),
        consistency: consistency_dto(backup.consistency),
        bytes,
        files: backup
            .files
            .into_iter()
            .map(|file| VmBackupFile {
                part: match file.part {
                    BackupPart::Disk { slot, volsize } => VmBackupFilePart::Disk { slot, volsize },
                    BackupPart::Nvram => VmBackupFilePart::Nvram,
                    BackupPart::Tpm => VmBackupFilePart::Tpm,
                },
                bytes: file.bytes,
                sha256: file.sha256,
            })
            .collect(),
        os: match backup.settings.os {
            domain::OsPreset::Linux => super::vms::VmOs::Linux,
            domain::OsPreset::Windows => super::vms::VmOs::Windows,
            domain::OsPreset::Other => super::vms::VmOs::Other,
        },
        cpus: backup.settings.cpus,
        memory_mib: backup.settings.memory_mib,
        disks_gib: backup.settings.disks.iter().map(|disk| disk.size_gib).collect(),
        tpm: backup.settings.tpm,
        secure_boot: backup.settings.secure_boot,
    }
}

/// Every VM's backups, newest first; those of VMs that went too. Needs
/// `vms.manage`.
#[utoipa::path(get, path = "/vms/backups", tag = "vms", security(("session" = [])), responses((status = OK, body = [VmBackup])))]
async fn list_vm_backups(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<VmBackup>>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(state.vms.backups().await?.into_iter().map(backup_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackUpVmRequest {
    pub target: VmBackupTarget,
    /// A folder of the share or drive; the backup goes into
    /// `<folder>/<id>/<YYYYMMDD-HHMMSS>`. None: its root.
    #[serde(default)]
    pub folder: String,
}

/// Backs the VM up, as a job: a snapshot of its disks taken (so it may
/// run: the backup is as consistent as that snapshot), each disk as a
/// compressed raw image, its UEFI variables, its TPM's state and its
/// settings with a manifest. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/backups",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = BackUpVmRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such VM or share"),
        (status = CONFLICT, body = Problem, description = "`vm_creating`; `refused`: it is being changed, backed up or restored"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a folder or drive that is none"),
    )
)]
async fn back_up_vm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<BackUpVmRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let target = match request.target {
        VmBackupTarget::Share { share_id } => domain::BackupTarget::Share { share_id },
        VmBackupTarget::Drive { drive } => domain::BackupTarget::Drive { drive },
    };
    let job = state.vms.back_up(&id, target, &request.folder, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Cancels the backup or restore the VM is in: its job ends as
/// `canceled`, the agent stops its unit, nothing half-written is kept, and a
/// restore over the VM returns its disks to what they were. Needs
/// `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/{id}/backups/cancel",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses((status = NO_CONTENT), (status = CONFLICT, body = Problem, description = "`refused`: none runs"))
)]
async fn cancel_vm_backup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.cancel_work(&id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// How a backup is restored.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VmRestoreAs {
    /// A new VM of that name, its disks on the location of `pool` (the
    /// first if none); new MAC addresses.
    New {
        name: String,
        #[serde(default)]
        pool: Option<String>,
    },
    /// Over the VM it came from, which must be shut off; its name typed
    /// again. A disk it has that the backup has not is destroyed.
    Over { confirm: String },
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreVmBackupRequest {
    #[serde(rename = "as")]
    pub restore_as: VmRestoreAs,
}

/// The VM a restore makes or restores, and its job.
#[derive(Debug, Serialize, ToSchema)]
pub struct RestoredVm {
    pub vm: String,
    pub job: Job,
}

/// Restores a backup, as a job: every file checked against its SHA-256 as
/// recorded; a failure removes a new VM again, or returns the old one's
/// disks to what they were. Images no longer in the library leave their
/// drives empty, cards on networks that went go through the NAS, and the
/// host's USB and PCI devices are not given again. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/backups/{backup}/restore",
    tag = "vms",
    security(("session" = [])),
    params(("backup" = i64, Path)),
    request_body = RestoreVmBackupRequest,
    responses(
        (status = ACCEPTED, body = RestoredVm),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such backup, or its VM went"),
        (status = CONFLICT, body = Problem, description = "`vm_running`; `name_taken`; `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`; `invalid_input`"),
    )
)]
async fn restore_vm_backup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(backup): Path<i64>,
    ApiJson(request): ApiJson<RestoreVmBackupRequest>,
) -> Result<(StatusCode, Json<RestoredVm>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let how = match request.restore_as {
        VmRestoreAs::New { name, pool } => domain::RestoreAs::New { name, pool: pool.unwrap_or_default() },
        VmRestoreAs::Over { confirm } => domain::RestoreAs::Over { confirm },
    };
    let (vm, job) = state.vms.restore(backup, how, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(RestoredVm { vm, job: job.into() })))
}

/// Deletes a backup: its files, then its record. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/backups/{backup}/delete",
    tag = "vms",
    security(("session" = [])),
    params(("backup" = i64, Path)),
    responses((status = NO_CONTENT), (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = CONFLICT, body = Problem, description = "`refused`: the share or drive cannot be used"))
)]
async fn delete_vm_backup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(backup): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.delete_backup(backup, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
