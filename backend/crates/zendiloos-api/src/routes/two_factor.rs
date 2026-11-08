//! Two-factor sign-in to the web interface: the second step of a sign-in,
//! the signed-in account's authenticator app (set up, turned off, new
//! recovery codes) and an administrator's reset of another account's.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::AccountId;
use zendiloos_kernel::Secret;

use super::identity::{SessionInfo, signed_in};
use crate::auth::{Auth, SameOrigin};
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(complete_sign_in))
        .routes(routes!(status))
        .routes(routes!(begin))
        .routes(routes!(confirm))
        .routes(routes!(disable))
        .routes(routes!(renew_recovery_codes))
        .routes(routes!(reset))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SecondFactorRequest {
    /// The `token` the password step gave.
    #[schema(format = Password)]
    pub token: String,
    /// Six digits from the authenticator app, or a recovery code.
    #[schema(format = Password)]
    pub code: String,
}

/// The second step of a sign-in: the code from the authenticator app, or a
/// recovery code. From the address the password came from, within five
/// minutes; three wrong codes end the sign-in.
#[utoipa::path(
    post,
    path = "/auth/login/second-factor",
    tag = "identity",
    request_body = SecondFactorRequest,
    responses(
        (status = OK, body = SessionInfo, description = "Signed in; the session cookie is set."),
        (status = UNAUTHORIZED, body = Problem, description = "`sign_in_expired`: sign in with the password again"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_code`"),
        (status = FORBIDDEN, body = Problem, description = "`account_disabled`, `csrf_rejected`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn complete_sign_in(
    State(state): State<ApiState>,
    _: SameOrigin,
    Client(client): Client,
    jar: CookieJar,
    ApiJson(request): ApiJson<SecondFactorRequest>,
) -> Result<(CookieJar, Json<SessionInfo>), ApiError> {
    let (token, code) = (Secret::new(request.token), Secret::new(request.code));
    let session = state.identity.complete_sign_in.execute(&token, &code, &client).await?;
    Ok(signed_in(jar, &session, &state))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TwoFactorStatus {
    /// Sign-ins of this account ask for a code from its authenticator app.
    pub enabled: bool,
    /// Recovery codes not used yet.
    pub recovery_codes_left: usize,
}

/// Whether the signed-in account's sign-ins ask for a second factor.
#[utoipa::path(
    get,
    path = "/auth/two-factor",
    tag = "identity",
    security(("session" = [])),
    responses((status = OK, body = TwoFactorStatus))
)]
async fn status(auth: Auth, State(state): State<ApiState>) -> Result<Json<TwoFactorStatus>, ApiError> {
    let status = state.identity.two_factor.execute(&auth.0).await?;
    Ok(Json(TwoFactorStatus { enabled: status.on, recovery_codes_left: status.recovery_codes_left }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TwoFactorSetup {
    /// The secret in base32, for typing into the app.
    pub secret: String,
    /// The `otpauth://` address, for the QR code.
    pub uri: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TwoFactorSetupRequest {
    /// The account's password: a session alone does not turn this on.
    #[schema(format = Password)]
    pub password: String,
}

/// Begins setting up the signed-in account's authenticator app, with its
/// password: a new secret, which waits for its first code. One begun
/// before is replaced. A wrong password counts as a failed login.
#[utoipa::path(
    post,
    path = "/auth/two-factor/setup",
    tag = "identity",
    security(("session" = [])),
    request_body = TwoFactorSetupRequest,
    responses(
        (status = OK, body = TwoFactorSetup),
        (status = CONFLICT, body = Problem, description = "`two_factor_on`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`wrong_password`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn begin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<TwoFactorSetupRequest>,
) -> Result<Json<TwoFactorSetup>, ApiError> {
    let password = Secret::new(request.password);
    let setup = state.identity.begin_two_factor.execute(&auth.0, &password, &client).await?;
    Ok(Json(TwoFactorSetup { secret: setup.secret.expose().to_owned(), uri: setup.uri.expose().to_owned() }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TwoFactorCodeRequest {
    /// Six digits from the authenticator app.
    #[schema(format = Password)]
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RecoveryCodes {
    /// Each signs in once instead of a code from the app. Shown this once.
    pub recovery_codes: Vec<String>,
}

/// Turns the second factor on with the first code from the app. Ends the
/// account's other sessions.
#[utoipa::path(
    post,
    path = "/auth/two-factor/confirm",
    tag = "identity",
    security(("session" = [])),
    request_body = TwoFactorCodeRequest,
    responses(
        (status = OK, body = RecoveryCodes),
        (status = CONFLICT, body = Problem, description = "`no_two_factor_setup`, `two_factor_on`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_code`"),
    )
)]
async fn confirm(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<TwoFactorCodeRequest>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    let code = Secret::new(request.code);
    let recovery_codes = state.identity.confirm_two_factor.execute(&auth.0, &code, &client).await?;
    Ok(Json(RecoveryCodes { recovery_codes }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DisableTwoFactorRequest {
    #[schema(format = Password)]
    pub password: String,
    /// Six digits from the app, or a recovery code.
    #[schema(format = Password)]
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenewRecoveryCodesRequest {
    #[schema(format = Password)]
    pub password: String,
    /// Six digits from the authenticator app; a recovery code does not do.
    #[schema(format = Password)]
    pub code: String,
}

/// Turns the signed-in account's second factor off. Ends its other sessions.
#[utoipa::path(
    post,
    path = "/auth/two-factor/disable",
    tag = "identity",
    security(("session" = [])),
    request_body = DisableTwoFactorRequest,
    responses(
        (status = NO_CONTENT, description = "Off: the password alone signs in."),
        (status = CONFLICT, body = Problem, description = "`two_factor_off`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`wrong_password`, `invalid_code`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn disable(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<DisableTwoFactorRequest>,
) -> Result<StatusCode, ApiError> {
    let (password, code) = (Secret::new(request.password), Secret::new(request.code));
    state.identity.disable_two_factor.execute(&auth.0, &password, &code, &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// New recovery codes for the password and a code from the app; the old
/// ones stop working. Ends the account's other sessions.
#[utoipa::path(
    post,
    path = "/auth/two-factor/recovery-codes",
    tag = "identity",
    security(("session" = [])),
    request_body = RenewRecoveryCodesRequest,
    responses(
        (status = OK, body = RecoveryCodes),
        (status = CONFLICT, body = Problem, description = "`two_factor_off`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`wrong_password`, `invalid_code`"),
        (status = TOO_MANY_REQUESTS, body = Problem, description = "`too_many_attempts`"),
    )
)]
async fn renew_recovery_codes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<RenewRecoveryCodesRequest>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    let (password, code) = (Secret::new(request.password), Secret::new(request.code));
    let recovery_codes =
        state.identity.renew_recovery_codes.execute(&auth.0, &password, &code, &client).await?;
    Ok(Json(RecoveryCodes { recovery_codes }))
}

/// Turns another account's second factor off, for someone who lost their
/// phone and their recovery codes. Ends that account's sessions.
#[utoipa::path(
    delete,
    path = "/accounts/{id}/two-factor",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Off: the password alone signs in."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`own_account`, `two_factor_off`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
    )
)]
async fn reset(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.identity.reset_two_factor.execute(&auth.0, AccountId::new(id), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
