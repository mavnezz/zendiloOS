//! The configuration backup (docs/architecture.md, 4.15): downloaded by
//! hand, written on a schedule into a folder of a share or a drive, and
//! restored, also instead of the first setup.

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::Secret;
use zendiloos_recovery::application::{PlanChange, PlanView, RecoveryError, Restored};
use zendiloos_recovery::domain::{Frequency, KEEP_MAX, LastRun, PASSPHRASE_MIN, Schedule, Target};

use crate::auth::{Auth, SameOrigin};
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

/// The passphrase of a restore, as base64 of its UTF-8: the body is the
/// backup itself.
pub(crate) const PASSPHRASE_HEADER: &str = "x-zendiloos-passphrase";
/// The setup code, for a restore instead of the first setup.
pub(crate) const SETUP_CODE_HEADER: &str = "x-zendiloos-setup-code";
/// Larger than any backup of zendiloOS's configuration.
const MAX_BACKUP_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(download_configuration))
        .routes(routes!(configuration_plan, set_configuration_plan))
        .routes(routes!(run_configuration_backup))
        .routes(routes!(restore_configuration))
        .routes(routes!(restore_instead_of_setup))
}

impl From<RecoveryError> for ApiError {
    fn from(error: RecoveryError) -> Self {
        let rejected = |status, code, detail: Option<String>| ApiError::Rejected { status, code, detail };
        match error {
            RecoveryError::Passphrase(e) => ApiError::InvalidInput(e.to_string()),
            RecoveryError::Plan(e) => ApiError::InvalidInput(e.to_string()),
            RecoveryError::WrongPassphrase => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "wrong_passphrase", None)
            }
            RecoveryError::NotABackup(why) => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "not_a_backup", Some(why))
            }
            RecoveryError::UnsupportedFormat(format) => rejected(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_backup",
                Some(format!("layout {format}")),
            ),
            error @ RecoveryError::TooNew { .. } => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "backup_too_new", Some(error.to_string()))
            }
            RecoveryError::JobsRunning(named) => {
                rejected(StatusCode::CONFLICT, "jobs_running", Some(named.join(", ")))
            }
            RecoveryError::NotPlanned => rejected(StatusCode::CONFLICT, "not_planned", None),
            RecoveryError::Refused(why) => rejected(StatusCode::CONFLICT, "refused", Some(why)),
            RecoveryError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadRequest {
    /// At least 12 characters; nobody opens or restores the backup
    /// without it.
    #[schema(format = Password)]
    pub passphrase: String,
}

/// A backup of the configuration as it is now: the database, the
/// certificates, Samba's passwords, the accounts' Linux users and the
/// host's settings, encrypted with the passphrase (age, scrypt). Needs
/// `system.manage`.
#[utoipa::path(
    post,
    path = "/configuration/backup/download",
    tag = "configuration",
    security(("session" = [])),
    request_body = DownloadRequest,
    responses(
        (status = OK, content_type = "application/octet-stream", body = Vec<u8>, description = "The backup, `zendiloos-config-<name>-<time>.tar.zst.age`."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a passphrase too short"),
    )
)]
async fn download_configuration(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<DownloadRequest>,
) -> Result<Response, ApiError> {
    auth.require(Permission::SystemManage)?;
    let archive = state.recovery.download(&request.passphrase, &auth.0.actor(), &client).await?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", archive.name))
        .map_err(ApiError::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        archive.bytes,
    )
        .into_response())
}

/// When the backups are made.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationFrequency {
    Daily,
    Weekly,
}

/// Where the backups go.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigurationTarget {
    /// A folder of a share.
    Share { share_id: i64, folder: String },
    /// A folder of a removable drive, by its file system's UUID; a run
    /// while it is not plugged in fails and is told.
    Drive { drive: String, folder: String },
}

impl From<Target> for ConfigurationTarget {
    fn from(target: Target) -> Self {
        match target {
            Target::Share { share_id, folder } => Self::Share { share_id, folder },
            Target::Drive { drive, folder } => Self::Drive { drive, folder },
        }
    }
}

