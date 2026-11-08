//! The ACME client plugin (acme.sh): its accounts, challenges and
//! certificates (`system.manage`; the plugin's state `system.view`), and
//! the web interface's ports and certificate, changed with confirmation.
//! Secret values of a DNS provider are never sent back.

use std::collections::BTreeMap;

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_certificates::application::{CONFIRM_WITHIN_SECONDS, CertificatesError, WebServerSettings};
use zendiloos_certificates::domain::{
    self as certificates, AcmeServer, ChallengeMethod, DNS_PROVIDERS, KeyType, dns_provider,
};
use zendiloos_identity::domain::Permission;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(acme_plugin))
        .routes(routes!(install_acme_plugin))
        .routes(routes!(acme_accounts, create_acme_account))
        .routes(routes!(update_acme_account, delete_acme_account))
        .routes(routes!(acme_challenges, create_acme_challenge))
        .routes(routes!(update_acme_challenge, delete_acme_challenge))
        .routes(routes!(list_certificates, create_certificate))
        .routes(routes!(update_certificate, delete_certificate))
        .routes(routes!(issue_certificate))
        .routes(routes!(web_server, apply_web_server))
        .routes(routes!(confirm_web_server))
}

impl From<CertificatesError> for ApiError {
    fn from(error: CertificatesError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            CertificatesError::NotFound => ApiError::NotFound,
            CertificatesError::Invalid(e) => ApiError::InvalidInput(e.0),
            CertificatesError::InUse(what) => {
                rejected(StatusCode::CONFLICT, "in_use", Some(format!("still used by {what}")))
            }
            CertificatesError::NotInstalled => rejected(StatusCode::CONFLICT, "acme_not_installed", None),
            CertificatesError::Installed => rejected(StatusCode::CONFLICT, "acme_installed", None),
            CertificatesError::PortBusy(port) => {
                rejected(StatusCode::CONFLICT, "port_in_use", Some(port.to_string()))
            }
            CertificatesError::Pending => rejected(StatusCode::CONFLICT, "change_pending", None),
            CertificatesError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            CertificatesError::HostFailed(detail) => {
                rejected(StatusCode::BAD_GATEWAY, "web_server_failed", Some(detail))
            }
            CertificatesError::Port(e) => ApiError::internal(e),
        }
    }
}

/// A variable a DNS provider's API script reads.
#[derive(Debug, Serialize, ToSchema)]
pub struct DnsVariable {
    /// As acme.sh names it, e.g. `IONOS_PREFIX`.
    pub name: String,
    /// Never sent back once saved.
    pub secret: bool,
    pub required: bool,
}

/// A DNS provider acme.sh can make the challenge's records with.
#[derive(Debug, Serialize, ToSchema)]
pub struct DnsProvider {
    /// acme.sh's script, e.g. `dns_ionos`.
    pub api: String,
    pub label: String,
    pub variables: Vec<DnsVariable>,
}

/// The ACME client plugin, and the DNS providers it offers.
#[derive(Debug, Serialize, ToSchema)]
pub struct AcmePlugin {
    /// acme.sh is installed from Debian.
    pub installed: bool,
    pub providers: Vec<DnsProvider>,
}

/// Whether acme.sh is installed, and the DNS providers to choose from.
/// Needs `system.view`.
#[utoipa::path(
    get,
    path = "/certificates/plugin",
    tag = "certificates",
    security(("session" = [])),
    responses((status = OK, body = AcmePlugin))
)]
async fn acme_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<AcmePlugin>, ApiError> {
    auth.require(Permission::SystemView)?;
    let installed = state.certificates.plugin(&client).await?.installed;
    let providers = DNS_PROVIDERS
        .iter()
        .map(|p| DnsProvider {
            api: p.api.into(),
            label: p.label.into(),
            variables: p
                .variables
                .iter()
                .map(|v| DnsVariable { name: v.name.into(), secret: v.secret, required: v.required })
                .collect(),
        })
        .collect();
    Ok(Json(AcmePlugin { installed, providers }))
}

