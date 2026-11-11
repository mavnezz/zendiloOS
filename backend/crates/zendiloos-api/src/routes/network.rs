//! How the machine is connected (`system.view`), changing an interface's
//! settings with confirmation, and taking the network over from ifupdown
//! (`system.manage`).

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_network::domain as net;

use zendiloos_network::application::{self as app, ConfigureError, refusals};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(network))
        .routes(routes!(network_setup))
        .routes(routes!(configure_interface))
        .routes(routes!(take_over_network))
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
            ConfigureError::NotCarriedOver(refused) => ApiError::Rejected {
                status: StatusCode::CONFLICT,
                code: "not_carried_over",
                detail: Some(refusals(&refused)),
            },
            ConfigureError::Port(e) => ApiError::internal(e),
        }
    }
}

/// What manages the network.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkManagerKind {
    /// systemd-networkd: zendiloOS changes it.
    Networkd,
    /// ifupdown (`/etc/network/interfaces`): zendiloOS can take it over.
    Ifupdown,
    /// NetworkManager: only shown.
    NetworkManager,
    /// Something else: only shown.
    Unknown,
}

/// Whether the network can be changed here, and a change that waits.
#[derive(Debug, Serialize, ToSchema)]
pub struct NetworkSetup {
    /// systemd-networkd manages the network; otherwise it is only shown,
    /// or with ifupdown, taken over first.
    pub managed: bool,
    pub manager: NetworkManagerKind,
    pub pending: Option<PendingNetworkChange>,
    /// Where ifupdown manages the network: what taking it over would do.
    pub takeover: Option<NetworkTakeover>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingNetworkChange {
    /// The interface whose settings changed; empty for a takeover.
    pub interface: String,
    /// zendiloOS took the network over from ifupdown: unless confirmed,
    /// ifupdown gets it back exactly as it was.
    pub takeover: bool,
    /// When the old settings return unless confirmed, in Unix seconds.
    pub revert_at: i64,
}

/// Taking the network over from ifupdown, as it would go now.
#[derive(Debug, Serialize, ToSchema)]
pub struct NetworkTakeover {
    /// Each interface ifupdown brings up, as systemd-networkd would set it
    /// up; the others stay down until set up here.
    pub interfaces: Vec<PlannedInterface>,
    /// What would not be carried over (bonds, bridges, VLANs, wireless,
    /// commands, search domains, …); while there is any, ifupdown stays.
    pub refused: Vec<NotCarriedOver>,
    /// systemd-resolved is installed from Debian first.
    pub installs_resolved: bool,
    /// `networking.service` runs; otherwise ifupdown has nothing to give.
    pub ifupdown_running: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlannedInterface {
    pub name: String,
    pub ipv4: PlannedIpv4,
    pub ipv6: Ipv6Input,
    #[schema(value_type = Vec<String>)]
    pub dns: Vec<std::net::IpAddr>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PlannedIpv4 {
    /// From DHCP, asking for the address the card has, if any: the lease
    /// is the card's (its MAC address).
    Dhcp {
        #[schema(value_type = Option<String>, format = Ipv4)]
        keep: Option<std::net::Ipv4Addr>,
    },
    Static {
        #[schema(value_type = String, format = Ipv4)]
        address: std::net::Ipv4Addr,
        prefix: u8,
        #[schema(value_type = Option<String>, format = Ipv4)]
        gateway: Option<std::net::Ipv4Addr>,
    },
    Off,
}

/// Where ifupdown sets up something a takeover would not carry over.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotCarriedOver {
    /// A file below `/etc/network` and its line, `interfaces:12`.
    pub place: String,
    pub reason: String,
}

fn setup_dto(setup: app::NetworkSetup) -> NetworkSetup {
    NetworkSetup {
        managed: setup.managed,
        manager: match setup.manager {
            app::Manager::Networkd => NetworkManagerKind::Networkd,
            app::Manager::Ifupdown => NetworkManagerKind::Ifupdown,
            app::Manager::NetworkManager => NetworkManagerKind::NetworkManager,
            app::Manager::Unknown => NetworkManagerKind::Unknown,
        },
        pending: setup.pending.map(|p| {
            let (interface, takeover) = match p.change {
                app::Change::Interface(interface) => (interface, false),
                app::Change::Takeover(_) => (String::new(), true),
            };
            PendingNetworkChange { interface, takeover, revert_at: p.revert_at.unix_seconds() }
        }),
        takeover: setup.takeover.map(|takeover| NetworkTakeover {
            interfaces: takeover
                .plan
                .interfaces
                .into_iter()
                .map(|settings| PlannedInterface {
                    ipv4: match settings.ipv4 {
                        net::Ipv4Setting::Dhcp { keep } => PlannedIpv4::Dhcp { keep },
                        net::Ipv4Setting::Static { address, prefix, gateway } => {
                            PlannedIpv4::Static { address, prefix, gateway }
                        }
                        net::Ipv4Setting::Off => PlannedIpv4::Off,
                    },
                    ipv6: match settings.ipv6 {
                        net::Ipv6Setting::Automatic => Ipv6Input::Automatic,
                        net::Ipv6Setting::Off => Ipv6Input::Off,
                    },
                    dns: settings.dns,
                    name: settings.interface,
                })
                .collect(),
            refused: takeover
                .plan
                .refused
                .into_iter()
                .map(|r| NotCarriedOver { place: r.place, reason: r.reason })
                .collect(),
            installs_resolved: takeover.installs_resolved,
            ifupdown_running: takeover.ifupdown_running,
        }),
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
    /// From DHCP, asking for the address the card has.
    Dhcp,
    Static {
        #[schema(value_type = String, format = Ipv4)]
        address: std::net::Ipv4Addr,
        /// 1 to 32.
        prefix: u8,
        #[schema(value_type = Option<String>, format = Ipv4)]
        gateway: Option<std::net::Ipv4Addr>,
    },
    /// None; with IPv6 off too, the interface is switched off (down).
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
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

/// Applies new settings to an interface at once: one that is down or has
/// no settings of zendiloOS's yet is switched on, IPv4 and IPv6 off
/// switch it off. Unless they are confirmed within two minutes (`POST
/// /network/confirm`, from the new address if it changed), the old
/// settings return by themselves. Needs `system.manage`.
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
            Ipv4Input::Dhcp => net::Ipv4Setting::Dhcp { keep: None },
            Ipv4Input::Static { address, prefix, gateway } => {
                net::Ipv4Setting::Static { address, prefix, gateway }
            }
            Ipv4Input::Off => net::Ipv4Setting::Off,
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

/// Lets zendiloOS manage the network where ifupdown does, as a job:
/// systemd-resolved installed from Debian if it is missing, then each
/// interface ifupdown brings up set up through systemd-networkd as `GET
/// /network/setup` says (DHCP keeps the address). Unless confirmed within
/// two minutes (`POST /network/confirm`), or should the NAS restart first,
/// ifupdown gets the network back exactly as it was. Needs
/// `system.manage`.
#[utoipa::path(
    post,
    path = "/network/takeover",
    tag = "network",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`not_carried_over` (the detail names what and where), `change_pending`, `refused`: not ifupdown, ifupdown not running"),
    )
)]
async fn take_over_network(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let job = state.network.configure.take_over(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Keeps the change that waits for confirmation, a takeover too. Needs
/// `system.manage`.
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
