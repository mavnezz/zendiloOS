use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use zendiloos_identity::domain::Permission;
use zendiloos_kernel::Timestamp;
use zendiloos_system::application::commands::{PowerError, SetSshError, SetTimeError};
use zendiloos_system::domain::{
    PowerAction, RootLogin, SshSettings, SshStatus, TimeSettings, TimeStatus, TlsMinimum,
};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(info))
        .routes(routes!(power))
        .routes(routes!(metrics_history))
        .routes(routes!(time, set_time))
        .routes(routes!(ssh, set_ssh))
        .routes(routes!(tls, set_tls))
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
    /// Physical cores, if known.
    pub cpu_cores: Option<usize>,
    pub memory_total_bytes: u64,
    pub swap_total_bytes: u64,
    pub uptime_seconds: u64,
    /// Version of Zendiloos.
    pub version: String,
    /// Maker of the machine, from its firmware.
    pub vendor: Option<String>,
    pub product: Option<String>,
    /// Mainboard maker and name.
    pub board: Option<String>,
    /// Firmware maker, version and date.
    pub firmware: Option<String>,
    /// Booted through UEFI rather than a legacy BIOS.
    pub uefi: bool,
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
            cpu_cores: info.cpu_cores,
            memory_total_bytes: info.memory_total_bytes,
            swap_total_bytes: info.swap_total_bytes,
            uptime_seconds: info.uptime_seconds,
            version: zendiloos_kernel::VERSION.to_owned(),
            vendor: info.hardware.vendor,
            product: info.hardware.product,
            board: info.hardware.board,
            firmware: info.hardware.firmware,
            uefi: info.hardware.uefi,
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

