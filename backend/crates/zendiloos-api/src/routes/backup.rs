//! Backup tasks (`backup.manage`, administrators): the App Center's plugin
//! and the NAS's SSH key, servers' host keys and a look into a server's
//! folder, tasks made, changed and removed, runs started and stopped, the
//! log with each run's transfer list, versions looked into and restored,
//! and the plugin's settings. A daemon's password is taken, never returned.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_backup::application::{self as backup, BackupError, RestoreRequest};
use zendiloos_backup::domain::{self as domain, Existing, NasPlace, Place, RemotePlace};
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::{Secret, Timestamp};

use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, ApiQuery as Query, Client};
use crate::routes::jobs::Job;
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(backup_plugin))
        .routes(routes!(install_backup_plugin))
        .routes(routes!(backup_key))
        .routes(routes!(backup_host_keys))
        .routes(routes!(check_backup_server))
        .routes(routes!(backup_share_readers))
        .routes(routes!(list_backup_tasks, create_backup_task))
        .routes(routes!(get_backup_task, update_backup_task, delete_backup_task))
        .routes(routes!(run_backup_task))
        .routes(routes!(stop_backup_task))
        .routes(routes!(backup_task_runs))
        .routes(routes!(backup_runs))
        .routes(routes!(backup_run_transfers))
        .routes(routes!(backup_task_versions))
        .routes(routes!(browse_backup_task))
        .routes(routes!(restore_backup_task))
        .routes(routes!(backup_settings, set_backup_settings))
}

impl From<BackupError> for ApiError {
    fn from(error: BackupError) -> Self {
        let rejected = |status, code, detail| ApiError::Rejected { status, code, detail };
        match error {
            BackupError::NotFound => ApiError::NotFound,
            BackupError::Invalid(e) => ApiError::InvalidInput(e.0),
            BackupError::NotInstalled => rejected(StatusCode::CONFLICT, "backup_not_installed", None),
            BackupError::Installed => rejected(StatusCode::CONFLICT, "backup_installed", None),
            BackupError::NameTaken => ApiError::NameTaken,
            BackupError::FolderTaken(task) => rejected(StatusCode::CONFLICT, "folder_taken", Some(task)),
            BackupError::Running => rejected(StatusCode::CONFLICT, "task_running", None),
            BackupError::NotRunning => rejected(StatusCode::CONFLICT, "task_not_running", None),
            BackupError::Restoring => rejected(StatusCode::CONFLICT, "restore_running", None),
            BackupError::ShareMissing => rejected(StatusCode::CONFLICT, "share_missing", None),
            BackupError::DriveMissing => rejected(StatusCode::CONFLICT, "drive_missing", None),
            BackupError::DriveReadOnly => rejected(StatusCode::CONFLICT, "drive_read_only", None),
            BackupError::HostKeyChanged(detail) => {
                rejected(StatusCode::CONFLICT, "host_key_changed", Some(detail))
            }
            BackupError::Refused(detail) => rejected(StatusCode::CONFLICT, "refused", Some(detail)),
            BackupError::HostFailed(detail) => {
                rejected(StatusCode::BAD_GATEWAY, "agent_failed", Some(detail))
            }
            BackupError::Port(e) => ApiError::internal(e),
        }
    }
}

/// The plugin: rsync and the SSH client installed, and the NAS's public
/// key for SSH servers once made.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupPlugin {
    pub installed: bool,
    pub ssh: bool,
    pub public_key: Option<String>,
}

