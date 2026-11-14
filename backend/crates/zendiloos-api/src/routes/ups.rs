//! The UPS: where it is, when the NAS shuts down on battery, the devices
//! that follow it, and its state (docs/architecture.md, 4.16).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_ups::application::{AccountInput, Overview, SettingsInput, SourceInput, UpsError};
use zendiloos_ups::domain::{
    DEFAULT_ACCOUNT, ShutdownWhen, SnmpVersion, Source, UPS_NAME, UPS_PORT, USB_DRIVERS, UpsState,
};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(ups, set_ups)).routes(routes!(install_ups)).routes(routes!(scan_ups))
}

impl From<UpsError> for ApiError {
    fn from(error: UpsError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            UpsError::Invalid(e) => ApiError::InvalidInput(e.to_string()),
            UpsError::NotInstalled => rejected(StatusCode::CONFLICT, "ups_not_installed", None),
            UpsError::Installed => rejected(StatusCode::CONFLICT, "ups_installed", None),
            UpsError::SnmpMissing => rejected(StatusCode::CONFLICT, "ups_snmp_missing", None),
            UpsError::PasswordNeeded => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "ups_password_needed", None)
            }
            UpsError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            UpsError::HostFailed(detail) => rejected(StatusCode::BAD_GATEWAY, "agent_failed", Some(detail)),
            UpsError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpsSnmpVersion {
    V1,
    V2c,
}

/// Where the UPS is.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpsSource {
    /// Plugged into the NAS.
    Usb {
        /// One of `drivers`.
        driver: String,
        /// Four hexadecimal digits; with the product id and the serial
        /// number they name one UPS among several.
        vendor_id: Option<String>,
        product_id: Option<String>,
        serial: Option<String>,
    },
    /// A UPS with a network card, asked over SNMP.
    Snmp { address: String, community: String, version: UpsSnmpVersion },
    /// The UPS of another device that runs NUT's server.
    Server {
        host: String,
        port: u16,
        /// The UPS's name there.
        ups: String,
        user: String,
        /// A password is kept; it is never shown.
        password_set: bool,
    },
    /// A UPS that NUT plays from a file, for tests.
    Simulated,
}

/// As [`UpsSource`], to be set.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpsSourceInput {
    Usb {
        driver: String,
        #[serde(default)]
        vendor_id: Option<String>,
        #[serde(default)]
        product_id: Option<String>,
        #[serde(default)]
        serial: Option<String>,
    },
    Snmp {
        address: String,
        community: String,
        version: UpsSnmpVersion,
    },
    Server {
        host: String,
        /// 3493 unless given.
        #[serde(default)]
        port: Option<u16>,
        /// `ups` unless given.
        #[serde(default)]
        ups: Option<String>,
        user: String,
        /// Left out or empty: the one kept stays, for the same host and
        /// account.
        #[schema(format = Password)]
        #[serde(default)]
        password: Option<String>,
    },
    Simulated,
}

/// When the NAS shuts down while the UPS runs on battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpsShutdown {
    /// When the UPS says its battery is low; following another device,
    /// when that one says to shut down.
    LowBattery,
    /// When the charge has fallen to `percent` (5 to 95); with a UPS on
    /// the NAS itself only.
    ChargeBelow { percent: u8 },
    /// After `minutes` on battery (1 to 720), or earlier when the battery
    /// is low.
    AfterMinutes { minutes: u16 },
}

