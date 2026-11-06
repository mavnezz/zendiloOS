//! The Docker engine: containers, images, networks and Compose projects,
//! whoever made them (`containers.manage`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_containers::application::ports::ProjectAction;
use zendiloos_containers::application::{ContainerView, ContainersError, ImageView, NewProject, Overview};
use zendiloos_containers::domain::{
    self as containers, ContainerAction, Origin, Risk, UpdateReport, UpdateSchedule, UpdateSettings,
    UpdateState,
};
use zendiloos_identity::domain::Permission;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiQuery, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(docker_overview))
        .routes(routes!(docker_usage))
        .routes(routes!(list_containers))
        .routes(routes!(control_container))
        .routes(routes!(remove_container))
        .routes(routes!(container_logs))
        .routes(routes!(list_images))
        .routes(routes!(pull_image))
        .routes(routes!(remove_image))
        .routes(routes!(prune_images))
        .routes(routes!(list_networks, create_network))
        .routes(routes!(remove_network))
        .routes(routes!(places))
        .routes(routes!(check_compose))
        .routes(routes!(list_projects, create_project))
        .routes(routes!(project, delete_project))
        .routes(routes!(control_project))
        .routes(routes!(redeploy_project))
        .routes(routes!(project_compose, change_compose))
        .routes(routes!(project_logs))
        .routes(routes!(update_settings, set_update_settings))
        .routes(routes!(updates))
        .routes(routes!(check_updates))
        .routes(routes!(apply_updates))
        .routes(routes!(data_location))
        .routes(routes!(move_data))
}

impl From<ContainersError> for ApiError {
    fn from(error: ContainersError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            ContainersError::NotFound => ApiError::NotFound,
            ContainersError::InvalidName(e) => ApiError::InvalidInput(e.to_string()),
            ContainersError::Invalid(detail) => ApiError::InvalidInput(detail),
            ContainersError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            ContainersError::Failed(detail) => rejected(StatusCode::BAD_GATEWAY, "host_failed", Some(detail)),
            ContainersError::NameTaken => rejected(StatusCode::CONFLICT, "project_exists", None),
            ContainersError::AppProject => rejected(StatusCode::CONFLICT, "app_project", None),
            ContainersError::NotOwn => rejected(StatusCode::CONFLICT, "not_own_project", None),
            ContainersError::Risky(risks) => rejected(
                StatusCode::CONFLICT,
                "compose_risks",
                Some(
                    risks
                        .iter()
                        .map(|r| format!("{}: {}", r.service(), r.kind()))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ),
            ContainersError::Denied => rejected(StatusCode::FORBIDDEN, "share_denied", None),
            ContainersError::Port(e) => ApiError::internal(e),
        }
    }
}

fn manage(auth: &Auth) -> Result<(), ApiError> {
    auth.require(Permission::ContainersManage)
}

