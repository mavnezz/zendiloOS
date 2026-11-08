//! Shares and who may use them (`shares.manage`).

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_sharing::application::{NewShareInput, ShareChanges, SharingError};
use zendiloos_sharing::domain::{self as sharing, NfsClient, NfsSquash, ShareId};

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
        .routes(routes!(file_services, set_file_services))
        .routes(routes!(subject_access, set_subject_access))
        .routes(routes!(homes, set_homes))
        .routes(routes!(set_account_home))
}

impl From<SharingError> for ApiError {
    fn from(error: SharingError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            SharingError::InvalidName(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidNetwork(e) => ApiError::InvalidInput(e.to_string()),
            SharingError::InvalidServices(e) => ApiError::InvalidInput(e.to_string()),
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
            SharingError::UsedByApps(apps) => {
                rejected(StatusCode::CONFLICT, "share_used_by_apps", Some(apps))
            }
            SharingError::UsedByProjects(projects) => {
                rejected(StatusCode::CONFLICT, "share_used_by_projects", Some(projects))
            }
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

/// What a rule says. `deny` keeps the account or group out, also where
/// another rule would let it in: a group's denial wins over its members'
/// own rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareRuleLevel {
    Deny,
    Read,
    ReadWrite,
}

impl From<sharing::Permission> for ShareRuleLevel {
    fn from(permission: sharing::Permission) -> Self {
        match permission {
            sharing::Permission::Deny => ShareRuleLevel::Deny,
            sharing::Permission::Read => ShareRuleLevel::Read,
            sharing::Permission::ReadWrite => ShareRuleLevel::ReadWrite,
        }
    }
}

impl From<ShareRuleLevel> for sharing::Permission {
    fn from(level: ShareRuleLevel) -> Self {
        match level {
            ShareRuleLevel::Deny => sharing::Permission::Deny,
            ShareRuleLevel::Read => sharing::Permission::Read,
            ShareRuleLevel::ReadWrite => sharing::Permission::ReadWrite,
        }
    }
}

impl From<sharing::Access> for ShareAccessLevel {
    fn from(access: sharing::Access) -> Self {
        match access {
            sharing::Access::Read => ShareAccessLevel::Read,
            sharing::Access::ReadWrite => ShareAccessLevel::ReadWrite,
        }
    }
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
    pub access: ShareRuleLevel,
}

/// A host or network that may mount the share over NFS, and how.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NfsRule {
    /// An IP address or CIDR network (`192.168.1.0/24`), a host name
    /// (`nas.local`) or a pattern with `*` and `?` (`*.example.com`).
    pub network: String,
    pub write: bool,
    /// Whose identity the client's requests keep; `root` if not given.
    #[serde(default)]
    pub squash: NfsSquashKind,
    /// Writes answered before they are on disk; off if not given.
    #[serde(default, rename = "async")]
    pub asynchronous: bool,
    /// Also from ports above 1024; off if not given.
    #[serde(default)]
    pub insecure: bool,
}

/// Whose identity an NFS client's requests keep: `none` (root stays root),
/// `root` (root becomes the anonymous user), `all` (everyone does).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NfsSquashKind {
    None,
    #[default]
    Root,
    All,
}

impl From<NfsSquash> for NfsSquashKind {
    fn from(squash: NfsSquash) -> Self {
        match squash {
            NfsSquash::None => Self::None,
            NfsSquash::Root => Self::Root,
            NfsSquash::All => Self::All,
        }
    }
}

impl From<NfsSquashKind> for NfsSquash {
    fn from(squash: NfsSquashKind) -> Self {
        match squash {
            NfsSquashKind::None => Self::None,
            NfsSquashKind::Root => Self::Root,
            NfsSquashKind::All => Self::All,
        }
    }
}

impl From<NfsClient> for NfsRule {
    fn from(client: NfsClient) -> Self {
        Self {
            network: client.network,
            write: client.write,
            squash: client.squash.into(),
            asynchronous: client.asynchronous,
            insecure: client.insecure,
        }
    }
}