/// Reboots or powers off the system. Needs `system.manage`. Refused while
/// background jobs run (`jobs_running`, the detail names them): they would
/// stop halfway.
#[utoipa::path(
    post,
    path = "/system/power",
    tag = "system",
    security(("session" = [])),
    request_body = PowerRequest,
    responses(
        (status = ACCEPTED, description = "The action is under way."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`jobs_running`: the detail names the jobs"),
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

/// The last minutes of the host's load as series for charts, oldest first;
/// one sample every two seconds, at most six minutes. Rates are summed over
/// all network interfaces and all disks.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsHistory {
    /// Unix time in seconds of each sample.
    pub at: Vec<i64>,
    pub cpu_percent: Vec<f32>,
    pub memory_used_bytes: Vec<u64>,
    pub received_bytes_per_second: Vec<u64>,
    pub sent_bytes_per_second: Vec<u64>,
    pub read_bytes_per_second: Vec<u64>,
    pub written_bytes_per_second: Vec<u64>,
}

/// What the dashboard's charts show before live samples arrive over the
/// WebSocket (topic `metrics`).
#[utoipa::path(
    get,
    path = "/system/metrics/history",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = OK, body = MetricsHistory),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn metrics_history(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<MetricsHistory>, ApiError> {
    auth.require(Permission::SystemView)?;
    let history = state.system.watch_metrics.history();
    Ok(Json(MetricsHistory {
        at: history.at.iter().map(|at| at.unix_seconds()).collect(),
        cpu_percent: history.cpu_percent,
        memory_used_bytes: history.memory_used_bytes,
        received_bytes_per_second: history.received_bytes_per_second,
        sent_bytes_per_second: history.sent_bytes_per_second,
        read_bytes_per_second: history.read_bytes_per_second,
        written_bytes_per_second: history.written_bytes_per_second,
    }))
}

impl From<SetTimeError> for ApiError {
    fn from(error: SetTimeError) -> Self {
        match error {
            SetTimeError::Invalid(e) => ApiError::InvalidInput(e.to_string()),
            SetTimeError::Port(e) => ApiError::internal(e),
        }
    }
}

/// The machine's clock: its time zone and how it is kept.
#[derive(Debug, Serialize, ToSchema)]
pub struct TimeState {
    /// A zone of the tz database: `Europe/Berlin`.
    pub zone: String,
    /// Time servers keep the clock.
    pub network_time: bool,
    /// A time service is installed: network time can be turned on.
    pub can_network_time: bool,
    /// The clock agrees with a time server.
    pub synchronized: bool,
    /// The servers set here; empty: Debian's pool.
    pub servers: Vec<String>,
    /// The server the clock comes from now.
    pub server_in_use: Option<String>,
    /// Every zone the system has, sorted.
    pub zones: Vec<String>,
    /// The machine's time, in Unix seconds, when this was read.
    pub now: i64,
}

impl From<TimeStatus> for TimeState {
    fn from(status: TimeStatus) -> Self {
        Self {
            zone: status.zone,
            network_time: status.network_time,
            can_network_time: status.can_network_time,
            synchronized: status.synchronized,
            servers: status.servers,
            server_in_use: status.server_in_use,
            zones: status.zones,
            now: status.now.unix_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeRequest {
    pub zone: String,
    pub network_time: bool,
    /// At most four host names or IP addresses; empty: Debian's pool.
    #[serde(default)]
    pub servers: Vec<String>,
    /// Only without network time: sets the clock, in Unix seconds.
    #[serde(default)]
    pub clock: Option<i64>,
}

/// The time zone and how the clock is kept. Needs `system.view`.
#[utoipa::path(
    get,
    path = "/system/time",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = OK, body = TimeState),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn time(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<TimeState>, ApiError> {
    auth.require(Permission::SystemView)?;
    let status = state.system.get_time.execute(&client).await?;
    Ok(Json(status.into()))
}

/// Sets the time zone and the time servers, or the time itself when none
/// keep it. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/system/time",
    tag = "system",
    security(("session" = [])),
    request_body = TimeRequest,
    responses(
        (status = OK, body = TimeState, description = "The clock afterwards."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: an unknown zone, a bad server, a time with servers on"),
    )
)]
async fn set_time(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<TimeRequest>,
) -> Result<Json<TimeState>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let settings = TimeSettings {
        zone: request.zone,
        network_time: request.network_time,
        servers: request.servers,
        clock: request.clock.map(Timestamp::from_unix_seconds),
    };
    let status = state.system.set_time.execute(settings, &auth.0.actor(), &client).await?;
    Ok(Json(status.into()))
}

impl From<PowerError> for ApiError {
    fn from(error: PowerError) -> Self {
        match error {
            PowerError::JobsRunning(named) => {
                ApiError::Rejected { status: StatusCode::CONFLICT, code: "jobs_running", detail: Some(named) }
            }
            PowerError::Port(e) => e.into(),
        }
    }
}

impl From<SetSshError> for ApiError {
    fn from(error: SetSshError) -> Self {
        match error {
            SetSshError::Invalid(e) => ApiError::InvalidInput(e.to_string()),
            SetSshError::NotInstalled => ApiError::Rejected {
                status: StatusCode::CONFLICT,
                code: "not_installed",
                detail: Some("the SSH server is not installed: apt install openssh-server".to_owned()),
            },
            SetSshError::PortInUse(port) => ApiError::Rejected {
                status: StatusCode::CONFLICT,
                code: "port_in_use",
                detail: Some(format!("another program listens on port {port}")),
            },
            SetSshError::Port(e) => ApiError::internal(e),
        }
    }
}

/// Whether root may log in over SSH.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RootLoginMode {
    No,
    /// With a key only.
    KeyOnly,
    Yes,
}

impl From<RootLogin> for RootLoginMode {
    fn from(root: RootLogin) -> Self {
        match root {
            RootLogin::No => Self::No,
            RootLogin::KeyOnly => Self::KeyOnly,
            RootLogin::Yes => Self::Yes,
        }
    }
}

impl From<RootLoginMode> for RootLogin {
    fn from(root: RootLoginMode) -> Self {
        match root {
            RootLoginMode::No => Self::No,
            RootLoginMode::KeyOnly => Self::KeyOnly,
            RootLoginMode::Yes => Self::Yes,
        }
    }
}

/// The SSH server: on or off, its port and two of its rules. Who may log
/// in otherwise stays as the system has it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SshState {
    /// The server is installed; without it, nothing can be turned on.
    pub installed: bool,
    /// Starts with the machine.
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    /// `null`: as the system's configuration has it.
    pub root_login: Option<RootLoginMode>,
    /// `null`: as the system's configuration has it.
    pub password_login: Option<bool>,
    /// When it turns itself off, in Unix seconds.
    pub off_at: Option<i64>,
}

