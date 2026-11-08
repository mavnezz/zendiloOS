//! SQLite: the database behind the repositories of all contexts.
//!
//! Opening the database also brings its schema up to date. Before any
//! migration touches an existing database, a copy is written to the backup
//! directory, and a database written by a newer release is refused instead
//! of being guessed at.

mod accounts;
mod app_sources;
mod audit;
mod backup;
mod certificates;
mod channels;
mod enclosure;
mod firewall;
mod groups;
mod installed_apps;
mod jobs;
mod notifications;
mod occupants;
mod preferences;
mod projects;
mod security;
mod self_tests;
mod sessions;
mod share_projects;
mod shares;
mod system_updates;
mod two_factor;
mod update_settings;

#[cfg(test)]
mod tests;

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

pub use accounts::SqliteAccounts;
pub use app_sources::SqliteAppSources;
pub use audit::SqliteAuditLog;
pub use certificates::SqliteCertificates;
pub use channels::{SqliteAccountAddresses, SqliteChannels};
pub use enclosure::{SqliteDiskInventory, SqliteEnclosure};
pub use firewall::SqliteFirewall;
pub use groups::SqliteGroups;
pub use installed_apps::SqliteInstalledApps;
pub use jobs::SqliteJobs;
pub use notifications::SqliteNotifications;
pub use occupants::Occupants;
pub use preferences::SqlitePreferences;
pub use projects::SqliteProjects;
pub use security::SqliteSecurity;
pub use self_tests::SqliteSelfTestPlans;
pub use sessions::SqliteSessions;
pub use share_projects::SqliteShareProjects;
pub use shares::SqliteShares;
pub use system_updates::SqliteSystemUpdates;
pub use two_factor::SqliteTwoFactor;
pub use update_settings::SqliteUpdateSettings;

static MIGRATOR: Migrator = sqlx::migrate!();

/// Connections in the pool. SQLite serializes writers anyway; this is plenty
/// for an appliance with a handful of concurrent administrators.
const MAX_CONNECTIONS: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("cannot create {path}")]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot open {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("{path} is not a Zendiloos database")]
    Foreign { path: PathBuf },
    #[error(
        "the database schema (version {found}) is newer than this release supports \
         (version {supported}); refusing to start to protect it"
    )]
    TooNew { found: i64, supported: i64 },
    #[error("cannot read the schema version")]
    Inspect(#[source] sqlx::Error),
    #[error("cannot back up the database to {dir}")]
    Backup {
        dir: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("database migration failed")]
    Migrate(#[source] MigrateError),
}

/// What [`Database::open`] did, for the startup log.
#[derive(Debug, PartialEq)]
pub struct OpenReport {
    /// The database file did not exist before.
    pub created: bool,
    /// Schema version found, `None` for a new database.
    pub schema_before: Option<i64>,
    pub schema_after: i64,
    /// Versions of the migrations applied now.
    pub applied: Vec<i64>,
    /// Copy made before migrating, if any.
    pub backup: Option<PathBuf>,
}

/// The database, ready for the repositories. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Opens the database at `path`, creating it if needed, and applies all
    /// pending migrations. Backups go to `backup_dir`.
    pub async fn open(path: &Path, backup_dir: &Path) -> Result<(Self, OpenReport), DatabaseError> {
        Self::open_with(path, backup_dir, &MIGRATOR, SystemTime::now()).await
    }

    async fn open_with(
        path: &Path,
        backup_dir: &Path,
        migrator: &Migrator,
        now: SystemTime,
    ) -> Result<(Self, OpenReport), DatabaseError> {
        let created = create_private_file(path)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|source| DatabaseError::Open { path: path.to_owned(), source })?;

        match migrate(&pool, path, backup_dir, migrator, now).await {
            Ok(report) => Ok((Self { pool }, OpenReport { created, ..report })),
            Err(error) => {
                pool.close().await;
                Err(error)
            }
        }
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Waits for running queries and closes all connections, which also
    /// checkpoints the write-ahead log into the database file.
    pub async fn close(self) {
        self.pool.close().await;
    }
}

/// Creates `path` readable only by its owner, since the database holds
/// password hashes. SQLite gives its `-wal` and `-shm` files the same mode.
/// Returns whether the file was created.
fn create_private_file(path: &Path) -> Result<bool, DatabaseError> {
    match OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(DatabaseError::Create { path: path.to_owned(), source }),
    }
}

enum Schema {
    /// No tables at all: a new database.
    Empty,
    /// Tables, but not ours.
    Foreign,
    /// Versions of the successfully applied migrations, ascending.
    Versions(Vec<i64>),
}

async fn read_schema(pool: &SqlitePool) -> Result<Schema, sqlx::Error> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'").fetch_all(pool).await?;
    if tables.is_empty() {
        return Ok(Schema::Empty);
    }
    if !tables.iter().any(|name| name == "_sqlx_migrations") {
        return Ok(Schema::Foreign);
    }
    let versions =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version")
            .fetch_all(pool)
            .await?;
    Ok(Schema::Versions(versions))
}

async fn migrate(
    pool: &SqlitePool,
    path: &Path,
    backup_dir: &Path,
    migrator: &Migrator,
    now: SystemTime,
) -> Result<OpenReport, DatabaseError> {
    let known: Vec<i64> =
        migrator.iter().filter(|m| m.migration_type.is_up_migration()).map(|m| m.version).collect();
    let supported = known.iter().copied().max().unwrap_or(0);

    let applied = match read_schema(pool).await.map_err(DatabaseError::Inspect)? {
        Schema::Empty => Vec::new(),
        Schema::Foreign => return Err(DatabaseError::Foreign { path: path.to_owned() }),
        Schema::Versions(versions) => versions,
    };
    let schema_before = applied.last().copied();
    if let Some(found) = schema_before.filter(|&found| found > supported) {
        return Err(DatabaseError::TooNew { found, supported });
    }

    let pending: Vec<i64> = known.iter().copied().filter(|version| !applied.contains(version)).collect();
    let backup = match schema_before {
        Some(schema) if !pending.is_empty() => Some(
            backup::create(pool, backup_dir, schema, now)
                .await
                .map_err(|source| DatabaseError::Backup { dir: backup_dir.to_owned(), source })?,
        ),
        _ => None,
    };

    migrator.run(pool).await.map_err(DatabaseError::Migrate)?;

    Ok(OpenReport { created: false, schema_before, schema_after: supported, applied: pending, backup })
}