impl From<NfsRule> for NfsClient {
    fn from(rule: NfsRule) -> Self {
        Self {
            network: rule.network,
            write: rule.write,
            squash: rule.squash.into(),
            asynchronous: rule.asynchronous,
            insecure: rule.insecure,
        }
    }
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
    /// Offered on the network, over SMB and to its NFS clients; otherwise
    /// used on the NAS only, with the same permissions.
    pub shared: bool,
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
            shared: share.shared,
            recycle_bin: share.recycle_bin,
            settings: share.settings.into(),
            used_bytes: None,
            available_bytes: None,
            access: share.access.into_iter().map(ShareRule::from).collect(),
            nfs: share.nfs.into_iter().map(NfsRule::from).collect(),
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
        Self { kind, id, access: rule.permission.into() }
    }
}

impl From<ShareRule> for sharing::AccessRule {
    fn from(rule: ShareRule) -> Self {
        let subject = match rule.kind {
            SubjectKind::Account => sharing::Subject::Account(rule.id),
            SubjectKind::Group => sharing::Subject::Group(rule.id),
        };
        Self { subject, permission: rule.access.into() }
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
    /// Offered on the network at once (the default); `false`: a folder on
    /// the volume used on the NAS only, shared later if wanted.
    #[serde(default = "yes")]
    pub shared: bool,
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

/// Creates a folder directly on a volume: a dataset with its permissions,
/// offered over SMB (and NFS, if clients are given) while shared. Needs
/// `shares.manage`.
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
        shared: request.shared,
        recycle_bin: request.recycle_bin,
        access: request.access.into_iter().map(Into::into).collect(),
        nfs: request.nfs.into_iter().map(NfsClient::from).collect(),
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
    /// On or off the network; the folder, its data and permissions stay.
    /// Refused off while Macs back up to it (`refused`).
    pub shared: Option<bool>,
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
        (status = CONFLICT, body = Problem, description = "`refused`: rules being applied to the files, or off the network while Macs back up to it"),
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
        shared: request.shared,
        recycle_bin: request.recycle_bin,
        access: request.access.map(|rules| rules.into_iter().map(Into::into).collect()),
        nfs: request.nfs.map(|clients| clients.into_iter().map(NfsClient::from).collect()),
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
        (status = CONFLICT, body = Problem, description = "`share_used_by_apps`, `share_used_by_projects`: the detail names them"),
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

/// The oldest SMB version the NAS answers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmbMinProtocol {
    /// SMB 2.0: Windows Vista, old printers and scanners.
    Smb2,
    /// SMB 2.1: Windows 7.
    #[serde(rename = "smb2_1")]
    Smb2_1,
    /// SMB 3: Windows 8 and every current system.
    Smb3,
}

/// Whether SMB traffic is encrypted.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmbEncryptionMode {
    /// As each client asks.
    #[default]
    Default,
    /// Always; clients that cannot encrypt cannot connect.
    Required,
    Off,
}

/// Whether SMB packets are signed.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmbSigningMode {
    /// As each client asks.
    #[default]
    Default,
    Mandatory,
    Disabled,
}

/// The share Macs back up to with Time Machine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeMachineShare {
    pub share_id: i64,
    /// At least 1 GiB; absent: as much as the share may hold.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