/// Installs acme.sh from Debian, as a job. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/certificates/plugin/install",
    tag = "certificates",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, body = Job),
        (status = CONFLICT, body = Problem, description = "`acme_installed`"),
    )
)]
async fn install_acme_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let job = state.certificates.install(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Which ACME server an account is at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcmeServerName {
    Letsencrypt,
    /// For trying out: certificates no browser trusts.
    LetsencryptStaging,
}

impl From<AcmeServer> for AcmeServerName {
    fn from(server: AcmeServer) -> Self {
        match server {
            AcmeServer::LetsEncrypt => Self::Letsencrypt,
            AcmeServer::LetsEncryptStaging => Self::LetsencryptStaging,
        }
    }
}

impl From<AcmeServerName> for AcmeServer {
    fn from(server: AcmeServerName) -> Self {
        match server {
            AcmeServerName::Letsencrypt => Self::LetsEncrypt,
            AcmeServerName::LetsencryptStaging => Self::LetsEncryptStaging,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AcmeAccount {
    pub id: i64,
    pub name: String,
    pub server: AcmeServerName,
    /// Let's Encrypt writes here about the account and its certificates.
    pub email: String,
    pub created_at: i64,
}

impl From<certificates::AcmeAccount> for AcmeAccount {
    fn from(account: certificates::AcmeAccount) -> Self {
        Self {
            id: account.id,
            name: account.name,
            server: account.server.into(),
            email: account.email,
            created_at: account.created_at.unix_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcmeAccountInput {
    pub name: String,
    pub server: AcmeServerName,
    pub email: String,
}

impl From<AcmeAccountInput> for certificates::NewAccount {
    fn from(input: AcmeAccountInput) -> Self {
        Self { name: input.name, server: input.server.into(), email: input.email }
    }
}

/// The accounts at ACME servers. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/certificates/accounts",
    tag = "certificates",
    security(("session" = [])),
    responses((status = OK, body = Vec<AcmeAccount>))
)]
async fn acme_accounts(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<AcmeAccount>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.certificates.accounts().await?.into_iter().map(Into::into).collect()))
}

/// Adds an account; it is registered with the server at its first
/// certificate. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/certificates/accounts",
    tag = "certificates",
    security(("session" = [])),
    request_body = AcmeAccountInput,
    responses(
        (status = CREATED, body = AcmeAccount),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_acme_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<AcmeAccountInput>,
) -> Result<(StatusCode, Json<AcmeAccount>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let account = state.certificates.create_account(input.into(), &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(account.into())))
}

/// Changes an account. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/certificates/accounts/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = AcmeAccountInput,
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_acme_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(input): ApiJson<AcmeAccountInput>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.update_account(id, input.into(), &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes an account no certificate uses. Needs `system.manage`.
#[utoipa::path(
    delete,
    path = "/certificates/accounts/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`in_use`"),
    )
)]
async fn delete_acme_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.delete_account(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// How the ACME server is shown the names are ours.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeKind {
    /// A TXT record, made through the DNS provider's API; wildcards need it.
    Dns,
    /// A file the web interface serves on port 80.
    Http,
}

