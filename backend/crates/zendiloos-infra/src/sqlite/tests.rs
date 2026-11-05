use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use tempfile::TempDir;

use super::*;

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap() }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("zendiloos.db")
    }

    fn backups(&self) -> PathBuf {
        self.dir.path().join("backup")
    }

    /// A migrator over the given `(file name, SQL)` pairs, in its own directory.
    async fn migrator(&self, name: &str, migrations: &[(&str, &str)]) -> Migrator {
        let dir = self.dir.path().join(name);
        fs::create_dir(&dir).unwrap();
        for (file, sql) in migrations {
            fs::write(dir.join(file), sql).unwrap();
        }
        Migrator::new(dir.as_path()).await.unwrap()
    }
}

const ITEMS_V1: (&str, &str) =
    ("0001_items.sql", "CREATE TABLE items (id INTEGER PRIMARY KEY); INSERT INTO items DEFAULT VALUES;");
const ITEMS_V2: (&str, &str) = ("0002_more_items.sql", "INSERT INTO items DEFAULT VALUES;");

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

async fn count_items(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM items").fetch_one(pool).await.unwrap()
}

async fn open(env: &Env) -> (Database, OpenReport) {
    Database::open(&env.db(), &env.backups()).await.unwrap()
}

/// A migrated database in a temporary directory, for repository tests. Keep
/// the directory alive as long as the database.
pub(crate) async fn temporary_database() -> (TempDir, Database) {
    let env = Env::new();
    let (db, _) = open(&env).await;
    (env.dir, db)
}

#[tokio::test]
async fn creates_a_private_database_and_applies_all_migrations() {
    let env = Env::new();
    let (db, report) = open(&env).await;

    assert_eq!(
        report,
        OpenReport {
            created: true,
            schema_before: None,
            schema_after: 12,
            applied: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            backup: None
        }
    );
    assert_eq!(mode(&env.db()), 0o600);
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&db.pool).await.unwrap();
    assert_eq!(journal_mode, "wal");
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&db.pool).await.unwrap();
    assert_eq!(foreign_keys, 1);
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE '\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        tables,
        [
            "account_preferences",
            "accounts",
            "audit_log",
            "file_favourites",
            "group_members",
            "groups",
            "jobs",
            "notification_channels",
            "notification_marks",
            "notifications",
            "self_test_plan_disks",
            "self_test_plans",
            "sessions",
            "share_access",
            "share_nfs_clients",
            "share_snapshot_schedules",
            "shares"
        ]
    );
    db.close().await;
}

#[tokio::test]
async fn reopening_an_up_to_date_database_changes_nothing() {
    let env = Env::new();
    open(&env).await.0.close().await;

    let (db, report) = open(&env).await;

    assert_eq!(
        report,
        OpenReport {
            created: false,
            schema_before: Some(12),
            schema_after: 12,
            applied: vec![],
            backup: None
        }
    );
    assert!(!env.backups().exists(), "no backup without pending migrations");
    db.close().await;
}

#[tokio::test]
async fn backs_up_before_migrating() {
    let env = Env::new();
    let v1 = env.migrator("v1", &[ITEMS_V1]).await;
    let v2 = env.migrator("v2", &[ITEMS_V1, ITEMS_V2]).await;
    let now = UNIX_EPOCH + Duration::from_secs(1_793_780_393);
    Database::open_with(&env.db(), &env.backups(), &v1, now).await.unwrap().0.close().await;

    let (db, report) = Database::open_with(&env.db(), &env.backups(), &v2, now).await.unwrap();

    let backup = report.backup.expect("a backup before migrating");
    assert_eq!(backup, env.backups().join("zendiloos-20261104T081953Z-schema-1.db"));
    assert_eq!((report.schema_before, report.schema_after, report.applied), (Some(1), 2, vec![2]));
    assert_eq!(mode(&backup), 0o600);
    assert_eq!(mode(&env.backups()), 0o700);
    assert_eq!(count_items(&db.pool).await, 2);

    let copy = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&backup)).await.unwrap();
    assert_eq!(count_items(&copy).await, 1, "the backup holds the state before the migration");
    copy.close().await;
    db.close().await;
}