/// Which file services run, and how Samba presents itself; one set for
/// all shares. Fields added later have defaults, as a fresh install has
/// them.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileServices {
    /// SMB for Windows, macOS and Linux.
    pub smb: bool,
    /// NFS for Linux and other Unix systems.
    pub nfs: bool,
    /// The Windows workgroup: 1 to 15 letters, digits, `-` or `_`.
    pub workgroup: String,
    pub smb_min_protocol: SmbMinProtocol,
    #[serde(default)]
    pub smb_encryption: SmbEncryptionMode,
    #[serde(default)]
    pub smb_signing: SmbSigningMode,
    /// Shares a user may not open are not listed to them.
    #[serde(default = "yes")]
    pub hide_unreadable: bool,
    /// SMB 3 over several connections at once.
    #[serde(default = "yes")]
    pub multichannel: bool,
    /// Clients may cache files they have open.
    #[serde(default = "yes")]
    pub oplocks: bool,
    /// SMB 2 leases; only with oplocks.
    #[serde(default = "yes")]
    pub leases: bool,
    /// Open files survive a short network loss; SMB's locks then no longer
    /// hold against NFS or the file manager.
    #[serde(default)]
    pub durable_handles: bool,
    /// Names with characters Windows does not allow, and Apple's
    /// extensions; needed for Time Machine.
    #[serde(default = "yes")]
    pub mac_compat: bool,
    /// Symbolic links inside a share are followed; never out of it.
    #[serde(default = "yes")]
    pub follow_symlinks: bool,
    /// Names kept out of the shares, e.g. `/*.tmp/Thumbs.db/`.
    #[serde(default)]
    pub veto_files: Option<String>,
    /// Folders holding only such files can be deleted.
    #[serde(default)]
    pub delete_veto_files: bool,
    /// Windows finds the NAS (WS-Discovery).
    #[serde(default = "yes")]
    pub wsdd: bool,
    /// Macs find the NAS and its Time Machine share (Bonjour).
    #[serde(default = "yes")]
    pub bonjour: bool,
    #[serde(default)]
    pub time_machine: Option<TimeMachineShare>,
}

impl From<sharing::FileServices> for FileServices {
    fn from(services: sharing::FileServices) -> Self {
        Self {
            smb: services.smb,
            nfs: services.nfs,
            workgroup: services.workgroup,
            smb_min_protocol: match services.smb_min_protocol {
                sharing::SmbProtocol::Smb2 => SmbMinProtocol::Smb2,
                sharing::SmbProtocol::Smb2_1 => SmbMinProtocol::Smb2_1,
                sharing::SmbProtocol::Smb3 => SmbMinProtocol::Smb3,
            },
            smb_encryption: match services.smb_encryption {
                sharing::SmbEncryption::Default => SmbEncryptionMode::Default,
                sharing::SmbEncryption::Required => SmbEncryptionMode::Required,
                sharing::SmbEncryption::Off => SmbEncryptionMode::Off,
            },
            smb_signing: match services.smb_signing {
                sharing::SmbSigning::Default => SmbSigningMode::Default,
                sharing::SmbSigning::Mandatory => SmbSigningMode::Mandatory,
                sharing::SmbSigning::Disabled => SmbSigningMode::Disabled,
            },
            hide_unreadable: services.hide_unreadable,
            multichannel: services.multichannel,
            oplocks: services.oplocks,
            leases: services.leases,
            durable_handles: services.durable_handles,
            mac_compat: services.mac_compat,
            follow_symlinks: services.follow_symlinks,
            veto_files: services.veto_files,
            delete_veto_files: services.delete_veto_files,
            wsdd: services.wsdd,
            bonjour: services.bonjour,
            time_machine: services.time_machine.map(|machine| TimeMachineShare {
                share_id: machine.share.get(),
                max_bytes: machine.max_bytes,
            }),
        }
    }
}

impl From<FileServices> for sharing::FileServices {
    fn from(request: FileServices) -> Self {
        Self {
            smb: request.smb,
            nfs: request.nfs,
            workgroup: request.workgroup,
            smb_min_protocol: match request.smb_min_protocol {
                SmbMinProtocol::Smb2 => sharing::SmbProtocol::Smb2,
                SmbMinProtocol::Smb2_1 => sharing::SmbProtocol::Smb2_1,
                SmbMinProtocol::Smb3 => sharing::SmbProtocol::Smb3,
            },
            smb_encryption: match request.smb_encryption {
                SmbEncryptionMode::Default => sharing::SmbEncryption::Default,
                SmbEncryptionMode::Required => sharing::SmbEncryption::Required,
                SmbEncryptionMode::Off => sharing::SmbEncryption::Off,
            },
            smb_signing: match request.smb_signing {
                SmbSigningMode::Default => sharing::SmbSigning::Default,
                SmbSigningMode::Mandatory => sharing::SmbSigning::Mandatory,
                SmbSigningMode::Disabled => sharing::SmbSigning::Disabled,
            },
            hide_unreadable: request.hide_unreadable,
            multichannel: request.multichannel,
            oplocks: request.oplocks,
            leases: request.leases,
            durable_handles: request.durable_handles,
            mac_compat: request.mac_compat,
            follow_symlinks: request.follow_symlinks,
            veto_files: request.veto_files,
            delete_veto_files: request.delete_veto_files,
            wsdd: request.wsdd,
            bonjour: request.bonjour,
            time_machine: request.time_machine.map(|machine| sharing::TimeMachine {
                share: ShareId::new(machine.share_id),
                max_bytes: machine.max_bytes,
            }),
        }
    }
}

