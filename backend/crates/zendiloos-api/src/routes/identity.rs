//! Initial setup, login, logout, the current session and password changes.

use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::application::commands::{ChangePasswordInput, LoginInput, SetupInput};
use zendiloos_identity::application::{AuthContext, SignedIn};
use zendiloos_identity::domain::Role;
use zendiloos_kernel::Secret;

use crate::auth::{AuthAllowPending, SameOrigin, removal_cookie, session_cookie};
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(setup_status, complete_setup))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(session))
        .routes(routes!(change_password))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatus {
    /// No administrator exists yet; the interface must offer the setup.
    pub required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountRole {
    Admin,
    User,
    Readonly,
}

impl From<Role> for AccountRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Admin => AccountRole::Admin,
            Role::User => AccountRole::User,
            Role::ReadOnly => AccountRole::Readonly,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentAccount {
    pub id: i64,
    pub name: String,
    pub full_name: Option<String>,
    pub role: AccountRole,
    /// Everything this account may do, e.g. `system.view`. The interface
    /// uses it to show only what is allowed; the server checks anyway.
    pub permissions: Vec<String>,
    /// Nothing but changing the password is allowed until this is false.
    pub password_change_required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionInfo {
    pub account: CurrentAccount,
    /// Send in the `X-CSRF-Token` header with every request that changes
    /// something.
    pub csrf_token: String,
    /// When the session ends at the latest, as Unix time in seconds.
    pub expires_at: i64,
}

impl From<&AuthContext> for SessionInfo {
    fn from(auth: &AuthContext) -> Self {
        let account = &auth.account;
        Self {
            account: CurrentAccount {
                id: account.id.get(),
                name: account.name.to_string(),
                full_name: account.full_name.clone(),
                role: account.role.into(),
                permissions: account.role.permissions().map(|p| p.as_str().to_owned()).collect(),
                password_change_required: account.password_change_required,
            },
            csrf_token: auth.csrf_token.expose().to_owned(),
            expires_at: auth.expires_at.unix_seconds(),
        }
    }
}

fn signed_in(jar: CookieJar, signed_in: &SignedIn, state: &ApiState) -> (CookieJar, axum::Json<SessionInfo>) {
    (
        jar.add(session_cookie(signed_in, state.secure_cookies)),
        axum::Json(SessionInfo::from(&signed_in.session)),
    )
}

/// Whether the initial setup is still pending.
#[utoipa::path(get, path = "/setup", tag = "identity", responses((status = OK, body = SetupStatus)))]
async fn setup_status(State(state): State<ApiState>) -> Result<axum::Json<SetupStatus>, ApiError> {
    let status = state.identity.setup_status.execute().await?;
    Ok(axum::Json(SetupStatus { required: status.required }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    /// The code shown on the system's console, e.g. `ABCD-EFGH-JKLM`.
    #[schema(format = Password)]
    pub setup_code: String,
    /// Name of the first administrator account.
    pub name: String,
    #[schema(format = Password)]
    pub password: String,
}

/// Creates the first administrator and signs them in.
#[utoipa::path(
    post,
    path = "/setup",
    tag = "identity",
    request_body = SetupRequest,
    responses(
        (status = OK, body = SessionInfo, description = "Signed in; the session cookie is set."),
        (status = CONFLICT, body = Problem, description = "`setup_completed`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_setup_code`, `invalid_input`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn complete_setup(
    State(state): State<ApiState>,
    _: SameOrigin,
    Client(client): Client,
    jar: CookieJar,
    ApiJson(request): ApiJson<SetupRequest>,
) -> Result<(CookieJar, axum::Json<SessionInfo>), ApiError> {
    let input = SetupInput {
        setup_code: Secret::new(request.setup_code),
        name: request.name,
        password: Secret::new(request.password),
    };
    let session = state.identity.complete_setup.execute(input, &client).await?;
    Ok(signed_in(jar, &session, &state))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub name: String,
    #[schema(format = Password)]
    pub password: String,
}

/// Signs in with name and password.
#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "identity",
    request_body = LoginRequest,
    responses(
        (status = OK, body = SessionInfo, description = "Signed in; the session cookie is set."),
        (status = UNAUTHORIZED, body = Problem, description = "`invalid_credentials`"),
        (status = FORBIDDEN, body = Problem, description = "`account_disabled`, `csrf_rejected`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn login(
    State(state): State<ApiState>,
    _: SameOrigin,
    Client(client): Client,
    jar: CookieJar,
    ApiJson(request): ApiJson<LoginRequest>,
) -> Result<(CookieJar, axum::Json<SessionInfo>), ApiError> {
    let input = LoginInput { name: request.name, password: Secret::new(request.password) };
    let session = state.identity.login.execute(input, &client).await?;
    Ok(signed_in(jar, &session, &state))
}

/// Ends the current session.
#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "identity",
    security(("session" = [])),
    responses((status = NO_CONTENT, description = "Signed out; the session cookie is removed."))
)]
async fn logout(
    AuthAllowPending(auth): AuthAllowPending,
    State(state): State<ApiState>,
    Client(client): Client,
    jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    state.identity.logout.execute(&auth, &client).await?;
    Ok((StatusCode::NO_CONTENT, jar.add(removal_cookie(state.secure_cookies))))
}

/// The signed-in account and the CSRF token of this session.
#[utoipa::path(
    get,
    path = "/auth/session",
    tag = "identity",
    security(("session" = [])),
    responses(
        (status = OK, body = SessionInfo),
        (status = UNAUTHORIZED, body = Problem, description = "`unauthenticated`"),
    )
)]
async fn session(AuthAllowPending(auth): AuthAllowPending) -> axum::Json<SessionInfo> {
    axum::Json(SessionInfo::from(&auth))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangePasswordRequest {
    #[schema(format = Password)]
    pub current_password: String,
    #[schema(format = Password)]
    pub new_password: String,
}

/// Changes the caller's password and ends their other sessions.
#[utoipa::path(
    post,
    path = "/auth/password",
    tag = "identity",
    security(("session" = [])),
    request_body = ChangePasswordRequest,
    responses(
        (status = NO_CONTENT, description = "Changed."),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`wrong_password`, `invalid_input`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn change_password(
    AuthAllowPending(auth): AuthAllowPending,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let input = ChangePasswordInput {
        current_password: Secret::new(request.current_password),
        new_password: Secret::new(request.new_password),
    };
    state.identity.change_password.execute(&auth, input, &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
