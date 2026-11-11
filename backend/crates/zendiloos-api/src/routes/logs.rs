//! The system journal and the audit log (`logs.view`).

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_system::domain::{self as system, AuditQuery, JournalCursor, JournalQuery};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::ApiQuery;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(journal)).routes(routes!(audit))
}

const DEFAULT_LIMIT: u16 = 200;

/// A fixed view of the journal.
#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    /// Everything.
    #[default]
    System,
    /// zendiloosd and its agent.
    Zendiloos,
    /// RAID and disk monitoring.
    Storage,
    /// The SMB server.
    Sharing,
    /// The kernel.
    Kernel,
}

/// syslog severity, from most to least severe.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogPriority {
    Emergency,
    Alert,
    Critical,
    Error,
    Warning,
    Notice,
    #[default]
    Info,
    Debug,
}

impl From<LogPriority> for system::LogPriority {
    fn from(priority: LogPriority) -> Self {
        match priority {
            LogPriority::Emergency => system::LogPriority::Emergency,
            LogPriority::Alert => system::LogPriority::Alert,
            LogPriority::Critical => system::LogPriority::Critical,
            LogPriority::Error => system::LogPriority::Error,
            LogPriority::Warning => system::LogPriority::Warning,
            LogPriority::Notice => system::LogPriority::Notice,
            LogPriority::Info => system::LogPriority::Info,
            LogPriority::Debug => system::LogPriority::Debug,
        }
    }
}

impl From<system::LogPriority> for LogPriority {
    fn from(priority: system::LogPriority) -> Self {
        match priority {
            system::LogPriority::Emergency => LogPriority::Emergency,
            system::LogPriority::Alert => LogPriority::Alert,
            system::LogPriority::Critical => LogPriority::Critical,
            system::LogPriority::Error => LogPriority::Error,
            system::LogPriority::Warning => LogPriority::Warning,
            system::LogPriority::Notice => LogPriority::Notice,
            system::LogPriority::Info => LogPriority::Info,
            system::LogPriority::Debug => LogPriority::Debug,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct JournalParams {
    /// Which view; `system` if absent.
    #[serde(default)]
    pub source: LogSource,
    /// This priority and everything more severe; `info` if absent.
    #[serde(default)]
    pub priority: LogPriority,
    /// At most 500; 200 if absent.
    pub limit: Option<u16>,
    /// `older` of the previous page, for the entries before it.
    pub before: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JournalEntry {
    /// Microseconds since the Unix epoch.
    pub at_micros: u64,
    pub priority: LogPriority,
    pub unit: Option<String>,
    /// Program name, e.g. `smbd` or `kernel`.
    pub identifier: Option<String>,
    pub pid: Option<u32>,
    pub message: String,
}

/// Newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct JournalPage {
    pub entries: Vec<JournalEntry>,
    /// Pass as `before` for the next, older page; absent at the end.
    pub older: Option<String>,
}

/// A page of the system journal, newest first.
#[utoipa::path(
    get,
    path = "/logs/journal",
    tag = "logs",
    security(("session" = [])),
    params(JournalParams),
    responses(
        (status = OK, body = JournalPage),
        (status = BAD_REQUEST, body = Problem, description = "`bad_request`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn journal(
    auth: Auth,
    State(state): State<ApiState>,
    ApiQuery(params): ApiQuery<JournalParams>,
) -> Result<Json<JournalPage>, ApiError> {
    auth.require(Permission::LogsView)?;
    let before = params
        .before
        .as_deref()
        .map(JournalCursor::parse)
        .transpose()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let query = JournalQuery {
        source: match params.source {
            LogSource::System => system::LogSource::System,
            LogSource::Zendiloos => system::LogSource::Zendiloos,
            LogSource::Storage => system::LogSource::Storage,
            LogSource::Sharing => system::LogSource::Sharing,
            LogSource::Kernel => system::LogSource::Kernel,
        },
        priority: params.priority.into(),
        limit: params.limit.unwrap_or(DEFAULT_LIMIT),
        before,
    };
    let page = state.system.read_journal.execute(query).await?;
    Ok(Json(JournalPage {
        entries: page
            .entries
            .into_iter()
            .map(|e| JournalEntry {
                at_micros: e.at_micros,
                priority: e.priority.into(),
                unit: e.unit,
                identifier: e.identifier,
                pid: e.pid,
                message: e.message,
            })
            .collect(),
        older: page.older.map(|cursor| cursor.as_str().to_owned()),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct AuditParams {
    /// At most 500; 200 if absent.
    pub limit: Option<u16>,
    /// `older` of the previous page.
    pub before: Option<i64>,
    /// Only entries of this action, such as `identity.login`.
    pub action: Option<String>,
    /// Only entries whose action starts so, such as `containers.`.
    pub prefix: Option<String>,
    /// Only entries whose actor, action, target, address or reason holds
    /// this, whatever the case; at most 100 characters.
    pub search: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditRecord {
    pub id: i64,
    /// Unix seconds.
    pub at: i64,
    /// Account name; for failed logins the name that was tried.
    pub actor: Option<String>,
    pub client_ip: Option<String>,
    /// Dotted name, e.g. `identity.login`.
    pub action: String,
    pub target: Option<String>,
    /// `success`, `failure` or `denied`.
    pub outcome: String,
    pub reason: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditPage {
    pub entries: Vec<AuditRecord>,
    /// Pass as `before` for the next, older page; absent at the end.
    pub older: Option<i64>,
}

/// A page of the audit log: who did what, newest first.
#[utoipa::path(
    get,
    path = "/logs/audit",
    tag = "logs",
    security(("session" = [])),
    params(AuditParams),
    responses(
        (status = OK, body = AuditPage),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a search too long or with control characters"),
    )
)]
async fn audit(
    auth: Auth,
    State(state): State<ApiState>,
    ApiQuery(params): ApiQuery<AuditParams>,
) -> Result<Json<AuditPage>, ApiError> {
    auth.require(Permission::LogsView)?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, AuditQuery::MAX_LIMIT);
    let search = zendiloos_system::domain::audit_search(params.search.as_deref())
        .map_err(|error| ApiError::InvalidInput(error.to_string()))?;
    let query =
        AuditQuery { limit, before: params.before, action: params.action, prefix: params.prefix, search };
    let records = state.system.read_audit_log.execute(query).await?;
    let older = (records.len() >= usize::from(limit)).then(|| records.last().map(|r| r.id)).flatten();
    Ok(Json(AuditPage {
        entries: records
            .into_iter()
            .map(|r| AuditRecord {
                id: r.id,
                at: r.at.unix_seconds(),
                actor: r.actor,
                client_ip: r.client_ip,
                action: r.action,
                target: r.target,
                outcome: r.outcome,
                reason: r.reason,
                request_id: r.request_id,
            })
            .collect(),
        older,
    }))
}
