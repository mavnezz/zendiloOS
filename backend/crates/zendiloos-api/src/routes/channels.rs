//! Notifications beyond the interface (`system.manage`): the mail server,
//! the channels (email recipients and webhooks) and where each kind of
//! notification goes.

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_notify::Level;
use zendiloos_notify::channels::{
    AccountAddress, Channel, ChannelError, ChannelSettings, Destination, KindChoice, KindRoute, MailServer,
    PasswordChange, Recipient, Security, Webhook, WebhookFormat,
};
use zendiloos_notify::message::Language;
use zendiloos_notify::routes::Group;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_channels, create_channel))
        .routes(routes!(update_channel, delete_channel))
        .routes(routes!(test_channel))
        .routes(routes!(get_mail_server, set_mail_server))
        .routes(routes!(get_routes, set_routes))
}

impl From<ChannelError> for ApiError {
    fn from(error: ChannelError) -> Self {
        let rejected = |status, code| ApiError::Rejected { status, code, detail: None };
        match error {
            ChannelError::NotFound => ApiError::NotFound,
            ChannelError::NameTaken => rejected(StatusCode::CONFLICT, "name_taken"),
            ChannelError::TooMany | ChannelError::Invalid(_) => ApiError::InvalidInput(error.to_string()),
            ChannelError::Delivery(detail) => ApiError::Rejected {
                status: StatusCode::BAD_GATEWAY,
                code: "delivery_failed",
                detail: Some(detail),
            },
            ChannelError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailSecurity {
    /// TLS from the start, usually port 465.
    Tls,
    /// Upgraded with STARTTLS, usually port 587.
    Starttls,
    /// Unencrypted: only for a relay in the own network, without password.
    None,
}

impl From<Security> for MailSecurity {
    fn from(security: Security) -> Self {
        match security {
            Security::Tls => Self::Tls,
            Security::StartTls => Self::Starttls,
            Security::None => Self::None,
        }
    }
}

impl From<MailSecurity> for Security {
    fn from(security: MailSecurity) -> Self {
        match security {
            MailSecurity::Tls => Self::Tls,
            MailSecurity::Starttls => Self::StartTls,
            MailSecurity::None => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookFormat {
    /// `title`, `message`, `text`, `content`, `level`, `code`, `params`,
    /// `host`, `at`.
    Json,
    /// The message as text, the title in a `Title` header (ntfy).
    Text,
}

/// Where a channel delivers.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelTarget {
    /// Through the mail server, to zendiloOS accounts (at the address each
    /// has when a mail goes out) and other addresses: 1 to 10 in all.
    Email {
        #[serde(default)]
        accounts: Vec<i64>,
        #[serde(default)]
        to: Vec<String>,
    },
    Webhook {
        url: String,
        format: HookFormat,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLanguage {
    En,
    De,
}

/// Why mail does not reach an account.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecipientProblem {
    /// The account has no email address.
    NoAddress,
    /// The account is disabled.
    Disabled,
    /// The account was deleted.
    Gone,
}

/// One recipient of an email channel, and where mail to it goes now.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelRecipient {
    /// The account, for one of zendiloOS's accounts.
    pub account_id: Option<i64>,
    /// The address mail goes to now; none if it reaches nobody.
    pub address: Option<String>,
    pub problem: Option<RecipientProblem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationChannel {
    pub id: i64,
    pub name: String,
    pub target: ChannelTarget,
    /// For an email channel: each recipient, as mail reaches it now.
    pub recipients: Vec<ChannelRecipient>,
    pub language: ChannelLanguage,
    /// The last delivery, in Unix seconds.
    pub last_delivery_at: Option<i64>,
    /// What went wrong then; none if it went out.
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelInput {
    pub name: String,
    pub target: ChannelTarget,
    pub language: ChannelLanguage,
}

/// The mail server every email channel sends through.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailServerSettings {
    pub server: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: Option<String>,
    /// The sender's address.
    pub from: String,
    /// A password is stored; it is never shown.
    pub password_set: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MailServerInput {
    pub server: String,
    pub port: u16,
    pub security: MailSecurity,
    #[serde(default)]
    pub username: Option<String>,
    pub from: String,
    /// Left out: the stored one stays (for the same server, port,
    /// encryption and user name only); empty: none.
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindGroup {
    Storage,
    Folders,
    System,
    Certificates,
    Security,
    Docker,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// Where one kind of notification goes.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationRoute {
    /// The kind: a notification's code, or `scrub_errors` for a data check
    /// that found errors.
    pub kind: String,
    pub group: KindGroup,
    /// The most serious level it is raised at, which decides where it goes
    /// by default.
    pub level: KindLevel,
    /// Kept in zendiloOS's notification center.
    pub in_app: bool,
    /// The channels it goes out on.
    pub channels: Vec<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteChoice {
    pub kind: String,
    pub in_app: bool,
    pub channels: Vec<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutesInput {
    /// The kinds to change; the others keep where they go.
    pub kinds: Vec<RouteChoice>,
}

fn target(destination: &Destination) -> ChannelTarget {
    match destination {
        Destination::Email { recipients } => {
            let (mut accounts, mut to) = (Vec::new(), Vec::new());
            for recipient in recipients {
                match recipient {
                    Recipient::Account(id) => accounts.push(*id),
                    Recipient::Address(address) => to.push(address.clone()),
                }
            }
            ChannelTarget::Email { accounts, to }
        }
        Destination::Webhook(w) => ChannelTarget::Webhook {
            url: w.url.clone(),
            format: match w.format {
                WebhookFormat::Json => HookFormat::Json,
                WebhookFormat::Text => HookFormat::Text,
            },
        },
    }
}

async fn dto(state: &ApiState, channel: Channel) -> Result<NotificationChannel, ApiError> {
    let recipients = state
        .channels
        .recipients(&channel)
        .await?
        .into_iter()
        .map(|state| {
            let account_id = match state.recipient {
                Recipient::Account(id) => Some(id),
                Recipient::Address(_) => None,
            };
            let (address, problem) = match state.address {
                AccountAddress::Address(address) => (Some(address), None),
                AccountAddress::Missing => (None, Some(RecipientProblem::NoAddress)),
                AccountAddress::Disabled => (None, Some(RecipientProblem::Disabled)),
                AccountAddress::Gone => (None, Some(RecipientProblem::Gone)),
            };
            ChannelRecipient { account_id, address, problem }
        })
        .collect();
    let settings = channel.settings;
    Ok(NotificationChannel {
        id: channel.id,
        name: settings.name,
        target: target(&settings.destination),
        recipients,
        language: match settings.language {
            Language::English => ChannelLanguage::En,
            Language::German => ChannelLanguage::De,
        },
        last_delivery_at: channel.last.as_ref().map(|d| d.at.unix_seconds()),
        last_error: channel.last.and_then(|d| d.error),
    })
}

/// The settings asked for. A channel's level only decides kinds nobody
/// chose for it yet: a new channel starts at warnings, a change keeps it.
fn settings(input: ChannelInput) -> ChannelSettings {
    let destination = match input.target {
        ChannelTarget::Email { accounts, to } => Destination::Email {
            recipients: accounts
                .into_iter()
                .map(Recipient::Account)
                .chain(to.into_iter().map(Recipient::Address))
                .collect(),
        },
        ChannelTarget::Webhook { url, format } => Destination::Webhook(Webhook {
            url,
            format: match format {
                HookFormat::Json => WebhookFormat::Json,
                HookFormat::Text => WebhookFormat::Text,
            },
        }),
    };
    ChannelSettings {
        name: input.name,
        destination,
        min_level: Level::Warning,
        language: match input.language {
            ChannelLanguage::En => Language::English,
            ChannelLanguage::De => Language::German,
        },
    }
}

fn route(route: KindRoute) -> NotificationRoute {
    NotificationRoute {
        kind: route.kind.key.to_owned(),
        group: match route.kind.group {
            Group::Storage => KindGroup::Storage,
            Group::Folders => KindGroup::Folders,
            Group::System => KindGroup::System,
            Group::Certificates => KindGroup::Certificates,
            Group::Security => KindGroup::Security,
            Group::Docker => KindGroup::Docker,
        },
        level: match route.kind.level {
            Level::Info => KindLevel::Info,
            Level::Success => KindLevel::Success,
            Level::Warning => KindLevel::Warning,
            Level::Error => KindLevel::Error,
        },
        in_app: route.in_app,
        channels: route.channels,
    }
}

/// The channels notifications go out on, besides the interface. Needs
/// `system.manage`.
#[utoipa::path(
    get,
    path = "/notification-channels",
    tag = "notifications",
    security(("session" = [])),
    responses((status = OK, body = Vec<NotificationChannel>))
)]
async fn list_channels(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<NotificationChannel>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let mut listed = Vec::new();
    for channel in state.channels.list().await? {
        listed.push(dto(&state, channel).await?);
    }
    Ok(Json(listed))
}

/// Adds a channel: email to recipients, or a webhook. It starts with the
/// warnings and errors; the routes change that. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/notification-channels",
    tag = "notifications",
    security(("session" = [])),
    request_body = ChannelInput,
    responses(
        (status = CREATED, body = NotificationChannel),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_channel(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<ChannelInput>,
) -> Result<(StatusCode, Json<NotificationChannel>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let created = state.channels.create(settings(input), &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(dto(&state, created).await?)))
}

/// Changes a channel's name, recipients or address, and language. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/notification-channels/{id}",
    tag = "notifications",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = ChannelInput,
    responses(
        (status = OK, body = NotificationChannel),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_channel(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(input): ApiJson<ChannelInput>,
) -> Result<Json<NotificationChannel>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let updated = state.channels.update(id, settings(input), &auth.0.actor(), &client).await?;
    Ok(Json(dto(&state, updated).await?))
}

/// Removes a channel, and where notifications went on it. Needs
/// `system.manage`.
#[utoipa::path(
    delete,
    path = "/notification-channels/{id}",
    tag = "notifications",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Removed."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn delete_channel(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.channels.delete(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sends a test message on this channel alone now and waits for the
/// outcome. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/notification-channels/{id}/test",
    tag = "notifications",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "It went out."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = BAD_GATEWAY, body = Problem, description = "`delivery_failed`, the reason in `detail`"),
    )
)]
async fn test_channel(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.channels.test(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn mail_server(server: MailServer) -> MailServerSettings {
    MailServerSettings {
        server: server.server,
        port: server.port,
        security: server.security.into(),
        username: server.username,
        from: server.from,
        password_set: server.password.is_some(),
    }
}

/// The mail server email channels send through; `null` until one is set
/// up. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/notification-mail-server",
    tag = "notifications",
    security(("session" = [])),
    responses((status = OK, body = Option<MailServerSettings>))
)]
async fn get_mail_server(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Option<MailServerSettings>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.channels.mail_server().await?.map(mail_server)))
}

/// Sets up the mail server. Without `password` the stored one stays, for
/// the same server, port, encryption and user name only. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/notification-mail-server",
    tag = "notifications",
    security(("session" = [])),
    request_body = MailServerInput,
    responses(
        (status = OK, body = MailServerSettings),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn set_mail_server(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<MailServerInput>,
) -> Result<Json<MailServerSettings>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let change = if input.password.is_some() { PasswordChange::Replace } else { PasswordChange::Keep };
    let server = MailServer {
        server: input.server,
        port: input.port,
        security: input.security.into(),
        username: input.username,
        password: input.password,
        from: input.from,
    };
    let saved = state.channels.set_mail_server(server, change, &auth.0.actor(), &client).await?;
    Ok(Json(mail_server(saved)))
}

/// Where each kind of notification goes: kept in zendiloOS, and on which
/// channels. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/notification-routes",
    tag = "notifications",
    security(("session" = [])),
    responses((status = OK, body = Vec<NotificationRoute>))
)]
async fn get_routes(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<NotificationRoute>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.channels.routes().await?.into_iter().map(route).collect()))
}

/// Changes where the kinds given go; the others keep theirs. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/notification-routes",
    tag = "notifications",
    security(("session" = [])),
    request_body = RoutesInput,
    responses(
        (status = OK, body = Vec<NotificationRoute>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such channel"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: no such kind"),
    )
)]
async fn set_routes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<RoutesInput>,
) -> Result<Json<Vec<NotificationRoute>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let choices = input
        .kinds
        .into_iter()
        .map(|choice| KindChoice { kind: choice.kind, in_app: choice.in_app, channels: choice.channels })
        .collect();
    let routes = state.channels.set_routes(choices, &auth.0.actor(), &client).await?;
    Ok(Json(routes.into_iter().map(route).collect()))
}
