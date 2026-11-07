//! Removable drives: USB sticks and disks, memory cards. For
//! administrators only (`storage.manage`): the drives are mounted with
//! the rights of every administrator, and nobody else may reach them.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_storage::application::removable::{Hold, ListedDrive};
use zendiloos_storage::domain as storage;

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::ApiPath as Path;
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(removable_drives))
        .routes(routes!(mount_removable))
        .routes(routes!(eject_removable))
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemovableFilesystem {
    /// FAT12, FAT16 or FAT32.
    Fat,
    Exfat,
    Ntfs,
    Ext4,
}

impl From<storage::RemovableFilesystem> for RemovableFilesystem {
    fn from(filesystem: storage::RemovableFilesystem) -> Self {
        match filesystem {
            storage::RemovableFilesystem::Fat => Self::Fat,
            storage::RemovableFilesystem::Exfat => Self::Exfat,
            storage::RemovableFilesystem::Ntfs => Self::Ntfs,
            storage::RemovableFilesystem::Ext4 => Self::Ext4,
        }
    }
}

/// Whether a drive can be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemovableState {
    Mounted,
    /// Ejected without being powered off: mounted again by hand only.
    Ejected,
    /// It could not be mounted; `problem` says why.
    Failed,
    /// Not mounted yet.
    Unmounted,
}

/// A removable drive: a file system on a USB stick or disk, a memory card.
#[derive(Debug, Serialize, ToSchema)]
pub struct RemovableDrive {
    /// The file system's UUID: names the drive here and in the paths of
    /// its files.
    pub id: String,
    pub label: Option<String>,
    pub filesystem: RemovableFilesystem,
    /// The device carrying it, e.g. `sdf1`.
    pub device: String,
    /// The disk it is on, e.g. `sdf`.
    pub disk: String,
    /// The disk's model.
    pub model: Option<String>,
    pub size_bytes: u64,
    pub state: RemovableState,
    /// Write-protected, or mounted so because it could not be written.
    pub read_only: bool,
    /// Where it is mounted on the machine, while it is:
    /// `/run/zendiloos-removable/<id>`.
    pub path: Option<String>,
    /// Only known while mounted.
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    /// Why it could not be mounted, in the agent's words.
    pub problem: Option<String>,
}

impl From<ListedDrive> for RemovableDrive {
    fn from(listed: ListedDrive) -> Self {
        let ListedDrive { drive, hold } = listed;
        let state = match (&drive.mount_point, &hold) {
            (Some(_), _) => RemovableState::Mounted,
            (None, Some(Hold::Ejected)) => RemovableState::Ejected,
            (None, Some(Hold::Failed(_))) => RemovableState::Failed,
            (None, None) => RemovableState::Unmounted,
        };
        Self {
            id: drive.id,
            label: drive.label,
            filesystem: drive.filesystem.into(),
            device: drive.device,
            disk: drive.disk,
            model: drive.model,
            size_bytes: drive.size_bytes,
            state,
            read_only: drive.read_only,
            path: drive.mount_point,
            used_bytes: drive.space.map(|s| s.used_bytes),
            available_bytes: drive.space.map(|s| s.available_bytes),
            problem: match hold {
                Some(Hold::Failed(problem)) => Some(problem),
                _ => None,
            },
        }
    }
}

/// The removable drives plugged in. New ones are mounted while they are
/// listed, as on Windows; zendiloOS also looks once a minute by itself.
/// Needs `storage.manage`.
#[utoipa::path(
    get,
    path = "/storage/removable",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<RemovableDrive>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn removable_drives(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<RemovableDrive>>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let drives = state.storage.removable.list(&client).await?;
    Ok(Json(drives.into_iter().map(RemovableDrive::from).collect()))
}

/// Mounts a drive that was ejected, or that could not be mounted before;
/// as it is if it is mounted. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/removable/{id}/mount",
    tag = "storage",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = OK, body = RemovableDrive),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such drive is plugged in"),
        (status = CONFLICT, body = Problem, description = "`refused`: the agent's checks, in its words"),
    )
)]
async fn mount_removable(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<Json<RemovableDrive>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let mounted = state.storage.removable.mount(&id, &auth.0.actor(), &client).await?;
    Ok(Json(mounted.into()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EjectRemovable {
    /// Also takes the disk off the bus, ready to be pulled out (the
    /// default). Without, the drive stays plugged in, not mounted, e.g. to
    /// make a pool of it.
    #[serde(default = "yes")]
    pub power_off: bool,
}

fn yes() -> bool {
    true
}

/// Ejects a drive: unmounts it, refused while files on it are open, and
/// powers its disk off. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/removable/{id}/eject",
    tag = "storage",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = EjectRemovable,
    responses(
        (status = NO_CONTENT, description = "Ejected."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such drive is plugged in"),
        (status = CONFLICT, body = Problem, description = "`drive_in_use`: files on it are open; `refused`"),
    )
)]
async fn eject_removable(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<EjectRemovable>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::StorageManage)?;
    state.storage.removable.eject(&id, request.power_off, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