/// Which file services run. Needs `system.view`.
#[utoipa::path(
    get,
    path = "/file-services",
    tag = "shares",
    security(("session" = [])),
    responses(
        (status = OK, body = FileServices),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn file_services(auth: Auth, State(state): State<ApiState>) -> Result<Json<FileServices>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(state.sharing.file_services().await?.into()))
}

/// Turns SMB and NFS on or off and sets the workgroup and the oldest SMB
/// version; applied with the shares at once. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/file-services",
    tag = "shares",
    security(("session" = [])),
    request_body = FileServices,
    responses(
        (status = OK, body = FileServices, description = "As stored: the workgroup uppercase."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`refused`: the host did not accept it; the previous settings apply"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: the Time Machine share"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn set_file_services(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<FileServices>,
) -> Result<Json<FileServices>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let services = request.into();
    let stored = state.sharing.set_file_services(services, &auth.0.actor(), &client).await?;
    Ok(Json(stored.into()))
}

/// One share as an account or group sees it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubjectShareAccess {
    pub share_id: i64,
    pub share: String,
    /// The account's or group's own rule; absent without one.
    pub rule: Option<ShareRuleLevel>,
    /// What it may do in the end. For an account: absent if any rule
    /// denies it (its own or a group's), else its own rule, else the
    /// broadest of its groups'. For a group: what its rule grants.
    pub effective: Option<ShareAccessLevel>,
}

fn subject(kind: SubjectKind, id: i64) -> sharing::Subject {
    match kind {
        SubjectKind::Account => sharing::Subject::Account(id),
        SubjectKind::Group => sharing::Subject::Group(id),
    }
}

