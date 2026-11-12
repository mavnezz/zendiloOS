//! Updates of the NAS: Debian's packages and zendiloOS's releases
//! (`system.manage`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_updates::application::ports::HostError;
use zendiloos_updates::application::{Phase, UpdateOverview, UpdatesError};
use zendiloos_updates::domain::{self as updates, Channel, RunState};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(system_updates))
        .routes(routes!(check_system_updates))
        .routes(routes!(apply_system_updates))
        .routes(routes!(finish_system_update))
        .routes(routes!(set_system_update_settings))
}

impl From<UpdatesError> for ApiError {
    fn from(error: UpdatesError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            UpdatesError::NotChecked => rejected(StatusCode::CONFLICT, "not_checked", None),
            UpdatesError::NothingToDo => rejected(StatusCode::CONFLICT, "nothing_to_update", None),
            UpdatesError::Busy => rejected(StatusCode::CONFLICT, "update_busy", None),
            UpdatesError::JobsRunning(named) => rejected(StatusCode::CONFLICT, "jobs_running", Some(named)),
            UpdatesError::Host(HostError::Refused(reason)) => {
                rejected(StatusCode::CONFLICT, "refused", Some(reason))
            }
            UpdatesError::Host(HostError::Failed(reason)) => {
                rejected(StatusCode::BAD_GATEWAY, "host_failed", Some(reason))
            }
            UpdatesError::Host(HostError::Port(error)) | UpdatesError::Port(error) => error.into(),
        }
    }
}

/// Which releases of zendiloOS are offered: each channel takes those of
/// the steadier ones too.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// Releases.
    Stable,
    /// Pre-releases (release candidates, betas) too.
    Test,
    /// The build of the latest commit too.
    Development,
}

impl From<Channel> for UpdateChannel {
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Stable => Self::Stable,
            Channel::Test => Self::Test,
            Channel::Development => Self::Development,
        }
    }
}