/// The engine at a glance.
#[derive(Debug, Serialize, ToSchema)]
pub struct DockerOverview {
    pub projects_running: usize,
    pub projects: usize,
    pub containers_running: usize,
    pub containers: usize,
    pub images: usize,
    /// What Docker's data takes up.
    pub data_bytes: u64,
    /// Of the machine's processors, 0 to 100; `null` before the first sample.
    pub cpu_share: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

impl From<Overview> for DockerOverview {
    fn from(o: Overview) -> Self {
        Self {
            projects_running: o.projects_running,
            projects: o.projects,
            containers_running: o.containers_running,
            containers: o.containers,
            images: o.images,
            data_bytes: o.data_bytes,
            cpu_share: o.cpu_share,
            memory_bytes: o.memory_bytes,
            memory_total_bytes: o.memory_total_bytes,
        }
    }
}

/// The engine at a glance. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/overview", tag = "docker", security(("session" = [])),
    responses((status = OK, body = DockerOverview), (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: Docker does not answer")))]
async fn docker_overview(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<DockerOverview>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.overview(&client).await?.into()))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UsageParams {
    /// How far back, at most twelve hours; one hour if absent.
    pub seconds: Option<u32>,
    /// Container names, comma separated; all if absent.
    pub containers: Option<String>,
}

/// One container at one moment.
#[derive(Debug, Serialize, ToSchema)]
pub struct UsagePoint {
    pub container: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub received_per_second: f64,
    pub sent_per_second: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageMoment {
    /// Unix seconds.
    pub at: i64,
    pub points: Vec<UsagePoint>,
}

/// What the containers used, sample by sample, oldest first. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/docker/usage", tag = "docker", security(("session" = [])), params(UsageParams),
    responses((status = OK, body = [UsageMoment]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn docker_usage(
    auth: Auth,
    State(state): State<ApiState>,
    ApiQuery(params): ApiQuery<UsageParams>,
) -> Result<Json<Vec<UsageMoment>>, ApiError> {
    manage(&auth)?;
    let seconds = params.seconds.unwrap_or(3600).clamp(60, 12 * 3600);
    let names: Vec<String> = params
        .containers
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    let moments = state.containers.usage_history(seconds, &names);
    Ok(Json(
        moments
            .into_iter()
            .map(|m| UsageMoment {
                at: m.at.unix_seconds(),
                points: m
                    .points
                    .into_iter()
                    .map(|p| UsagePoint {
                        container: p.container,
                        cpu_percent: p.cpu_percent,
                        memory_bytes: p.memory_bytes,
                        received_per_second: p.received_per_second,
                        sent_per_second: p.sent_per_second,
                    })
                    .collect(),
            })
            .collect(),
    ))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishedPort {
    /// `null`: every address.
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContainerMount {
    /// `bind` or `volume`.
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub read_only: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContainerUsage {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    /// `created`, `running`, `paused`, `restarting`, `removing`, `exited`, `dead`.
    pub state: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    /// Unix seconds.
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub project: Option<String>,
    pub service: Option<String>,
    pub ports: Vec<PublishedPort>,
    pub networks: Vec<String>,
    pub mounts: Vec<ContainerMount>,
    pub restart_policy: String,
    /// The newest sample, while it runs.
    pub usage: Option<ContainerUsage>,
}

impl From<ContainerView> for DockerContainer {
    fn from(view: ContainerView) -> Self {
        let c = view.container;
        Self {
            id: c.id,
            name: c.name,
            image: c.image,
            image_id: c.image_id,
            state: c.state.as_str().to_owned(),
            health: c.health,
            exit_code: c.exit_code,
            created_at: c.created_at.unix_seconds(),
            started_at: c.started_at.map(|t| t.unix_seconds()),
            project: c.project,
            service: c.service,
            ports: c
                .ports
                .into_iter()
                .map(|p| PublishedPort {
                    host_ip: p.host_ip,
                    host_port: p.host_port,
                    container_port: p.container_port,
                    protocol: p.protocol,
                })
                .collect(),
            networks: c.networks,
            mounts: c
                .mounts
                .into_iter()
                .map(|m| ContainerMount {
                    kind: m.kind,
                    source: m.source,
                    destination: m.destination,
                    read_only: m.read_only,
                })
                .collect(),
            restart_policy: c.restart_policy,
            usage: view.usage.map(|u| ContainerUsage {
                cpu_percent: u.cpu_percent,
                memory_bytes: u.memory_bytes,
                memory_limit_bytes: u.memory_limit_bytes,
                received_bytes: u.received_bytes,
                sent_bytes: u.sent_bytes,
            }),
        }
    }
}

/// Every container, by name, with its newest use. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/containers", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerContainer]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn list_containers(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerContainer>>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.containers(&client).await?.into_iter().map(Into::into).collect()))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAction {
    Start,
    Stop,
    Restart,
}

/// Starts, stops or restarts a container. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/containers/{id}/{action}", tag = "docker", security(("session" = [])),
    params(("id" = String, Path), ("action" = String, Path, description = "`start`, `stop` or `restart`")),
    responses((status = NO_CONTENT, description = "Done."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = BAD_GATEWAY, body = Problem, description = "`host_failed`")))]
async fn control_container(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, action)): Path<(String, RunAction)>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    let action = match action {
        RunAction::Start => ContainerAction::Start,
        RunAction::Stop => ContainerAction::Stop,
        RunAction::Restart => ContainerAction::Restart,
    };
    state.containers.control_container(&id, action, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a container that does not run. Needs `containers.manage`.
#[utoipa::path(delete, path = "/docker/containers/{id}", tag = "docker", security(("session" = [])), params(("id" = String, Path)),
    responses((status = NO_CONTENT, description = "Removed."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: it runs")))]
async fn remove_container(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    state.containers.control_container(&id, ContainerAction::Remove, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogParams {
    /// At most 10 000; 500 if absent.
    pub lines: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogText {
    /// With timestamps, oldest first.
    pub text: String,
}

/// The last lines a container wrote. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/containers/{id}/logs", tag = "docker", security(("session" = [])), params(("id" = String, Path), LogParams),
    responses((status = OK, body = LogText), (status = FORBIDDEN, body = Problem, description = "`forbidden`"), (status = NOT_FOUND, body = Problem, description = "`not_found`")))]
async fn container_logs(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<LogParams>,
) -> Result<Json<LogText>, ApiError> {
    manage(&auth)?;
    let text = state.containers.container_logs(&id, params.lines.unwrap_or(500), &client).await?;
    Ok(Json(LogText { text }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerImage {
    pub id: String,
    /// `repository:tag` each.
    pub references: Vec<String>,
    pub digests: Vec<String>,
    pub size_bytes: u64,
    pub created_at: Option<i64>,
    /// The containers made from it, by name.
    pub used_by: Vec<String>,
}

impl From<ImageView> for DockerImage {
    fn from(view: ImageView) -> Self {
        let i = view.image;
        Self {
            id: i.id,
            references: i.references,
            digests: i.digests,
            size_bytes: i.size_bytes,
            created_at: i.created_at.map(|t| t.unix_seconds()),
            used_by: view.used_by,
        }
    }
}

/// Every image, the newest first. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/images", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerImage]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn list_images(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerImage>>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.images(&client).await?.into_iter().map(Into::into).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    /// `postgres:17`, `ghcr.io/owner/app:latest`.
    pub reference: String,
}

/// Fetches an image, in a job. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/images/pull", tag = "docker", security(("session" = [])), request_body = PullRequest,
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn pull_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<PullRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state.containers.pull_image(&request.reference, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Removes an image no container uses. Needs `containers.manage`.
#[utoipa::path(delete, path = "/docker/images/{id}", tag = "docker", security(("session" = [])), params(("id" = String, Path)),
    responses((status = NO_CONTENT, description = "Removed."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: in use")))]
async fn remove_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    state.containers.remove_image(&id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes every image no container uses; says what was freed. Needs
/// `containers.manage`.
#[utoipa::path(post, path = "/docker/images/prune", tag = "docker", security(("session" = [])),
    responses((status = OK, body = LogText), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`")))]
async fn prune_images(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<LogText>, ApiError> {
    manage(&auth)?;
    Ok(Json(LogText { text: state.containers.prune_images(&auth.0.actor(), &client).await? }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub internal: bool,
    pub subnets: Vec<String>,
    pub gateways: Vec<String>,
    /// The containers in it now, by name.
    pub containers: Vec<String>,
    pub project: Option<String>,
    /// Docker's own: it cannot be removed.
    pub built_in: bool,
}

/// Every network, Docker's own first. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/networks", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerNetwork]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn list_networks(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerNetwork>>, ApiError> {
    manage(&auth)?;
    let networks = state.containers.networks(&client).await?;
    Ok(Json(
        networks
            .into_iter()
            .map(|n| DockerNetwork {
                built_in: n.is_built_in(),
                id: n.id,
                name: n.name,
                driver: n.driver,
                internal: n.internal,
                subnets: n.subnets,
                gateways: n.gateways,
                containers: n.containers,
                project: n.project,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewNetwork {
    pub name: String,
    /// `172.30.0.0/24`; Docker's choice if absent.
    #[serde(default)]
    pub subnet: Option<String>,
    /// Only with a subnet.
    #[serde(default)]
    pub gateway: Option<String>,
}

/// Creates a bridge network. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/networks", tag = "docker", security(("session" = [])), request_body = NewNetwork,
    responses((status = CREATED, description = "Created."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = CONFLICT, body = Problem, description = "`refused`"), (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`")))]
async fn create_network(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<NewNetwork>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    let blank = |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let (subnet, gateway) = (blank(request.subnet), blank(request.gateway));
    state
        .containers
        .create_network(request.name.trim(), subnet.as_deref(), gateway.as_deref(), &auth.0.actor(), &client)
        .await?;
    Ok(StatusCode::CREATED)
}

/// Removes a network no container is in. Needs `containers.manage`.
#[utoipa::path(delete, path = "/docker/networks/{name}", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = NO_CONTENT, description = "Removed."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = CONFLICT, body = Problem, description = "`refused`: Docker's own"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: in use")))]
async fn remove_network(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    state.containers.remove_network(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A share a project's folder can go in.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectPlace {
    pub share_id: i64,
    pub share: String,
}

/// The shares the caller may put a project's folder in. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/docker/places", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [ProjectPlace]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn places(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<ProjectPlace>>, ApiError> {
    manage(&auth)?;
    let places = state.containers.places(&auth.0.actor()).await?;
    Ok(Json(places.into_iter().map(|p| ProjectPlace { share_id: p.share_id, share: p.share }).collect()))
}

/// Something a service may do beyond its container.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComposeRisk {
    pub service: String,
    /// `privileged`, `docker_socket`, `system_path`, `host_network`,
    /// `host_namespace`, `devices`, `capabilities`, `unconfined`.
    pub kind: String,
    /// The path, namespace, devices or capabilities.
    pub detail: Option<String>,
}

impl From<Risk> for ComposeRisk {
    fn from(risk: Risk) -> Self {
        Self { service: risk.service().to_owned(), kind: risk.kind().to_owned(), detail: risk.detail() }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeCheck {
    pub name: String,
    /// For a new project: the share and the folder in it. Without, the
    /// existing project's folder.
    #[serde(default)]
    pub share_id: Option<i64>,
    #[serde(default)]
    pub folder: Option<String>,
    pub compose: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ComposeCheckResult {
    /// To be confirmed before it runs; empty if nothing reaches beyond the
    /// containers.
    pub risks: Vec<ComposeRisk>,
}

/// Reads a Compose file as Compose would and names what it lets
/// containers do beyond themselves. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/compose/check", tag = "docker", security(("session" = [])), request_body = ComposeCheck,
    responses((status = OK, body = ComposeCheckResult), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`, `share_denied`"),
        (status = CONFLICT, body = Problem, description = "`refused`: Compose cannot read it, in its words")))]
async fn check_compose(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<ComposeCheck>,
) -> Result<Json<ComposeCheckResult>, ApiError> {
    manage(&auth)?;
    let place = request.share_id.map(|share| (share, request.folder.as_deref().unwrap_or_default()));
    let risks = state
        .containers
        .check_compose(&request.name, place, &request.compose, &auth.0.actor(), &client)
        .await?;
    Ok(Json(ComposeCheckResult { risks: risks.into_iter().map(Into::into).collect() }))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// Made here: its Compose file is edited here.
    Own,
    /// An app of a catalog: the App Center changes it.
    App,
    /// Made another way.
    Foreign,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerProject {
    pub name: String,
    pub kind: ProjectKind,
    /// For an app: its id in the App Center.
    pub app: Option<String>,
    /// Where its Compose file is: on the machine for every kind, and in the
    /// share for one made here (`docker/teslamate/compose.yaml`).
    pub files: Vec<String>,
    pub share_id: Option<i64>,
    pub folder: Option<String>,
    pub created_at: Option<i64>,
    pub created_by: Option<String>,
    pub running: usize,
    pub containers: Vec<DockerContainer>,
}

fn project_dto(project: containers::Project, usage: &[ContainerView]) -> DockerProject {
    let running = project.running();
    let containers = project
        .containers
        .into_iter()
        .map(|container| {
            let usage = usage.iter().find(|v| v.container.id == container.id).and_then(|v| v.usage.clone());
            ContainerView { container, usage }.into()
        })
        .collect();
    let (kind, app, files, share_id, folder, created_at, created_by) = match project.origin {
        Origin::Own(own) => (
            ProjectKind::Own,
            None,
            vec![own.file],
            Some(own.share_id),
            Some(own.folder),
            Some(own.created_at.unix_seconds()),
            Some(own.created_by),
        ),
        Origin::App { app } => (ProjectKind::App, Some(app), vec![], None, None, None, None),
        Origin::Foreign { files } => (ProjectKind::Foreign, None, files, None, None, None, None),
    };
    DockerProject {
        name: project.name,
        kind,
        app,
        files,
        share_id,
        folder,
        created_at,
        created_by,
        running,
        containers,
    }
}

/// Every Compose project, by name. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/projects", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerProject]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn list_projects(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerProject>>, ApiError> {
    manage(&auth)?;
    let projects = state.containers.projects(&client).await?;
    let usage = state.containers.containers(&client).await?;
    Ok(Json(projects.into_iter().map(|p| project_dto(p, &usage)).collect()))
}

/// One project. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/projects/{name}", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = OK, body = DockerProject), (status = FORBIDDEN, body = Problem, description = "`forbidden`"), (status = NOT_FOUND, body = Problem, description = "`not_found`")))]
async fn project(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<Json<DockerProject>, ApiError> {
    manage(&auth)?;
    let project = state.containers.project(&name, &client).await?;
    let usage = state.containers.containers(&client).await?;
    Ok(Json(project_dto(project, &usage)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewDockerProject {
    pub name: String,
    pub share_id: i64,
    /// In the share: `docker/teslamate`; made if missing.
    pub folder: String,
    pub compose: String,
    /// Deploys it right away.
    #[serde(default)]
    pub start: bool,
    /// What the file lets containers do beyond themselves was shown and
    /// accepted.
    #[serde(default)]
    pub accept_risks: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectCreated {
    /// Deploying it, if asked to start.
    pub job: Option<Job>,
}

/// Makes a project: writes its Compose file into the folder, with the
/// caller's permissions, and deploys it if asked. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/projects", tag = "docker", security(("session" = [])), request_body = NewDockerProject,
    responses((status = CREATED, body = ProjectCreated), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`, `share_denied`"),
        (status = CONFLICT, body = Problem, description = "`project_exists`; `refused`: Compose cannot read it; `compose_risks`: not accepted"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn create_project(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<NewDockerProject>,
) -> Result<(StatusCode, Json<ProjectCreated>), ApiError> {
    manage(&auth)?;
    let new = NewProject {
        name: request.name,
        share_id: request.share_id,
        folder: request.folder,
        compose: request.compose,
        start: request.start,
        accept_risks: request.accept_risks,
    };
    let job = state.containers.create_project(new, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(ProjectCreated { job: job.map(Into::into) })))
}

/// Removes a project's containers and networks, in a job; images, data and
/// its folder stay. Not for an app of a catalog. Needs `containers.manage`.
#[utoipa::path(delete, path = "/docker/projects/{name}", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = CONFLICT, body = Problem, description = "`app_project`")))]
async fn delete_project(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state.containers.delete_project(&name, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Starts, stops or restarts a project's containers. Needs
/// `containers.manage`.
#[utoipa::path(post, path = "/docker/projects/{name}/{action}", tag = "docker", security(("session" = [])),
    params(("name" = String, Path), ("action" = String, Path, description = "`start`, `stop` or `restart`")),
    responses((status = NO_CONTENT, description = "Done."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = BAD_GATEWAY, body = Problem, description = "`host_failed`")))]
async fn control_project(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((name, action)): Path<(String, RunAction)>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    let action = match action {
        RunAction::Start => ProjectAction::Start,
        RunAction::Stop => ProjectAction::Stop,
        RunAction::Restart => ProjectAction::Restart,
    };
    state.containers.control_project(&name, action, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fetches the images anew and recreates what changed, in a job. Not for
/// an app of a catalog. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/projects/{name}/redeploy", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = CONFLICT, body = Problem, description = "`app_project`")))]
async fn redeploy_project(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state.containers.redeploy(&name, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ComposeFile {
    /// On the machine.
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectCompose {
    pub files: Vec<ComposeFile>,
    /// Made here: changed and deployed again with `PUT`.
    pub editable: bool,
}

/// A project's Compose files. Not for an app of a catalog: they hold its
/// generated settings. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/projects/{name}/compose", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = OK, body = ProjectCompose), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `share_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"), (status = CONFLICT, body = Problem, description = "`app_project`")))]
async fn project_compose(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<Json<ProjectCompose>, ApiError> {
    manage(&auth)?;
    let source = state.containers.project_compose(&name, &auth.0.actor(), &client).await?;
    Ok(Json(ProjectCompose {
        files: source.files.into_iter().map(|(path, content)| ComposeFile { path, content }).collect(),
        editable: source.editable,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeChange {
    pub compose: String,
    #[serde(default)]
    pub accept_risks: bool,
}

/// Writes a project's changed Compose file and deploys it again, in a job.
/// Only for a project made here. Needs `containers.manage`.
#[utoipa::path(put, path = "/docker/projects/{name}/compose", tag = "docker", security(("session" = [])), params(("name" = String, Path)), request_body = ComposeChange,
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`, `share_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`app_project`, `not_own_project`; `refused`: Compose cannot read it; `compose_risks`")))]
async fn change_compose(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(request): ApiJson<ComposeChange>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state
        .containers
        .change_compose(&name, &request.compose, request.accept_risks, &auth.0.actor(), &client)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// The last lines of the project's containers. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/projects/{name}/logs", tag = "docker", security(("session" = [])), params(("name" = String, Path), LogParams),
    responses((status = OK, body = LogText), (status = FORBIDDEN, body = Problem, description = "`forbidden`"), (status = NOT_FOUND, body = Problem, description = "`not_found`")))]
async fn project_logs(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiQuery(params): ApiQuery<LogParams>,
) -> Result<Json<LogText>, ApiError> {
    manage(&auth)?;
    let text = state.containers.project_logs(&name, params.lines.unwrap_or(500), &client).await?;
    Ok(Json(LogText { text }))
}

/// When newer images are taken: on these days at this time.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageUpdateSchedule {
    /// 0 is Monday.
    pub days: Vec<u8>,
    /// `05:40`, in the machine's time zone.
    pub time: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageUpdateSettings {
    /// Registries are asked every six hours.
    pub check: bool,
    /// `null`: newer images are taken by hand only.
    pub schedule: Option<ImageUpdateSchedule>,
}

impl From<UpdateSettings> for ImageUpdateSettings {
    fn from(settings: UpdateSettings) -> Self {
        Self {
            check: settings.check,
            schedule: settings.schedule.map(|s| ImageUpdateSchedule {
                days: (0..7u8).filter(|day| s.days[usize::from(*day)]).collect(),
                time: format!("{:02}:{:02}", s.minute / 60, s.minute % 60),
            }),
        }
    }
}

fn schedule(dto: &ImageUpdateSchedule) -> Result<UpdateSchedule, ApiError> {
    let invalid = || ApiError::InvalidInput(format!("{:?} is no time of day such as 05:40", dto.time));
    let (hours, minutes) = dto.time.split_once(':').ok_or_else(invalid)?;
    let (hours, minutes): (u16, u16) =
        (hours.parse().map_err(|_| invalid())?, minutes.parse().map_err(|_| invalid())?);
    if hours > 23 || minutes > 59 {
        return Err(invalid());
    }
    let mut days = [false; 7];
    for day in &dto.days {
        *days.get_mut(usize::from(*day)).ok_or_else(|| ApiError::InvalidInput("days are 0 to 6".into()))? =
            true;
    }
    Ok(UpdateSchedule { days, minute: hours * 60 + minutes })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageUpdateState {
    Current,
    Available,
    Unknown,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImageUpdate {
    /// As the containers name it: `postgres:17`.
    pub reference: String,
    pub state: ImageUpdateState,
    /// Why it cannot be told, for `unknown`.
    pub reason: Option<String>,
    /// The containers made from it, by name.
    pub containers: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImageUpdates {
    /// Unix seconds.
    pub checked_at: i64,
    pub images: Vec<ImageUpdate>,
}

impl From<UpdateReport> for ImageUpdates {
    fn from(report: UpdateReport) -> Self {
        Self {
            checked_at: report.checked_at.unix_seconds(),
            images: report
                .images
                .into_iter()
                .map(|image| {
                    let (state, reason) = match image.state {
                        UpdateState::Current => (ImageUpdateState::Current, None),
                        UpdateState::Available => (ImageUpdateState::Available, None),
                        UpdateState::Unknown(reason) => (ImageUpdateState::Unknown, Some(reason)),
                    };
                    ImageUpdate { reference: image.reference, state, reason, containers: image.containers }
                })
                .collect(),
        }
    }
}

/// When newer images are looked for and taken. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/updates/settings", tag = "docker", security(("session" = [])),
    responses((status = OK, body = ImageUpdateSettings), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn update_settings(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<ImageUpdateSettings>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.update_settings().await?.into()))
}

/// Sets them. Needs `containers.manage`.
#[utoipa::path(put, path = "/docker/updates/settings", tag = "docker", security(("session" = [])), request_body = ImageUpdateSettings,
    responses((status = OK, body = ImageUpdateSettings), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn set_update_settings(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<ImageUpdateSettings>,
) -> Result<Json<ImageUpdateSettings>, ApiError> {
    manage(&auth)?;
    let settings = UpdateSettings {
        check: request.check,
        schedule: request.schedule.as_ref().map(schedule).transpose()?,
    };
    Ok(Json(state.containers.set_update_settings(settings, &auth.0.actor(), &client).await?.into()))
}

/// The newest look for newer images, `null` before the first. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/docker/updates", tag = "docker", security(("session" = [])),
    responses((status = OK, body = Option<ImageUpdates>), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn updates(auth: Auth, State(state): State<ApiState>) -> Result<Json<Option<ImageUpdates>>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.update_report().map(Into::into)))
}

/// Asks the registries now. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/updates/check", tag = "docker", security(("session" = [])),
    responses((status = OK, body = ImageUpdates), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`")))]
async fn check_updates(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<ImageUpdates>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.check_updates(&client).await?.into()))
}

/// Updates every project with a newer image, in a job; `job` is `null` if
/// there is none. Apps of catalogs are the App Center's. Needs
/// `containers.manage`.
#[utoipa::path(post, path = "/docker/updates/apply", tag = "docker", security(("session" = [])),
    responses((status = ACCEPTED, body = ProjectCreated), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`")))]
async fn apply_updates(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<ProjectCreated>), ApiError> {
    manage(&auth)?;
    let job = state.containers.apply_updates(Some(&auth.0.actor()), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(ProjectCreated { job: job.map(Into::into) })))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerDataPlace {
    pub pool: String,
    pub volume: u16,
    /// Free for files, if known.
    pub available_bytes: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerDataLocation {
    /// `null`: not on a pool zendiloOS made; it cannot be moved here.
    pub current: Option<DockerDataPlace>,
    /// The other pools.
    pub targets: Vec<DockerDataPlace>,
    pub data_bytes: u64,
}

fn place(p: zendiloos_containers::application::ports::DataPlace) -> DockerDataPlace {
    DockerDataPlace { pool: p.pool, volume: p.volume, available_bytes: p.available_bytes }
}

/// Where Docker's data is, and the pools it could go to. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/docker/data", tag = "docker", security(("session" = [])),
    responses((status = OK, body = DockerDataLocation), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn data_location(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<DockerDataLocation>, ApiError> {
    manage(&auth)?;
    let location = state.containers.data_location(&client).await?;
    Ok(Json(DockerDataLocation {
        current: location.current.map(place),
        targets: location.targets.into_iter().map(place).collect(),
        data_bytes: location.data_bytes,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataMove {
    /// The pool to move Docker's data to.
    pub pool: String,
}

/// Moves Docker's data to another pool, in a job: every container stops
/// meanwhile. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/data/move", tag = "docker", security(("session" = [])), request_body = DataMove,
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such other pool"),
        (status = CONFLICT, body = Problem, description = "`refused`: too little space, or not on a pool zendiloOS made")))]
async fn move_data(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<DataMove>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state.containers.move_data(&request.pool, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}