/// Where a task reads or writes: a folder of a share, of a removable drive
/// (by its file system's UUID), or of the task's server.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupPlace {
    Share {
        share_id: i64,
        path: String,
    },
    Drive {
        drive: String,
        path: String,
    },
    /// In the daemon's module, or on the SSH server (from the user's home
    /// unless it starts with `/`).
    Server {
        path: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupTransport {
    /// An rsync daemon, unencrypted.
    Daemon,
    /// rsync over SSH, the server's host key pinned.
    Ssh,
}

/// An rsync server. A daemon needs its module; SSH its host key, as the
/// administrator compared and pinned it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupServer {
    pub transport: BackupTransport,
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub module: Option<String>,
    /// `ssh-ed25519 AAAA…`
    #[serde(default)]
    pub host_key: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupMode {
    /// An exact copy; what a run deletes or replaces kept that many days,
    /// or not at all.
    Mirror { keep_changed_days: Option<u16> },
    /// A dated version per run; the newest `keep_last` and those younger
    /// than `keep_days` kept (neither: all).
    Versioned { keep_last: Option<u16>, keep_days: Option<u16> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupFrequency {
    Hourly,
    Daily,
    Weekly,
}

/// When a task backs up on its own, in the NAS's time zone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupSchedule {
    pub frequency: BackupFrequency,
    /// 0 (Monday) to 6, weekly only.
    #[serde(default)]
    pub weekday: Option<u8>,
    /// Not used hourly.
    #[serde(default)]
    pub hour: u8,
    pub minute: u8,
}

/// A task as made or changed.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupTaskRequest {
    pub name: String,
    pub source: BackupPlace,
    pub target: BackupPlace,
    /// When one place is the server's.
    #[serde(default)]
    pub server: Option<BackupServer>,
    /// A daemon's password; left out or empty, a stored one stays for the
    /// same daemon, port, user and module.
    #[serde(default)]
    pub password: Option<String>,
    pub mode: BackupMode,
    #[serde(default)]
    pub schedule: Option<BackupSchedule>,
    /// Back up when its target drive is plugged in.
    #[serde(default)]
    pub on_plug: bool,
    pub excludes: Vec<String>,
    /// Whether names starting with `.` are backed up.
    pub hidden: bool,
    /// KiB/s; none follows the settings, 0 is no limit.
    #[serde(default)]
    pub bwlimit_kib: Option<u32>,
    #[serde(default)]
    pub notify_success: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupTaskKind {
    Local,
    Push,
    Pull,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupOutcome {
    Running,
    Succeeded,
    Partial,
    Failed,
    Stopped,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    Manual,
    Schedule,
    Plug,
}

/// A run, as logged.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupRun {
    pub id: i64,
    pub task_id: i64,
    pub trigger: BackupTrigger,
    /// Who started it by hand.
    pub by: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub outcome: BackupOutcome,
    pub files: u64,
    pub bytes: u64,
    /// The version it made.
    pub version: Option<String>,
    pub error: Option<String>,
    /// Entries of its transfer list (the first 5000 are kept).
    pub transfers: u64,
    pub job_id: Option<i64>,
}

fn run_dto(run: domain::Run) -> BackupRun {
    BackupRun {
        id: run.id,
        task_id: run.task_id,
        trigger: match run.trigger {
            domain::Trigger::Manual => BackupTrigger::Manual,
            domain::Trigger::Schedule => BackupTrigger::Schedule,
            domain::Trigger::Plug => BackupTrigger::Plug,
        },
        by: run.by,
        started_at: run.started_at.unix_seconds(),
        finished_at: run.finished_at.map(Timestamp::unix_seconds),
        outcome: match run.outcome {
            domain::Outcome::Running => BackupOutcome::Running,
            domain::Outcome::Succeeded => BackupOutcome::Succeeded,
            domain::Outcome::Partial => BackupOutcome::Partial,
            domain::Outcome::Failed => BackupOutcome::Failed,
            domain::Outcome::Stopped => BackupOutcome::Stopped,
            domain::Outcome::Interrupted => BackupOutcome::Interrupted,
        },
        files: run.files,
        bytes: run.bytes,
        version: run.version,
        error: run.error,
        transfers: run.transfers,
        job_id: run.job_id,
    }
}

/// A task with how it stands. A daemon's password is never shown, only
/// whether one is stored.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupTask {
    pub id: i64,
    pub name: String,
    pub kind: BackupTaskKind,
    /// The folder in the target holding its backup.
    pub folder: String,
    pub source: BackupPlace,
    pub target: BackupPlace,
    pub server: Option<BackupServer>,
    pub password_set: bool,
    pub mode: BackupMode,
    pub schedule: Option<BackupSchedule>,
    pub on_plug: bool,
    pub excludes: Vec<String>,
    pub hidden: bool,
    pub bwlimit_kib: Option<u32>,
    pub notify_success: bool,
    pub created_at: i64,
    pub running: bool,
    /// Its places on the NAS are there now (a server's is not looked at).
    pub reachable: bool,
    /// When its schedule runs it next.
    pub next_at: Option<i64>,
    pub last_run: Option<BackupRun>,
}

fn place_dto(place: &Place) -> BackupPlace {
    match place {
        Place::Nas(NasPlace::Share { share_id, path }) => {
            BackupPlace::Share { share_id: *share_id, path: path.clone() }
        }
        Place::Nas(NasPlace::Drive { drive, path }) => {
            BackupPlace::Drive { drive: drive.clone(), path: path.clone() }
        }
        Place::Remote(remote) => BackupPlace::Server { path: remote.path.clone() },
    }
}

fn server_dto(server: &domain::Server) -> BackupServer {
    let (transport, module, host_key, fingerprint) = match &server.transport {
        domain::Transport::Daemon { module } => (BackupTransport::Daemon, Some(module.clone()), None, None),
        domain::Transport::Ssh { host_key, fingerprint } => {
            (BackupTransport::Ssh, None, Some(host_key.clone()), Some(fingerprint.clone()))
        }
    };
    BackupServer {
        transport,
        host: server.host.clone(),
        port: server.port,
        user: server.user.clone(),
        module,
        host_key,
        fingerprint,
    }
}

fn task_dto(view: backup::TaskView) -> BackupTask {
    let task = view.task;
    let settings = &task.settings;
    BackupTask {
        id: task.id,
        name: settings.name.clone(),
        kind: match task.kind() {
            domain::TaskKind::Local => BackupTaskKind::Local,
            domain::TaskKind::Push => BackupTaskKind::Push,
            domain::TaskKind::Pull => BackupTaskKind::Pull,
        },
        folder: task.folder.clone(),
        source: place_dto(&settings.source),
        target: place_dto(&settings.target),
        server: task.server().map(server_dto),
        password_set: view.password_set,
        mode: match settings.mode {
            domain::Mode::Mirror { keep_changed_days } => BackupMode::Mirror { keep_changed_days },
            domain::Mode::Versioned { keep_last, keep_days } => {
                BackupMode::Versioned { keep_last, keep_days }
            }
        },
        schedule: settings.schedule.map(|schedule| BackupSchedule {
            frequency: match schedule.frequency {
                domain::Frequency::Hourly => BackupFrequency::Hourly,
                domain::Frequency::Daily => BackupFrequency::Daily,
                domain::Frequency::Weekly { .. } => BackupFrequency::Weekly,
            },
            weekday: match schedule.frequency {
                domain::Frequency::Weekly { weekday } => Some(weekday),
                _ => None,
            },
            hour: schedule.hour,
            minute: schedule.minute,
        }),
        on_plug: settings.on_plug,
        excludes: settings.filters.excludes.clone(),
        hidden: settings.filters.hidden,
        bwlimit_kib: settings.bwlimit_kib,
        notify_success: settings.notify_success,
        created_at: task.created_at.unix_seconds(),
        running: view.running,
        reachable: view.reachable,
        next_at: view.next_at.map(Timestamp::unix_seconds),
        last_run: view.last_run.map(run_dto),
    }
}

fn server_domain(server: BackupServer) -> Result<domain::Server, ApiError> {
    let transport = match server.transport {
        BackupTransport::Daemon => domain::Transport::Daemon {
            module: server
                .module
                .ok_or_else(|| ApiError::InvalidInput("a daemon needs its module".into()))?,
        },
        BackupTransport::Ssh => domain::Transport::Ssh {
            host_key: server
                .host_key
                .ok_or_else(|| ApiError::InvalidInput("the server's host key is not pinned".into()))?,
            fingerprint: server.fingerprint.unwrap_or_default(),
        },
    };
    Ok(domain::Server { transport, host: server.host, port: server.port, user: server.user })
}

fn place_domain(place: BackupPlace, server: Option<&domain::Server>) -> Result<Place, ApiError> {
    Ok(match place {
        BackupPlace::Share { share_id, path } => Place::Nas(NasPlace::Share { share_id, path }),
        BackupPlace::Drive { drive, path } => Place::Nas(NasPlace::Drive { drive, path }),
        BackupPlace::Server { path } => Place::Remote(RemotePlace {
            server: server
                .cloned()
                .ok_or_else(|| ApiError::InvalidInput("a server's folder needs the server".into()))?,
            path,
        }),
    })
}

fn settings_domain(request: BackupTaskRequest) -> Result<(domain::TaskSettings, Option<Secret>), ApiError> {
    let server = request.server.map(server_domain).transpose()?;
    let schedule = request
        .schedule
        .map(|schedule| {
            let frequency = match schedule.frequency {
                BackupFrequency::Hourly => domain::Frequency::Hourly,
                BackupFrequency::Daily => domain::Frequency::Daily,
                BackupFrequency::Weekly => domain::Frequency::Weekly {
                    weekday: schedule.weekday.ok_or_else(|| {
                        ApiError::InvalidInput("a weekly schedule needs its weekday".into())
                    })?,
                },
            };
            Ok::<_, ApiError>(domain::Schedule { frequency, hour: schedule.hour, minute: schedule.minute })
        })
        .transpose()?;
    let settings = domain::TaskSettings {
        name: request.name,
        source: place_domain(request.source, server.as_ref())?,
        target: place_domain(request.target, server.as_ref())?,
        mode: match request.mode {
            BackupMode::Mirror { keep_changed_days } => domain::Mode::Mirror { keep_changed_days },
            BackupMode::Versioned { keep_last, keep_days } => {
                domain::Mode::Versioned { keep_last, keep_days }
            }
        },
        schedule,
        on_plug: request.on_plug,
        filters: domain::Filters { excludes: request.excludes, hidden: request.hidden },
        bwlimit_kib: request.bwlimit_kib,
        notify_success: request.notify_success,
    };
    Ok((settings, request.password.filter(|p| !p.is_empty()).map(Secret::new)))
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupEntryKind {
    Folder,
    File,
    Link,
    Other,
}

/// An entry of a folder in a backup or on a server.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupEntry {
    pub name: String,
    pub kind: BackupEntryKind,
    pub bytes: u64,
    pub modified: Option<i64>,
}

fn entry_dto(entry: domain::Entry) -> BackupEntry {
    BackupEntry {
        name: entry.name,
        kind: match entry.kind {
            domain::EntryKind::Folder => BackupEntryKind::Folder,
            domain::EntryKind::File => BackupEntryKind::File,
            domain::EntryKind::Link => BackupEntryKind::Link,
            domain::EntryKind::Other => BackupEntryKind::Other,
        },
        bytes: entry.bytes,
        modified: entry.modified.map(Timestamp::unix_seconds),
    }
}

/// What the plugin offers. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/plugin", tag = "backup", security(("session" = [])), responses((status = OK, body = BackupPlugin)))]
async fn backup_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<BackupPlugin>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let facts = state.backup.plugin(&client).await?.facts;
    Ok(Json(BackupPlugin { installed: facts.rsync, ssh: facts.ssh, public_key: facts.public_key }))
}