impl From<UpdateChannel> for Channel {
    fn from(channel: UpdateChannel) -> Self {
        match channel {
            UpdateChannel::Stable => Self::Stable,
            UpdateChannel::Test => Self::Test,
            UpdateChannel::Development => Self::Development,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettings {
    pub channel: UpdateChannel,
    /// Look once a day and say what there is; installing stays a click.
    pub check_daily: bool,
}

/// What zendiloosd is doing before a run on the NAS starts.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Idle,
    Checking,
    Downloading,
    /// Starting failed; see `phase_error`.
    Failed,
}

/// A newer release of zendiloOS on the channel.
#[derive(Debug, Serialize, ToSchema)]
pub struct AvailableRelease {
    pub tag: String,
    pub version: String,
    pub channel: UpdateChannel,
    /// Unix seconds.
    pub published_at: Option<i64>,
    /// What changed, in Markdown.
    pub notes: String,
    /// Of the package, in bytes.
    pub size: u64,
}

/// A Debian package an update brings.
#[derive(Debug, Serialize, ToSchema)]
pub struct PackageUpdate {
    pub name: String,
    /// None for one that comes in new.
    pub from: Option<String>,
    pub to: String,
    pub security: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateRunState {
    Running,
    Succeeded,
    Failed,
}

/// The update on the NAS, running or the last one.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateRun {
    pub id: String,
    pub state: UpdateRunState,
    /// It finishes an update that was cut off rather than updating.
    pub finishes: bool,
    /// `lists`, `debian`, `zendiloos`, `modules`; finishing: `configure`,
    /// `repair`, `modules`.
    pub step: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    /// The version of zendiloOS it installs.
    pub zendiloos: Option<String>,
    /// Another kernel waits for a restart.
    pub restart_needed: bool,
    /// The ZFS module is built for the newest kernel.
    pub zfs_ready: bool,
}

/// Why GRUB starts the kernel it starts.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateBootReason {
    /// The newest kernel, which has its ZFS module.
    Newest,
    /// The newest kernel has no ZFS module yet: GRUB keeps starting the
    /// newest one that has, until it is built.
    NewestLacksZfs,
    /// No kernel has a ZFS module: the newest starts.
    NoZfs,
    /// GRUB's configuration does not start the entry zendiloOS saves;
    /// `update-grub` writes it anew.
    NotSaved,
    /// The NAS does not start through GRUB: zendiloOS chooses no kernel.
    NotGrub,
    /// Saving GRUB's default failed.
    Failed,
}

/// Which kernel the NAS starts next: the newest one that has its ZFS
/// module, never a newer one without.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateBoot {
    /// Null where zendiloOS does not choose it (`not_saved`, `not_grub`,
    /// `failed`).
    pub kernel: Option<String>,
    pub reason: UpdateBootReason,
    /// What stood in the way, where something did.
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdatesOverview {
    /// The version of zendiloOS that runs.
    pub current: String,
    /// What changed in it, in Markdown, as its package brought it along;
    /// null for a build without.
    pub current_notes: Option<String>,
    pub settings: UpdateSettings,
    pub phase: UpdatePhase,
    pub phase_error: Option<String>,
    /// When the last check was, Unix seconds.
    pub checked_at: Option<i64>,
    pub release: Option<AvailableRelease>,
    /// Why GitHub could not be asked.
    pub release_error: Option<String>,
    pub packages: Vec<PackageUpdate>,
    /// Why apt could not be asked.
    pub packages_error: Option<String>,
    pub running_kernel: Option<String>,
    pub newest_kernel: Option<String>,
    /// A restart starts another kernel than the running one: the one
    /// `boot` names, or the newest where zendiloOS does not choose.
    pub restart_needed: bool,
    /// The ZFS module is built for the newest kernel. Without it GRUB keeps
    /// starting an older kernel that has it (`boot`), or, where zendiloOS
    /// does not choose, a restart would leave the pools behind.
    pub zfs_ready: bool,
    /// dpkg left packages half installed or half configured: an update
    /// was cut off, and is finished with `POST /system/updates/finish`.
    pub interrupted: bool,
    /// Which kernel starts next and why; null until a check of this
    /// version said.
    pub boot: Option<UpdateBoot>,
    pub run: Option<UpdateRun>,
}

fn boot_dto(boot: &updates::BootChoice) -> UpdateBoot {
    UpdateBoot {
        kernel: boot.kernel.clone(),
        reason: match boot.reason {
            updates::BootReason::Newest => UpdateBootReason::Newest,
            updates::BootReason::NewestLacksZfs => UpdateBootReason::NewestLacksZfs,
            updates::BootReason::NoZfs => UpdateBootReason::NoZfs,
            updates::BootReason::NotSaved => UpdateBootReason::NotSaved,
            updates::BootReason::NotGrub => UpdateBootReason::NotGrub,
            updates::BootReason::Failed => UpdateBootReason::Failed,
        },
        detail: boot.detail.clone(),
    }
}

fn overview_dto(overview: UpdateOverview) -> UpdatesOverview {
    let check = overview.check;
    let host = check.as_ref().and_then(|check| check.host.clone());
    let (phase, phase_error) = match overview.phase {
        Phase::Idle => (UpdatePhase::Idle, None),
        Phase::Checking => (UpdatePhase::Checking, None),
        Phase::Downloading => (UpdatePhase::Downloading, None),
        Phase::Failed(reason) => (UpdatePhase::Failed, Some(reason)),
    };
    UpdatesOverview {
        current: overview.current,
        current_notes: overview.current_notes,
        settings: UpdateSettings {
            channel: overview.settings.channel.into(),
            check_daily: overview.settings.check_daily,
        },
        phase,
        phase_error,
        checked_at: check.as_ref().map(|check| check.checked_at.unix_seconds()),
        release: check.as_ref().and_then(|check| check.release.as_ref()).map(|release| AvailableRelease {
            tag: release.tag.clone(),
            version: release.version.clone(),
            channel: release.channel.into(),
            published_at: release.published_at.map(|at| at.unix_seconds()),
            notes: release.notes.clone(),
            size: release.package.size,
        }),
        release_error: check.as_ref().and_then(|check| check.release_error.clone()),
        packages: host
            .iter()
            .flat_map(|host| host.packages.iter())
            .map(|p: &updates::PackageUpdate| PackageUpdate {
                name: p.name.clone(),
                from: p.from.clone(),
                to: p.to.clone(),
                security: p.security,
            })
            .collect(),
        packages_error: check.as_ref().and_then(|check| check.host_error.clone()),
        running_kernel: host.as_ref().map(|host| host.running_kernel.clone()),
        newest_kernel: host.as_ref().and_then(|host| host.newest_kernel.clone()),
        restart_needed: host.as_ref().is_some_and(updates::HostReport::restart_needed),
        zfs_ready: host.as_ref().is_none_or(|host| host.zfs_ready),
        interrupted: host.as_ref().is_some_and(|host| host.interrupted),
        boot: host.as_ref().and_then(|host| host.boot.as_ref()).map(boot_dto),
        run: overview.run.map(|run| UpdateRun {
            restart_needed: run.report.as_ref().is_some_and(updates::HostReport::restart_needed),
            zfs_ready: run.report.as_ref().is_none_or(|report| report.zfs_ready),
            finishes: run.finishes,
            id: run.id,
            state: match run.state {
                RunState::Running => UpdateRunState::Running,
                RunState::Succeeded => UpdateRunState::Succeeded,
                RunState::Failed => UpdateRunState::Failed,
            },
            step: run.step,
            started_at: run.started_at.unix_seconds(),
            finished_at: run.finished_at.map(|at| at.unix_seconds()),
            error: run.error,
            zendiloos: run.zendiloos,
        }),
    }
}

/// What there is to update, what is going on and how the NAS looks for
/// updates. Needs `system.manage`.
#[utoipa::path(get, path = "/system/updates", tag = "system", security(("session" = [])), responses((status = OK, body = UpdatesOverview)))]
async fn system_updates(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<UpdatesOverview>, ApiError> {
    auth.require(Permission::SystemManage)?;
    Ok(Json(overview_dto(state.updates.overview(&client).await?)))
}

/// Looks for a newer release and for Debian's updates, in the background;
/// the overview says when it is done. Needs `system.manage`.
#[utoipa::path(
    post,
    path = "/system/updates/check",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, description = "Looking."),
        (status = CONFLICT, body = Problem, description = "`update_busy`"),
    )
)]
async fn check_system_updates(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.updates.start_check(&auth.0.actor(), &client).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Installs what the last check found: Debian's packages, then zendiloOS,
/// whose interface restarts on the way. A restart of the NAS is asked for
/// separately. Needs `system.manage`. Refused while background jobs run
/// (`jobs_running`, the detail names them): the agents would see them
/// through before taking the new release.
#[utoipa::path(
    post,
    path = "/system/updates/apply",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, description = "The update begins."),
        (status = CONFLICT, body = Problem, description = "`update_busy`, `jobs_running` (the detail names the jobs), `not_checked`, `nothing_to_update`"),
    )
)]
async fn apply_system_updates(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.updates.apply(&auth.0.actor(), &client).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Finishes an update that was cut off (`interrupted` in the overview):
/// dpkg configures what it unpacked, apt completes what is missing, in a
/// run on the NAS the overview follows as `run`. Needs `system.manage`.
/// Refused while background jobs run, as an update is.
#[utoipa::path(
    post,
    path = "/system/updates/finish",
    tag = "system",
    security(("session" = [])),
    responses(
        (status = ACCEPTED, description = "Finishing begins."),
        (status = CONFLICT, body = Problem, description = "`update_busy`, `jobs_running` (the detail names the jobs)"),
    )
)]
async fn finish_system_update(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SystemManage)?;
    state.updates.finish(&auth.0.actor(), &client).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Which releases are offered and whether the NAS looks daily. Needs
/// `system.manage`.
#[utoipa::path(
    put,
    path = "/system/updates/settings",
    tag = "system",
    security(("session" = [])),
    request_body = UpdateSettings,
    responses((status = OK, body = UpdateSettings))
)]
async fn set_system_update_settings(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<UpdateSettings>,
) -> Result<Json<UpdateSettings>, ApiError> {
    auth.require(Permission::SystemManage)?;
    let settings =
        updates::UpdateSettings { channel: request.channel.into(), check_daily: request.check_daily };
    let saved = state.updates.set_settings(settings, &auth.0.actor(), &client).await?;
    Ok(Json(UpdateSettings { channel: saved.channel.into(), check_daily: saved.check_daily }))
}
