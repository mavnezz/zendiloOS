//! How the machine is connected (`system.view`), and changing an
//! interface's settings with confirmation (`system.manage`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_network::domain as net;

use zendiloos_network::application::ConfigureError;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(network))
        .routes(routes!(network_setup))
        .routes(routes!(configure_interface))
        .routes(routes!(confirm_network))
}

impl From<ConfigureError> for ApiError {
    fn from(error: ConfigureError) -> Self {
        match error {
            ConfigureError::Refused(detail) => {
                ApiError::Rejected { status: StatusCode::CONFLICT, code: "refused", detail: Some(detail) }
            }
            ConfigureError::Pending => {
                ApiError::Rejected { status: StatusCode::CONFLICT, code: "change_pending", detail: None }
            }
            ConfigureError::Invalid(e) => ApiError::InvalidInput(e.to_string()),
            ConfigureError::Port(e) => ApiError::internal(e),
        }
    }
}

/// Whether the network can be changed here, and a change that waits.
#[derive(Debug, Serialize, ToSchema)]
pub struct NetworkSetup {
    /// systemd-networkd manages the network; otherwise it is only shown.
    pub managed: bool,
    pub pending: Option<PendingNetworkChange>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingNetworkChange {
    pub interface: String,
    /// When the old settings return unless confirmed, in Unix seconds.
    pub revert_at: i64,
}

fn setup_dto(setup: zendiloos_network::application::NetworkSetup) -> NetworkSetup {
    NetworkSetup {
        managed: setup.managed,
        pending: setup
            .pending
            .map(|p| PendingNetworkChange { interface: p.interface, revert_at: p.revert_at.unix_seconds() }),
    }
}

/// Whether the network can be changed here, and a change waiting to be
/// confirmed. Needs `system.view`.
#[utoipa::path(
    get,
    path = "/network/setup",
    tag = "network",
    security(("session" = [])),
    responses((status = OK, body = NetworkSetup))
)]
async fn network_setup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<NetworkSetup>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(setup_dto(state.network.configure.setup(&client).await?)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ipv4Input {
    Dhcp,
    Static {
        #[schema(value_type = String, format = Ipv4)]
        address: std::net::Ipv4Addr,
        /// 1 to 32.
        prefix: u8,
        #[schema(value_type = Option<String>, format = Ipv4)]
        gateway: Option<std::net::Ipv4Addr>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ipv6Input {
    /// From router advertisements, and DHCPv6 where they say so.
    Automatic,
    Off,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSettingsInput {
    pub ipv4: Ipv4Input,
    pub ipv6: Ipv6Input,
    /// Up to three; empty: those DHCP and the router give.
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub dns: Vec<std::net::IpAddr>,
}

/// Applies new settings to an interface at once. Unless they are confirmed
/// within two minutes (`POST /network/confirm`, from the new address if
/// it changed), the old settings return by themselves. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/network/interfaces/{name}",
    tag = "network",
    security(("session" = [])),
    params(("name" = String, Path)),
    request_body = InterfaceSettingsInput,
    responses(
        (status = ACCEPTED, body = NetworkSetup, description = "Applied; waits for confirmation."),
        (status = CONFLICT, body = Problem, description = "`change_pending`, or `refused`: not managed by systemd-networkd, no such interface"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn configure_interface(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(input): ApiJson<InterfaceSettingsInput>,
) -> Result<(StatusCode, Json<NetworkSetup>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let settings = net::InterfaceSettings {
        interface: name,
        ipv4: match input.ipv4 {
            Ipv4Input::Dhcp => net::Ipv4Setting::Dhcp,
            Ipv4Input::Static { address, prefix, gateway } => {
                net::Ipv4Setting::Static { address, prefix, gateway }
            }
        },
        ipv6: match input.ipv6 {
            Ipv6Input::Automatic => net::Ipv6Setting::Automatic,
            Ipv6Input::Off => net::Ipv6Setting::Off,
        },
        dns: input.dns,
    };
    let setup = state.network.configure.apply(settings, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(setup_dto(setup))))
}

/// Keeps the change that waits for confirmation. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/network/confirm",
    tag = "network",
    security(("session" = [])),
    responses(
        (status = NO_CONTENT, description = "Kept."),
        (status = CONFLICT, body = Problem, description = "`refused`: nothing waits"),
    )
)]
async fn confirm_network(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.network.configure.confirm(&auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceState {
    Up,
    /// Switched on, but no cable or no partner.
    NoCarrier,
    Down,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressScope {
    Global,
    Site,
    Link,
    Host,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InterfaceAddress {
    /// E.g. `192.168.1.20` or `2001:db8::20`.
    pub address: String,
    pub prefix: u8,
    pub scope: AddressScope,
    /// From DHCP or router advertisements rather than configured.
    pub dynamic: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NetworkInterface {
    pub name: String,
    /// `ethernet`, `wireless`, `bridge`, `bond`, `vlan`, or the kernel's
    /// name for other kinds, e.g. `wireguard`.
    pub kind: String,
    pub state: InterfaceState,
    pub mac: Option<String>,
    pub mtu: u32,
    /// The bridge or bond it belongs to; the addresses are on that one.
    pub member_of: Option<String>,
    /// Global ones first.
    pub addresses: Vec<InterfaceAddress>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DefaultRoute {
    pub gateway: String,
    pub interface: String,
    pub metric: u32,
}

/// How the machine is connected: its interfaces (without the loopback and
/// the plumbing of containers), default routes and name servers.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NetworkInfo {
    pub interfaces: Vec<NetworkInterface>,
    /// The preferred one first.
    pub default_routes: Vec<DefaultRoute>,
    pub name_servers: Vec<String>,
}

fn kind_name(kind: &net::InterfaceKind) -> String {
    match kind {
        net::InterfaceKind::Ethernet => "ethernet".into(),
        net::InterfaceKind::Loopback => "loopback".into(),
        net::InterfaceKind::Bridge => "bridge".into(),
        net::InterfaceKind::Bond => "bond".into(),
        net::InterfaceKind::Vlan => "vlan".into(),
        net::InterfaceKind::Wireless => "wireless".into(),
        net::InterfaceKind::Other(other) => other.clone(),
    }
}

impl From<net::Network> for NetworkInfo {
    fn from(network: net::Network) -> Self {
        let interfaces = network
            .interfaces
            .into_iter()
            .map(|interface| NetworkInterface {
                kind: kind_name(&interface.kind),
                state: match interface.state {
                    net::LinkState::Up => InterfaceState::Up,
                    net::LinkState::NoCarrier => InterfaceState::NoCarrier,
                    net::LinkState::Down => InterfaceState::Down,
                    net::LinkState::Unknown => InterfaceState::Unknown,
                },
                addresses: interface
                    .addresses
                    .into_iter()
                    .map(|address| InterfaceAddress {
                        address: address.ip.to_string(),
                        prefix: address.prefix,
                        scope: match address.scope {
                            net::AddressScope::Global => AddressScope::Global,
                            net::AddressScope::Site => AddressScope::Site,
                            net::AddressScope::Link => AddressScope::Link,
                            net::AddressScope::Host => AddressScope::Host,
                        },
                        dynamic: address.dynamic,
                    })
                    .collect(),
                name: interface.name,
                mac: interface.mac,
                mtu: interface.mtu,
                member_of: interface.member_of,
            })
            .collect();
        Self {
            interfaces,
            default_routes: network
                .default_routes
                .into_iter()
                .map(|route| DefaultRoute {
                    gateway: route.gateway.to_string(),
                    interface: route.interface,
                    metric: route.metric,
                })
                .collect(),
            name_servers: network.name_servers.iter().map(ToString::to_string).collect(),
        }
    }
}

/// Interfaces, addresses, default routes and name servers. Needs
/// `system.view`.
#[utoipa::path(
    get,
    path = "/network",
    tag = "network",
    security(("session" = [])),
    responses(
        (status = OK, body = NetworkInfo),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn network(auth: Auth, State(state): State<ApiState>) -> Result<Json<NetworkInfo>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(state.network.get_network.execute().await?.into()))
}
