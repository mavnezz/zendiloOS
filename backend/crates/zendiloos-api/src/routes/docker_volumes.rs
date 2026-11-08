//! Docker's volumes, their backup plans and backups, and restoring them
//! (`containers.manage`).

use std::collections::BTreeMap;

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_containers::application::{PlanInput, PlanView, RestoreTarget, VolumeView};
use zendiloos_containers::domain::{
    Backup, BackupFrequency, BackupOutcome, BackupSchedule, BackupTarget, DEFAULT_KEEP, VolumeSize,
};

use super::docker::{LogText, manage};
use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_volumes))
        .routes(routes!(volume_sizes))
        .routes(routes!(prune_volumes))
        .routes(routes!(remove_volume))
        .routes(routes!(backup_plan, set_backup_plan, delete_backup_plan))
        .routes(routes!(list_backups, back_up_now))
        .routes(routes!(restore_backup))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VolumeUserView {
    pub name: String,
    pub running: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanFrequency {
    Daily,
    Weekly,
    Monthly,
}

/// How a volume is backed up.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupPlanView {
    /// The share the archives go to.
    pub share_id: i64,
    /// The folder in it, `""` for its root; the archives land in
    /// `<folder>/<volume>/`.
    pub folder: String,
    pub frequency: PlanFrequency,
    /// For weekly plans: 0 is Monday.
    pub weekday: Option<u8>,
    /// For monthly plans: 1 to 31, the month's last day where it has fewer.
    pub day: Option<u8>,
    /// The host's local hour, 0 to 23.
    pub hour: u8,
    /// Archives kept, the newest.
    pub keep: u32,
    /// The containers that use the volume are stopped while it is backed up.
    pub stop_containers: bool,
    /// When it runs next, Unix seconds.
    pub next_at: i64,
}

