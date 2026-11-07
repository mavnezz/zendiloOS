//! Notification channels: email and webhooks (`system.manage`).

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
    self as notify, Channel, ChannelError, ChannelSettings, PasswordChange, Security, WebhookFormat,
};
use zendiloos_notify::message::Language;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_channels, create_channel))
        .routes(routes!(update_channel, delete_channel))
        .routes(routes!(test_channel))
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
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelTarget {
    Email {
        server: String,
        port: u16,
        security: MailSecurity,
        #[serde(default)]
        username: Option<String>,
        from: String,
        to: Vec<String>,
    },
    Webhook {
        url: String,
        format: HookFormat,
    },
}

/// The least serious notifications a channel carries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLevel {
    Info,
    /// Warnings, errors, and the good news that ends them.
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLanguage {
    En,
    De,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationChannel {
    pub id: i64,
    pub name: String,
    pub target: ChannelTarget,
    /// A mail password is stored; it is never shown.
    pub password_set: bool,
    pub min_level: ChannelLevel,
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
    /// The mail password. Left out when changing a channel: the stored one
    /// stays; empty: none.
    #[serde(default)]
    pub password: Option<String>,
    pub min_level: ChannelLevel,
    pub language: ChannelLanguage,
}

fn dto(channel: Channel) -> NotificationChannel {
    let settings = channel.settings;
    let (target, password_set) = match settings.target {
        notify::Target::Email(e) => (
            ChannelTarget::Email {
                server: e.server,
                port: e.port,
                security: match e.security {
                    Security::Tls => MailSecurity::Tls,
                    Security::StartTls => MailSecurity::Starttls,
                    Security::None => MailSecurity::None,
                },
                username: e.username,
                from: e.from,
                to: e.to,
            },
            e.password.is_some(),
        ),
        notify::Target::Webhook(w) => (
            ChannelTarget::Webhook {
                url: w.url,
                format: match w.format {
                    WebhookFormat::Json => HookFormat::Json,
                    WebhookFormat::Text => HookFormat::Text,
                },
            },
            false,
        ),
    };
    NotificationChannel {
        id: channel.id,
        name: settings.name,
        target,
        password_set,
        min_level: match settings.min_level {
            Level::Info => ChannelLevel::Info,
            Level::Success | Level::Warning => ChannelLevel::Warning,
            Level::Error => ChannelLevel::Error,
        },
        language: match settings.language {
            Language::English => ChannelLanguage::En,
            Language::German => ChannelLanguage::De,
        },
        last_delivery_at: channel.last.as_ref().map(|d| d.at.unix_seconds()),
        last_error: channel.last.and_then(|d| d.error),
    }
}

fn settings(input: ChannelInput) -> (ChannelSettings, PasswordChange) {
    let change = if input.password.is_some() { PasswordChange::Replace } else { PasswordChange::Keep };
    let target = match input.target {
        ChannelTarget::Email { server, port, security, username, from, to } => {
            notify::Target::Email(notify::Email {
                server,
                port,
                security: match security {
                    MailSecurity::Tls => Security::Tls,
                    MailSecurity::Starttls => Security::StartTls,
                    MailSecurity::None => Security::None,
                },
                username,
                password: input.password,
                from,
                to,
            })
        }
        ChannelTarget::Webhook { url, format } => notify::Target::Webhook(notify::Webhook {
            url,
            format: match format {
                HookFormat::Json => WebhookFormat::Json,
                HookFormat::Text => WebhookFormat::Text,
            },
        }),
    };
    let settings = ChannelSettings {
        name: input.name,
        target,
        min_level: match input.min_level {
            ChannelLevel::Info => Level::Info,
            ChannelLevel::Warning => Level::Warning,
            ChannelLevel::Error => Level::Error,
        },
        language: match input.language {
            ChannelLanguage::En => Language::English,
            ChannelLanguage::De => Language::German,
        },
    };
    (settings, change)
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
    Ok(Json(state.channels.list().await?.into_iter().map(dto).collect()))
}

/// Adds a channel: email over SMTP, or a webhook. Needs `system.manage`.
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
    let (settings, _) = settings(input);
    let created = state.channels.create(settings, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(dto(created))))
}

/// Changes a channel; without `password`, a stored mail password stays.
/// Needs `system.manage`.
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
    let (settings, password) = settings(input);
    let updated = state.channels.update(id, settings, password, &auth.0.actor(), &client).await?;
    Ok(Json(dto(updated)))
}

/// Removes a channel. Needs `system.manage`.
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

/// Sends a test message now and waits for the outcome. Needs
/// `system.manage`.
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