/// The account other devices follow the UPS with.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpsAccount {
    pub user: String,
    pub password_set: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpsAccountInput {
    pub user: String,
    /// Left out or empty: the one kept stays, for the same account.
    #[schema(format = Password)]
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpsSettings {
    /// None: no UPS.
    pub source: Option<UpsSource>,
    pub shutdown: UpsShutdown,
    /// After the shutdown the UPS cuts its output, so that the NAS starts
    /// again by itself when the power is back.
    pub cut_power: bool,
    /// Other devices may follow the UPS through the NAS: NUT's server
    /// listens on the network (TCP port 3493).
    pub serve: Option<UpsAccount>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpsRequest {
    pub source: Option<UpsSourceInput>,
    pub shutdown: UpsShutdown,
    #[serde(default)]
    pub cut_power: bool,
    #[serde(default)]
    pub serve: Option<UpsAccountInput>,
}

/// What the UPS says of itself.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpsReading {
    /// The power has failed: the UPS runs on battery.
    pub on_battery: bool,
    /// The UPS calls its battery low. It still does for a while after the
    /// power is back, until the battery has charged.
    pub low_battery: bool,
    /// The NAS is about to shut down: on battery with the battery low, or
    /// because the time on battery is over.
    pub shutting_down: bool,
    pub replace_battery: bool,
    pub charging: bool,
    /// NUT's `ups.status`, e.g. `OL CHRG`.
    pub status: String,
    pub charge_percent: Option<u8>,
    pub runtime_seconds: Option<u32>,
    pub load_percent: Option<u8>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
}

impl From<UpsState> for UpsReading {
    fn from(state: UpsState) -> Self {
        Self {
            on_battery: state.on_battery,
            low_battery: state.low_battery,
            shutting_down: state.shutting_down(),
            replace_battery: state.replace_battery,
            charging: state.charging,
            status: state.status,
            charge_percent: state.charge_percent,
            runtime_seconds: state.runtime_seconds,
            load_percent: state.load_percent,
            manufacturer: state.manufacturer,
            model: state.model,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpsOverview {
    /// Network UPS Tools are installed.
    pub installed: bool,
    /// Their SNMP driver too.
    pub snmp: bool,
    pub settings: UpsSettings,
    /// None without a UPS, and while it does not answer.
    pub reading: Option<UpsReading>,
    /// NUT's drivers for a UPS on USB.
    pub drivers: Vec<String>,
    /// The UPS's name in NUT on this NAS, which devices that follow ask for.
    pub name: String,
    /// The port NUT's server listens on.
    pub port: u16,
    /// The account's name other devices sign in with unless another is set.
    pub default_account: String,
}

impl From<Overview> for UpsOverview {
    fn from(overview: Overview) -> Self {
        let settings = overview.settings;
        Self {
            installed: overview.facts.installed,
            snmp: overview.facts.snmp,
            settings: UpsSettings {
                source: settings.source.map(|source| match source {
                    Source::Usb { driver, vendor_id, product_id, serial } => {
                        UpsSource::Usb { driver, vendor_id, product_id, serial }
                    }
                    Source::Snmp { address, community, version } => UpsSource::Snmp {
                        address,
                        community,
                        version: match version {
                            SnmpVersion::V1 => UpsSnmpVersion::V1,
                            SnmpVersion::V2c => UpsSnmpVersion::V2c,
                        },
                    },
                    Source::Server { host, port, ups, user, .. } => {
                        UpsSource::Server { host, port, ups, user, password_set: true }
                    }
                    Source::Simulated => UpsSource::Simulated,
                }),
                shutdown: match settings.shutdown {
                    ShutdownWhen::LowBattery => UpsShutdown::LowBattery,
                    ShutdownWhen::ChargeBelow(percent) => UpsShutdown::ChargeBelow { percent },
                    ShutdownWhen::AfterMinutes(minutes) => UpsShutdown::AfterMinutes { minutes },
                },
                cut_power: settings.cut_power,
                serve: settings.serve.map(|account| UpsAccount { user: account.user, password_set: true }),
            },
            reading: overview.state.map(UpsReading::from),
            drivers: USB_DRIVERS.iter().map(|driver| (*driver).to_owned()).collect(),
            name: UPS_NAME.to_owned(),
            port: UPS_PORT,
            default_account: DEFAULT_ACCOUNT.to_owned(),
        }
    }
}

impl From<UpsRequest> for SettingsInput {
    fn from(request: UpsRequest) -> Self {
        Self {
            source: request.source.map(|source| match source {
                UpsSourceInput::Usb { driver, vendor_id, product_id, serial } => {
                    SourceInput::Usb { driver, vendor_id, product_id, serial }
                }
                UpsSourceInput::Snmp { address, community, version } => SourceInput::Snmp {
                    address,
                    community,
                    version: match version {
                        UpsSnmpVersion::V1 => SnmpVersion::V1,
                        UpsSnmpVersion::V2c => SnmpVersion::V2c,
                    },
                },
                UpsSourceInput::Server { host, port, ups, user, password } => SourceInput::Server {
                    host,
                    port: port.unwrap_or(UPS_PORT),
                    ups: ups.filter(|ups| !ups.is_empty()).unwrap_or_else(|| UPS_NAME.to_owned()),
                    user,
                    password,
                },
                UpsSourceInput::Simulated => SourceInput::Simulated,
            }),
            shutdown: match request.shutdown {
                UpsShutdown::LowBattery => ShutdownWhen::LowBattery,
                UpsShutdown::ChargeBelow { percent } => ShutdownWhen::ChargeBelow(percent),
                UpsShutdown::AfterMinutes { minutes } => ShutdownWhen::AfterMinutes(minutes),
            },
            cut_power: request.cut_power,
            serve: request
                .serve
                .map(|account| AccountInput { user: account.user, password: account.password }),
        }
    }
}

/// The UPS: what is installed, its settings and what it says. Needs
/// `system.view`.
#[utoipa::path(
    get,
    path = "/ups",
    tag = "ups",
    security(("session" = [])),
    responses((status = OK, body = UpsOverview), (status = FORBIDDEN, body = Problem, description = "`forbidden`"))
)]
async fn ups(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<UpsOverview>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(state.ups.overview(&client).await?.into()))
}

/// Sets where the UPS is and what the NAS does on battery, and puts it in
/// force: NUT's files are written and its services started, or stopped
/// without a UPS. What NUT's host refuses is not kept. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/ups",
    tag = "ups",
    security(("session" = [])),
    request_body = UpsRequest,
    responses(
        (status = OK, body = UpsOverview),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`ups_not_installed`, `ups_snmp_missing`, `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`; `ups_password_needed`: no password is kept for that account"),
        (status = BAD_GATEWAY, body = Problem, description = "`agent_failed`"),
    )
)]
async fn set_ups(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<UpsRequest>,
) -> Result<Json<UpsOverview>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.ups.set(request.into(), &auth.0.actor(), &client).await?.into()))
}

/// Installs Network UPS Tools from Debian, as a job; settings that are
/// there already are put in force when it is done. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/ups/install",
    tag = "ups",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`ups_installed`"),
    )
)]
async fn install_ups(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let job = state.ups.install(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// A UPS found on USB.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpsDevice {
    /// NUT's driver for it.
    pub driver: String,
    /// Whether that driver is one of `drivers`, which can be set.
    pub supported: bool,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpsDevices {
    pub devices: Vec<UpsDevice>,
}

/// Looks for UPSes on USB (`nut-scanner`). Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/ups/scan",
    tag = "ups",
    security(("session" = [])),
    responses(
        (status = OK, body = UpsDevices),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`ups_not_installed`"),
    )
)]
async fn scan_ups(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<UpsDevices>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let devices = state
        .ups
        .scan(&client)
        .await?
        .into_iter()
        .map(|device| UpsDevice {
            supported: USB_DRIVERS.contains(&device.driver.as_str()),
            driver: device.driver,
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            vendor: device.vendor,
            product: device.product,
            serial: device.serial,
        })
        .collect();
    Ok(Json(UpsDevices { devices }))
}
