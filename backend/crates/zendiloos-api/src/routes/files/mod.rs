//! The file manager: shares' contents as the signed-in user (`files.read`,
//! `files.write`). The host acts with the user's own credentials, so the
//! same permissions apply as over SMB and NFS.

mod download;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_sharing::application::{FilesError, Transfer, Upload, UsableShare};
use zendiloos_sharing::domain::{self as sharing, FilePath, ShareId};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiQuery, Client};
use crate::routes::jobs::Job;
use crate::routes::shares::ShareAccessLevel;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(my_shares))
        .routes(routes!(list_folder))
        .routes(routes!(download))
        .routes(routes!(upload))
        .routes(routes!(create_folder))
        .routes(routes!(rename_entry))
        .routes(routes!(transfer_entries))
        .routes(routes!(delete_entries))
        .routes(routes!(restore_entries))
}

impl From<FilesError> for ApiError {
    fn from(error: FilesError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            FilesError::ShareNotFound | FilesError::NotFound => ApiError::NotFound,
            FilesError::Denied => rejected(StatusCode::FORBIDDEN, "permission_denied", None),
            FilesError::Exists => ApiError::NameTaken,
            FilesError::InvalidName | FilesError::InvalidPath(_) => ApiError::InvalidInput(error.to_string()),
            FilesError::Invalid(reason) => {
                rejected(StatusCode::UNPROCESSABLE_ENTITY, "invalid_operation", Some(reason))
            }
            FilesError::Port(e) => ApiError::internal(e),
        }
    }
}

fn path(value: &str) -> Result<FilePath, ApiError> {
    FilePath::parse(value).map_err(|e| ApiError::InvalidInput(e.to_string()))
}

fn paths(values: &[String]) -> Result<Vec<FilePath>, ApiError> {
    values.iter().map(|value| path(value)).collect()
}

/// A share the signed-in user may use.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileShare {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub access: ShareAccessLevel,
    /// Deleted entries go to `.recycle/<user>` in the share.
    pub recycle_bin: bool,
}

impl From<UsableShare> for FileShare {
    fn from(usable: UsableShare) -> Self {
        Self {
            id: usable.share.id.get(),
            name: usable.share.name.to_string(),
            description: usable.share.description,
            access: match usable.access {
                sharing::Access::Read => ShareAccessLevel::Read,
                sharing::Access::ReadWrite => ShareAccessLevel::ReadWrite,
            },
            recycle_bin: usable.share.recycle_bin,
        }
    }
}

/// Shares whose rules name the signed-in user or one of their groups.
/// Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares",
    tag = "files",
    security(("session" = [])),
    responses((status = OK, body = Vec<FileShare>)),
)]
async fn my_shares(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<FileShare>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let shares = state.files.shares(&auth.0.actor()).await?;
    Ok(Json(shares.into_iter().map(FileShare::from).collect()))
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FileEntry {
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    /// Last modification, Unix seconds.
    pub modified: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Folder {
    /// The folder, relative to the share's root; empty for the root.
    pub path: String,
    pub entries: Vec<FileEntry>,
    /// Too many entries; the rest is left out.
    pub truncated: bool,
    /// The user may create and delete entries here.
    pub writable: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FolderQuery {
    /// Relative to the share's root, e.g. `photos/2024`; empty for the root.
    #[serde(default)]
    pub path: String,
}

/// A folder's contents. Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares/{id}/folder",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), FolderQuery),
    responses(
        (status = OK, body = Folder),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such share, or not shared with the user, or no such folder"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
    )
)]
async fn list_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<FolderQuery>,
) -> Result<Json<Folder>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let folder = path(&query.path)?;
    let listing = state.files.list(&auth.0.actor(), ShareId::new(id), folder.clone(), &client).await?;
    Ok(Json(Folder {
        path: folder.as_str().to_owned(),
        entries: listing
            .entries
            .into_iter()
            .map(|entry| FileEntry {
                name: entry.name,
                kind: match entry.kind {
                    sharing::EntryKind::File => FileKind::File,
                    sharing::EntryKind::Directory => FileKind::Directory,
                    sharing::EntryKind::Symlink => FileKind::Symlink,
                    sharing::EntryKind::Other => FileKind::Other,
                },
                size: entry.size,
                modified: entry.modified.unix_seconds(),
            })
            .collect(),
        truncated: listing.truncated,
        writable: listing.writable,
    }))
}

