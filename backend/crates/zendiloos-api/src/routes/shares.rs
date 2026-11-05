//! Shares and who may use them (`shares.manage`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_sharing::application::{NewShareInput, ShareChanges, SharingError};
use zendiloos_sharing::domain::{self as sharing, ShareId};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_shares, create_share))
        .routes(routes!(update_share))
        .routes(routes!(delete_share))
        .routes(routes!(snapshots, take_snapshot))
        .routes(routes!(delete_snapshot))
        .routes(routes!(rollback_snapshot))
        .routes(routes!(set_snapshot_schedules))
}

impl From<SharingError> for ApiError {
    fn from(error: SharingError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            SharingError::InvalidName(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidNetwork(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidDescription => ApiError::InvalidInput(error.to_string()),
            SharingError::NameTaken => rejected(StatusCode::CONFLICT, "name_taken", None),
            SharingError::VolumeNotFound => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "volume_not_found", None)
            }
            SharingError::ShareNotFound => ApiError::NotFound,
            SharingError::UnknownSubject => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "unknown_subject", None)
            }
            SharingError::Duplicate => rejected(StatusCode::UNPROCESSABLE_ENTITY, "duplicate_rule", None),
            SharingError::ConfirmationMismatch => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch", None)
            }
            SharingError::Refused(reason) => rejected(StatusCode::CONFLICT, "refused", Some(reason)),
            SharingError::InvalidSettings(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidSchedule(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidNote(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::SnapshotNotFound => ApiError::NotFound,
            SharingError::SnapshotTaken => rejected(StatusCode::CONFLICT, "snapshot_taken", None),
            SharingError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareAccessLevel {
    Read,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Account,
    Group,
}

/// An account or group and what it may do in the share.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareRule {
    pub kind: SubjectKind,
    /// Account or group ID.
    pub id: i64,
    pub access: ShareAccessLevel,
}

/// A host or network that may mount the share over NFS.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NfsRule {
    /// An IP address or CIDR network, e.g. `192.168.1.0/24`.
    pub network: String,
    pub write: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Share {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub pool: String,
    pub volume: u16,
    /// E.g. `/volume1/photos`.
    pub path: String,
    pub recycle_bin: bool,
    pub settings: ShareSettingsDto,
    /// What the share holds; absent while its volume is not there.
    pub used_bytes: Option<u64>,
    /// What it may still take: up to its quota, or what the pool has left.
    pub available_bytes: Option<u64>,
    pub access: Vec<ShareRule>,
    /// Empty: not exported over NFS.
    pub nfs: Vec<NfsRule>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// How a share's dataset compresses data.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareCompressionKind {
    Off,
    /// Fast; the default.
    Lz4,
    /// Smaller, costs more CPU.
    Zstd,
}

/// How a share's dataset stores data.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareSettingsDto {
    /// Limits the share's own data (snapshots not counted); at least 1 GiB.
    /// `null`: no quota.
    pub quota_bytes: Option<u64>,
    pub compression: ShareCompressionKind,
    /// 16384, 131072 (the default) or 1048576.
    pub record_size_bytes: u64,
}

impl From<sharing::ShareSettings> for ShareSettingsDto {
    fn from(settings: sharing::ShareSettings) -> Self {
        Self {
            quota_bytes: settings.quota_bytes,
            compression: match settings.compression {
                sharing::Compression::Off => ShareCompressionKind::Off,
                sharing::Compression::Lz4 => ShareCompressionKind::Lz4,
                sharing::Compression::Zstd => ShareCompressionKind::Zstd,
            },
            record_size_bytes: settings.record_size.bytes(),
        }
    }
}

impl TryFrom<ShareSettingsDto> for sharing::ShareSettings {
    type Error = ApiError;

    fn try_from(settings: ShareSettingsDto) -> Result<Self, ApiError> {
        Ok(Self {
            quota_bytes: settings.quota_bytes,
            compression: match settings.compression {
                ShareCompressionKind::Off => sharing::Compression::Off,
                ShareCompressionKind::Lz4 => sharing::Compression::Lz4,
                ShareCompressionKind::Zstd => sharing::Compression::Zstd,
            },
            record_size: sharing::RecordSize::from_bytes(settings.record_size_bytes).ok_or_else(|| {
                ApiError::InvalidInput("the record size is 16384, 131072 or 1048576 bytes".to_owned())
            })?,
        })
    }
}

impl Share {
    fn with_usage(share: sharing::Share, usage: Option<&sharing::ShareUsage>) -> Self {
        let mut dto = Self::from(share);
        dto.used_bytes = usage.map(|u| u.used_bytes);
        dto.available_bytes = usage.map(|u| u.available_bytes);
        dto
    }
}

impl From<sharing::Share> for Share {
    fn from(share: sharing::Share) -> Self {
        Self {
            id: share.id.get(),
            path: share.path(),
            name: share.name.to_string(),
            description: share.description,
            pool: share.pool,
            volume: share.volume,
            recycle_bin: share.recycle_bin,
            settings: share.settings.into(),
            used_bytes: None,
            available_bytes: None,
            access: share.access.into_iter().map(ShareRule::from).collect(),
            nfs: share.nfs.into_iter().map(|c| NfsRule { network: c.network, write: c.write }).collect(),
            created_at: share.created_at.unix_seconds(),
            updated_at: share.updated_at.unix_seconds(),
        }
    }
}

impl From<sharing::AccessRule> for ShareRule {
    fn from(rule: sharing::AccessRule) -> Self {
        let (kind, id) = match rule.subject {
            sharing::Subject::Account(id) => (SubjectKind::Account, id),
            sharing::Subject::Group(id) => (SubjectKind::Group, id),
        };
        let access = match rule.access {
            sharing::Access::Read => ShareAccessLevel::Read,
            sharing::Access::ReadWrite => ShareAccessLevel::ReadWrite,
        };
        Self { kind, id, access }
    }
}

impl From<ShareRule> for sharing::AccessRule {
    fn from(rule: ShareRule) -> Self {
        let subject = match rule.kind {
            SubjectKind::Account => sharing::Subject::Account(rule.id),
            SubjectKind::Group => sharing::Subject::Group(rule.id),
        };
        let access = match rule.access {
            ShareAccessLevel::Read => sharing::Access::Read,
            ShareAccessLevel::ReadWrite => sharing::Access::ReadWrite,
        };
        Self { subject, access }
    }
}

/// All shares, ordered by name. Needs `shares.manage`.
#[utoipa::path(
    get,
    path = "/shares",
    tag = "shares",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Share>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn list_shares(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<Share>>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let shares = state.sharing.list().await?;
    // Without the agent, the shares are still listed, just without usage.
    let usage = state.sharing.usage(&client).await.unwrap_or_else(|error| {
        tracing::warn!(%error, "cannot read what the shares hold");
        Vec::new()
    });
    Ok(Json(
        shares
            .into_iter()
            .map(|share| {
                let found = usage.iter().find(|(id, _)| *id == share.id).map(|(_, u)| u);
                Share::with_usage(share, found)
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    pub name: String,
    pub description: Option<String>,
    /// The pool (volume) to create it on.
    pub pool: String,
    #[serde(default = "yes")]
    pub recycle_bin: bool,
    #[serde(default)]
    pub access: Vec<ShareRule>,
    #[serde(default)]
    pub nfs: Vec<NfsRule>,
    /// Absent: no quota, lz4, 128 KiB records.
    #[serde(default)]
    pub settings: Option<ShareSettingsDto>,
}

fn yes() -> bool {
    true
}

/// Creates a share: a dataset on the volume with its permissions, offered
/// over SMB (and NFS, if clients are given). Needs `shares.manage`.
#[utoipa::path(
    post,
    path = "/shares",
    tag = "shares",
    security(("session" = [])),
    request_body = CreateShareRequest,
    responses(
        (status = CREATED, body = Share),
        (status = CONFLICT, body = Problem, description = "`name_taken`, `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `volume_not_found`, `unknown_subject`, `duplicate_rule`"),
    )
)]
async fn create_share(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<CreateShareRequest>,
) -> Result<(StatusCode, Json<Share>), ApiError> {
    auth.require(Permission::SharesManage)?;
    let input = NewShareInput {
        name: request.name,
        description: request.description,
        pool: request.pool,
        recycle_bin: request.recycle_bin,
        access: request.access.into_iter().map(Into::into).collect(),
        nfs: request.nfs.into_iter().map(|c| (c.network, c.write)).collect(),
        settings: request.settings.map(TryInto::try_into).transpose()?.unwrap_or_default(),
    };
    let share = state.sharing.create(input, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(share.into())))
}

/// Absent fields stay as they are.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateShareRequest {
    /// An empty string removes the description.
    pub description: Option<String>,
    pub recycle_bin: Option<bool>,
    /// Replaces all rules.
    pub access: Option<Vec<ShareRule>>,
    /// Replaces all NFS clients; empty stops exporting.
    pub nfs: Option<Vec<NfsRule>>,
    /// Replaces quota, compression and record size. A quota below what the
    /// share holds is refused.
    pub settings: Option<ShareSettingsDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareUpdate {
    pub share: Share,
    /// Applies changed permissions to existing files, if they changed.
    pub job: Option<Job>,
}

/// Changes a share. New permissions apply to the share at once and to its
/// existing files in a job. Needs `shares.manage`.
#[utoipa::path(
    patch,
    path = "/shares/{id}",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = UpdateShareRequest,
    responses(
        (status = OK, body = ShareUpdate),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `unknown_subject`, `duplicate_rule`"),
    )
)]
async fn update_share(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<UpdateShareRequest>,
) -> Result<Json<ShareUpdate>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let changes = ShareChanges {
        description: request.description.map(Some),
        recycle_bin: request.recycle_bin,
        access: request.access.map(|rules| rules.into_iter().map(Into::into).collect()),
        nfs: request.nfs.map(|clients| clients.into_iter().map(|c| (c.network, c.write)).collect()),
        settings: request.settings.map(TryInto::try_into).transpose()?,
    };
    let (share, job) = state.sharing.update(ShareId::new(id), changes, &auth.0.actor(), &client).await?;
    Ok(Json(ShareUpdate { share: share.into(), job: job.map(Job::from) }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteShareRequest {
    /// The share's name, typed again.
    pub confirm: String,
}

/// Stops offering the share and destroys it with all its files. Answers
/// with the job. Needs `shares.manage`.
#[utoipa::path(
    post,
    path = "/shares/{id}/delete",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = DeleteShareRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn delete_share(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<DeleteShareRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SharesManage)?;
    let job = state.sharing.delete(ShareId::new(id), &request.confirm, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// How often a schedule takes a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFrequency {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl From<sharing::Frequency> for SnapshotFrequency {
    fn from(frequency: sharing::Frequency) -> Self {
        match frequency {
            sharing::Frequency::Hourly => Self::Hourly,
            sharing::Frequency::Daily => Self::Daily,
            sharing::Frequency::Weekly => Self::Weekly,
            sharing::Frequency::Monthly => Self::Monthly,
        }
    }
}

impl From<SnapshotFrequency> for sharing::Frequency {
    fn from(frequency: SnapshotFrequency) -> Self {
        match frequency {
            SnapshotFrequency::Hourly => Self::Hourly,
            SnapshotFrequency::Daily => Self::Daily,
            SnapshotFrequency::Weekly => Self::Weekly,
            SnapshotFrequency::Monthly => Self::Monthly,
        }
    }
}

/// A snapshot of a share: what it looked like at a moment. Windows offers
/// them as "Previous Versions".
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ShareSnapshot {
    /// Its name, the UTC time it was taken: `zdl-YYYY-MM-DD_HH.MM.SS`.
    pub label: String,
    /// Unix seconds.
    pub taken_at: i64,
    /// The schedules it was taken for; empty if someone took it by hand.
    pub schedules: Vec<SnapshotFrequency>,
    pub note: Option<String>,
    /// Space it holds on its own: data that changed since.
    pub used_bytes: u64,
    /// What the share held then.
    pub referenced_bytes: u64,
}

impl From<sharing::Snapshot> for ShareSnapshot {
    fn from(snapshot: sharing::Snapshot) -> Self {
        Self {
            label: snapshot.label,
            taken_at: snapshot.taken_at.unix_seconds(),
            schedules: snapshot.schedules.into_iter().map(SnapshotFrequency::from).collect(),
            note: snapshot.note,
            used_bytes: snapshot.used_bytes,
            referenced_bytes: snapshot.referenced_bytes,
        }
    }
}

/// How often to snapshot a share and how many of those to keep.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareSnapshotSchedule {
    pub frequency: SnapshotFrequency,
    /// 1 to 100.
    pub keep: u32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ShareSnapshots {
    /// Newest first.
    pub snapshots: Vec<ShareSnapshot>,
    pub schedules: Vec<ShareSnapshotSchedule>,
}

fn schedule_dto(schedule: sharing::SnapshotSchedule) -> ShareSnapshotSchedule {
    ShareSnapshotSchedule { frequency: schedule.frequency.into(), keep: schedule.keep }
}

/// The share's snapshots, newest first, and its schedules. Needs
/// `shares.manage`.
#[utoipa::path(
    get,
    path = "/shares/{id}/snapshots",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = OK, body = ShareSnapshots),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn snapshots(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<Json<ShareSnapshots>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let (snapshots, schedules) = state.sharing.snapshots(ShareId::new(id), &client).await?;
    Ok(Json(ShareSnapshots {
        snapshots: snapshots.into_iter().map(ShareSnapshot::from).collect(),
        schedules: schedules.into_iter().map(schedule_dto).collect(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TakeSnapshotRequest {
    /// Up to 200 characters, one line.
    #[serde(default)]
    pub note: String,
}

/// Takes a snapshot now. Schedules never delete one taken by hand. Needs
/// `shares.manage`.
#[utoipa::path(
    post,
    path = "/shares/{id}/snapshots",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = TakeSnapshotRequest,
    responses(
        (status = CREATED, body = ShareSnapshot),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`snapshot_taken`: one this second already; `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn take_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<TakeSnapshotRequest>,
) -> Result<(StatusCode, Json<ShareSnapshot>), ApiError> {
    auth.require(Permission::SharesManage)?;
    let snapshot =
        state.sharing.take_snapshot(ShareId::new(id), &request.note, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(snapshot.into())))
}

/// Deletes one snapshot; the share's data stays as it is. Needs
/// `shares.manage`.
#[utoipa::path(
    post,
    path = "/shares/{id}/snapshots/{label}/delete",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path), ("label" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Deleted."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn delete_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, label)): Path<(i64, String)>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SharesManage)?;
    state.sharing.delete_snapshot(ShareId::new(id), &label, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RollbackRequest {
    /// The share's name, typed by the user.
    pub confirm: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RolledBack {
    /// Labels of the later snapshots that went with the rollback.
    pub destroyed: Vec<String>,
}

/// Returns the share to a snapshot: everything changed since is lost, and
/// so are the snapshots taken after it. Needs `shares.manage` and the
/// share's name as confirmation.
#[utoipa::path(
    post,
    path = "/shares/{id}/snapshots/{label}/rollback",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path), ("label" = String, Path)),
    request_body = RollbackRequest,
    responses(
        (status = OK, body = RolledBack),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn rollback_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, label)): Path<(i64, String)>,
    ApiJson(request): ApiJson<RollbackRequest>,
) -> Result<Json<RolledBack>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let destroyed = state
        .sharing
        .rollback_snapshot(ShareId::new(id), &label, &request.confirm, &auth.0.actor(), &client)
        .await?;
    Ok(Json(RolledBack { destroyed }))
}

/// Replaces how often the share is snapshotted and how many to keep, one
/// schedule per frequency. Snapshots of a schedule that is removed stay
/// until deleted by hand. Needs `shares.manage`.
#[utoipa::path(
    put,
    path = "/shares/{id}/snapshots/schedules",
    tag = "shares",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = Vec<ShareSnapshotSchedule>,
    responses(
        (status = OK, body = Vec<ShareSnapshotSchedule>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn set_snapshot_schedules(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<Vec<ShareSnapshotSchedule>>,
) -> Result<Json<Vec<ShareSnapshotSchedule>>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let schedules = request
        .into_iter()
        .map(|s| sharing::SnapshotSchedule { frequency: s.frequency.into(), keep: s.keep })
        .collect();
    let saved =
        state.sharing.set_snapshot_schedules(ShareId::new(id), schedules, &auth.0.actor(), &client).await?;
    Ok(Json(saved.into_iter().map(schedule_dto).collect()))
}