impl From<ConfigurationTarget> for Target {
    fn from(target: ConfigurationTarget) -> Self {
        match target {
            ConfigurationTarget::Share { share_id, folder } => Self::Share { share_id, folder },
            ConfigurationTarget::Drive { drive, folder } => Self::Drive { drive, folder },
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigurationLastRun {
    pub at: i64,
    pub succeeded: bool,
    pub file: Option<String>,
    pub error: Option<String>,
}

impl From<LastRun> for ConfigurationLastRun {
    fn from(run: LastRun) -> Self {
        Self { at: run.at.unix_seconds(), succeeded: run.succeeded, file: run.file, error: run.error }
    }
}

/// The backups made on their own; the passphrase is never sent back.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigurationPlan {
    pub enabled: bool,
    pub frequency: ConfigurationFrequency,
    /// 0 is Monday; weekly only.
    pub weekday: Option<u8>,
    /// The NAS's local time.
    pub hour: u8,
    pub minute: u8,
    pub target: Option<ConfigurationTarget>,
    /// How many are kept in the folder, the newest; 1 to 60.
    pub keep: u8,
    pub passphrase_set: bool,
    /// The administrator who set it up: the backups are written with that
    /// account's access to the folder.
    pub account: Option<String>,
    pub next_at: Option<i64>,
    pub last_run: Option<ConfigurationLastRun>,
    /// The fewest characters a passphrase has.
    pub passphrase_min: usize,
    pub keep_max: u8,
}

impl From<PlanView> for ConfigurationPlan {
    fn from(plan: PlanView) -> Self {
        let (frequency, weekday) = match plan.schedule.frequency {
            Frequency::Daily => (ConfigurationFrequency::Daily, None),
            Frequency::Weekly { weekday } => (ConfigurationFrequency::Weekly, Some(weekday)),
        };
        Self {
            enabled: plan.enabled,
            frequency,
            weekday,
            hour: plan.schedule.hour,
            minute: plan.schedule.minute,
            target: plan.target.map(Into::into),
            keep: plan.keep,
            passphrase_set: plan.passphrase_set,
            account: plan.account,
            next_at: plan.next_at.map(|at| at.unix_seconds()),
            last_run: plan.last.map(Into::into),
            passphrase_min: PASSPHRASE_MIN,
            keep_max: KEEP_MAX,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPlanRequest {
    pub enabled: bool,
    pub frequency: ConfigurationFrequency,
    #[serde(default)]
    pub weekday: Option<u8>,
    pub hour: u8,
    pub minute: u8,
    #[serde(default)]
    pub target: Option<ConfigurationTarget>,
    pub keep: u8,
    /// Left out or empty: the one kept stays.
    #[schema(format = Password)]
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// The plan of the backups made on their own. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/configuration/backup",
    tag = "configuration",
    security(("session" = [])),
    responses((status = OK, body = ConfigurationPlan), (status = FORBIDDEN, body = Problem, description = "`forbidden`"))
)]
async fn configuration_plan(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<ConfigurationPlan>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.recovery.plan().await?.into()))
}

/// Sets when and where the backups are made, how many are kept and the
/// passphrase; they are written as the administrator setting it. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/configuration/backup",
    tag = "configuration",
    security(("session" = [])),
    request_body = ConfigurationPlanRequest,
    responses(
        (status = OK, body = ConfigurationPlan),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a time, a folder, how many kept; on without a folder or passphrase"),
    )
)]
async fn set_configuration_plan(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<ConfigurationPlanRequest>,
) -> Result<Json<ConfigurationPlan>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let frequency = match request.frequency {
        ConfigurationFrequency::Daily => Frequency::Daily,
        ConfigurationFrequency::Weekly => Frequency::Weekly {
            weekday: request
                .weekday
                .ok_or_else(|| ApiError::InvalidInput("a weekly plan names its day".into()))?,
        },
    };
    let change = PlanChange {
        enabled: request.enabled,
        schedule: Schedule { frequency, hour: request.hour, minute: request.minute },
        target: request.target.map(Into::into),
        keep: request.keep,
        passphrase: request.passphrase,
    };
    Ok(Json(state.recovery.set_plan(change, &auth.0.actor(), &client).await?.into()))
}

