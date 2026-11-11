//! Backup tasks, their runs with their transfer lists, and the plugin's
//! settings. A daemon's password is a column of its task's row, which only
//! zendiloosd reads; nothing returns it but [`TaskRepository::password`].

use async_trait::async_trait;
use zendiloos_backup::application::{
    NewRun, NewTask, PasswordChange, RunEnd, RunRepository, SettingsRepository, TaskRepository,
};
use zendiloos_backup::domain::{
    Filters, Frequency, Mode, NasPlace, Outcome, Place, RemotePlace, Run, Schedule, Server, Settings, Task,
    TaskSettings, Transfer, Transport, Trigger,
};
use zendiloos_kernel::{PortError, Secret, Timestamp};

use super::Database;

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("backup tasks", error)
}

fn stored(what: String) -> PortError {
    PortError::new("stored backup task", what)
}

/// [`TaskRepository`] on `backup_tasks`.
#[derive(Debug, Clone)]
pub struct SqliteBackupTasks {
    db: Database,
}

impl SqliteBackupTasks {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

/// A task's row, as its columns are.
#[derive(Debug, Default)]
struct Row {
    id: i64,
    name: String,
    folder: String,
    source_kind: String,
    source_share: Option<i64>,
    source_path: String,
    target_kind: String,
    target_share: Option<i64>,
    target_drive: Option<String>,
    target_path: String,
    server_transport: Option<String>,
    server_host: Option<String>,
    server_port: Option<i64>,
    server_user: Option<String>,
    server_module: Option<String>,
    server_host_key: Option<String>,
    server_fingerprint: Option<String>,
    mode: String,
    keep_changed_days: Option<i64>,
    keep_last: Option<i64>,
    keep_days: Option<i64>,
    schedule: Option<String>,
    schedule_weekday: Option<i64>,
    schedule_hour: i64,
    schedule_minute: i64,
    on_plug: bool,
    excludes: String,
    hidden: bool,
    bwlimit_kib: Option<i64>,
    notify_success: bool,
    created_at: i64,
    since: i64,
    last_plug: Option<String>,
}

fn small<T: TryFrom<i64>>(value: i64, what: &str) -> Result<T, PortError> {
    T::try_from(value).map_err(|_| stored(format!("{what} {value} is out of range")))
}

fn optional<T: TryFrom<i64>>(value: Option<i64>, what: &str) -> Result<Option<T>, PortError> {
    value.map(|v| small(v, what)).transpose()
}

impl Row {
    fn server(&self) -> Result<Server, PortError> {
        let missing = |what: &str| stored(format!("task {} has no server {what}", self.id));
        let transport = match self.server_transport.as_deref() {
            Some("daemon") => {
                Transport::Daemon { module: self.server_module.clone().ok_or_else(|| missing("module"))? }
            }
            Some("ssh") => Transport::Ssh {
                host_key: self.server_host_key.clone().ok_or_else(|| missing("host key"))?,
                fingerprint: self.server_fingerprint.clone().unwrap_or_default(),
            },
            other => return Err(stored(format!("task {} has the transport {other:?}", self.id))),
        };
        Ok(Server {
            transport,
            host: self.server_host.clone().ok_or_else(|| missing("host"))?,
            port: small(self.server_port.ok_or_else(|| missing("port"))?, "port")?,
            user: self.server_user.clone().ok_or_else(|| missing("user"))?,
        })
    }

