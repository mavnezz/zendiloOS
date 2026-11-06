//! App catalogs and their sources (`containers.manage`).

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_apps::application::{AppsError, InstallRequest, InstalledView, ListedApp};
use zendiloos_apps::domain as apps;
use zendiloos_identity::domain::Permission;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(sources, add_source))
        .routes(routes!(remove_source))
        .routes(routes!(refresh_sources))
        .routes(routes!(catalog))
        .routes(routes!(catalog_app))
        .routes(routes!(catalog_icon))
        .routes(routes!(docker, enable_docker))
        .routes(routes!(install_docker))
        .routes(routes!(installed, install))
        .routes(routes!(installed_app, uninstall))
        .routes(routes!(retry_app))
        .routes(routes!(update_app))
        .routes(routes!(start_app))
        .routes(routes!(stop_app))
        .routes(routes!(app_logs))
}

impl From<AppsError> for ApiError {
    fn from(error: AppsError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            AppsError::SourceNotFound | AppsError::AppNotFound => ApiError::NotFound,
            AppsError::InvalidSource(e) => ApiError::InvalidInput(e.to_string()),
            AppsError::Duplicate => rejected(StatusCode::CONFLICT, "source_exists", None),
            AppsError::Official => rejected(StatusCode::CONFLICT, "official_source", None),
            AppsError::NotACatalog(detail) => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "not_a_catalog", Some(detail))
            }
            AppsError::Unreachable(detail) => {
                rejected(StatusCode::BAD_GATEWAY, "catalog_unreachable", Some(detail))
            }
            AppsError::PoolNotFound => ApiError::NotFound,
            AppsError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            AppsError::NotInstalled => ApiError::NotFound,
            AppsError::AlreadyInstalled => rejected(StatusCode::CONFLICT, "app_installed", None),
            AppsError::IdTooLong => rejected(StatusCode::UNPROCESSABLE_ENTITY, "app_id_too_long", None),
            AppsError::DockerNotReady => rejected(StatusCode::CONFLICT, "docker_not_ready", None),
            AppsError::DockerInstalled => rejected(StatusCode::CONFLICT, "docker_installed", None),
            AppsError::PermissionsNotConfirmed => {
                rejected(StatusCode::CONFLICT, "permissions_not_confirmed", None)
            }
            AppsError::InvalidValue(e) => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "invalid_app_value", Some(e.to_string()))
            }
            AppsError::Busy => rejected(StatusCode::CONFLICT, "app_busy", None),
            AppsError::ConfirmationMismatch => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch", None)
            }
            AppsError::UpToDate => rejected(StatusCode::CONFLICT, "up_to_date", None),
            AppsError::HostFailed(detail) => rejected(StatusCode::BAD_GATEWAY, "host_failed", Some(detail)),
            AppsError::Port(e) => ApiError::internal(e),
        }
    }
}

/// A catalog apps come from.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppSource {
    pub id: i64,
    /// Where its index lies.
    pub address: String,
    /// The community catalog: built in. Apps of other catalogs are not
    /// reviewed by it.
    pub official: bool,
    /// Unix seconds.
    pub added_at: i64,
    /// The last time its index was fetched, in Unix seconds.
    pub refreshed_at: Option<i64>,
    /// Why that fetch failed; the apps read before stay listed.
    pub refresh_error: Option<String>,
    /// Apps left out because they fail the format or the policy.
    pub skipped: usize,
}

fn source_dto(source: apps::Source) -> AppSource {
    AppSource {
        id: source.id,
        address: source.index,
        official: source.official,
        added_at: source.added_at.unix_seconds(),
        refreshed_at: source.refreshed.as_ref().map(|r| r.at.unix_seconds()),
        refresh_error: source.refreshed.as_ref().and_then(|r| r.error.clone()),
        skipped: source.refreshed.map_or(0, |r| r.skipped),
    }
}

/// The catalogs apps come from: the community catalog first. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/apps/sources", tag = "apps", security(("session" = [])), responses((status = OK, body = Vec<AppSource>)))]
async fn sources(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<AppSource>>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(state.apps.sources().await?.into_iter().map(source_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddSourceRequest {
    /// A GitHub repository (`github.com/owner/repository`) or the https
    /// address of an `index.json`.
    pub address: String,
}

/// Adds a catalog. It must answer with an index this version reads; its
/// apps are not reviewed by the community catalog. Needs
/// `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/sources",
    tag = "apps",
    security(("session" = [])),
    request_body = AddSourceRequest,
    responses(
        (status = CREATED, body = AppSource),
        (status = CONFLICT, body = Problem, description = "`source_exists`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `not_a_catalog`"),
        (status = BAD_GATEWAY, body = Problem, description = "`catalog_unreachable`"),
    )
)]
async fn add_source(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<AddSourceRequest>,
) -> Result<(StatusCode, Json<AppSource>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let source = state.apps.add_source(&request.address, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(source_dto(source))))
}

