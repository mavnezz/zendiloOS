//! How the web interface defends itself: idle sessions, locked accounts,
//! blocked addresses, and the sessions of every account (`system.manage`).

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::application::queries::ActiveSession as Active;
use zendiloos_identity::domain::{self as identity, AddressRange, SessionId};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(settings, set_settings))
        .routes(routes!(blocked, block))
        .routes(routes!(unblock))
        .routes(routes!(sessions))
        .routes(routes!(end_session))
}

/// So many failed logins within so many minutes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureLimit {
    /// 3 to 100.
    pub failures: u16,
    /// 1 to 1440.
    pub window_minutes: u16,
}

/// Addresses that fail too often are blocked.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AutoBlock {
    /// 3 to 100.
    pub failures: u16,
    /// 1 to 1440.
    pub window_minutes: u16,
    /// 1 to 365; `null`: until lifted by hand.
    pub release_after_days: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SecuritySettings {
    /// A session ends after this many minutes without requests: 5 to 1440.
    pub idle_minutes: u16,
    /// `null`: accounts never lock.
    pub account_lock: Option<FailureLimit>,
    /// `null`: no address is blocked automatically.
    pub auto_block: Option<AutoBlock>,
    /// Addresses and networks never blocked, such as `192.168.1.0/24`; at
    /// most 64. This machine itself never is.
    #[serde(default)]
    pub allowed: Vec<String>,
}

impl From<identity::SecuritySettings> for SecuritySettings {
    fn from(settings: identity::SecuritySettings) -> Self {
        Self {
            idle_minutes: settings.idle_minutes,
            account_lock: settings
                .account_lock
                .map(|l| FailureLimit { failures: l.failures, window_minutes: l.window_minutes }),
            auto_block: settings.auto_block.map(|b| AutoBlock {
                failures: b.limit.failures,
                window_minutes: b.limit.window_minutes,
                release_after_days: b.release_after_days,
            }),
            allowed: settings.allowed.iter().map(ToString::to_string).collect(),
        }
    }
}

/// An address or network the web interface refuses.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlockedAddress {
    /// `192.0.2.7` or `198.51.100.0/24`.
    pub address: String,
    /// Unix seconds.
    pub blocked_at: i64,
    /// Unix seconds; `null`: until lifted by hand.
    pub until: Option<i64>,
    /// Blocked after failed logins, not by an administrator.
    pub automatic: bool,
}

impl From<identity::BlockedAddress> for BlockedAddress {
    fn from(block: identity::BlockedAddress) -> Self {
        Self {
            address: block.range.to_string(),
            blocked_at: block.blocked_at.unix_seconds(),
            until: block.until.map(|t| t.unix_seconds()),
            automatic: block.automatic,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressRequest {
    /// An IP address or a network such as `198.51.100.0/24`.
    pub address: String,
}

/// A session of some account that has not ended.
#[derive(Debug, Serialize, ToSchema)]
pub struct ActiveSession {
    pub id: i64,
    pub account: String,
    pub address: Option<String>,
    pub user_agent: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub last_seen_at: i64,
    /// The caller's own.
    pub current: bool,
}

impl From<Active> for ActiveSession {
    fn from(session: Active) -> Self {
        Self {
            id: session.id.get(),
            account: session.account.to_string(),
            address: session.client_ip.map(|ip| ip.to_canonical().to_string()),
            user_agent: session.user_agent,
            created_at: session.created_at.unix_seconds(),
            last_seen_at: session.last_seen_at.unix_seconds(),
            current: session.current,
        }
    }
}

/// When sessions end, when accounts lock and addresses are blocked. Needs
/// `system.manage`.
#[utoipa::path(
    get,
    path = "/security/settings",
    tag = "security",
    security(("session" = [])),
    responses(
        (status = OK, body = SecuritySettings),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn settings(auth: Auth, State(state): State<ApiState>) -> Result<Json<SecuritySettings>, ApiError> {
    Ok(Json(state.identity.security.execute(&auth.0).await?.into()))
}

/// Sets them; counts of failed attempts start anew where a limit changed,
/// blocks in place stay. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/security/settings",
    tag = "security",
    security(("session" = [])),
    request_body = SecuritySettings,
    responses(
        (status = OK, body = SecuritySettings),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn set_settings(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<SecuritySettings>,
) -> Result<Json<SecuritySettings>, ApiError> {
    let allowed = request
        .allowed
        .iter()
        .map(|text| AddressRange::parse(text))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::InvalidInput(e.to_string()))?;
    let settings = identity::SecuritySettings {
        idle_minutes: request.idle_minutes,
        account_lock: request
            .account_lock
            .map(|l| identity::FailureLimit { failures: l.failures, window_minutes: l.window_minutes }),
        auto_block: request.auto_block.map(|b| identity::AutoBlock {
            limit: identity::FailureLimit { failures: b.failures, window_minutes: b.window_minutes },
            release_after_days: b.release_after_days,
        }),
        allowed,
    };
    Ok(Json(state.identity.set_security.execute(&auth.0, settings, &client).await?.into()))
}

/// The blocks in force, the newest first. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/security/blocked",
    tag = "security",
    security(("session" = [])),
    responses(
        (status = OK, body = [BlockedAddress]),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn blocked(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<BlockedAddress>>, ApiError> {
    let blocked = state.identity.blocked_addresses.execute(&auth.0).await?;
    Ok(Json(blocked.into_iter().map(Into::into).collect()))
}

/// Blocks an address or network until the block is lifted. Needs
/// `system.manage`.
#[utoipa::path(
    post,
    path = "/security/blocked",
    tag = "security",
    security(("session" = [])),
    request_body = AddressRequest,
    responses(
        (status = CREATED, body = BlockedAddress),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`own_address`: the caller would be blocked; `address_allowed`: it is never blocked"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn block(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<AddressRequest>,
) -> Result<(StatusCode, Json<BlockedAddress>), ApiError> {
    let block = state.identity.block_address.execute(&auth.0, &request.address, &client).await?;
    Ok((StatusCode::CREATED, Json(block.into())))
}

/// Lifts a block. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/security/unblock",
    tag = "security",
    security(("session" = [])),
    request_body = AddressRequest,
    responses(
        (status = NO_CONTENT, description = "Lifted."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: it is not blocked"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn unblock(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<AddressRequest>,
) -> Result<StatusCode, ApiError> {
    state.identity.unblock_address.execute(&auth.0, &request.address, &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The sessions of every account that have not ended, the most recently
/// active first. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/security/sessions",
    tag = "security",
    security(("session" = [])),
    responses(
        (status = OK, body = [ActiveSession]),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn sessions(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<ActiveSession>>, ApiError> {
    let sessions = state.identity.sessions.execute(&auth.0).await?;
    Ok(Json(sessions.into_iter().map(Into::into).collect()))
}

/// Ends a session; its browser is signed out with its next request. Needs
/// `system.manage`.
#[utoipa::path(
    delete,
    path = "/security/sessions/{id}",
    tag = "security",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Ended."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn end_session(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.identity.end_session.execute(&auth.0, SessionId::new(id), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