    fn task(self) -> Result<Task, PortError> {
        let source = match self.source_kind.as_str() {
            "share" => Place::Nas(NasPlace::Share {
                share_id: self.source_share.ok_or_else(|| stored("no source share".into()))?,
                path: self.source_path.clone(),
            }),
            "server" => Place::Remote(RemotePlace { server: self.server()?, path: self.source_path.clone() }),
            other => return Err(stored(format!("a source of the kind {other:?}"))),
        };
        let target = match self.target_kind.as_str() {
            "share" => Place::Nas(NasPlace::Share {
                share_id: self.target_share.ok_or_else(|| stored("no target share".into()))?,
                path: self.target_path.clone(),
            }),
            "drive" => Place::Nas(NasPlace::Drive {
                drive: self.target_drive.clone().ok_or_else(|| stored("no target drive".into()))?,
                path: self.target_path.clone(),
            }),
            "server" => Place::Remote(RemotePlace { server: self.server()?, path: self.target_path.clone() }),
            other => return Err(stored(format!("a target of the kind {other:?}"))),
        };
        let mode = match self.mode.as_str() {
            "mirror" => Mode::Mirror { keep_changed_days: optional(self.keep_changed_days, "days")? },
            "versioned" => Mode::Versioned {
                keep_last: optional(self.keep_last, "versions")?,
                keep_days: optional(self.keep_days, "days")?,
            },
            other => return Err(stored(format!("the mode {other:?}"))),
        };
        let frequency = match (self.schedule.as_deref(), self.schedule_weekday) {
            (None, _) => None,
            (Some("hourly"), _) => Some(Frequency::Hourly),
            (Some("daily"), _) => Some(Frequency::Daily),
            (Some("weekly"), Some(weekday)) => {
                Some(Frequency::Weekly { weekday: small(weekday, "weekday")? })
            }
            (other, _) => return Err(stored(format!("the schedule {other:?}"))),
        };
        let schedule = match frequency {
            Some(frequency) => Some(Schedule {
                frequency,
                hour: small(self.schedule_hour, "hour")?,
                minute: small(self.schedule_minute, "minute")?,
            }),
            None => None,
        };
        Ok(Task {
            id: self.id,
            settings: TaskSettings {
                name: self.name,
                source,
                target,
                mode,
                schedule,
                on_plug: self.on_plug,
                filters: Filters {
                    excludes: self
                        .excludes
                        .lines()
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    hidden: self.hidden,
                },
                bwlimit_kib: optional(self.bwlimit_kib, "bandwidth")?,
                notify_success: self.notify_success,
            },
            folder: self.folder,
            created_at: Timestamp::from_unix_seconds(self.created_at),
            since: Timestamp::from_unix_seconds(self.since),
            last_plug: self.last_plug,
        })
    }
}

/// The columns of a task's settings.
struct Columns<'a> {
    source_kind: &'static str,
    source_share: Option<i64>,
    source_path: &'a str,
    target_kind: &'static str,
    target_share: Option<i64>,
    target_drive: Option<&'a str>,
    target_path: &'a str,
    server_transport: Option<&'static str>,
    server_host: Option<&'a str>,
    server_port: Option<i64>,
    server_user: Option<&'a str>,
    server_module: Option<&'a str>,
    server_host_key: Option<&'a str>,
    server_fingerprint: Option<&'a str>,
    mode: &'static str,
    keep_changed_days: Option<i64>,
    keep_last: Option<i64>,
    keep_days: Option<i64>,
    schedule: Option<&'static str>,
    schedule_weekday: Option<i64>,
    schedule_hour: i64,
    schedule_minute: i64,
    excludes: String,
    bwlimit_kib: Option<i64>,
}

fn columns(settings: &TaskSettings) -> Columns<'_> {
    let mut columns = Columns {
        source_kind: "share",
        source_share: None,
        source_path: "",
        target_kind: "share",
        target_share: None,
        target_drive: None,
        target_path: "",
        server_transport: None,
        server_host: None,
        server_port: None,
        server_user: None,
        server_module: None,
        server_host_key: None,
        server_fingerprint: None,
        mode: settings.mode.as_str(),
        keep_changed_days: None,
        keep_last: None,
        keep_days: None,
        schedule: None,
        schedule_weekday: None,
        schedule_hour: 0,
        schedule_minute: 0,
        excludes: settings.filters.excludes.join("\n"),
        bwlimit_kib: settings.bwlimit_kib.map(i64::from),
    };
    match &settings.source {
        Place::Nas(nas) => {
            if let NasPlace::Share { share_id, .. } = nas {
                columns.source_share = Some(*share_id);
            }
            columns.source_path = nas.path();
        }
        Place::Remote(place) => {
            columns.source_kind = "server";
            columns.source_path = &place.path;
        }
    }
    match &settings.target {
        Place::Nas(NasPlace::Share { share_id, path }) => {
            columns.target_share = Some(*share_id);
            columns.target_path = path;
        }
        Place::Nas(NasPlace::Drive { drive, path }) => {
            columns.target_kind = "drive";
            columns.target_drive = Some(drive);
            columns.target_path = path;
        }
        Place::Remote(place) => {
            columns.target_kind = "server";
            columns.target_path = &place.path;
        }
    }
    if let Some(server) = settings.source.remote().or(settings.target.remote()).map(|place| &place.server) {
        columns.server_host = Some(&server.host);
        columns.server_port = Some(i64::from(server.port));
        columns.server_user = Some(&server.user);
        match &server.transport {
            Transport::Daemon { module } => {
                columns.server_transport = Some("daemon");
                columns.server_module = Some(module);
            }
            Transport::Ssh { host_key, fingerprint } => {
                columns.server_transport = Some("ssh");
                columns.server_host_key = Some(host_key);
                columns.server_fingerprint = Some(fingerprint);
            }
        }
    }
    match settings.mode {
        Mode::Mirror { keep_changed_days } => columns.keep_changed_days = keep_changed_days.map(i64::from),
        Mode::Versioned { keep_last, keep_days } => {
            columns.keep_last = keep_last.map(i64::from);
            columns.keep_days = keep_days.map(i64::from);
        }
    }
    if let Some(schedule) = &settings.schedule {
        columns.schedule = Some(match schedule.frequency {
            Frequency::Hourly => "hourly",
            Frequency::Daily => "daily",
            Frequency::Weekly { weekday } => {
                columns.schedule_weekday = Some(i64::from(weekday));
                "weekly"
            }
        });
        columns.schedule_hour = i64::from(schedule.hour);
        columns.schedule_minute = i64::from(schedule.minute);
    }
    columns
}