impl From<PlanView> for BackupPlanView {
    fn from(view: PlanView) -> Self {
        let plan = view.plan;
        let (frequency, weekday, day) = match plan.schedule.frequency {
            BackupFrequency::Daily => (PlanFrequency::Daily, None, None),
            BackupFrequency::Weekly { weekday } => (PlanFrequency::Weekly, Some(weekday), None),
            BackupFrequency::Monthly { day } => (PlanFrequency::Monthly, None, Some(day)),
        };
        Self {
            share_id: plan.target.share_id,
            folder: plan.target.folder,
            frequency,
            weekday,
            day,
            hour: plan.schedule.hour,
            keep: plan.keep,
            stop_containers: plan.stop_containers,
            next_at: view.next_at.unix_seconds(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupState {
    Running,
    Succeeded,
    Failed,
}

/// One run of a volume's backup.
#[derive(Debug, Serialize, ToSchema)]
pub struct VolumeBackup {
    pub id: i64,
    pub volume: String,
    pub share_id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: BackupState,
    /// Why it failed.
    pub error: Option<String>,
    /// The archive in the share, from its root.
    pub archive: Option<String>,
    pub archive_bytes: Option<u64>,
    /// The manifest beside it.
    pub manifest: Option<String>,
    pub sha256: Option<String>,
    /// What the volume's files held.
    pub data_bytes: Option<u64>,
    /// Who started it; `null` for the schedule.
    pub started_by: Option<String>,
}

impl From<Backup> for VolumeBackup {
    fn from(backup: Backup) -> Self {
        let (state, error) = match &backup.outcome {
            BackupOutcome::Running => (BackupState::Running, None),
            BackupOutcome::Succeeded(_) => (BackupState::Succeeded, None),
            BackupOutcome::Failed(reason) => (BackupState::Failed, Some(reason.clone())),
        };
        let written = backup.written().cloned();
        Self {
            id: backup.id,
            volume: backup.volume,
            share_id: backup.share_id,
            started_at: backup.started_at.unix_seconds(),
            finished_at: backup.finished_at.map(|t| t.unix_seconds()),
            state,
            error,
            archive: written.as_ref().map(|w| w.archive.path.clone()),
            archive_bytes: written.as_ref().map(|w| w.archive.bytes),
            manifest: written.as_ref().map(|w| w.manifest.path.clone()),
            sha256: written.as_ref().map(|w| w.sha256.clone()),
            data_bytes: written.as_ref().map(|w| w.data_bytes),
            started_by: backup.by,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerVolume {
    pub name: String,
    pub driver: String,
    /// The Compose project that made it.
    pub project: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub created_at: Option<i64>,
    /// A local volume whose data lies in Docker's folder: zendiloOS backs
    /// it up and restores it.
    pub backed_up_here: bool,
    /// The containers that mount it, running or not.
    pub used_by: Vec<VolumeUserView>,
    pub plan: Option<BackupPlanView>,
    /// Its newest backup, whatever came of it.
    pub last_backup: Option<VolumeBackup>,
}

impl From<VolumeView> for DockerVolume {
    fn from(view: VolumeView) -> Self {
        let volume = view.volume;
        Self {
            project: volume.project().map(str::to_owned),
            backed_up_here: volume.backed_up_here(),
            name: volume.name,
            driver: volume.driver,
            labels: volume.labels,
            created_at: volume.created_at.map(|t| t.unix_seconds()),
            used_by: view
                .users
                .into_iter()
                .map(|u| VolumeUserView { name: u.name, running: u.running })
                .collect(),
            plan: view.plan.map(Into::into),
            last_backup: view.last_backup.map(Into::into),
        }
    }
}

/// Every volume, by name, with who uses it and how it is backed up. Needs
/// `containers.manage`.
#[utoipa::path(get, path = "/docker/volumes", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerVolume]), (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
        (status = BAD_GATEWAY, body = Problem, description = "`host_failed`: Docker does not answer")))]
async fn list_volumes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerVolume>>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.volumes(&client).await?.into_iter().map(Into::into).collect()))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DockerVolumeSize {
    pub name: String,
    pub bytes: u64,
    /// `false`: counting stopped at its limits; the data takes up at least
    /// `bytes`.
    pub complete: bool,
}

/// What each local volume's data takes up, counted on the NAS; takes a
/// while, so it is asked apart from the list. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/volumes/sizes", tag = "docker", security(("session" = [])),
    responses((status = OK, body = [DockerVolumeSize]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn volume_sizes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<DockerVolumeSize>>, ApiError> {
    manage(&auth)?;
    let sizes = state.containers.volume_sizes(&client).await?;
    Ok(Json(
        sizes
            .into_iter()
            .map(|VolumeSize { name, bytes, complete }| DockerVolumeSize { name, bytes, complete })
            .collect(),
    ))
}

/// Removes every volume no container uses, named ones too, with their
/// data; says what was freed. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/volumes/prune", tag = "docker", security(("session" = [])),
    responses((status = OK, body = LogText), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`")))]
async fn prune_volumes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<LogText>, ApiError> {
    manage(&auth)?;
    Ok(Json(LogText { text: state.containers.prune_volumes(&auth.0.actor(), &client).await? }))
}

/// Removes a volume no container uses, with its data and its backup plan;
/// its backups stay. Needs `containers.manage`.
#[utoipa::path(delete, path = "/docker/volumes/{name}", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = NO_CONTENT, description = "Removed."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`refused`: a container uses it; `volume_busy`: being backed up or restored")))]
async fn remove_volume(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    state.containers.remove_volume(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The volume's backup plan. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/volumes/{name}/backup-plan", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = OK, body = BackupPlanView), (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: it has none")))]
async fn backup_plan(
    auth: Auth,
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<BackupPlanView>, ApiError> {
    manage(&auth)?;
    let plan = state.containers.backup_plan(&name).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(plan.into()))
}

/// A volume's backup plan as set.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupPlanRequest {
    /// A share the administrator may write.
    pub share_id: i64,
    /// The folder in it, `""` for its root.
    #[serde(default)]
    pub folder: String,
    pub frequency: PlanFrequency,
    /// For weekly plans: 0 is Monday.
    #[serde(default)]
    pub weekday: Option<u8>,
    /// For monthly plans: 1 to 31.
    #[serde(default)]
    pub day: Option<u8>,
    /// The host's local hour, 0 to 23.
    pub hour: u8,
    /// 1 to 365; 7 if absent.
    #[serde(default)]
    pub keep: Option<u32>,
    /// Stop the containers that use it meanwhile; yes if absent.
    #[serde(default)]
    pub stop_containers: Option<bool>,
}

/// Makes or replaces the volume's backup plan: into a folder of a share
/// the administrator may write. Needs `containers.manage`.
#[utoipa::path(put, path = "/docker/volumes/{name}/backup-plan", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    request_body = BackupPlanRequest,
    responses((status = OK, body = BackupPlanView), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`; `share_denied`: a share the administrator may not write"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such volume"),
        (status = CONFLICT, body = Problem, description = "`refused`: not a local volume with its data in Docker's folder"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn set_backup_plan(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(request): ApiJson<BackupPlanRequest>,
) -> Result<Json<BackupPlanView>, ApiError> {
    manage(&auth)?;
    let frequency = match (request.frequency, request.weekday, request.day) {
        (PlanFrequency::Daily, _, _) => BackupFrequency::Daily,
        (PlanFrequency::Weekly, Some(weekday), _) => BackupFrequency::Weekly { weekday },
        (PlanFrequency::Monthly, _, Some(day)) => BackupFrequency::Monthly { day },
        (PlanFrequency::Weekly, None, _) => {
            return Err(ApiError::InvalidInput("a weekly plan needs its weekday".into()));
        }
        (PlanFrequency::Monthly, _, None) => {
            return Err(ApiError::InvalidInput("a monthly plan needs its day".into()));
        }
    };
    let input = PlanInput {
        target: BackupTarget { share_id: request.share_id, folder: request.folder },
        schedule: BackupSchedule { frequency, hour: request.hour },
        keep: request.keep.unwrap_or(DEFAULT_KEEP),
        stop_containers: request.stop_containers.unwrap_or(true),
    };
    Ok(Json(state.containers.set_backup_plan(&name, input, &auth.0.actor(), &client).await?.into()))
}

/// Ends the volume's backup plan; its backups stay. Needs
/// `containers.manage`.
#[utoipa::path(delete, path = "/docker/volumes/{name}/backup-plan", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = NO_CONTENT, description = "Ended."), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: it has none")))]
async fn delete_backup_plan(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    manage(&auth)?;
    state.containers.delete_backup_plan(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The volume's backups, the newest first: those kept, and the runs that
/// failed lately. Needs `containers.manage`.
#[utoipa::path(get, path = "/docker/volumes/{name}/backups", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = OK, body = [VolumeBackup]), (status = FORBIDDEN, body = Problem, description = "`forbidden`")))]
async fn list_backups(
    auth: Auth,
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<VolumeBackup>>, ApiError> {
    manage(&auth)?;
    Ok(Json(state.containers.backups(&name).await?.into_iter().map(Into::into).collect()))
}

/// Backs the volume up now as its plan says, in a job. Needs
/// `containers.manage`.
#[utoipa::path(post, path = "/docker/volumes/{name}/backups", tag = "docker", security(("session" = [])), params(("name" = String, Path)),
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: it has no plan"),
        (status = CONFLICT, body = Problem, description = "`volume_busy`: a backup or restore of it is under way")))]
async fn back_up_now(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let job = state.containers.back_up_now(&name, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestoreInto {
    /// The volume it was taken of, made again if it is gone.
    OwnName,
    /// Another volume that exists, named in `volume`.
    Existing,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    pub target: RestoreInto,
    /// The volume, for `existing`.
    #[serde(default)]
    pub volume: Option<String>,
    /// Stop the running containers that use the volume meanwhile, and start
    /// them again after; without, such a volume is not restored.
    #[serde(default)]
    pub stop_containers: bool,
}

/// Replaces a volume's contents with a backup, in a job: the archive is
/// checked against its record first. Needs `containers.manage`.
#[utoipa::path(post, path = "/docker/backups/{id}/restore", tag = "docker", security(("session" = [])), params(("id" = i64, Path)),
    request_body = RestoreRequest,
    responses((status = ACCEPTED, body = Job), (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such backup or volume"),
        (status = CONFLICT, body = Problem, description = "`volume_in_use`: running containers use it (named); `volume_busy`; `refused`: a backup that failed, or a volume not backed up here"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn restore_backup(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<RestoreRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    manage(&auth)?;
    let target = match (request.target, request.volume) {
        (RestoreInto::OwnName, _) => RestoreTarget::OwnName,
        (RestoreInto::Existing, Some(volume)) if !volume.trim().is_empty() => {
            RestoreTarget::Existing(volume.trim().to_owned())
        }
        (RestoreInto::Existing, _) => {
            return Err(ApiError::InvalidInput("name the volume to restore into".into()));
        }
    };
    let job = state
        .containers
        .restore_backup(id, target, request.stop_containers, &auth.0.actor(), &client)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}