/// Removes a catalog; the community catalog stays. Needs
/// `containers.manage`.
#[utoipa::path(
    delete,
    path = "/apps/sources/{id}",
    tag = "apps",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Removed."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`official_source`"),
    )
)]
async fn remove_source(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::ContainersManage)?;
    state.apps.remove_source(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fetches every catalog again. Needs `containers.manage`.
#[utoipa::path(post, path = "/apps/sources/refresh", tag = "apps", security(("session" = [])), responses((status = OK, body = Vec<AppSource>)))]
async fn refresh_sources(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<Vec<AppSource>>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(state.apps.refresh().await?.into_iter().map(source_dto).collect()))
}

/// Name and tagline in another language.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppTexts {
    pub name: Option<String>,
    pub tagline: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppPermissions {
    pub devices: Vec<String>,
    pub capabilities: Vec<String>,
    pub host_network: bool,
    /// The Docker socket: control over every container, so over the NAS.
    pub docker_socket: bool,
}

fn permissions_dto(permissions: &apps::Permissions) -> AppPermissions {
    AppPermissions {
        devices: permissions.devices.clone(),
        capabilities: permissions.capabilities.clone(),
        host_network: permissions.host_network,
        docker_socket: permissions.docker_socket,
    }
}

/// A newer state of an installed app.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppUpdate {
    /// The version its catalog names, if it names one.
    pub version: Option<String>,
}

/// An app as the store lists it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogApp {
    pub source_id: i64,
    /// From the community catalog.
    pub official: bool,
    pub id: String,
    pub name: String,
    /// The app's own version, if its catalog names one.
    pub version: Option<String>,
    pub tagline: String,
    pub categories: Vec<String>,
    /// Exceptions from the policy, confirmed when installing.
    pub permissions: AppPermissions,
    /// By ISO 639-1 language; English is the default.
    pub translations: BTreeMap<String, AppTexts>,
    /// Changes with every new version.
    pub digest: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppPort {
    pub id: String,
    pub label: String,
    pub default: u16,
    pub fixed: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppShare {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub read_write: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppSetting {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    /// `text`, `password`, `number` or `boolean`.
    pub kind: String,
    pub default: Option<String>,
    pub required: bool,
    /// zendiloOS makes it up; nobody is asked.
    pub generate: bool,
    /// How many characters a generated value has.
    pub length: Option<u8>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppWeb {
    pub port: String,
    pub path: String,
    pub https: bool,
}

/// Everything about an app before installing it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogAppDetail {
    #[serde(flatten)]
    pub summary: CatalogApp,
    pub description: String,
    pub developer: String,
    pub website: String,
    pub source_code: String,
    pub license: String,
    pub architectures: Vec<String>,
    pub notes: Option<String>,
    pub web: Option<AppWeb>,
    pub ports: Vec<AppPort>,
    pub shares: Vec<AppShare>,
    pub settings: Vec<AppSetting>,
    /// The Compose file as written; zendiloOS fills in the variables.
    pub compose: String,
}

fn summary(listed: &ListedApp) -> CatalogApp {
    let app = &listed.app;
    CatalogApp {
        source_id: listed.source.id,
        official: listed.source.official,
        id: app.id.clone(),
        name: app.name.clone(),
        version: app.version.clone(),
        tagline: app.tagline.clone(),
        categories: app.categories.clone(),
        permissions: permissions_dto(&app.permissions),
        translations: app
            .translations
            .iter()
            .map(|(language, t)| {
                let texts = AppTexts {
                    name: t.name.clone(),
                    tagline: t.tagline.clone(),
                    description: t.description.clone(),
                    notes: t.notes.clone(),
                };
                (language.clone(), texts)
            })
            .collect(),
        digest: app.digest.clone(),
    }
}

fn detail(listed: ListedApp) -> CatalogAppDetail {
    let summary = summary(&listed);
    let app = listed.app;
    CatalogAppDetail {
        summary,
        description: app.description,
        developer: app.developer,
        website: app.website,
        source_code: app.source_code,
        license: app.license,
        architectures: app.architectures,
        notes: app.notes,
        web: app.web.map(|w| AppWeb {
            port: w.port,
            path: w.path,
            https: w.scheme == apps::WebScheme::Https,
        }),
        ports: app
            .ports
            .into_iter()
            .map(|p| AppPort { id: p.id, label: p.label, default: p.default, fixed: p.fixed })
            .collect(),
        shares: app
            .shares
            .into_iter()
            .map(|s| AppShare {
                id: s.id,
                label: s.label,
                description: s.description,
                read_write: s.access == apps::Access::ReadWrite,
            })
            .collect(),
        settings: app
            .settings
            .into_iter()
            .map(|s| AppSetting {
                id: s.id,
                label: s.label,
                description: s.description,
                kind: match s.kind {
                    apps::SettingKind::Text => "text",
                    apps::SettingKind::Password => "password",
                    apps::SettingKind::Number => "number",
                    apps::SettingKind::Boolean => "boolean",
                }
                .to_owned(),
                default: s.default,
                required: s.required,
                generate: s.generate,
                length: s.generate.then_some(s.length),
            })
            .collect(),
        compose: app.compose,
    }
}

/// Every app of every catalog, by name. Needs `containers.manage`.
#[utoipa::path(get, path = "/apps/catalog", tag = "apps", security(("session" = [])), responses((status = OK, body = Vec<CatalogApp>)))]
async fn catalog(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<CatalogApp>>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(state.apps.apps().await?.iter().map(summary).collect()))
}

/// One app of a catalog, with what it needs from the NAS. Needs
/// `containers.manage`.
#[utoipa::path(
    get,
    path = "/apps/catalog/{source}/{app}",
    tag = "apps",
    security(("session" = [])),
    params(("source" = i64, Path), ("app" = String, Path)),
    responses((status = OK, body = CatalogAppDetail), (status = NOT_FOUND, body = Problem, description = "`not_found`"))
)]
async fn catalog_app(
    auth: Auth,
    State(state): State<ApiState>,
    Path((source, app)): Path<(i64, String)>,
) -> Result<Json<CatalogAppDetail>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(detail(state.apps.app(source, &app).await?)))
}

