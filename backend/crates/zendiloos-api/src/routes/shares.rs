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
    pub access: Vec<ShareRule>,
    /// Empty: not exported over NFS.
    pub nfs: Vec<NfsRule>,
    pub created_at: i64,
    pub updated_at: i64,
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
async fn list_shares(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<Share>>, ApiError> {
    auth.require(Permission::SharesManage)?;
    let shares = state.sharing.list().await?;
    Ok(Json(shares.into_iter().map(Share::from).collect()))
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