#[tokio::test]
async fn refuses_a_database_from_a_newer_release() {
    let env = Env::new();
    let v1 = env.migrator("v1", &[ITEMS_V1]).await;
    let v2 = env.migrator("v2", &[ITEMS_V1, ITEMS_V2]).await;
    Database::open_with(&env.db(), &env.backups(), &v2, SystemTime::now()).await.unwrap().0.close().await;

    let error = Database::open_with(&env.db(), &env.backups(), &v1, SystemTime::now()).await.unwrap_err();

    assert!(matches!(error, DatabaseError::TooNew { found: 2, supported: 1 }), "{error:?}");
    assert!(!env.backups().exists());
}

#[tokio::test]
async fn refuses_a_foreign_database() {
    let env = Env::new();
    let other =
        SqlitePool::connect_with(SqliteConnectOptions::new().filename(env.db()).create_if_missing(true))
            .await
            .unwrap();
    sqlx::query("CREATE TABLE something_else (x)").execute(&other).await.unwrap();
    other.close().await;

    let error = Database::open(&env.db(), &env.backups()).await.unwrap_err();

    assert!(matches!(error, DatabaseError::Foreign { .. }), "{error:?}");
}

pub(crate) async fn insert_account(pool: &SqlitePool, name: &str, role: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, role, password_hash, created_at, updated_at) \
         VALUES (?, ?, 'hash', 0, 0) RETURNING id",
    )
    .bind(name)
    .bind(role)
    .fetch_one(pool)
    .await
}

#[tokio::test]
async fn accounts_reject_unknown_roles_and_uppercase_names() {
    let env = Env::new();
    let (db, _) = open(&env).await;

    assert!(insert_account(&db.pool, "alice", "admin").await.is_ok());
    assert!(insert_account(&db.pool, "bob", "root").await.is_err());
    assert!(insert_account(&db.pool, "Carol", "user").await.is_err());
    assert!(insert_account(&db.pool, "", "user").await.is_err());
    assert!(insert_account(&db.pool, "alice", "user").await.is_err(), "names are unique");
    db.close().await;
}

#[tokio::test]
async fn audit_log_is_append_only() {
    let env = Env::new();
    let (db, _) = open(&env).await;
    sqlx::query("INSERT INTO audit_log (at, action, outcome) VALUES (0, 'identity.login', 'failure')")
        .execute(&db.pool)
        .await
        .unwrap();

    let error = sqlx::query("UPDATE audit_log SET outcome = 'success'").execute(&db.pool).await.unwrap_err();

    assert!(error.to_string().contains("append-only"), "{error}");
    db.close().await;
}

#[tokio::test]
async fn deleting_an_account_removes_its_sessions_and_keeps_its_audit_trail() {
    let env = Env::new();
    let (db, _) = open(&env).await;
    let id = insert_account(&db.pool, "alice", "user").await.unwrap();
    sqlx::query(
        "INSERT INTO sessions (token_hash, account_id, csrf_token, created_at, last_seen_at, expires_at) \
         VALUES (zeroblob(32), ?, 'csrf', 0, 0, 0)",
    )
    .bind(id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_log (at, account_id, actor, action, outcome) \
         VALUES (0, ?, 'alice', 'identity.login', 'success')",
    )
    .bind(id)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query("DELETE FROM accounts WHERE id = ?").bind(id).execute(&db.pool).await.unwrap();

    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sessions").fetch_one(&db.pool).await.unwrap();
    assert_eq!(sessions, 0);
    let audit: (Option<i64>, String) =
        sqlx::query_as("SELECT account_id, actor FROM audit_log").fetch_one(&db.pool).await.unwrap();
    assert_eq!(audit, (Some(id), "alice".to_owned()), "audit rows are history and stay as written");
    db.close().await;
}

#[tokio::test]
async fn account_ids_are_never_reused() {
    let env = Env::new();
    let (db, _) = open(&env).await;
    let alice = insert_account(&db.pool, "alice", "user").await.unwrap();
    sqlx::query("DELETE FROM accounts WHERE id = ?").bind(alice).execute(&db.pool).await.unwrap();

    let bob = insert_account(&db.pool, "bob", "user").await.unwrap();

    assert_ne!(alice, bob, "the audit log would attribute alice's actions to bob");
    db.close().await;
}
