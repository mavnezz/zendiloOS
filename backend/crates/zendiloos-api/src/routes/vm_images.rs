//! The VMs' image library (`vms.manage`): ISOs added from a share, as a
//! job, or uploaded from the browser, streamed into the library; renamed,
//! and removed while no VM has them in a drive.

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_vms::application::{ImageUpload, VmImageView};
use zendiloos_vms::domain::{ImageOrigin, IsoChoice};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, ApiQuery, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_vm_images, import_vm_image))
        .routes(routes!(upload_vm_image))
        .routes(routes!(rename_vm_image, delete_vm_image))
}

/// Where an image came from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmImageOrigin {
    /// Copied from a file in a share.
    Share { share: String, path: String },
    /// Uploaded from a browser, a file of that name.
    Upload { file: String },
}

/// An ISO of the library, kept once for as many VMs as want it.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmImage {
    /// Twelve hexadecimal digits.
    pub id: String,
    pub name: String,
    /// None while it is not known: an ISO a VM had before the library,
    /// until the library is looked at.
    pub bytes: Option<u64>,
    pub origin: VmImageOrigin,
    pub added_at: i64,
    /// In the library: false while it is being copied or uploaded.
    pub ready: bool,
    /// The VMs that have it in a drive; it stays while there are any.
    pub used_by: Vec<String>,
}

fn image_dto(view: VmImageView) -> VmImage {
    let image = view.image;
    VmImage {
        id: image.id,
        name: image.name,
        bytes: image.bytes,
        origin: match image.origin {
            ImageOrigin::Share { share, path } => VmImageOrigin::Share { share, path },
            ImageOrigin::Upload { file } => VmImageOrigin::Upload { file },
        },
        added_at: image.added_at.unix_seconds(),
        ready: image.ready,
        used_by: view.used_by,
    }
}

/// The library, oldest first, with the VMs each image is in. Needs
/// `vms.manage`.
#[utoipa::path(get, path = "/vms/images", tag = "vms", security(("session" = [])), responses((status = OK, body = [VmImage])))]
async fn list_vm_images(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<VmImage>>, ApiError> {
    auth.require(Permission::VmsManage)?;
    Ok(Json(state.vms.images(&client).await?.into_iter().map(image_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportVmImageRequest {
    pub share_id: i64,
    /// In the share: `isos/debian-13.1.0-amd64-netinst.iso`.
    pub path: String,
    /// What it is called; its file's name if none.
    #[serde(default)]
    pub name: Option<String>,
}

/// An image recorded, and the job that copies it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportedVmImage {
    pub image: VmImage,
    pub job: Job,
}

/// Adds an ISO from a share to the library, as a job: the file, which the
/// administrator must be able to read, copied into the VMs' storage (its
/// blocks cloned where the pool can). Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/images",
    tag = "vms",
    security(("session" = [])),
    request_body = ImportVmImageRequest,
    responses(
        (status = ACCEPTED, body = ImportedVmImage),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such file"),
        (status = CONFLICT, body = Problem, description = "`vms_not_installed`, `vms_not_set_up`, `name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: no .iso, no name; `iso_denied`"),
    )
)]
async fn import_vm_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<ImportVmImageRequest>,
) -> Result<(StatusCode, Json<ImportedVmImage>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let choice = IsoChoice { share_id: request.share_id, path: request.path };
    let (image, job) =
        state.vms.import_image(choice, request.name.as_deref(), &auth.0.actor(), &client).await?;
    let image = image_dto(VmImageView { image, used_by: Vec::new() });
    Ok((StatusCode::ACCEPTED, Json(ImportedVmImage { image, job: job.into() })))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadVmImageQuery {
    /// What the image is called: the file's name, as a rule.
    pub name: String,
}

/// Uploads an ISO into the library: the request body, unchanged, of the
/// length `Content-Length` says, at most the plugin's
/// `image_upload_max_bytes`. It goes through a pipe into a unit of the
/// agent's, which names it only once it came whole. Needs `vms.manage`.
#[utoipa::path(
    post,
    path = "/vms/images/upload",
    tag = "vms",
    security(("session" = [])),
    params(UploadVmImageQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = CREATED, body = VmImage),
        (status = BAD_REQUEST, body = Problem, description = "`bad_request`: no length, or the upload did not complete"),
        (status = CONFLICT, body = Problem, description = "`vms_not_set_up`, `name_taken`; `refused`: no room on the pool"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: no name, larger than allowed"),
    )
)]
async fn upload_vm_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<UploadVmImageQuery>,
    body: Body,
) -> Result<(StatusCode, Json<VmImage>), ApiError> {
    auth.require(Permission::VmsManage)?;
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| ApiError::BadRequest("an upload says how long it is (Content-Length)".into()))?;
    let actor = auth.0.actor();
    let ImageUpload { mut file, ticket } = state.vms.begin_upload(&query.name, announced, &client).await?;
    // In a task of its own: if the client goes away, the handler is
    // dropped, but what was begun must still be given up.
    let task = tokio::spawn(async move {
        let written = async {
            let mut chunks = body.into_data_stream();
            let mut received = 0u64;
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(std::io::Error::other)?;
                received += chunk.len() as u64;
                if received > announced {
                    return Err(std::io::Error::other(format!("more than the {announced} bytes announced")));
                }
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            if received == announced {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("it ended after {received} of {announced} bytes")))
            }
        }
        .await;
        // Closed: the agent's unit sees the end.
        drop(file);
        match written {
            Ok(()) => state.vms.finish_upload(ticket, &actor, &client).await.map_err(ApiError::from),
            Err(error) => {
                state.vms.abort_upload(ticket, &actor, &client).await;
                Err(ApiError::BadRequest(format!("the upload did not complete: {error}")))
            }
        }
    });
    let image = task.await.map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, Json(image_dto(VmImageView { image, used_by: Vec::new() }))))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameVmImageRequest {
    pub name: String,
}

/// Renames an image; its file stays as it is. Needs `vms.manage`.
#[utoipa::path(
    put,
    path = "/vms/images/{id}",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    request_body = RenameVmImageRequest,
    responses(
        (status = OK, body = VmImage),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn rename_vm_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<RenameVmImageRequest>,
) -> Result<Json<VmImage>, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.rename_image(&id, &request.name, &auth.0.actor(), &client).await?;
    let view = state
        .vms
        .images(&client)
        .await?
        .into_iter()
        .find(|view| view.image.id == id)
        .ok_or(ApiError::NotFound)?;
    Ok(Json(image_dto(view)))
}

/// Removes an image from the library. Refused while a VM has it in a
/// drive, the VMs named. Needs `vms.manage`.
#[utoipa::path(
    delete,
    path = "/vms/images/{id}",
    tag = "vms",
    security(("session" = [])),
    params(("id" = String, Path)),
    responses(
        (status = NO_CONTENT),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`image_in_use`: the VMs that have it, in `detail`; `refused`: still being added"),
    )
)]
async fn delete_vm_image(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::VmsManage)?;
    state.vms.delete_image(&id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