impl From<SshStatus> for SshState {
    fn from(status: SshStatus) -> Self {
        Self {
            installed: status.installed,
            enabled: status.enabled,
            running: status.running,
            port: status.port,
            root_login: status.root_login.map(Into::into),
            password_login: status.password_login,
            off_at: status.off_at.map(Timestamp::unix_seconds),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SshRequest {
    pub enabled: bool,
    /// 22, or a port from 1024 that no service of the NAS uses.
    pub port: u16,
    /// `null`: as the system's configuration has it.
    #[serde(default)]
    pub root_login: Option<RootLoginMode>,
    /// `null`: as the system's configuration has it.
    #[serde(default)]
    pub password_login: Option<bool>,
    /// Turns SSH off again after 1 to 1440 minutes; `null`: never.
    #[serde(default)]
    pub off_after_minutes: Option<u16>,
}

/// The SSH server. Needs `system.view`.
#[utoipa::path(
    get,
    path = "/system/ssh",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = OK, body = SshState),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn ssh(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<SshState>, ApiError> {
    auth.require(Permission::SystemView)?;
    let status = state.system.get_ssh.execute(&client).await?;
    Ok(Json(status.into()))
}

/// Turns the SSH server on or off and sets its port and rules; open
/// sessions stay. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/system/ssh",
    tag = "system",
    security(("session" = [])),
    request_body = SshRequest,
    responses(
        (status = OK, body = SshState, description = "The server afterwards."),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`not_installed`: there is no SSH server; `port_in_use`: another program listens on the port"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a port below 1024 or in use, a time to turn off out of range"),
    )
)]
async fn set_ssh(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<SshRequest>,
) -> Result<Json<SshState>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let settings = SshSettings {
        enabled: request.enabled,
        port: request.port,
        root_login: request.root_login.map(Into::into),
        password_login: request.password_login,
        off_after_minutes: request.off_after_minutes,
    };
    let status = state.system.set_ssh.execute(settings, &auth.0.actor(), &client).await?;
    Ok(Json(status.into()))
}

/// The oldest TLS version the web interface accepts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub enum TlsVersion {
    #[serde(rename = "tls1_2")]
    Tls12,
    #[serde(rename = "tls1_3")]
    Tls13,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebTls {
    /// TLS 1.2 serves older clients too; 1.3 alone refuses them.
    pub minimum: TlsVersion,
}

impl From<TlsMinimum> for WebTls {
    fn from(minimum: TlsMinimum) -> Self {
        let minimum = match minimum {
            TlsMinimum::Tls12 => TlsVersion::Tls12,
            TlsMinimum::Tls13 => TlsVersion::Tls13,
        };
        Self { minimum }
    }
}

/// The oldest TLS version the web interface accepts. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/system/tls",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = OK, body = WebTls),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn tls(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<WebTls>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.system.get_tls.execute(&client).await?.into()))
}

/// Sets it; connections already open stay. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/system/tls",
    tag = "system",
    security(("session" = [])),
    request_body = WebTls,
    responses(
        (status = OK, body = WebTls),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
    )
)]
async fn set_tls(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<WebTls>,
) -> Result<Json<WebTls>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let minimum = match request.minimum {
        TlsVersion::Tls12 => TlsMinimum::Tls12,
        TlsVersion::Tls13 => TlsMinimum::Tls13,
    };
    Ok(Json(state.system.set_tls.execute(minimum, &auth.0.actor(), &client).await?.into()))
}
