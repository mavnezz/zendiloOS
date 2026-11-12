//! The firewall: its rules, applied at once and kept only once confirmed
//! from the browser, which must still get through; the services the rules
//! block from everywhere. Needs `system.manage`.

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_firewall::application::{CONFIRM_WITHIN_SECONDS, Connection, FirewallError, FirewallView};
use zendiloos_firewall::domain::{
    self as firewall, Exposed, Facts, KnownService, PortRange, Reach, Service, Source, Transport,
    parse_ports, ports_text,
};
use zendiloos_identity::domain::Permission;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(firewall_setup, apply_firewall))
        .routes(routes!(confirm_firewall))
        .routes(routes!(apply_restored_firewall))
        .routes(routes!(forget_restored_firewall))
}

impl From<FirewallError> for ApiError {
    fn from(error: FirewallError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            FirewallError::Invalid(e) => ApiError::InvalidInput(e.0),
            FirewallError::WouldLockOut => rejected(StatusCode::CONFLICT, "would_lock_out", None),
            FirewallError::Pending => rejected(StatusCode::CONFLICT, "change_pending", None),
            FirewallError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            FirewallError::HostFailed(detail) => {
                rejected(StatusCode::BAD_GATEWAY, "firewall_failed", Some(detail))
            }
            FirewallError::Port(e) => ApiError::internal(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirewallAction {
    Allow,
    Deny,
}

impl From<firewall::Action> for FirewallAction {
    fn from(action: firewall::Action) -> Self {
        match action {
            firewall::Action::Allow => Self::Allow,
            firewall::Action::Deny => Self::Deny,
        }
    }
}

impl From<FirewallAction> for firewall::Action {
    fn from(action: FirewallAction) -> Self {
        match action {
            FirewallAction::Allow => Self::Allow,
            FirewallAction::Deny => Self::Deny,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirewallProtocol {
    Tcp,
    Udp,
    Both,
}

/// What a rule is about: every port, one of the NAS's services, a Docker
/// project's published ports, or ports by number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirewallServiceKind {
    All,
    WebInterface,
    Smb,
    Nfs,
    Ssh,
    Discovery,
    Ping,
    Project,
    Ports,
}

fn known(kind: FirewallServiceKind) -> Option<KnownService> {
    KnownService::parse(match kind {
        FirewallServiceKind::WebInterface => "web_interface",
        FirewallServiceKind::Smb => "smb",
        FirewallServiceKind::Nfs => "nfs",
        FirewallServiceKind::Ssh => "ssh",
        FirewallServiceKind::Discovery => "discovery",
        FirewallServiceKind::Ping => "ping",
        _ => return None,
    })
}

fn kind_of(service: KnownService) -> FirewallServiceKind {
    match service {
        KnownService::WebInterface => FirewallServiceKind::WebInterface,
        KnownService::Smb => FirewallServiceKind::Smb,
        KnownService::Nfs => FirewallServiceKind::Nfs,
        KnownService::Ssh => FirewallServiceKind::Ssh,
        KnownService::Discovery => FirewallServiceKind::Discovery,
        KnownService::Ping => FirewallServiceKind::Ping,
    }
}

/// One rule. The first that matches a new connection decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FirewallRule {
    pub enabled: bool,
    /// None: every interface.
    #[serde(default)]
    pub interface: Option<String>,
    pub service: FirewallServiceKind,
    /// The Docker project, for `project`.
    #[serde(default)]
    pub project: Option<String>,
    /// For `ports`.
    #[serde(default)]
    pub protocol: Option<FirewallProtocol>,
    /// For `ports`: `80, 443, 8000-8100`.
    #[serde(default)]
    pub ports: Option<String>,
    /// `any`, an address, a network (`192.168.1.0/24`) or a range
    /// (`192.168.1.10-192.168.1.20`); IPv4 or IPv6.
    pub source: String,
    pub action: FirewallAction,
}

impl From<&firewall::Rule> for FirewallRule {
    fn from(rule: &firewall::Rule) -> Self {
        let (service, project, protocol, ports) = match &rule.service {
            Service::All => (FirewallServiceKind::All, None, None, None),
            Service::Known(known) => (kind_of(*known), None, None, None),
            Service::Project(name) => (FirewallServiceKind::Project, Some(name.clone()), None, None),
            Service::Ports { protocol, ports } => (
                FirewallServiceKind::Ports,
                None,
                Some(match protocol {
                    firewall::Protocol::Tcp => FirewallProtocol::Tcp,
                    firewall::Protocol::Udp => FirewallProtocol::Udp,
                    firewall::Protocol::Both => FirewallProtocol::Both,
                }),
                Some(ports_text(ports)),
            ),
        };
        Self {
            enabled: rule.enabled,
            interface: rule.interface.clone(),
            service,
            project,
            protocol,
            ports,
            source: rule.source.to_string(),
            action: rule.action.into(),
        }
    }
}

impl TryFrom<FirewallRule> for firewall::Rule {
    type Error = ApiError;

    fn try_from(rule: FirewallRule) -> Result<Self, ApiError> {
        let invalid = |text: String| ApiError::InvalidInput(text);
        let service = match rule.service {
            FirewallServiceKind::All => Service::All,
            FirewallServiceKind::Project => Service::Project(
                rule.project.ok_or_else(|| invalid("a project rule names the project".into()))?,
            ),
            FirewallServiceKind::Ports => Service::Ports {
                protocol: match rule.protocol.unwrap_or(FirewallProtocol::Both) {
                    FirewallProtocol::Tcp => firewall::Protocol::Tcp,
                    FirewallProtocol::Udp => firewall::Protocol::Udp,
                    FirewallProtocol::Both => firewall::Protocol::Both,
                },
                ports: parse_ports(rule.ports.as_deref().unwrap_or_default()).map_err(|e| invalid(e.0))?,
            },
            kind => Service::Known(known(kind).ok_or_else(|| invalid("not a service".into()))?),
        };
        Ok(Self {
            enabled: rule.enabled,
            interface: rule.interface.filter(|name| !name.is_empty()),
            service,
            source: Source::parse(&rule.source).map_err(|e| invalid(e.0))?,
            action: rule.action.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FirewallSettings {
    pub enabled: bool,
    /// What happens to a connection no rule matches.
    pub fallback: FirewallAction,
    pub rules: Vec<FirewallRule>,
}

impl From<&firewall::FirewallSettings> for FirewallSettings {
    fn from(settings: &firewall::FirewallSettings) -> Self {
        Self {
            enabled: settings.enabled,
            fallback: settings.fallback.into(),
            rules: settings.rules.iter().map(FirewallRule::from).collect(),
        }
    }
}

/// New settings. `lock_out`: apply them although they would keep out
/// the browser sending them (they go back unless confirmed anyway).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FirewallChange {
    pub enabled: bool,
    pub fallback: FirewallAction,
    pub rules: Vec<FirewallRule>,
    #[serde(default)]
    pub lock_out: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingFirewall {
    pub settings: FirewallSettings,
    /// When the rules from before return unless confirmed, in Unix seconds.
    pub revert_at: i64,
}

/// A service rules can name, with the ports it stands for now.
#[derive(Debug, Serialize, ToSchema)]
pub struct FirewallService {
    /// `smb`, `project:<name>`: as `blocked` names it.
    pub key: String,
    pub service: FirewallServiceKind,
    pub project: Option<String>,
    /// It runs (the web interface always; a project when it publishes
    /// ports).
    pub on: bool,
    /// The TCP and UDP ports, as `80, 443`; empty for none.
    pub tcp: String,
    pub udp: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FirewallSetup {
    pub current: FirewallSettings,
    pub pending: Option<PendingFirewall>,
    /// How long a change waits for confirmation, in seconds.
    pub confirm_within: u16,
    /// Services on that the rules in force block from everywhere, by key.
    pub blocked: Vec<String>,
    /// The NAS's services and the Docker projects that publish ports.
    pub services: Vec<FirewallService>,
    /// Interfaces rules can name.
    pub interfaces: Vec<String>,
    /// The networks the NAS is in, as sources.
    pub local_networks: Vec<String>,
    /// The settings a restored configuration backup brought: not in force
    /// (rules made for another network could shut the browser out) until
    /// applied and confirmed (`POST /security/firewall/restored/apply`).
    pub restored: Option<RestoredFirewall>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RestoredFirewall {
    pub settings: FirewallSettings,
    /// When they were set aside, at the first start after the restore, in
    /// Unix seconds.
    pub restored_at: i64,
}

fn ports_of(reach: &Reach, transport: Transport) -> String {
    let ranges: &[PortRange] = match transport {
        Transport::Tcp => &reach.tcp,
        Transport::Udp => &reach.udp,
    };
    ports_text(ranges)
}

fn services(facts: &Facts) -> Vec<FirewallService> {
    let on = firewall::exposed(facts);
    let mut listed: Vec<FirewallService> = KnownService::ALL
        .into_iter()
        .map(|known| {
            let reach = Service::Known(known).reach(facts);
            FirewallService {
                key: known.key().to_owned(),
                service: kind_of(known),
                project: None,
                on: known == KnownService::Ping || on.contains(&Exposed::Known(known)),
                tcp: ports_of(&reach, Transport::Tcp),
                udp: ports_of(&reach, Transport::Udp),
            }
        })
        .collect();
    listed.extend(facts.projects.iter().map(|project| {
        let reach = Service::Project(project.name.clone()).reach(facts);
        FirewallService {
            key: Exposed::Project(project.name.clone()).key(),
            service: FirewallServiceKind::Project,
            project: Some(project.name.clone()),
            on: !project.ports.is_empty(),
            tcp: ports_of(&reach, Transport::Tcp),
            udp: ports_of(&reach, Transport::Udp),
        }
    }));
    listed
}

fn setup(view: &FirewallView) -> FirewallSetup {
    FirewallSetup {
        current: (&view.current).into(),
        pending: view.pending.as_ref().map(|(settings, until)| PendingFirewall {
            settings: settings.into(),
            revert_at: until.unix_seconds(),
        }),
        confirm_within: CONFIRM_WITHIN_SECONDS,
        blocked: view.blocked.iter().map(Exposed::key).collect(),
        services: services(&view.facts),
        interfaces: view.interfaces.clone(),
        local_networks: view.local_networks.iter().map(ToString::to_string).collect(),
        restored: view.restored.as_ref().map(|restored| RestoredFirewall {
            settings: (&restored.settings).into(),
            restored_at: restored.restored_at.unix_seconds(),
        }),
    }
}

/// Where the request reached the NAS, as nginx says (`X-Server-Addr`,
/// `X-Server-Port`); only nginx may say so, as with the client's address.
struct Reached {
    server: Option<IpAddr>,
    port: Option<u16>,
}

impl<S: Send + Sync> FromRequestParts<S> for Reached {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(addr)| addr.ip());
        let from_nginx = peer.is_none_or(|peer| peer.is_loopback());
        let header = |name: &str| {
            from_nginx.then(|| parts.headers.get(name).and_then(|value| value.to_str().ok())).flatten()
        };
        Ok(Self {
            server: header("x-server-addr").and_then(|value| value.parse().ok()),
            port: header("x-server-port").and_then(|value| value.parse().ok()),
        })
    }
}

/// The rules, a change waiting for confirmation, the services they can
/// name with their ports, and those the rules block. Needs
/// `system.manage`.
#[utoipa::path(
    get,
    path = "/security/firewall",
    tag = "security",
    security(("session" = [])),
    responses(
        (status = OK, body = FirewallSetup),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn firewall_setup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<FirewallSetup>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(setup(&state.firewall.view(&client).await?)))
}

/// Applies new settings at once. Unless confirmed within `confirm_within`
/// seconds (`POST /security/firewall/confirm`), the rules from before
/// return by themselves. Settings that would keep out the browser sending
/// them are refused unless `lock_out` is set. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/security/firewall",
    tag = "security",
    security(("session" = [])),
    request_body = FirewallChange,
    responses(
        (status = ACCEPTED, body = FirewallSetup, description = "Applied; waits for confirmation."),
        (status = CONFLICT, body = Problem, description = "`would_lock_out`, `change_pending`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
        (status = BAD_GATEWAY, body = Problem, description = "`firewall_failed`: nftables refused the rules; nothing changed"),
    )
)]
async fn apply_firewall(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    reached: Reached,
    ApiJson(change): ApiJson<FirewallChange>,
) -> Result<(StatusCode, Json<FirewallSetup>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let settings = firewall::FirewallSettings {
        enabled: change.enabled,
        fallback: change.fallback.into(),
        rules: change.rules.into_iter().map(firewall::Rule::try_from).collect::<Result<_, _>>()?,
    };
    let connection = Connection { client: client.client_ip, server: reached.server, port: reached.port };
    state.firewall.apply(settings, connection, change.lock_out, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(setup(&state.firewall.view(&client).await?))))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoredFirewallChange {
    /// Apply them though they would keep out this browser.
    #[serde(default)]
    pub lock_out: bool,
}

/// Applies the settings a restored configuration backup brought, as any
/// change (`PUT /security/firewall`): the rules from before, the defaults
/// on a machine installed afresh, return unless confirmed in time; rules
/// that would keep out the browser asking are refused unless `lock_out` is
/// set. Once confirmed they are this machine's, offered no more. Needs
/// `system.manage`.
#[utoipa::path(
    post,
    path = "/security/firewall/restored/apply",
    tag = "security",
    security(("session" = [])),
    request_body = RestoredFirewallChange,
    responses(
        (status = ACCEPTED, body = FirewallSetup, description = "Applied; waits for confirmation."),
        (status = CONFLICT, body = Problem, description = "`would_lock_out`, `change_pending`, `refused`: no settings of a restored backup"),
        (status = BAD_GATEWAY, body = Problem, description = "`firewall_failed`: nftables refused the rules; nothing changed"),
    )
)]
async fn apply_restored_firewall(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    reached: Reached,
    ApiJson(change): ApiJson<RestoredFirewallChange>,
) -> Result<(StatusCode, Json<FirewallSetup>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let connection = Connection { client: client.client_ip, server: reached.server, port: reached.port };
    state.firewall.apply_restored(connection, change.lock_out, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(setup(&state.firewall.view(&client).await?))))
}

/// Forgets the settings a restored configuration backup brought. Needs
/// `system.manage`.
#[utoipa::path(
    delete,
    path = "/security/firewall/restored",
    tag = "security",
    security(("session" = [])),
    responses((status = NO_CONTENT, description = "Forgotten."))
)]
async fn forget_restored_firewall(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.firewall.forget_restored(&auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Keeps the change waiting for confirmation. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/security/firewall/confirm",
    tag = "security",
    security(("session" = [])),
    responses(
        (status = NO_CONTENT, description = "Kept."),
        (status = CONFLICT, body = Problem, description = "`refused`: nothing waits"),
    )
)]
async fn confirm_firewall(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.firewall.confirm(&auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