/// Installs rsync and the SSH client from Debian, as a job. Needs
/// `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/plugin/install",
    tag = "backup",
    security(("session" = [])),
    responses((status = ACCEPTED, body = Job), (status = CONFLICT, body = Problem, description = "`backup_installed`"))
)]
async fn install_backup_plugin(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::BackupManage)?;
    let job = state.backup.install(&auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BackupKey {
    /// The NAS's public key, to put into a server's `authorized_keys`.
    pub public_key: String,
}

/// The NAS's public key for SSH servers, made the first time it is asked
/// for; its private key never leaves the NAS. Needs `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/key",
    tag = "backup",
    security(("session" = [])),
    responses((status = OK, body = BackupKey), (status = CONFLICT, body = Problem, description = "`backup_not_installed`"))
)]
async fn backup_key(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<BackupKey>, ApiError> {
    auth.require(Permission::BackupManage)?;
    Ok(Json(BackupKey { public_key: state.backup.public_key(&auth.0.actor(), &client).await? }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupHostKeysRequest {
    pub host: String,
    pub port: u16,
}

/// A host key an SSH server offers, to compare with the server's own and pin.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupHostKey {
    pub line: String,
    /// `SHA256:…`, as `ssh-keygen -lf` shows it.
    pub fingerprint: String,
}

/// The host keys an SSH server offers. Needs `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/servers/host-keys",
    tag = "backup",
    security(("session" = [])),
    request_body = BackupHostKeysRequest,
    responses((status = OK, body = [BackupHostKey]), (status = BAD_GATEWAY, body = Problem, description = "`agent_failed`: the server did not answer"))
)]
async fn backup_host_keys(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<BackupHostKeysRequest>,
) -> Result<Json<Vec<BackupHostKey>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let keys = state.backup.host_keys(&request.host, request.port, &client).await?;
    Ok(Json(
        keys.into_iter().map(|key| BackupHostKey { line: key.line, fingerprint: key.fingerprint }).collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupServerCheck {
    pub server: BackupServer,
    pub path: String,
    /// A daemon's password; left out, the one task `task_id` has stored for
    /// the same daemon.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub task_id: Option<i64>,
}

/// Lists a folder of a server, to see that it can be reached as given.
/// Needs `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/servers/check",
    tag = "backup",
    security(("session" = [])),
    request_body = BackupServerCheck,
    responses(
        (status = OK, body = [BackupEntry]),
        (status = CONFLICT, body = Problem, description = "`host_key_changed`"),
        (status = BAD_GATEWAY, body = Problem, description = "`agent_failed`: rsync could not reach it, in its words"),
    )
)]
async fn check_backup_server(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<BackupServerCheck>,
) -> Result<Json<Vec<BackupEntry>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let place = RemotePlace { server: server_domain(request.server)?, path: request.path };
    let password = request.password.filter(|p| !p.is_empty()).map(Secret::new);
    let entries = state.backup.check_server(&place, password, request.task_id, &client).await?;
    Ok(Json(entries.into_iter().filter(|e| e.name != ".").map(entry_dto).collect()))
}