/// `Content-Disposition` for `name`: an ASCII fallback and the exact name
/// (RFC 6266, RFC 8187); `inline` lets the browser show it.
fn disposition(name: &str, inline: bool) -> HeaderValue {
    let fallback: String = name
        .chars()
        .map(|c| if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' { c } else { '_' })
        .collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    let kind = if inline { "inline" } else { "attachment" };
    HeaderValue::from_str(&format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FileQuery {
    /// Relative to the share's root, e.g. `photos/2024/beach.jpg`.
    pub path: String,
    /// Let the browser show the file, if it is a picture, sound, video or
    /// plain text; anything else is saved regardless.
    #[serde(default)]
    pub inline: bool,
}

/// Downloads a file, as an attachment unless a harmless kind is asked for
/// `inline`. One byte range (`Range`) can be asked for, to resume a
/// download (`If-Range` with the `Last-Modified` given) or seek in a video.
/// Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares/{id}/download",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), FileQuery),
    responses(
        (status = OK, content_type = "application/octet-stream", body = Vec<u8>),
        (status = PARTIAL_CONTENT, content_type = "application/octet-stream", body = Vec<u8>),
        (status = RANGE_NOT_SATISFIABLE, description = "The range starts beyond the end."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: not a file"),
    )
)]
async fn download(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<FileQuery>,
    request_headers: HeaderMap,
) -> Result<Response, ApiError> {
    auth.require(Permission::FilesRead)?;
    let file_path = path(&query.path)?;
    let name = file_path.name().ok_or_else(|| ApiError::InvalidInput("not a file".into()))?.to_owned();
    let (mut file, size) = state.files.open(&auth.0.actor(), ShareId::new(id), file_path, &client).await?;

    let modified = file.metadata().await.ok().and_then(|m| m.modified().ok()).map(httpdate::fmt_http_date);
    let preview = query.inline.then(|| download::preview_type(&name)).flatten();
    let mut headers = HeaderMap::new();
    let content_type = preview.unwrap_or("application/octet-stream");
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CONTENT_DISPOSITION, disposition(&name, preview.is_some()));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // Shown in the browser, it still may not run anything.
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    if let Some(modified) = modified.as_deref().and_then(|m| HeaderValue::from_str(m).ok()) {
        headers.insert(header::LAST_MODIFIED, modified);
    }

    // A range applies to the file as it was when the browser got its date;
    // without one, or if it changed since, the whole file comes again.
    let range = request_headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let unchanged = match request_headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(date) => modified.as_deref() == Some(date),
    };
    let part = if unchanged { download::part(range, size) } else { download::Part::Whole };
    let (status, length) = match part {
        download::Part::Whole => (StatusCode::OK, size),
        download::Part::Bytes { start, end } => {
            file.seek(std::io::SeekFrom::Start(start)).await.map_err(ApiError::internal)?;
            let range = format!("bytes {start}-{end}/{size}");
            headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&range).map_err(ApiError::internal)?);
            (StatusCode::PARTIAL_CONTENT, end - start + 1)
        }
        download::Part::Unsatisfiable => {
            let range = HeaderValue::from_str(&format!("bytes */{size}")).map_err(ApiError::internal)?;
            return Ok((StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, range)]).into_response());
        }
    };
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    let chunks = futures_util::stream::try_unfold(file.take(length), |mut file| async move {
        let mut buffer = vec![0; 256 * 1024];
        let read = file.read(&mut buffer).await?;
        buffer.truncate(read);
        Ok::<_, std::io::Error>((read > 0).then(|| (Bytes::from(buffer), file)))
    });
    Ok((status, headers, Body::from_stream(chunks)).into_response())
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadQuery {
    /// The folder, relative to the share's root; empty for the root.
    #[serde(default)]
    pub path: String,
    /// The new file's name.
    pub name: String,
    /// Replace a file of that name.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UploadedFile {
    pub path: String,
}