/// The app's icon, a PNG checked when it was fetched. Needs
/// `containers.manage`.
#[utoipa::path(
    get,
    path = "/apps/catalog/{source}/{app}/icon",
    tag = "apps",
    security(("session" = [])),
    params(("source" = i64, Path), ("app" = String, Path)),
    responses(
        (status = OK, content_type = "image/png", body = Vec<u8>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = BAD_GATEWAY, body = Problem, description = "`catalog_unreachable`"),
    )
)]
async fn catalog_icon(
    auth: Auth,
    State(state): State<ApiState>,
    Path((source, app)): Path<(i64, String)>,
) -> Result<Response, ApiError> {
    auth.require(Permission::ContainersManage)?;
    let png = state.apps.icon(source, &app).await?;
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
        // The address carries the app's digest, so a new version is a new address.
        (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400")),
        (header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'")),
    ];
    Ok((headers, png).into_response())
}

/// A pool apps can keep Docker's data on.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppVolume {
    pub pool: String,
    pub number: u16,
}

/// Whether apps can run on this NAS.
#[derive(Debug, Serialize, ToSchema)]
pub struct DockerSetup {
    /// Debian's `docker.io` is installed.
    pub installed: bool,
    pub running: bool,
    /// The engine's version, while it runs.
    pub version: Option<String>,
    /// Where Docker's data is, if zendiloOS put it on a pool.
    pub volume: Option<AppVolume>,
    /// Pools it could go to.
    pub volumes: Vec<AppVolume>,
}

fn docker_dto(setup: zendiloos_apps::application::DockerSetup) -> DockerSetup {
    let volume = |v: apps::AppVolume| AppVolume { pool: v.pool, number: v.number };
    DockerSetup {
        installed: setup.installed,
        running: setup.running,
        version: setup.version,
        volume: setup.volume.map(volume),
        volumes: setup.volumes.into_iter().map(volume).collect(),
    }
}