/// Who reads what is backed up into a share.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupShareReaders {
    pub accounts: Vec<String>,
    pub groups: Vec<String>,
    /// Computers it is exported to over NFS.
    pub nfs: Vec<String>,
}

/// Who may read share `id`: whoever may, reads what is backed up into it.
/// Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/shares/{id}/readers", tag = "backup", security(("session" = [])), params(("id" = i64, Path)), responses((status = OK, body = BackupShareReaders), (status = NOT_FOUND, body = Problem)))]
async fn backup_share_readers(
    auth: Auth,
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<BackupShareReaders>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let readers = state.backup.readers(id).await?;
    Ok(Json(BackupShareReaders { accounts: readers.accounts, groups: readers.groups, nfs: readers.nfs }))
}

/// Every task with how it stands. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/tasks", tag = "backup", security(("session" = [])), responses((status = OK, body = [BackupTask])))]
async fn list_backup_tasks(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<BackupTask>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    Ok(Json(state.backup.tasks(&client).await?.into_iter().map(task_dto).collect()))
}

/// Makes a task. Needs `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/tasks",
    tag = "backup",
    security(("session" = [])),
    request_body = BackupTaskRequest,
    responses(
        (status = CREATED, body = BackupTask),
        (status = CONFLICT, body = Problem, description = "`backup_not_installed`, `name_taken`, `folder_taken`, `share_missing`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<BackupTaskRequest>,
) -> Result<(StatusCode, Json<BackupTask>), ApiError> {
    auth.require(Permission::BackupManage)?;
    let (settings, password) = settings_domain(request)?;
    let made = state.backup.create(settings, password, &auth.0.actor(), &client).await?;
    Ok((StatusCode::CREATED, Json(task_dto(made))))
}

/// A task. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/tasks/{id}", tag = "backup", security(("session" = [])), params(("id" = i64, Path)), responses((status = OK, body = BackupTask), (status = NOT_FOUND, body = Problem)))]
async fn get_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<Json<BackupTask>, ApiError> {
    auth.require(Permission::BackupManage)?;
    Ok(Json(task_dto(state.backup.task(id, &client).await?)))
}

/// Changes a task; its folder in the target stays. Needs `backup.manage`.
#[utoipa::path(
    put,
    path = "/backup/tasks/{id}",
    tag = "backup",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = BackupTaskRequest,
    responses(
        (status = OK, body = BackupTask),
        (status = NOT_FOUND, body = Problem),
        (status = CONFLICT, body = Problem, description = "`task_running`, `name_taken`, `folder_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<BackupTaskRequest>,
) -> Result<Json<BackupTask>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let (settings, password) = settings_domain(request)?;
    Ok(Json(task_dto(state.backup.update(id, settings, password, &auth.0.actor(), &client).await?)))
}

/// Forgets a task and its log; what it backed up stays. Needs
/// `backup.manage`.
#[utoipa::path(delete, path = "/backup/tasks/{id}", tag = "backup", security(("session" = [])), params(("id" = i64, Path)), responses((status = NO_CONTENT), (status = NOT_FOUND, body = Problem), (status = CONFLICT, body = Problem, description = "`task_running`")))]
async fn delete_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::BackupManage)?;
    state.backup.delete(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// "Back up now", as a job. Needs `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/tasks/{id}/run",
    tag = "backup",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem),
        (status = CONFLICT, body = Problem, description = "`task_running`, `restore_running`, `drive_missing`, `share_missing`, `update_running`"),
    )
)]
async fn run_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::BackupManage)?;
    let job = state.backup.run_now(id, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// Stops a task's run. Needs `backup.manage`.
#[utoipa::path(post, path = "/backup/tasks/{id}/stop", tag = "backup", security(("session" = [])), params(("id" = i64, Path)), responses((status = NO_CONTENT), (status = CONFLICT, body = Problem, description = "`task_not_running`")))]
async fn stop_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::BackupManage)?;
    state.backup.stop(id, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunsQuery {
    /// 1 to 500, 50 unless told otherwise.
    pub limit: Option<u32>,
}

/// A task's runs, newest first. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/tasks/{id}/runs", tag = "backup", security(("session" = [])), params(("id" = i64, Path), RunsQuery), responses((status = OK, body = [BackupRun]), (status = NOT_FOUND, body = Problem)))]
async fn backup_task_runs(
    auth: Auth,
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Vec<BackupRun>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    Ok(Json(state.backup.runs(Some(id), query.limit.unwrap_or(50)).await?.into_iter().map(run_dto).collect()))
}

/// Every task's runs, newest first. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/runs", tag = "backup", security(("session" = [])), params(RunsQuery), responses((status = OK, body = [BackupRun])))]
async fn backup_runs(
    auth: Auth,
    State(state): State<ApiState>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Vec<BackupRun>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    Ok(Json(state.backup.runs(None, query.limit.unwrap_or(50)).await?.into_iter().map(run_dto).collect()))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TransfersQuery {
    pub offset: Option<u32>,
    /// 1 to 1000, 200 unless told otherwise.
    pub limit: Option<u32>,
}

/// One entry of a run's transfer list.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupTransfer {
    /// rsync's itemized change: `>f+++++++++` a new file, `*deleting` one
    /// deleted.
    pub change: String,
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BackupTransfers {
    pub run: BackupRun,
    pub transfers: Vec<BackupTransfer>,
}

/// A run's transfer list, as kept. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/runs/{id}/transfers", tag = "backup", security(("session" = [])), params(("id" = i64, Path), TransfersQuery), responses((status = OK, body = BackupTransfers), (status = NOT_FOUND, body = Problem)))]
async fn backup_run_transfers(
    auth: Auth,
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(query): Query<TransfersQuery>,
) -> Result<Json<BackupTransfers>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let (run, transfers) =
        state.backup.transfers(id, query.offset.unwrap_or(0), query.limit.unwrap_or(200)).await?;
    Ok(Json(BackupTransfers {
        run: run_dto(run),
        transfers: transfers.into_iter().map(|t| BackupTransfer { change: t.change, path: t.path }).collect(),
    }))
}

/// What a task's backup holds to restore from.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupVersions {
    /// A mirror's copy is there.
    pub current: bool,
    /// Complete versions, by when their runs started, newest first.
    pub versions: Vec<i64>,
}

