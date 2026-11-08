//! Initial setup, login, logout, the current session, password changes and
//! the account's interface preferences.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::application::commands::{ChangePasswordInput, LoginInput, SetupInput};
use zendiloos_identity::application::{AuthContext, SignedIn};
use zendiloos_identity::domain::{Language, Layout, Preferences, Role, Theme, Wallpaper};
use zendiloos_kernel::Secret;

use crate::auth::{Auth, AuthAllowPending, SameOrigin, removal_cookie, session_cookie};
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
        .routes(routes!(preferences, save_preferences))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatus {
    /// No administrator exists yet; the interface must offer the setup.
    pub required: bool,
    /// The setup asks for the code shown on the console. Not for an open
    /// setup (a machine installed from the ISO), which is completed from the
    /// NAS's own network without one.
    pub code_required: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    /// Only administrators set this account's password.
    pub password_locked: bool,
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
                password_locked: account.password_locked,
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
    Ok(axum::Json(SetupStatus { required: status.required, code_required: status.code_required }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    /// The code shown on the system's console, e.g. `ABCD-EFGH-JKLM`; left
    /// out for an open setup (`code_required` false).
    #[schema(format = Password)]
    #[serde(default)]
    pub setup_code: Option<String>,
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
        (status = FORBIDDEN, body = Problem, description = "`setup_not_local`: an open setup from outside the NAS's own network"),
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
        setup_code: request.setup_code.map(Secret::new),
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

/// A language of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceLanguage {
    En,
    De,
}

/// Light or dark, or as the viewer's operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceTheme {
    Light,
    Dark,
    System,
}

/// How the signed-in account wants the interface. `null` means not chosen:
/// each browser keeps what it shows.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InterfacePreferences {
    #[serde(default)]
    pub language: Option<InterfaceLanguage>,
    #[serde(default)]
    pub theme: Option<InterfaceTheme>,
    /// The desktop's background, by the interface's name for it (lowercase
    /// letters).
    #[serde(default)]
    pub wallpaper: Option<String>,
    /// The widgets beside the desktop, in order, by id; empty for none.
    #[serde(default)]
    pub widgets: Option<Vec<String>>,
    /// The app icons on the desktop, in order, by id.
    #[serde(default)]
    pub desktop_icons: Option<Vec<String>>,
}

impl From<Preferences> for InterfacePreferences {
    fn from(preferences: Preferences) -> Self {
        Self {
            language: preferences.language.map(|language| match language {
                Language::English => InterfaceLanguage::En,
                Language::German => InterfaceLanguage::De,
            }),
            theme: preferences.theme.map(|theme| match theme {
                Theme::Light => InterfaceTheme::Light,
                Theme::Dark => InterfaceTheme::Dark,
                Theme::System => InterfaceTheme::System,
            }),
            wallpaper: preferences.wallpaper.map(|wallpaper| wallpaper.as_str().to_owned()),
            widgets: preferences.widgets.map(|layout| layout.ids().to_vec()),
            desktop_icons: preferences.desktop_icons.map(|layout| layout.ids().to_vec()),
        }
    }
}

impl TryFrom<InterfacePreferences> for Preferences {
    type Error = ApiError;

    fn try_from(preferences: InterfacePreferences) -> Result<Self, Self::Error> {
        let invalid = |what: &str| ApiError::InvalidInput(format!("not a valid {what}"));
        let layout = |ids: Option<Vec<String>>, what: &str| {
            ids.map(|ids| Layout::new(ids).ok_or_else(|| invalid(what))).transpose()
        };
        Ok(Self {
            wallpaper: preferences
                .wallpaper
                .map(|name| Wallpaper::parse(&name).ok_or_else(|| invalid("wallpaper")))
                .transpose()?,
            widgets: layout(preferences.widgets, "widget layout")?,
            desktop_icons: layout(preferences.desktop_icons, "icon layout")?,
            language: preferences.language.map(|language| match language {
                InterfaceLanguage::En => Language::English,
                InterfaceLanguage::De => Language::German,
            }),
            theme: preferences.theme.map(|theme| match theme {
                InterfaceTheme::Light => Theme::Light,
                InterfaceTheme::Dark => Theme::Dark,
                InterfaceTheme::System => Theme::System,
            }),
        })
    }
}

/// The signed-in account's interface preferences, the same in every browser.
#[utoipa::path(
    get,
    path = "/preferences",
    tag = "identity",
    security(("session" = [])),
    responses((status = OK, body = InterfacePreferences))
)]
async fn preferences(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<InterfacePreferences>, ApiError> {
    Ok(Json(state.identity.preferences.execute(&auth.0).await?.into()))
}

/// Replaces the signed-in account's interface preferences.
#[utoipa::path(
    put,
    path = "/preferences",
    tag = "identity",
    security(("session" = [])),
    request_body = InterfacePreferences,
    responses(
        (status = NO_CONTENT, description = "Saved."),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: no such wallpaper name or layout"),
    )
)]
async fn save_preferences(
    auth: Auth,
    State(state): State<ApiState>,
    ApiJson(preferences): ApiJson<InterfacePreferences>,
) -> Result<StatusCode, ApiError> {
    state.identity.save_preferences.execute(&auth.0, preferences.try_into()?).await?;
    Ok(StatusCode::NO_CONTENT)
}