/// Whether apps can run: Docker installed, its data on a pool. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/apps/docker", tag = "apps", security(("session" = [])), responses((status = OK, body = DockerSetup)))]
async fn docker(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<DockerSetup>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(docker_dto(state.docker.setup(&client).await?)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnableDockerRequest {
    /// A pool zendiloOS created.
    pub pool: String,
}

/// Puts Docker's data (images, containers) on a pool; Docker restarts.
/// Needs `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/docker",
    tag = "apps",
    security(("session" = [])),
    request_body = EnableDockerRequest,
    responses(
        (status = OK, body = DockerSetup),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such pool"),
        (status = CONFLICT, body = Problem, description = "`refused`: Docker missing, or its data on another pool"),
    )
)]
async fn enable_docker(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<EnableDockerRequest>,
) -> Result<Json<DockerSetup>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(docker_dto(state.docker.enable(&request.pool, &auth.0.actor(), &client).await?)))
}

/// Installs Docker from Debian (`docker.io`, `docker-compose`) and puts
/// its data on a pool, as a job; apps can be installed afterwards. Needs
/// `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/docker/install",
    tag = "apps",
    security(("session" = [])),
    request_body = EnableDockerRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such pool"),
        (status = CONFLICT, body = Problem, description = "`docker_installed`"),
    )
)]
async fn install_docker(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<EnableDockerRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let job = state.docker.install(&request.pool, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// What Docker makes of an installed app.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// Some of its containers run, others do not.
    Partly,
    /// A container keeps restarting: usually the app fails.
    Restarting,
    Stopped,
}

/// Where the app's web interface is, on the NAS's own address.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppWebAddress {
    pub port: u16,
    pub path: String,
    pub https: bool,
}

/// An installed app.
#[derive(Debug, Serialize, ToSchema)]
pub struct InstalledApp {
    pub id: String,
    /// The catalog it came from; it may be gone since.
    pub source_id: i64,
    pub name: String,
    pub version: Option<String>,
    pub tagline: String,
    pub translations: BTreeMap<String, AppTexts>,
    /// Of the version installed; for its icon.
    pub digest: String,
    /// `installing`, `installed`, `failed` or `removing`.
    pub state: String,
    /// `None` while Docker cannot be asked.
    pub run_state: Option<RunState>,
    pub web: Option<AppWebAddress>,
    /// Port id → host port.
    pub ports: BTreeMap<String, u16>,
    /// Share id → the share it got: its name, or a folder in it
    /// (`media/Films`).
    pub shares: BTreeMap<String, String>,
    pub permissions: AppPermissions,
    /// What its catalog offers now, if it is newer.
    pub update: Option<AppUpdate>,
    /// The pool its data and Docker's are on.
    pub pool: String,
    pub installed_at: i64,
    pub updated_at: i64,
}

fn installed_dto(view: InstalledView) -> InstalledApp {
    let InstalledView { app, containers, update } = view;
    let web = app.web().map(|web| AppWebAddress {
        port: web.port,
        path: web.path,
        https: web.scheme == apps::WebScheme::Https,
    });
    let run_state = containers.map(|containers| match containers.state() {
        apps::RunState::Running => RunState::Running,
        apps::RunState::Partly => RunState::Partly,
        apps::RunState::Restarting => RunState::Restarting,
        apps::RunState::Stopped => RunState::Stopped,
    });
    let catalog = app.app;
    InstalledApp {
        id: app.id,
        source_id: app.source_id,
        name: catalog.name,
        version: catalog.version,
        tagline: catalog.tagline,
        translations: catalog
            .translations
            .into_iter()
            .map(|(language, t)| {
                let texts =
                    AppTexts { name: t.name, tagline: t.tagline, description: t.description, notes: t.notes };
                (language, texts)
            })
            .collect(),
        digest: catalog.digest,
        state: app.state.as_str().to_owned(),
        run_state,
        web,
        ports: app.values.ports,
        shares: app.values.shares,
        permissions: permissions_dto(&catalog.permissions),
        update: update.map(|update| AppUpdate { version: update.version }),
        pool: app.pool,
        installed_at: app.installed_at.unix_seconds(),
        updated_at: app.updated_at.unix_seconds(),
    }
}

