//! The file manager: shares' contents as the signed-in user (`files.read`,
//! `files.write`). The host acts with the user's own credentials, so the
//! same permissions apply as over SMB and NFS.

mod download;
mod removable;

use crate::extract::ApiPath as Path;
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::RequestContext;
use zendiloos_sharing::application::{FileVolume, FilesError, Transfer, Upload, UsableShare, VolumeFolder};
use zendiloos_sharing::domain::{self as sharing, FilePath, PlaceId, ShareId};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiQuery, Client};
use crate::routes::jobs::Job;
use crate::routes::shares::ShareAccessLevel;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(my_shares))
        .routes(routes!(file_volumes))
        .routes(routes!(favourites, set_favourites))
        .routes(routes!(list_folder))
        .routes(routes!(search_folder))
        .routes(routes!(download))
        .routes(routes!(download_folder))
        .routes(routes!(upload))
        .routes(routes!(create_folder))
        .routes(routes!(rename_entry))
        .routes(routes!(transfer_entries))
        .routes(routes!(delete_entries))
        .routes(routes!(restore_entries))
        .routes(routes!(file_snapshots))
        .routes(routes!(restore_from_snapshot))
        .merge(removable::routes())
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

/// A share the signed-in user may use, or their personal folder.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileShare {
    /// `0` for the personal folder, which only its owner sees.
    pub id: i64,
    /// `home` for the personal folder.
    pub name: String,
    /// The personal folder of the signed-in user.
    pub home: bool,
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
            home: false,
            description: usable.share.description,
            access: match usable.access {
                sharing::Access::Read => ShareAccessLevel::Read,
                sharing::Access::ReadWrite => ShareAccessLevel::ReadWrite,
            },
            recycle_bin: usable.share.recycle_bin,
        }
    }
}

/// The signed-in user's personal folder, if they have one, then the
/// shares whose rules let them in. Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares",
    tag = "files",
    security(("session" = [])),
    responses((status = OK, body = Vec<FileShare>)),
)]
async fn my_shares(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<FileShare>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let actor = auth.0.actor();
    let mut listed: Vec<FileShare> = Vec::new();
    if state.files.home(&actor).await?.is_some() {
        listed.push(FileShare {
            id: sharing::HOME_SHARE.get(),
            name: "home".to_owned(),
            home: true,
            description: None,
            access: ShareAccessLevel::ReadWrite,
            recycle_bin: true,
        });
    }
    listed.extend(state.files.shares(&actor).await?.into_iter().map(FileShare::from));
    Ok(Json(listed))
}

/// A folder directly on a volume: a share, whether shared on the network
/// or not.
#[derive(Debug, Serialize, ToSchema)]
pub struct VolumeFolderDto {
    /// The share's ID, which the file routes take.
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// Offered on the network (SMB, NFS); otherwise used on the NAS only.
    pub shared: bool,
    /// What the signed-in user may do in it; `null`: listed for someone
    /// who manages shares, but they may not open it.
    pub access: Option<ShareAccessLevel>,
    /// Deleted entries go to `.recycle/<user>` in it.
    pub recycle_bin: bool,
}

impl From<VolumeFolder> for VolumeFolderDto {
    fn from(folder: VolumeFolder) -> Self {
        Self {
            id: folder.share.id.get(),
            name: folder.share.name.to_string(),
            description: folder.share.description,
            shared: folder.share.shared,
            access: folder.access.map(ShareAccessLevel::from),
            recycle_bin: folder.share.recycle_bin,
        }
    }
}

/// A volume and its folders the signed-in user sees.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileVolumeDto {
    /// `1` for `/volume1`.
    pub volume: u16,
    pub pool: String,
    pub mounted: bool,
    /// By name.
    pub folders: Vec<VolumeFolderDto>,
}

impl From<FileVolume> for FileVolumeDto {
    fn from(volume: FileVolume) -> Self {
        Self {
            volume: volume.number,
            pool: volume.pool,
            mounted: volume.mounted,
            folders: volume.folders.into_iter().map(VolumeFolderDto::from).collect(),
        }
    }
}