/// A value of a DNS provider's variable, as shown: secret ones only say
/// that they are set.
#[derive(Debug, Serialize, ToSchema)]
pub struct DnsValue {
    pub name: String,
    pub secret: bool,
    /// The value, unless it is secret.
    pub value: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Challenge {
    pub id: i64,
    pub name: String,
    pub method: ChallengeKind,
    /// acme.sh's DNS API script, for DNS.
    pub provider: Option<String>,
    pub values: Vec<DnsValue>,
    pub created_at: i64,
}

impl From<certificates::Challenge> for Challenge {
    fn from(challenge: certificates::Challenge) -> Self {
        let (method, provider, values) = match challenge.method {
            ChallengeMethod::Dns { provider, values } => {
                let secret = |name: &str| {
                    dns_provider(&provider)
                        .and_then(|p| p.variables.iter().find(|v| v.name == name))
                        .is_none_or(|v| v.secret)
                };
                let values = values
                    .into_iter()
                    .map(|(name, value)| {
                        let secret = secret(&name);
                        DnsValue { value: (!secret).then_some(value), name, secret }
                    })
                    .collect();
                (ChallengeKind::Dns, Some(provider), values)
            }
            ChallengeMethod::Http => (ChallengeKind::Http, None, Vec::new()),
        };
        Self {
            id: challenge.id,
            name: challenge.name,
            method,
            provider,
            values,
            created_at: challenge.created_at.unix_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChallengeInput {
    pub name: String,
    pub method: ChallengeKind,
    /// For DNS: one of the plugin's providers.
    #[serde(default)]
    pub provider: Option<String>,
    /// For DNS: the provider's variables. A secret left empty keeps the
    /// value saved before.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

impl ChallengeInput {
    fn method(self) -> Result<(String, ChallengeMethod), ApiError> {
        let method = match self.method {
            ChallengeKind::Dns => ChallengeMethod::Dns {
                provider: self
                    .provider
                    .ok_or_else(|| ApiError::InvalidInput("a DNS challenge needs a provider".into()))?,
                values: self.values,
            },
            ChallengeKind::Http => ChallengeMethod::Http,
        };
        Ok((self.name, method))
    }
}

/// The challenges certificates can use. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/certificates/challenges",
    tag = "certificates",
    security(("session" = [])),
    responses((status = OK, body = Vec<Challenge>))
)]
async fn acme_challenges(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<Challenge>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.certificates.challenges().await?.into_iter().map(Into::into).collect()))
}

/// Adds a challenge. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/certificates/challenges",
    tag = "certificates",
    security(("session" = [])),
    request_body = ChallengeInput,
    responses(
        (status = CREATED, body = Challenge),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_acme_challenge(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<ChallengeInput>,
) -> Result<(StatusCode, Json<Challenge>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let (name, method) = input.method()?;
    let challenge = state.certificates.create_challenge(&name, method, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(challenge.into())))
}

/// Changes a challenge; secrets left empty stay as they were. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/certificates/challenges/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = ChallengeInput,
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`in_use`: a wildcard certificate needs DNS"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_acme_challenge(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(input): ApiJson<ChallengeInput>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    let (name, method) = input.method()?;
    state.certificates.update_challenge(id, &name, method, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a challenge no certificate uses. Needs `system.manage`.
#[utoipa::path(
    delete,
    path = "/certificates/challenges/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`in_use`"),
    )
)]
async fn delete_acme_challenge(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.delete_challenge(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CertificateKey {
    /// ECDSA P-256: small and fast; the default.
    Ec256,
    Rsa2048,
    Rsa4096,
}

impl From<KeyType> for CertificateKey {
    fn from(key: KeyType) -> Self {
        match key {
            KeyType::Ec256 => Self::Ec256,
            KeyType::Rsa2048 => Self::Rsa2048,
            KeyType::Rsa4096 => Self::Rsa4096,
        }
    }
}

impl From<CertificateKey> for KeyType {
    fn from(key: CertificateKey) -> Self {
        match key {
            CertificateKey::Ec256 => Self::Ec256,
            CertificateKey::Rsa2048 => Self::Rsa2048,
            CertificateKey::Rsa4096 => Self::Rsa4096,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Certificate {
    pub id: i64,
    /// The first is its common name; `*.` leads a wildcard.
    pub domains: Vec<String>,
    pub key: CertificateKey,
    pub account_id: i64,
    pub challenge_id: i64,
    /// Renewed by itself a month before it ends.
    pub auto_renew: bool,
    pub created_at: i64,
    /// The last issuance that succeeded, and until when it holds.
    pub issued_at: Option<i64>,
    pub not_after: Option<i64>,
    /// The last try, and what went wrong if it did.
    pub last_attempt_at: Option<i64>,
    pub last_error: Option<String>,
    /// What acme.sh wrote at the last try. Text, never markup.
    pub last_output: Vec<String>,
}

impl From<certificates::Certificate> for Certificate {
    fn from(c: certificates::Certificate) -> Self {
        Self {
            id: c.id,
            domains: c.domains,
            key: c.key.into(),
            account_id: c.account_id,
            challenge_id: c.challenge_id,
            auto_renew: c.auto_renew,
            created_at: c.created_at.unix_seconds(),
            issued_at: c.issued_at.map(|t| t.unix_seconds()),
            not_after: c.not_after.map(|t| t.unix_seconds()),
            last_attempt_at: c.last_attempt_at.map(|t| t.unix_seconds()),
            last_error: c.last_error,
            last_output: c.last_output,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CertificateInput {
    /// 1 to 50 host names; a wildcard (`*.example.org`) needs a DNS
    /// challenge.
    pub domains: Vec<String>,
    #[serde(default = "default_key")]
    pub key: CertificateKey,
    pub account_id: i64,
    pub challenge_id: i64,
    #[serde(default = "yes")]
    pub auto_renew: bool,
}

fn default_key() -> CertificateKey {
    CertificateKey::Ec256
}

fn yes() -> bool {
    true
}

impl From<CertificateInput> for certificates::NewCertificate {
    fn from(input: CertificateInput) -> Self {
        Self {
            domains: input.domains,
            key: input.key.into(),
            account_id: input.account_id,
            challenge_id: input.challenge_id,
            auto_renew: input.auto_renew,
        }
    }
}

/// The certificates. Needs `system.manage`.
#[utoipa::path(
    get,
    path = "/certificates",
    tag = "certificates",
    security(("session" = [])),
    responses((status = OK, body = Vec<Certificate>))
)]
async fn list_certificates(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<Certificate>>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(state.certificates.certificates().await?.into_iter().map(Into::into).collect()))
}

/// Adds a certificate; it is issued with `POST /certificates/{id}/issue`.
/// Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/certificates",
    tag = "certificates",
    security(("session" = [])),
    request_body = CertificateInput,
    responses(
        (status = CREATED, body = Certificate),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_certificate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<CertificateInput>,
) -> Result<(StatusCode, Json<Certificate>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let certificate = state.certificates.create_certificate(input.into(), &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(certificate.into())))
}

/// Changes a certificate; it takes effect at its next issuance. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/certificates/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = CertificateInput,
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_certificate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(input): ApiJson<CertificateInput>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.update_certificate(id, input.into(), &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a certificate the web interface does not show. Needs
/// `system.manage`.
#[utoipa::path(
    delete,
    path = "/certificates/{id}",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`in_use`: the web interface shows it"),
    )
)]
async fn delete_certificate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.delete_certificate(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Issues the certificate now (or again), as a job whose output is what
/// acme.sh writes. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/certificates/{id}/issue",
    tag = "certificates",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`acme_not_installed`, or `refused`: an HTTP challenge without plain HTTP on port 80"),
    )
)]
async fn issue_certificate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::SystemManage)?;
    let job = state.certificates.issue(id, Some(&auth.0.actor()), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Where nginx serves the web interface, and with which certificate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebServer {
    /// Plain HTTP, which points to HTTPS and answers HTTP challenges;
    /// none: not served.
    pub http_port: Option<u16>,
    pub https_port: u16,
    /// An issued certificate; none: the machine's own, self-signed one.
    pub certificate_id: Option<i64>,
}

impl From<certificates::WebServer> for WebServer {
    fn from(web: certificates::WebServer) -> Self {
        Self { http_port: web.http_port, https_port: web.https_port, certificate_id: web.certificate_id }
    }
}

impl From<WebServer> for certificates::WebServer {
    fn from(web: WebServer) -> Self {
        Self { http_port: web.http_port, https_port: web.https_port, certificate_id: web.certificate_id }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingWebServer {
    pub settings: WebServer,
    /// When the old settings return unless confirmed, in Unix seconds.
    pub revert_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebServerSetup {
    pub current: WebServer,
    pub pending: Option<PendingWebServer>,
    /// How long a change waits for confirmation, in seconds.
    pub confirm_within: u16,
}

fn setup_dto(settings: WebServerSettings) -> WebServerSetup {
    WebServerSetup {
        current: settings.current.into(),
        pending: settings
            .pending
            .map(|(web, until)| PendingWebServer { settings: web.into(), revert_at: until.unix_seconds() }),
        confirm_within: CONFIRM_WITHIN_SECONDS,
    }
}

/// The web interface's ports and certificate, and a change waiting to be
/// confirmed. Needs `system.view`.
#[utoipa::path(
    get,
    path = "/system/web-server",
    tag = "system",
    security(("session" = [])),
    responses((status = OK, body = WebServerSetup))
)]
async fn web_server(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<WebServerSetup>, ApiError> {
    auth.require(Permission::SystemView)?;
    Ok(Json(setup_dto(state.certificates.web_server(&client).await?)))
}

/// Applies new ports or another certificate at once; nginx is checked and
/// reloaded. Unless they are confirmed within `confirm_within` seconds
/// (`POST /system/web-server/confirm`, from the new address), the old ones
/// return by themselves. Needs `system.manage`.
#[utoipa::path(
    put,
    path = "/system/web-server",
    tag = "system",
    security(("session" = [])),
    request_body = WebServer,
    responses(
        (status = ACCEPTED, body = WebServerSetup, description = "Applied; waits for confirmation."),
        (status = CONFLICT, body = Problem, description = "`change_pending`, `port_in_use`, or `refused`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: a reserved port, the same port twice, a certificate that does not hold"),
        (status = BAD_GATEWAY, body = Problem, description = "`web_server_failed`: nginx refused the settings; nothing changed"),
    )
)]
async fn apply_web_server(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(input): ApiJson<WebServer>,
) -> Result<(StatusCode, Json<WebServerSetup>), ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.apply_web_server(input.into(), &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(setup_dto(state.certificates.web_server(&client).await?))))
}

/// Keeps the change that waits for confirmation. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/system/web-server/confirm",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = NO_CONTENT, description = "Kept."),
        (status = CONFLICT, body = Problem, description = "`refused`: nothing waits"),
    )
)]
async fn confirm_web_server(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.certificates.confirm_web_server(&auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