/// What a task's backup holds: looked at on its target. Needs
/// `backup.manage`.
#[utoipa::path(get, path = "/backup/tasks/{id}/versions", tag = "backup", security(("session" = [])), params(("id" = i64, Path)), responses((status = OK, body = BackupVersions), (status = NOT_FOUND, body = Problem), (status = CONFLICT, body = Problem, description = "`drive_missing`, `host_key_changed`")))]
async fn backup_task_versions(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<Json<BackupVersions>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let versions = state.backup.versions(id, &client).await?;
    Ok(Json(BackupVersions {
        current: versions.current,
        versions: versions.versions.into_iter().map(Timestamp::unix_seconds).collect(),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BrowseQuery {
    /// A version, by when its run started; left out: the mirror's copy.
    pub version: Option<i64>,
    /// A folder in it; left out: its top.
    pub path: Option<String>,
}

/// The entries of a folder of a version. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/tasks/{id}/browse", tag = "backup", security(("session" = [])), params(("id" = i64, Path), BrowseQuery), responses((status = OK, body = [BackupEntry]), (status = NOT_FOUND, body = Problem)))]
async fn browse_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<Vec<BackupEntry>>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let version = query.version.map(Timestamp::from_unix_seconds);
    let entries = state.backup.browse(id, version, query.path.as_deref().unwrap_or(""), &client).await?;
    Ok(Json(entries.into_iter().map(entry_dto).collect()))
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupExisting {
    Skip,
    Replace,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupRestoreRequest {
    /// A version, by when its run started; left out: the mirror's copy.
    #[serde(default)]
    pub version: Option<i64>,
    /// Folders and files in it; none for all.
    #[serde(default)]
    pub paths: Vec<String>,
    /// A folder of a share or a drive; left out: where the task's source is.
    #[serde(default)]
    pub to: Option<BackupPlace>,
    pub existing: BackupExisting,
}

/// Restores from a task's backup, as a job: what is restored into a share
/// takes the folder's owner and the share's group and ACL. Needs
/// `backup.manage`.
#[utoipa::path(
    post,
    path = "/backup/tasks/{id}/restore",
    tag = "backup",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = BackupRestoreRequest,
    responses(
        (status = ACCEPTED, body = Job),
        (status = NOT_FOUND, body = Problem),
        (status = CONFLICT, body = Problem, description = "`task_running`, `restore_running`, `drive_missing`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn restore_backup_task(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<BackupRestoreRequest>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    auth.require(Permission::BackupManage)?;
    let to = match request.to {
        None => None,
        Some(BackupPlace::Share { share_id, path }) => Some(NasPlace::Share { share_id, path }),
        Some(BackupPlace::Drive { drive, path }) => Some(NasPlace::Drive { drive, path }),
        Some(BackupPlace::Server { .. }) => {
            return Err(ApiError::InvalidInput("a restore goes into a folder of the NAS".into()));
        }
    };
    let request = RestoreRequest {
        version: request.version.map(Timestamp::from_unix_seconds),
        paths: request.paths,
        to,
        existing: match request.existing {
            BackupExisting::Skip => Existing::Skip,
            BackupExisting::Replace => Existing::Replace,
        },
    };
    let job = state.backup.restore(id, request, &auth.0.actor(), &client).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// The plugin's settings.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupSettings {
    /// Days the log is kept.
    pub log_days: u16,
    /// KiB/s for tasks that follow the default; 0 is no limit.
    pub bwlimit_kib: u32,
}

/// The plugin's settings. Needs `backup.manage`.
#[utoipa::path(get, path = "/backup/settings", tag = "backup", security(("session" = [])), responses((status = OK, body = BackupSettings)))]
async fn backup_settings(
    auth: Auth,
    State(state): State<ApiState>,
) -> Result<Json<BackupSettings>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let settings = state.backup.settings().await?;
    Ok(Json(BackupSettings { log_days: settings.log_days, bwlimit_kib: settings.bwlimit_kib }))
}

/// Changes the plugin's settings. Needs `backup.manage`.
#[utoipa::path(put, path = "/backup/settings", tag = "backup", security(("session" = [])), request_body = BackupSettings, responses((status = OK, body = BackupSettings), (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`")))]
async fn set_backup_settings(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<BackupSettings>,
) -> Result<Json<BackupSettings>, ApiError> {
    auth.require(Permission::BackupManage)?;
    let settings = domain::Settings { log_days: request.log_days, bwlimit_kib: request.bwlimit_kib };
    let saved = state.backup.set_settings(settings, &auth.0.actor(), &client).await?;
    Ok(Json(BackupSettings { log_days: saved.log_days, bwlimit_kib: saved.bwlimit_kib }))
}