/// The volumes with their folders, for the file manager's tree. Someone
/// who manages shares (`shares.manage`) sees every volume and every folder
/// on it, those they may not open marked; everyone else only the folders
/// they may use, and the volumes holding one. Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/volumes",
    tag = "files",
    security(("session" = [])),
    responses((status = OK, body = Vec<FileVolumeDto>)),
)]
async fn file_volumes(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<FileVolumeDto>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let everything = auth.0.can(Permission::SharesManage);
    let volumes = state.files.volumes(&auth.0.actor(), everything, &client).await?;
    Ok(Json(volumes.into_iter().map(FileVolumeDto::from).collect()))
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
    /// The name is not UTF-8: `name` is only what it looks like, another
    /// entry may have exactly that name, and a request with it reaches that
    /// one. Shown, never acted on; rename it over SMB or NFS.
    pub unreadable: bool,
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
    /// In this snapshot of the share, read only (its label).
    #[serde(default)]
    pub snapshot: Option<String>,
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
    folder_in(&state, &auth, &client, ShareId::new(id).into(), query).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// The folder to search, with everything below it; empty for the root.
    #[serde(default)]
    pub path: String,
    /// What names contain, whatever the case: 1 to 255 bytes, no control
    /// characters.
    pub text: String,
    /// Also names starting with `.`, and what is in such folders.
    #[serde(default)]
    pub hidden: bool,
    /// In this snapshot of the share, read only (its label).
    #[serde(default)]
    pub snapshot: Option<String>,
}

/// An entry a search found.
#[derive(Debug, Serialize, ToSchema)]
pub struct FoundFile {
    /// Where it is, relative to the root searched in, its name included.
    pub path: String,
    #[serde(flatten)]
    pub entry: FileEntry,
}

/// Why a search ended before it had looked everywhere.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchStop {
    /// It found as many as it answers with (500).
    Results,
    /// Its time was up (a few seconds).
    Time,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResults {
    /// The folder searched, relative to the root.
    pub path: String,
    /// In the order they were come across.
    pub entries: Vec<FoundFile>,
    /// Set when it did not look everywhere.
    pub stopped: Option<SearchStop>,
}

/// Looks for names in a folder and everything below it, as the user may
/// see them: not into links, other file systems, other people's recycle
/// bins (nor into the bins at all unless searched from inside one), and
/// never by a name that is not UTF-8. At most 500 entries within a few
/// seconds. Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares/{id}/search",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), SearchQuery),
    responses(
        (status = OK, body = SearchResults),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: no such share, or not shared with the user, or no such folder"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: no text to look for, or too much"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
    )
)]
async fn search_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<SearchResults>, ApiError> {
    auth.require(Permission::FilesRead)?;
    search_in(&state, &auth, &client, ShareId::new(id).into(), query).await
}

fn file_entry(entry: sharing::Entry) -> FileEntry {
    FileEntry {
        name: entry.name,
        kind: match entry.kind {
            sharing::EntryKind::File => FileKind::File,
            sharing::EntryKind::Directory => FileKind::Directory,
            sharing::EntryKind::Symlink => FileKind::Symlink,
            sharing::EntryKind::Other => FileKind::Other,
        },
        size: entry.size,
        modified: entry.modified.unix_seconds(),
        unreadable: entry.unreadable,
    }
}

async fn search_in(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    query: SearchQuery,
) -> Result<Json<SearchResults>, ApiError> {
    let folder = path(&query.path)?;
    let snapshot = query.snapshot.as_deref();
    let found = state
        .files
        .search(&auth.0.actor(), place, folder.clone(), &query.text, query.hidden, snapshot, client)
        .await?;
    Ok(Json(SearchResults {
        path: folder.as_str().to_owned(),
        entries: found
            .entries
            .into_iter()
            .map(|found| FoundFile { path: found.path.as_str().to_owned(), entry: file_entry(found.entry) })
            .collect(),
        stopped: found.stopped.map(|stop| match stop {
            sharing::SearchStop::Results => SearchStop::Results,
            sharing::SearchStop::Time => SearchStop::Time,
        }),
    }))
}

/// A removable drive, for someone who manages storage and may use files
/// that way (`files.read` or `files.write`).
fn drive(auth: &Auth, id: String, files: Permission) -> Result<PlaceId, ApiError> {
    auth.require(files)?;
    auth.require(Permission::StorageManage)?;
    Ok(PlaceId::Removable(id))
}