/// Every share with an account's or group's rule and what it may do.
/// Needs `shares.manage`.
#[utoipa::path(
    get,
    path = "/shares/access/{kind}/{id}",
    tag = "shares",
    security(("session" = [])),
    params(("kind" = SubjectKind, Path), ("id" = i64, Path)),
    responses(
        (status = OK, body = Vec<SubjectShareAccess>),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`unknown_subject`"),
    )
)]
async fn subject_access(
    auth: Auth,
    State(state): State<ApiState>,
    Path((kind, id)): Path<(SubjectKind, i64)>,
) -> Result<Json<Vec<SubjectShareAccess>>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let seen = state.sharing.subject_access(subject(kind, id)).await?;
    Ok(Json(
        seen.into_iter()
            .map(|entry| SubjectShareAccess {
                share_id: entry.share.get(),
                share: entry.name.to_string(),
                rule: entry.rule.map(Into::into),
                effective: entry.effective.map(Into::into),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectRule {
    pub share_id: i64,
    /// Absent or null: no rule.
    #[serde(default)]
    pub rule: Option<ShareRuleLevel>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSubjectAccessRequest {
    /// The shares to change; others keep their rules.
    pub rules: Vec<SubjectRule>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubjectAccessUpdate {
    /// One per share that changed: its existing files follow.
    pub jobs: Vec<Job>,
}

/// Sets an account's or group's rules on several shares at once. Each
/// share that changes gets its permissions at once and its existing files
/// in a job. Needs `shares.manage`.
#[utoipa::path(
    put,
    path = "/shares/access/{kind}/{id}",
    tag = "shares",
    security(("session" = [])),
    params(("kind" = SubjectKind, Path), ("id" = i64, Path)),
    request_body = SetSubjectAccessRequest,
    responses(
        (status = OK, body = SubjectAccessUpdate),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: a share"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`unknown_subject`, `duplicate_rule`"),
    )
)]
async fn set_subject_access(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((kind, id)): Path<(SubjectKind, i64)>,
    ApiJson(request): ApiJson<SetSubjectAccessRequest>,
) -> Result<Json<SubjectAccessUpdate>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let rules: Vec<(ShareId, Option<sharing::Permission>)> = request
        .rules
        .into_iter()
        .map(|rule| (ShareId::new(rule.share_id), rule.rule.map(Into::into)))
        .collect();
    let jobs = state.sharing.set_subject_access(subject(kind, id), &rules, &auth.0.actor(), &client).await?;
    Ok(Json(SubjectAccessUpdate { jobs: jobs.into_iter().map(Job::from).collect() }))
}

/// Where the personal folders are.
#[derive(Debug, Serialize, ToSchema)]
pub struct HomesLocation {
    pub pool: String,
    pub volume: u16,
}

/// An account with a personal folder.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccountHome {
    pub account_id: i64,
    /// How much it may store there; absent: as much as the pool has.
    pub quota_bytes: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Homes {
    /// Absent while personal folders are off.
    pub location: Option<HomesLocation>,
    pub homes: Vec<AccountHome>,
}

impl From<zendiloos_sharing::application::HomesSettings> for Homes {
    fn from(settings: zendiloos_sharing::application::HomesSettings) -> Self {
        Self {
            location: settings.location.map(|l| HomesLocation { pool: l.pool, volume: l.volume }),
            homes: settings
                .homes
                .into_iter()
                .map(|h| AccountHome { account_id: h.account, quota_bytes: h.quota_bytes })
                .collect(),
        }
    }
}

/// Where the personal folders are and who has one. Needs `users.manage`.
#[utoipa::path(
    get,
    path = "/homes",
    tag = "shares",
    security(("session" = [])),
    responses((status = OK, body = Homes), (status = FORBIDDEN, body = Problem, description = "`forbidden`"))
)]
async fn homes(auth: Auth, State(state): State<ApiState>) -> Result<Json<Homes>, ApiError> {
    auth.require(Permission::UsersManage)?;
    Ok(Json(state.sharing.homes_settings().await?.into()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetHomesRequest {
    /// The pool to keep the personal folders on; null turns them off,
    /// keeping their files.
    pub pool: Option<String>,
}

/// Turns personal folders on, on a pool, or off. Needs `users.manage`.
#[utoipa::path(
    put,
    path = "/homes",
    tag = "shares",
    security(("session" = [])),
    request_body = SetHomesRequest,
    responses(
        (status = OK, body = Homes),
        (status = CONFLICT, body = Problem, description = "`refused`: e.g. a share is called home"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`volume_not_found`"),
    )
)]
async fn set_homes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<SetHomesRequest>,
) -> Result<Json<Homes>, ApiError> {
    auth.require(Permission::UsersManage)?;
    let actor = auth.0.actor();
    match request.pool {
        Some(pool) => Ok(Json(state.sharing.enable_homes(&pool, &actor, &client).await?.into())),
        None => {
            state.sharing.disable_homes(&actor, &client).await?;
            Ok(Json(state.sharing.homes_settings().await?.into()))
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAccountHomeRequest {
    pub enabled: bool,
    /// At least 1 GiB; absent: no limit.
    #[serde(default)]
    pub quota_bytes: Option<u64>,
}

/// Gives an account a personal folder, changes its quota or takes it away
/// (its files stay). Needs `users.manage`.
#[utoipa::path(
    put,
    path = "/homes/{account_id}",
    tag = "shares",
    security(("session" = [])),
    params(("account_id" = i64, Path)),
    request_body = SetAccountHomeRequest,
    responses(
        (status = OK, body = Option<AccountHome>),
        (status = CONFLICT, body = Problem, description = "`refused`: personal folders are off"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`unknown_subject`, `invalid_input`"),
    )
)]
async fn set_account_home(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(account_id): Path<i64>,
    ApiJson(request): ApiJson<SetAccountHomeRequest>,
) -> Result<Json<Option<AccountHome>>, ApiError> {
    auth.require(Permission::UsersManage)?;
    let home = state
        .sharing
        .set_account_home(account_id, request.enabled, request.quota_bytes, &auth.0.actor(), &client)
        .await?;
    Ok(Json(home.map(|h| AccountHome { account_id: h.account, quota_bytes: h.quota_bytes })))
}
