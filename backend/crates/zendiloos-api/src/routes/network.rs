//! How the machine is connected (`system.view`). Read-only for now.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_network::domain as net;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new().routes(routes!(network))
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
