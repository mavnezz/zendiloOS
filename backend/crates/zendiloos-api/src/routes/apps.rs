//! App catalogs and their sources (`containers.manage`).

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_apps::application::{AppsError, ListedApp};
use zendiloos_apps::domain as apps;
use zendiloos_identity::domain::Permission;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(sources, add_source))
        .routes(routes!(remove_source))
        .routes(routes!(refresh_sources))
        .routes(routes!(catalog))
        .routes(routes!(catalog_app))
        .routes(routes!(catalog_icon))
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
}

/// An app as the store lists it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CatalogApp {
    pub source_id: i64,
    /// From the community catalog.
    pub official: bool,
    pub id: String,
    pub name: String,
    pub version: String,
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
        permissions: AppPermissions {
            devices: app.permissions.devices.clone(),
            capabilities: app.permissions.capabilities.clone(),
            host_network: app.permissions.host_network,
        },
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