/// The installed apps, with what Docker makes of them. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/apps/installed", tag = "apps", security(("session" = [])), responses((status = OK, body = Vec<InstalledApp>)))]
async fn installed(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(state.installs.list(&client).await?.into_iter().map(installed_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallAppRequest {
    pub source: i64,
    pub app: String,
    /// Share id → the share picked for it: its name, or a folder in it
    /// (`media/Films`), which the app then mounts instead of the whole
    /// share.
    #[serde(default)]
    pub shares: BTreeMap<String, String>,
    /// Port id → host port; missing ones get their default.
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
    /// Setting id → value; missing ones get their default. Generated
    /// settings are made by zendiloOS.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// The administrator confirmed the app's permissions.
    #[serde(default)]
    pub confirm_permissions: bool,
    /// The pool the app's data goes on, one of `volumes` of `/apps/docker`;
    /// the one Docker's data is on if absent.
    #[serde(default)]
    pub pool: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Installing {
    pub app: InstalledApp,
    pub job: Job,
}

/// Installs an app from a catalog: its data and Docker's on the same pool,
/// an account of its own, access to the shares picked; the agent checks it
/// against the catalog's policy before anything runs. Needs
/// `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/installed",
    tag = "apps",
    security(("session" = [])),
    request_body = InstallAppRequest,
    responses(
        (status = ACCEPTED, body = Installing),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such app"),
        (status = CONFLICT, body = Problem, description = "`app_installed`, `docker_not_ready`, `permissions_not_confirmed`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_app_value`, `app_id_too_long`"),
    )
)]
async fn install(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<InstallAppRequest>,
) -> Result<(StatusCode, Json<Installing>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let request = InstallRequest {
        source: request.source,
        app: request.app,
        shares: request.shares,
        ports: request.ports,
        settings: request.settings,
        permissions_confirmed: request.confirm_permissions,
        pool: request.pool,
    };
    let (app, job) = state.installs.install(request, &auth.0.actor(), &client).await?;
    let view = InstalledView { app, containers: None, update: None };
    Ok((StatusCode::ACCEPTED, Json(Installing { app: installed_dto(view), job: job.into() })))
}

/// One installed app. Needs `containers.manage`.
#[utoipa::path(
    get,
    path = "/apps/installed/{id}",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses((status = OK, body = InstalledApp), (status = NOT_FOUND, body = Problem, description = "`not_found`"))
)]
async fn installed_app(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<InstalledApp>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    Ok(Json(installed_dto(state.installs.get(&id, &client).await?)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UninstallRequest {
    /// The app's id, repeated: its data goes with it.
    pub confirm: String,
}

/// Removes an app with its containers, images and data, then its access
/// to shares and its account, as a job. Needs `containers.manage`.
#[utoipa::path(
    delete,
    path = "/apps/installed/{id}",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = UninstallRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`app_busy`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`confirmation_mismatch`"),
    )
)]
async fn uninstall(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<UninstallRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let job = state.installs.uninstall(&id, &request.confirm, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Installs a failed app again, with its values. Needs
/// `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/installed/{id}/retry",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`app_busy`: not failed"),
    )
)]
async fn retry_app(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let job = state.installs.retry(&id, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAppRequest {
    /// The administrator confirmed what the new version may do beyond the
    /// old one.
    #[serde(default)]
    pub confirm_permissions: bool,
}

/// Brings an app to the version its catalog offers now, keeping its
/// values. Needs `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/installed/{id}/update",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = UpdateAppRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: the app or its catalog entry"),
        (status = CONFLICT, body = Problem, description = "`up_to_date`, `app_busy`, `permissions_not_confirmed`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_app_value`: the new version needs a value"),
    )
)]
async fn update_app(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<UpdateAppRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::ContainersManage)?;
    let job = state.installs.update(&id, request.confirm_permissions, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Starts an installed app's containers. Needs `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/installed/{id}/start",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Started."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`app_busy`, `refused`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: Docker's words"),
    )
)]
async fn start_app(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::ContainersManage)?;
    state.installs.start(&id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stops an installed app's containers. Needs `containers.manage`.
#[utoipa::path(
    post,
    path = "/apps/installed/{id}/stop",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Stopped."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`app_busy`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: Docker's words"),
    )
)]
async fn stop_app(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::ContainersManage)?;
    state.installs.stop(&id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogsQuery {
    /// The last this many lines, 1 to 5000; 200 if left out.
    pub lines: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppLogs {
    /// Each line with the container's name and a timestamp, oldest first.
    pub text: String,
}

/// The last lines its containers wrote. Needs `containers.manage`.
#[utoipa::path(
    get,
    path = "/apps/installed/{id}/logs",
    tag = "apps",
    security(("session" = [])),
    params(("id" = String, Path), LogsQuery),
    responses(
        (status = OK, body = AppLogs),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: Docker's words"),
    )
)]
async fn app_logs(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<AppLogs>, ApiError> {
    auth.require(Permission::ContainersManage)?;
    let lines = query.lines.unwrap_or(200).clamp(1, 5000);
    Ok(Json(AppLogs { text: state.installs.logs(&id, lines, &client).await? }))
}
