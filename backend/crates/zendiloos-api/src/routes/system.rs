use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use zendiloos_identity::domain::Permission;
use zendiloos_system::domain::PowerAction;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(health)).routes(routes!(info)).routes(routes!(power))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    /// `ok` whenever the server answers.
    pub status: String,
}

/// Liveness check for monitoring and the reverse proxy.
///
/// Public, so it deliberately reveals nothing about the system, not even the
/// software version.
#[utoipa::path(get, path = "/health", tag = "system", responses((status = OK, body = Health)))]
async fn health() -> Json<Health> {
    Json(Health { status: "ok".to_owned() })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemInfo {
    pub hostname: String,
    /// Operating system, e.g. `Debian GNU/Linux 13 (trixie)`.
    pub os_name: Option<String>,
    pub kernel_release: String,
    /// Machine architecture, e.g. `x86_64`.
    pub architecture: String,
    pub cpu_model: Option<String>,
    /// Logical CPUs (hardware threads).
    pub cpu_threads: usize,
    pub memory_total_bytes: u64,
    pub uptime_seconds: u64,
    /// Version of Zendiloos.
    pub version: String,
}

impl From<zendiloos_system::domain::SystemInfo> for SystemInfo {
    fn from(info: zendiloos_system::domain::SystemInfo) -> Self {
        Self {
            hostname: info.hostname,
            os_name: info.os_name,
            kernel_release: info.kernel_release,
            architecture: info.architecture,
            cpu_model: info.cpu_model,
            cpu_threads: info.cpu_threads,
            memory_total_bytes: info.memory_total_bytes,
            uptime_seconds: info.uptime_seconds,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Facts about the host and the installed Zendiloos version.
#[utoipa::path(
    get,
    path = "/system/info",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = OK, body = SystemInfo),
        (status = UNAUTHORIZED, body = Problem, description = "`unauthenticated`"),
        (status = INTERNAL_SERVER_ERROR, body = Problem, content_type = "application/problem+json"),
    )
)]
async fn info(auth: Auth, State(state): State<ApiState>) -> Result<Json<SystemInfo>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(state.system.get_info.execute().await?.into()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PowerActionKind {
    Reboot,
    PowerOff,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PowerRequest {
    pub action: PowerActionKind,
}

/// Reboots or powers off the system. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/system/power",
    tag = "system",
    security(("session" = [])),
    request_body = PowerRequest,
    responses(
        (status = ACCEPTED, description = "The action is under way."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
    )
)]
async fn power(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<PowerRequest>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    let action = match request.action {
        PowerActionKind::Reboot => PowerAction::Reboot,
        PowerActionKind::PowerOff => PowerAction::PowerOff,
    };
    state.system.request_power_action.execute(action, &auth.0.actor(), &client).await?;
    Ok(StatusCode::ACCEPTED)
}
