//! The file manager on a removable drive (`/files/removable/{drive}/…`),
//! as on a share, for administrators (`storage.manage` besides
//! `files.read` or `files.write`) and only while the drive is mounted. The
//! files agent works there with root's access to files; a drive has no
//! recycle bin and no snapshots, so deleting is for good. Copies and moves
//! between a drive and a share go through either side's `transfer`.

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;

use super::{
    CreateFolderRequest, DeleteEntriesRequest, FileQuery, Folder, FolderQuery, RenameRequest, SearchQuery,
    SearchResults, TransferRequest, UploadQuery, UploadedFile, archive_of, deleted, download_from, drive,
    folder_in, folder_made, renamed, search_in, transferred, upload_into,
};
use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::ApiPath as Path;
use crate::extract::{ApiJson, ApiQuery, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(drive_folder))
        .routes(routes!(drive_search))
        .routes(routes!(drive_download))
        .routes(routes!(drive_archive))
        .routes(routes!(drive_upload))
        .routes(routes!(drive_create_folder))
        .routes(routes!(drive_rename))
        .routes(routes!(drive_transfer))
        .routes(routes!(drive_delete))
}

/// A folder's contents on a removable drive. Needs `files.read` and
/// `storage.manage`.
#[utoipa::path(
    get,
    path = "/files/removable/{drive}/folder",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path, description = "The drive's ID, as `/storage/removable` lists it"), FolderQuery),
    responses(
        (status = OK, body = Folder),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such drive mounted, or no such folder"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`: not someone who manages storage"),
    )
)]
async fn drive_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<FolderQuery>,
) -> Result<Json<Folder>, ApiError> {
    let place = drive(&auth, id, Permission::FilesRead)?;
    folder_in(&state, &auth, &client, place, query).await
}

/// Looks for names in a folder of a removable drive and everything below
/// it, as in a share. Needs `files.read` and `storage.manage`.
#[utoipa::path(
    get,
    path = "/files/removable/{drive}/search",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path, description = "The drive's ID, as `/storage/removable` lists it"), SearchQuery),
    responses(
        (status = OK, body = SearchResults),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such drive mounted, or no such folder"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: no text to look for, or too much"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`: not someone who manages storage"),
    )
)]
async fn drive_search(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<SearchResults>, ApiError> {
    let place = drive(&auth, id, Permission::FilesRead)?;
    search_in(&state, &auth, &client, place, query).await
}

/// Downloads a file from a removable drive, as from a share. Needs
/// `files.read` and `storage.manage`.
#[utoipa::path(
    get,
    path = "/files/removable/{drive}/download",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path), FileQuery),
    responses(
        (status = OK, content_type = "application/octet-stream", body = Vec<u8>),
        (status = PARTIAL_CONTENT, content_type = "application/octet-stream", body = Vec<u8>),
        (status = RANGE_NOT_SATISFIABLE, description = "The range starts beyond the end."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn drive_download(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<FileQuery>,
    request_headers: HeaderMap,
) -> Result<Response, ApiError> {
    let place = drive(&auth, id, Permission::FilesRead)?;
    download_from(&state, &auth, &client, place, query, &request_headers).await
}

/// Downloads a folder of a removable drive as a tar archive. Needs
/// `files.read` and `storage.manage`.
#[utoipa::path(
    get,
    path = "/files/removable/{drive}/archive",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path), FolderQuery),
    responses(
        (status = OK, content_type = "application/x-tar", body = Vec<u8>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: not a folder, too many files"),
    )
)]
async fn drive_archive(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<FolderQuery>,
) -> Result<Response, ApiError> {
    let place = drive(&auth, id, Permission::FilesRead)?;
    archive_of(&state, &auth, &client, place, query).await
}

/// Uploads a file onto a removable drive. Needs `files.write` and
/// `storage.manage`.
#[utoipa::path(
    post,
    path = "/files/removable/{drive}/upload",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path), UploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = CREATED, body = UploadedFile),
        (status = BAD_REQUEST, body = Problem, description = "`bad_request`: the upload did not complete"),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`; `permission_denied`: the drive is read only"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn drive_upload(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<UploadedFile>), ApiError> {
    let place = drive(&auth, id, Permission::FilesWrite)?;
    upload_into(state, &auth, client, place, query, body).await
}

/// Creates a folder on a removable drive. Needs `files.write` and
/// `storage.manage`.
#[utoipa::path(
    post,
    path = "/files/removable/{drive}/folders",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path)),
    request_body = CreateFolderRequest,
    responses(
        (status = CREATED),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`; `permission_denied`: the drive is read only"),
    )
)]
async fn drive_create_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<StatusCode, ApiError> {
    let place = drive(&auth, id, Permission::FilesWrite)?;
    folder_made(&state, &auth, &client, place, request).await
}

/// Renames a file or folder on a removable drive. Needs `files.write` and
/// `storage.manage`.
#[utoipa::path(
    post,
    path = "/files/removable/{drive}/rename",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path)),
    request_body = RenameRequest,
    responses(
        (status = OK, body = UploadedFile),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`; `permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn drive_rename(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<RenameRequest>,
) -> Result<Json<UploadedFile>, ApiError> {
    let place = drive(&auth, id, Permission::FilesWrite)?;
    renamed(&state, &auth, &client, place, request).await
}

/// Copies or moves entries of a removable drive into a folder of it, of
/// another drive or of a share, as a job. What lands in a share is the
/// administrator's and takes the folder's permissions, as an upload does.
/// Needs `files.write` and `storage.manage`.
#[utoipa::path(
    post,
    path = "/files/removable/{drive}/transfer",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path)),
    request_body = TransferRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`; `permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `invalid_operation`"),
    )
)]
async fn drive_transfer(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<TransferRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let place = drive(&auth, id, Permission::FilesWrite)?;
    transferred(&state, &auth, &client, place, request).await
}

/// Deletes entries of a removable drive for good (a drive has no recycle
/// bin), as a job. Needs `files.write` and `storage.manage`.
#[utoipa::path(
    post,
    path = "/files/removable/{drive}/delete",
    tag = "files",
    security(("session" = [])),
    params(("drive" = String, Path)),
    request_body = DeleteEntriesRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`; `permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `invalid_operation`"),
    )
)]
async fn drive_delete(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<DeleteEntriesRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let place = drive(&auth, id, Permission::FilesWrite)?;
    deleted(&state, &auth, &client, place, request).await
}