/// Whether `error` is the name of a task being taken.
fn name_taken(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|error| error.is_unique_violation())
}

macro_rules! select_tasks {
    ($tail:literal $(, $arg:expr)*) => {
        sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", name, folder, source_kind, source_share, source_path, target_kind,
                      target_share, target_drive, target_path, server_transport, server_host, server_port,
                      server_user, server_module, server_host_key, server_fingerprint, mode, keep_changed_days,
                      keep_last, keep_days, schedule, schedule_weekday, schedule_hour, schedule_minute,
                      on_plug AS "on_plug: bool", excludes, hidden AS "hidden: bool", bwlimit_kib,
                      notify_success AS "notify_success: bool", created_at, since, last_plug
               FROM backup_tasks "# + $tail
            $(, $arg)*
        )
    };
}

#[async_trait]
impl TaskRepository for SqliteBackupTasks {
    async fn list(&self) -> Result<Vec<Task>, PortError> {
        let rows = select_tasks!("ORDER BY id").fetch_all(self.db.pool()).await.map_err(db_error)?;
        rows.into_iter().map(Row::task).collect()
    }

    async fn find(&self, id: i64) -> Result<Option<Task>, PortError> {
        let row = select_tasks!("WHERE id = ?", id).fetch_optional(self.db.pool()).await.map_err(db_error)?;
        row.map(Row::task).transpose()
    }