async fn folder_in(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    query: FolderQuery,
) -> Result<Json<Folder>, ApiError> {
    let folder = path(&query.path)?;
    let snapshot = query.snapshot.as_deref();
    let listing = state.files.list(&auth.0.actor(), place, folder.clone(), snapshot, client).await?;
    Ok(Json(Folder {
        path: folder.as_str().to_owned(),
        entries: listing.entries.into_iter().map(file_entry).collect(),
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
    /// From this snapshot of the share (its label).
    #[serde(default)]
    pub snapshot: Option<String>,
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
    download_from(&state, &auth, &client, ShareId::new(id).into(), query, &request_headers).await
}

async fn download_from(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    query: FileQuery,
    request_headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let file_path = path(&query.path)?;
    let name = file_path.name().ok_or_else(|| ApiError::InvalidInput("not a file".into()))?.to_owned();
    let snapshot = query.snapshot.as_deref();
    let (mut file, size) = state.files.open(&auth.0.actor(), place, file_path, snapshot, client).await?;

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
    Ok((status, headers, Body::from_stream(chunks(file.take(length)))).into_response())
}

/// Downloads a folder with everything in it as a tar archive, which
/// Windows, macOS and Linux open without extra software. Refused if
/// anything inside cannot be read; the share's recycle bins are left out.
/// Its length is known ahead, so one that breaks off shows as a failed
/// download. Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares/{id}/archive",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), FolderQuery),
    responses(
        (status = OK, content_type = "application/x-tar", body = Vec<u8>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`: something inside cannot be read"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: not a folder, too many files"),
    )
)]
async fn download_folder(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<FolderQuery>,
) -> Result<Response, ApiError> {
    auth.require(Permission::FilesRead)?;
    archive_of(&state, &auth, &client, ShareId::new(id).into(), query).await
}

async fn archive_of(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    query: FolderQuery,
) -> Result<Response, ApiError> {
    let folder = path(&query.path)?;
    let (archive, size, name) =
        state.files.archive(&auth.0.actor(), place, folder, query.snapshot.as_deref(), client).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-tar"));
    headers.insert(header::CONTENT_DISPOSITION, disposition(&format!("{name}.tar"), false));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, Body::from_stream(chunks(archive.take(size)))).into_response())
}

/// A file's bytes as a body, read as they are sent.
fn chunks(
    file: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures_util::stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0; 256 * 1024];
        let read = file.read(&mut buffer).await?;
        buffer.truncate(read);
        Ok::<_, std::io::Error>((read > 0).then(|| (Bytes::from(buffer), file)))
    })
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
        (status = BAD_REQUEST, body = Problem, description = "`bad_request`: the upload did not complete"),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`: no valid name or folder"),
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
    upload_into(state, &auth, client, ShareId::new(id).into(), query, body).await
}

async fn upload_into(
    state: ApiState,
    auth: &Auth,
    client: RequestContext,
    place: PlaceId,
    query: UploadQuery,
    body: Body,
) -> Result<(StatusCode, Json<UploadedFile>), ApiError> {
    let actor = auth.0.actor();
    let folder = path(&query.path)?;
    let Upload { mut file, ticket } =
        state.files.begin_upload(&actor, place, folder, &query.name, query.replace, &client).await?;
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
    folder_made(&state, &auth, &client, ShareId::new(id).into(), request).await
}

async fn folder_made(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    request: CreateFolderRequest,
) -> Result<StatusCode, ApiError> {
    state.files.make_directory(&auth.0.actor(), place, path(&request.path)?, client).await?;
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
    renamed(&state, &auth, &client, ShareId::new(id).into(), request).await
}

async fn renamed(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    request: RenameRequest,
) -> Result<Json<UploadedFile>, ApiError> {
    let renamed =
        state.files.rename(&auth.0.actor(), place, path(&request.path)?, &request.name, client).await?;
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
    /// Entries of this share or drive, relative to its root.
    pub paths: Vec<String>,
    /// The share to copy or move into; may be this one. Either this or
    /// `target_drive`.
    #[serde(default)]
    pub target_share: Option<i64>,
    /// The removable drive to copy or move into (its ID, as listed); needs
    /// `storage.manage`.
    #[serde(default)]
    pub target_drive: Option<String>,
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
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `invalid_operation` (also more than 1000 entries)"),
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
    transferred(&state, &auth, &client, ShareId::new(id).into(), request).await
}

async fn transferred(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    request: TransferRequest,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let mode = match request.mode {
        TransferMode::Copy => Transfer::Copy,
        TransferMode::Move => Transfer::Move,
    };
    let target = match (request.target_share, request.target_drive) {
        (Some(share), None) => ShareId::new(share).into(),
        (None, Some(id)) => drive(auth, id, Permission::FilesWrite)?,
        _ => return Err(ApiError::InvalidInput("either target_share or target_drive".into())),
    };
    let (paths, directory) = (paths(&request.paths)?, path(&request.target_path)?);
    let job = state.files.transfer(&auth.0.actor(), mode, place, paths, target, directory, client).await?;
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
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, `invalid_operation` (also more than 1000 entries)"),
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
    deleted(&state, &auth, &client, ShareId::new(id).into(), request).await
}

async fn deleted(
    state: &ApiState,
    auth: &Auth,
    client: &RequestContext,
    place: PlaceId,
    request: DeleteEntriesRequest,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let job = state.files.delete(&auth.0.actor(), place, paths(&request.paths)?, client).await?;
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

/// A snapshot of a share, as the file manager shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileSnapshot {
    /// Pass as `snapshot` to look into it.
    pub label: String,
    /// Unix seconds.
    pub taken_at: i64,
    pub note: Option<String>,
}

/// The share's snapshots, newest first: what it looked like then. Needs
/// `files.read`.
#[utoipa::path(
    get,
    path = "/files/shares/{id}/snapshots",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = OK, body = Vec<FileSnapshot>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn file_snapshots(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<Json<Vec<FileSnapshot>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let list = state.files.snapshots(&auth.0.actor(), ShareId::new(id), &client).await?;
    Ok(Json(
        list.into_iter()
            .map(|s| FileSnapshot { label: s.label, taken_at: s.taken_at.unix_seconds(), note: s.note })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreFromSnapshotRequest {
    /// Entries of the snapshot, relative to the share's root.
    pub paths: Vec<String>,
}

/// Copies entries out of a snapshot back into the share, each into the
/// folder it was in; a name that is taken gets `name (2).ext`, so nothing
/// is replaced. As a job. Needs `files.write`.
#[utoipa::path(
    post,
    path = "/files/shares/{id}/snapshots/{label}/restore",
    tag = "files",
    security(("session" = [])),
    params(("id" = i64, Path), ("label" = String, Path)),
    request_body = RestoreFromSnapshotRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = FORBIDDEN, body = Problem, description = "`permission_denied`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn restore_from_snapshot(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path((id, label)): Path<(i64, String)>,
    ApiJson(request): ApiJson<RestoreFromSnapshotRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::FilesWrite)?;
    let paths = paths(&request.paths)?;
    let job =
        state.files.restore_from_snapshot(&auth.0.actor(), ShareId::new(id), &label, paths, &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// A folder kept at hand.
#[derive(Debug, Serialize, ToSchema)]
pub struct Favourite {
    pub share_id: i64,
    pub share_name: String,
    /// Relative to the share's root; empty for the share itself.
    pub path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FavouriteFolder {
    pub share_id: i64,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetFavouritesRequest {
    /// In the order to show them; at most 50.
    pub favourites: Vec<FavouriteFolder>,
}

fn favourite_dtos(list: Vec<(sharing::Favourite, sharing::ShareName)>) -> Vec<Favourite> {
    list.into_iter()
        .map(|(favourite, share)| Favourite {
            share_id: favourite.share.get(),
            share_name: share.to_string(),
            path: favourite.path.as_str().to_owned(),
        })
        .collect()
}

/// The signed-in account's favourite folders, in shares it may use.
/// Needs `files.read`.
#[utoipa::path(
    get,
    path = "/files/favourites",
    tag = "files",
    security(("session" = [])),
    responses((status = OK, body = Vec<Favourite>))
)]
async fn favourites(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<Favourite>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    Ok(Json(favourite_dtos(state.files.favourites(&auth.0.actor()).await?)))
}

/// Replaces the signed-in account's favourite folders. Needs `files.read`.
#[utoipa::path(
    put,
    path = "/files/favourites",
    tag = "files",
    security(("session" = [])),
    request_body = SetFavouritesRequest,
    responses(
        (status = OK, body = Vec<Favourite>),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: a share the account may not use"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_operation`: too many, or twice"),
    )
)]
async fn set_favourites(
    auth: Auth,
    State(state): State<ApiState>,
    ApiJson(request): ApiJson<SetFavouritesRequest>,
) -> Result<Json<Vec<Favourite>>, ApiError> {
    auth.require(Permission::FilesRead)?;
    let list = request
        .favourites
        .into_iter()
        .map(|f| Ok(sharing::Favourite { share: ShareId::new(f.share_id), path: path(&f.path)? }))
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(favourite_dtos(state.files.set_favourites(&auth.0.actor(), list).await?)))
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