/// Backs up into the plan's folder now, in a job. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/configuration/backup/run",
    tag = "configuration",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`not_planned`: no folder or passphrase yet"),
    )
)]
async fn run_configuration_backup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let job = state.recovery.run_now(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// What came of a restore; zendiloosd starts again with it a moment later.
#[derive(Debug, Serialize, ToSchema)]
pub struct RestoreResult {
    /// The name of the machine the backup was made of, which this one has now.
    pub host_name: String,
    /// When it was made, and by which version of zendiloOS.
    pub created_at: i64,
    pub version: String,
    /// What was not put back; the rest was.
    pub warnings: Vec<String>,
    /// The web interface's HTTPS port as the backup has it: where the
    /// interface is once zendiloosd started again.
    pub https_port: Option<u16>,
    /// The network's files in the backup. Not put in force: the network is
    /// set on its page, each change confirmed.
    pub network: Vec<String>,
}

impl From<Restored> for RestoreResult {
    fn from(restored: Restored) -> Self {
        Self {
            host_name: restored.manifest.host_name,
            created_at: restored.manifest.created_at.unix_seconds(),
            version: restored.manifest.version,
            warnings: restored.warnings,
            https_port: restored.https_port,
            network: restored.network,
        }
    }
}

/// The passphrase from its header, base64 of UTF-8.
fn passphrase(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(PASSPHRASE_HEADER)
        .ok_or_else(|| ApiError::InvalidInput("the passphrase is missing".into()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .map_err(|_| ApiError::InvalidInput("the passphrase is not base64".into()))?;
    String::from_utf8(bytes).map_err(|_| ApiError::InvalidInput("the passphrase is not UTF-8".into()))
}

/// The body, at most [`MAX_BACKUP_BYTES`].
async fn backup(body: Body) -> Result<Vec<u8>, ApiError> {
    let mut chunks = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| ApiError::BadRequest(format!("the upload did not complete: {e}")))?;
        if bytes.len() + chunk.len() > MAX_BACKUP_BYTES {
            return Err(ApiError::InvalidInput("larger than any backup of the configuration".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Restores a backup on this NAS: the body is the backup, the passphrase
/// in `X-Zendiloos-Passphrase` (base64 of its UTF-8). Checked first as a
/// whole: refused with the passphrase wrong, of another layout, or of a
/// database newer than this release knows (an older one is migrated).
/// The accounts' Linux users, the certificates, Samba's passwords and the
/// host's settings go back at once, the database at zendiloosd's start,
/// which follows; whoever is signed in signs in again with the accounts of
/// the backup. The network stays as it is. Needs `system.manage`; refused
/// while jobs run.
#[utoipa::path(
    post,
    path = "/configuration/restore",
    tag = "configuration",
    security(("session" = [])),
    params(("x-zendiloos-passphrase" = String, Header, description = "Base64 of the passphrase's UTF-8.")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = OK, body = RestoreResult),
        (status = CONFLICT, body = Problem, description = "`jobs_running`; `refused`: the host would not take it back (a name or ID taken), in its words"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`wrong_passphrase`, `not_a_backup`, `unsupported_backup`, `backup_too_new`, `invalid_input`"),
    )
)]
async fn restore_configuration(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<RestoreResult>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let passphrase = passphrase(&headers)?;
    let archive = backup(body).await?;
    let actor = auth.0.actor();
    let restored = state.recovery.restore(archive, &passphrase, Some(&actor), &client).await?;
    Ok(Json(restored.into()))
}

/// Restores a backup instead of creating the first administrator, as
/// `/configuration/restore` does; allowed as the setup is (the setup code
/// in `X-Zendiloos-Setup-Code`, or an open setup from the NAS's own
/// network). Afterwards the backup's accounts sign in.
#[utoipa::path(
    post,
    path = "/setup/restore",
    tag = "identity",
    params(
        ("x-zendiloos-passphrase" = String, Header, description = "Base64 of the passphrase's UTF-8."),
        ("x-zendiloos-setup-code" = Option<String>, Header, description = "The code on the console; none for an open setup."),
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = OK, body = RestoreResult),
        (status = FORBIDDEN, body = Problem, description = "`setup_not_local`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`setup_completed`, `jobs_running`, `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_setup_code`, `wrong_passphrase`, `not_a_backup`, `unsupported_backup`, `backup_too_new`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn restore_instead_of_setup(
    State(state): State<ApiState>,
    _: SameOrigin,
    Client(client): Client,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<RestoreResult>, ApiError> {
    let code = headers.get(SETUP_CODE_HEADER).and_then(|value| value.to_str().ok()).map(Secret::new);
    state.identity.restore_setup.allow(code, &client).await?;
    let passphrase = passphrase(&headers)?;
    let archive = backup(body).await?;
    let restored = state.recovery.restore(archive, &passphrase, None, &client).await?;
    if let Err(error) = state.identity.restore_setup.close().await {
        tracing::warn!(%error, "restore: the setup's code stays until the next start");
    }
    Ok(Json(restored.into()))
}