    async fn create(&self, task: &NewTask, password: Option<&Secret>) -> Result<Option<i64>, PortError> {
        let settings = &task.settings;
        let c = columns(settings);
        let password = password.map(Secret::expose);
        let created_at = task.created_at.unix_seconds();
        let done = sqlx::query!(
            "INSERT INTO backup_tasks (name, folder, source_kind, source_share, source_path, target_kind,
                 target_share, target_drive, target_path, server_transport, server_host, server_port, server_user,
                 server_module, server_host_key, server_fingerprint, server_password, mode, keep_changed_days,
                 keep_last, keep_days, schedule, schedule_weekday, schedule_hour, schedule_minute, on_plug,
                 excludes, hidden, bwlimit_kib, notify_success, created_at, since, last_plug)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            settings.name,
            task.folder,
            c.source_kind,
            c.source_share,
            c.source_path,
            c.target_kind,
            c.target_share,
            c.target_drive,
            c.target_path,
            c.server_transport,
            c.server_host,
            c.server_port,
            c.server_user,
            c.server_module,
            c.server_host_key,
            c.server_fingerprint,
            password,
            c.mode,
            c.keep_changed_days,
            c.keep_last,
            c.keep_days,
            c.schedule,
            c.schedule_weekday,
            c.schedule_hour,
            c.schedule_minute,
            settings.on_plug,
            c.excludes,
            settings.filters.hidden,
            c.bwlimit_kib,
            settings.notify_success,
            created_at,
            created_at,
            task.last_plug
        )
        .execute(self.db.pool())
        .await;
        match done {
            Ok(done) => Ok(Some(done.last_insert_rowid())),
            Err(error) if name_taken(&error) => Ok(None),
            Err(error) => Err(db_error(error)),
        }
    }

    async fn update(&self, task: &Task, password: PasswordChange) -> Result<bool, PortError> {
        let settings = &task.settings;
        let c = columns(settings);
        let since = task.since.unix_seconds();
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let done = sqlx::query!(
            "UPDATE backup_tasks SET name = ?, source_kind = ?, source_share = ?, source_path = ?, target_kind = ?,
                 target_share = ?, target_drive = ?, target_path = ?, server_transport = ?, server_host = ?,
                 server_port = ?, server_user = ?, server_module = ?, server_host_key = ?, server_fingerprint = ?,
                 mode = ?, keep_changed_days = ?, keep_last = ?, keep_days = ?, schedule = ?, schedule_weekday = ?,
                 schedule_hour = ?, schedule_minute = ?, on_plug = ?, excludes = ?, hidden = ?, bwlimit_kib = ?,
                 notify_success = ?, since = ?, last_plug = ?
             WHERE id = ?",
            settings.name,
            c.source_kind,
            c.source_share,
            c.source_path,
            c.target_kind,
            c.target_share,
            c.target_drive,
            c.target_path,
            c.server_transport,
            c.server_host,
            c.server_port,
            c.server_user,
            c.server_module,
            c.server_host_key,
            c.server_fingerprint,
            c.mode,
            c.keep_changed_days,
            c.keep_last,
            c.keep_days,
            c.schedule,
            c.schedule_weekday,
            c.schedule_hour,
            c.schedule_minute,
            settings.on_plug,
            c.excludes,
            settings.filters.hidden,
            c.bwlimit_kib,
            settings.notify_success,
            since,
            task.last_plug,
            task.id
        )
        .execute(&mut *transaction)
        .await;
        let done = match done {
            Ok(done) => done,
            Err(error) if name_taken(&error) => return Ok(false),
            Err(error) => return Err(db_error(error)),
        };
        if done.rows_affected() != 1 {
            return Ok(false);
        }
        match password {
            PasswordChange::Keep => {}
            PasswordChange::Set(password) => {
                let password = password.expose();
                sqlx::query!("UPDATE backup_tasks SET server_password = ? WHERE id = ?", password, task.id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
            }
            PasswordChange::Clear => {
                sqlx::query!("UPDATE backup_tasks SET server_password = NULL WHERE id = ?", task.id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
            }
        }
        transaction.commit().await.map_err(db_error)?;
        Ok(true)
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM backup_tasks WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn password(&self, id: i64) -> Result<Option<Secret>, PortError> {
        let row = sqlx::query!("SELECT server_password FROM backup_tasks WHERE id = ?", id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(row.and_then(|row| row.server_password).filter(|p| !p.is_empty()).map(Secret::new))
    }

    async fn noted(&self, id: i64, since: Timestamp) -> Result<(), PortError> {
        let since = since.unix_seconds();
        sqlx::query!("UPDATE backup_tasks SET since = ? WHERE id = ?", since, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn plugged(&self, id: i64, plug: Option<&str>) -> Result<(), PortError> {
        sqlx::query!("UPDATE backup_tasks SET last_plug = ? WHERE id = ?", plug, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }
}

/// [`RunRepository`] on `backup_runs` and `backup_transfers`.
#[derive(Debug, Clone)]
pub struct SqliteBackupRuns {
    db: Database,
}

impl SqliteBackupRuns {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

struct RunRow {
    id: i64,
    task_id: i64,
    cause: String,
    started_by: Option<String>,
    started_at: i64,
    finished_at: Option<i64>,
    outcome: String,
    files: i64,
    bytes: i64,
    version: Option<String>,
    error: Option<String>,
    transfers: i64,
    job_id: Option<i64>,
}

impl RunRow {
    fn run(self) -> Result<Run, PortError> {
        let unsigned = |value: i64| u64::try_from(value).unwrap_or(0);
        Ok(Run {
            id: self.id,
            task_id: self.task_id,
            trigger: Trigger::parse(&self.cause)
                .ok_or_else(|| stored(format!("the cause {:?}", self.cause)))?,
            by: self.started_by,
            started_at: Timestamp::from_unix_seconds(self.started_at),
            finished_at: self.finished_at.map(Timestamp::from_unix_seconds),
            outcome: Outcome::parse(&self.outcome)
                .ok_or_else(|| stored(format!("the outcome {:?}", self.outcome)))?,
            files: unsigned(self.files),
            bytes: unsigned(self.bytes),
            version: self.version,
            error: self.error,
            transfers: unsigned(self.transfers),
            job_id: self.job_id,
        })
    }
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[async_trait]
impl RunRepository for SqliteBackupRuns {
    async fn start(&self, run: &NewRun) -> Result<i64, PortError> {
        let (cause, started_at) = (run.trigger.as_str(), run.started_at.unix_seconds());
        let done = sqlx::query!(
            "INSERT INTO backup_runs (task_id, cause, started_by, started_at, outcome, job_id)
             VALUES (?, ?, ?, ?, 'running', ?)",
            run.task_id,
            cause,
            run.by,
            started_at,
            run.job_id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.last_insert_rowid())
    }

    async fn finish(&self, id: i64, end: &RunEnd) -> Result<(), PortError> {
        let (finished_at, outcome) = (end.finished_at.unix_seconds(), end.outcome.as_str());
        let (files, bytes, transfers) = (signed(end.files), signed(end.bytes), signed(end.transfers_total));
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!(
            "UPDATE backup_runs SET finished_at = ?, outcome = ?, files = ?, bytes = ?, version = ?, error = ?,
                 transfers = ?
             WHERE id = ?",
            finished_at,
            outcome,
            files,
            bytes,
            end.version,
            end.error,
            transfers,
            id
        )
        .execute(&mut *transaction)
        .await
        .map_err(db_error)?;
        for (seq, transfer) in end.transfers.iter().enumerate() {
            let seq = signed(seq as u64);
            sqlx::query!(
                "INSERT INTO backup_transfers (run_id, seq, change, path) VALUES (?, ?, ?, ?)",
                id,
                seq,
                transfer.change,
                transfer.path
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }

    async fn find(&self, id: i64) -> Result<Option<Run>, PortError> {
        let row = sqlx::query_as!(
            RunRow,
            r#"SELECT id AS "id!", task_id, cause, started_by, started_at, finished_at, outcome, files, bytes,
                      version, error, transfers, job_id
               FROM backup_runs WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(RunRow::run).transpose()
    }

    async fn list(&self, task: Option<i64>, limit: u32) -> Result<Vec<Run>, PortError> {
        let limit = i64::from(limit);
        let rows = sqlx::query_as!(
            RunRow,
            r#"SELECT id AS "id!", task_id, cause, started_by, started_at, finished_at, outcome, files, bytes,
                      version, error, transfers, job_id
               FROM backup_runs WHERE ?1 IS NULL OR task_id = ?1 ORDER BY id DESC LIMIT ?2"#,
            task,
            limit
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter().map(RunRow::run).collect()
    }

    async fn transfers(&self, run: i64, offset: u32, limit: u32) -> Result<Vec<Transfer>, PortError> {
        let (offset, limit) = (i64::from(offset), i64::from(limit));
        let rows = sqlx::query!(
            "SELECT change, path FROM backup_transfers WHERE run_id = ? ORDER BY seq LIMIT ? OFFSET ?",
            run,
            limit,
            offset
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(|row| Transfer { change: row.change, path: row.path }).collect())
    }

    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError> {
        let at = at.unix_seconds();
        let done = sqlx::query!(
            "UPDATE backup_runs SET outcome = 'interrupted', finished_at = ? WHERE outcome = 'running'",
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.rows_affected())
    }

    async fn forget_before(&self, at: Timestamp) -> Result<u64, PortError> {
        let at = at.unix_seconds();
        let done = sqlx::query!("DELETE FROM backup_runs WHERE started_at < ? AND outcome <> 'running'", at)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected())
    }
}

/// [`SettingsRepository`] on `backup_settings`; the defaults until saved.
#[derive(Debug, Clone)]
pub struct SqliteBackupSettings {
    db: Database,
}

impl SqliteBackupSettings {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SettingsRepository for SqliteBackupSettings {
    async fn get(&self) -> Result<Settings, PortError> {
        let row = sqlx::query!("SELECT log_days, bwlimit_kib FROM backup_settings WHERE id = 1")
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(match row {
            Some(row) => Settings {
                log_days: small(row.log_days, "days")?,
                bwlimit_kib: small(row.bwlimit_kib, "bandwidth")?,
            },
            None => Settings::default(),
        })
    }

    async fn set(&self, settings: Settings) -> Result<(), PortError> {
        let (days, limit) = (i64::from(settings.log_days), i64::from(settings.bwlimit_kib));
        sqlx::query!(
            "INSERT INTO backup_settings (id, log_days, bwlimit_kib) VALUES (1, ?, ?)
             ON CONFLICT (id) DO UPDATE SET log_days = excluded.log_days, bwlimit_kib = excluded.bwlimit_kib",
            days,
            limit
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
