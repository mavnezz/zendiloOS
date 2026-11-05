//! Long-running operations and their progress (`system.view`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_jobs::{self as jobs, JobId};
use zendiloos_notify as notify;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiQuery};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_jobs))
        .routes(routes!(job))
        .routes(routes!(notifications))
        .routes(routes!(mark_notifications_read))
        .routes(routes!(clear_notifications))
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    /// zendiloOS restarted while it ran; check the result yourself.
    Interrupted,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Job {
    pub id: i64,
    /// Dotted name, e.g. `storage.create_pool`.
    pub kind: String,
    /// What it works on, e.g. a pool name.
    pub target: Option<String>,
    pub state: JobState,
    /// Current step as a stable code, e.g. `partitioning`.
    pub step: Option<String>,
    /// 0 to 1, if known.
    pub progress: Option<f32>,
    /// Stable code of the failure, e.g. `disk_changed`.
    pub error_code: Option<String>,
    /// Details for administrators, e.g. a tool's message.
    pub error_detail: Option<String>,
    /// Who started it.
    pub actor: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

impl From<jobs::Job> for Job {
    fn from(job: jobs::Job) -> Self {
        let (error_code, error_detail) = match job.error {
            Some(error) => (Some(error.code), error.detail),
            None => (None, None),
        };
        Self {
            id: job.id.get(),
            kind: job.kind,
            target: job.target,
            state: match job.state {
                jobs::JobState::Running => JobState::Running,
                jobs::JobState::Succeeded => JobState::Succeeded,
                jobs::JobState::Failed => JobState::Failed,
                jobs::JobState::Interrupted => JobState::Interrupted,
            },
            step: job.step,
            progress: job.progress,
            error_code,
            error_detail,
            actor: job.actor,
            created_at: job.created_at.unix_seconds(),
            updated_at: job.updated_at.unix_seconds(),
            finished_at: job.finished_at.map(|t| t.unix_seconds()),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct JobsParams {
    /// At most 200; 50 if absent.
    pub limit: Option<u32>,
}

/// The most recent jobs, newest first. Changes arrive on the WebSocket
/// topic `jobs`.
#[utoipa::path(
    get,
    path = "/jobs",
    tag = "jobs",
    security(("session" = [])),
    params(JobsParams),
    responses(
        (status = OK, body = Vec<Job>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn list_jobs(
    auth: Auth,
    State(state): State<ApiState>,
    ApiQuery(params): ApiQuery<JobsParams>,
) -> Result<Json<Vec<Job>>, ApiError> {
    auth.require(Permission::SystemView)?;
    let jobs = state.jobs.recent(params.limit.unwrap_or(50)).await?;
    Ok(Json(jobs.into_iter().map(Job::from).collect()))
}

/// One job.
#[utoipa::path(
    get,
    path = "/jobs/{id}",
    tag = "jobs",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = OK, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn job(auth: Auth, State(state): State<ApiState>, Path(id): Path<i64>) -> Result<Json<Job>, ApiError> {
    auth.require(Permission::SystemView)?;
    let job = state.jobs.find(JobId::new(id)).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(job.into()))
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// Something an administrator should know, as a code with parameters for
/// the interface to word, e.g. `pool_trouble` with `pool` and `state`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Notification {
    pub id: i64,
    /// Unix seconds.
    pub at: i64,
    pub level: NotificationLevel,
    pub code: String,
    pub params: std::collections::BTreeMap<String, String>,
}

impl From<notify::Notification> for Notification {
    fn from(n: notify::Notification) -> Self {
        Self {
            id: n.id,
            at: n.at.unix_seconds(),
            level: match n.level {
                notify::Level::Info => NotificationLevel::Info,
                notify::Level::Success => NotificationLevel::Success,
                notify::Level::Warning => NotificationLevel::Warning,
                notify::Level::Error => NotificationLevel::Error,
            },
            code: n.code,
            params: n.params,
        }
    }
}

/// The caller's notifications and how far they have read them.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NotificationList {
    /// Newest first, without those the caller cleared away.
    pub notifications: Vec<Notification>,
    /// Notifications up to this ID count as read by the caller.
    pub read_up_to: i64,
}

/// The most recent notifications, newest first, as the signed-in account
/// left them: what it cleared away is gone, what it read is marked. New ones
/// arrive on the WebSocket topic `notifications`.
#[utoipa::path(
    get,
    path = "/notifications",
    tag = "jobs",
    security(("session" = [])),
    params(JobsParams),
    responses(
        (status = OK, body = NotificationList),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn notifications(
    auth: Auth,
    State(state): State<ApiState>,
    ApiQuery(params): ApiQuery<JobsParams>,
) -> Result<Json<NotificationList>, ApiError> {
    auth.require(Permission::SystemView)?;
    let account = auth.0.account.id.get();
    let (recent, marks) = state.notifier.for_account(account, params.limit.unwrap_or(50)).await?;
    Ok(Json(NotificationList {
        notifications: recent.into_iter().map(Notification::from).collect(),
        read_up_to: marks.read_up_to,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationMark {
    /// ID of the newest notification the mark covers.
    pub up_to: i64,
}

/// Marks the caller's notifications up to `up_to` as read, in every browser.
#[utoipa::path(
    post,
    path = "/notifications/read",
    tag = "jobs",
    security(("session" = [])),
    request_body = NotificationMark,
    responses(
        (status = NO_CONTENT, description = "Marked."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn mark_notifications_read(
    auth: Auth,
    State(state): State<ApiState>,
    ApiJson(mark): ApiJson<NotificationMark>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemView)?;
    state.notifier.mark_read(auth.0.account.id.get(), mark.up_to).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Clears the caller's notifications up to `up_to` away; other accounts
/// still see them.
#[utoipa::path(
    post,
    path = "/notifications/clear",
    tag = "jobs",
    security(("session" = [])),
    request_body = NotificationMark,
    responses(
        (status = NO_CONTENT, description = "Cleared."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn clear_notifications(
    auth: Auth,
    State(state): State<ApiState>,
    ApiJson(mark): ApiJson<NotificationMark>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemView)?;
    state.notifier.clear(auth.0.account.id.get(), mark.up_to).await?;
    Ok(StatusCode::NO_CONTENT)
}