/// Uploads a file: the request body, unchanged, any size the volume has
/// room for. Written under a hidden name and renamed when complete. Needs
/// `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/upload",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), UploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = CREATED, body = UploadedFile),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn upload(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<UploadedFile>), ApiError> {
    auth.require(Permission::FilesWrite)?;
    let actor = auth.0.actor();
    let folder = path(&query.path)?;
    let Upload { mut file, ticket } = state
        .files
        .begin_upload(&actor, ShareId::new(id), folder, &query.name, query.replace, &client)
        .await?;
    // In a task of its own: if the client goes away, the handler is
    // dropped, but the half-written file must still be removed.
    let task = tokio::spawn(async move {
        let written = async {
            let mut chunks = body.into_data_stream();
            while let Some(chunk) = chunks.next().await {
                file.write_all(&chunk.map_err(std::io::Error::other)?).await?;
            }
            file.sync_all().await
        }
        .await;
        match written {
            Ok(()) => state.files.finish_upload(&actor, ticket, &client).await.map_err(ApiError::from),
            Err(error) => {
                state.files.abort_upload(&actor, ticket, &client).await;
                Err(ApiError::BadRequest(format!("the upload did not complete: {error}")))
            }
        }
    });
    let stored = task.await.map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, Json(UploadedFile { path: stored.as_str().to_owned() })))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateFolderRequest {
    /// The new folder, relative to the share's root.
    pub path: String,
}

/// Creates a folder. Needs `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/folders",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = CreateFolderRequest,
    responses(
        (status = CREATED),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
    )
)]
async fn create_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::FilesWrite)?;
    state.files.make_directory(&auth.0.actor(), ShareId::new(id), path(&request.path)?, &client).await?;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameRequest {
    pub path: String,
    /// The new name, in the same folder.
    pub name: String,
}

/// Renames a file or folder. Needs `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/rename",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = RenameRequest,
    responses(
        (status = OK, body = UploadedFile),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn rename_entry(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<RenameRequest>,
) -> Result<Json<UploadedFile>, ApiError> {
    auth.require(Permission::FilesWrite)?;
    let renamed = state
        .files
        .rename(&auth.0.actor(), ShareId::new(id), path(&request.path)?, &request.name, &client)
        .await?;
    Ok(Json(UploadedFile { path: renamed.as_str().to_owned() }))
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    Copy,
    Move,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    pub mode: TransferMode,
    /// Entries of this share, relative to its root.
    pub paths: Vec<String>,
    /// The share to copy or move into; may be this one.
    pub target_share: i64,
    /// The folder there, relative to its root.
    #[serde(default)]
    pub target_path: String,
}

/// Copies or moves entries into a folder, possibly of another share, as a
/// job. A copy whose name is taken gets a free one, `name (2).ext`; a move
/// fails. Needs `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/transfer",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = TransferRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `invalid_operation`"),
    )
)]
async fn transfer_entries(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<TransferRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::FilesWrite)?;
    let mode = match request.mode {
        TransferMode::Copy => Transfer::Copy,
        TransferMode::Move => Transfer::Move,
    };
    let job = state
        .files
        .transfer(
            &auth.0.actor(),
            mode,
            ShareId::new(id),
            paths(&request.paths)?,
            ShareId::new(request.target_share),
            path(&request.target_path)?,
            &client,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteEntriesRequest {
    pub paths: Vec<String>,
}

/// Moves entries into the user's recycle bin if the share has one, deletes
/// them for good otherwise (and inside the recycle bin), as a job. Needs
/// `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/delete",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = DeleteEntriesRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn delete_entries(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<DeleteEntriesRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::FilesWrite)?;
    let job = state.files.delete(&auth.0.actor(), ShareId::new(id), paths(&request.paths)?, &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreEntriesRequest {
    /// Entries of the user's recycle bin: `.recycle/<user>/…`.
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RestoredEntries {
    /// Where each went, in the order asked.
    pub paths: Vec<String>,
}

/// Moves entries out of the user's recycle bin back to where they were
/// deleted from; folders on the way are made again, and a taken name gets
/// a free one beside it. Stops at the first that fails. Needs `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/restore",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = RestoreEntriesRequest,
    responses(
        (status = OK, body = RestoredEntries),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: not in your bin"),
    )
)]
async fn restore_entries(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<RestoreEntriesRequest>,
) -> Result<Json<RestoredEntries>, ApiError> {
    auth.require(Permission::FilesWrite)?;
    let restored =
        state.files.restore(&auth.0.actor(), ShareId::new(id), paths(&request.paths)?, &client).await?;
    Ok(Json(RestoredEntries { paths: restored.iter().map(|p| p.as_str().to_owned()).collect() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_keep_the_exact_name() {
        assert_eq!(
            disposition("Urlaub 2024 ü \"x\".jpg", false),
            "attachment; filename=\"Urlaub 2024 _ _x_.jpg\"; filename*=UTF-8''Urlaub%202024%20%C3%BC%20%22x%22.jpg"
        );
    }
}
