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
            schema_after: 51,
            applied: (1..=51).collect(),
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
            "account_two_factor",
            "accounts",
            "acme_accounts",
            "acme_challenges",
            "allowed_addresses",
            "app_icons",
            "app_sources",
            "audit_log",
            "blocked_addresses",
            "certificates",
            "disk_inventory",
            "disk_shared_serials",
            "docker_backup_plans",
            "docker_backups",
            "docker_owed_starts",
            "docker_projects",
            "docker_update_settings",
            "enclosure",
            "enclosure_bindings",
            "enclosure_groups",
            "enclosure_slots",
            "file_favourites",
            "file_services",
            "firewall_blocked",
            "firewall_project_ports",
            "firewall_rules",
            "firewall_settings",
            "group_members",
            "groups",
            "homes",
            "homes_location",
            "installed_apps",
            "jobs",
            "notification_channels",
            "notification_mail_server",
            "notification_marks",
            "notification_routes",
            "notifications",
            "password_policy",
            "pending_uploads",
            "removable_ejected",
            "security_settings",
            "self_test_plan_disks",
            "self_test_plans",
            "sessions",
            "share_access",
            "share_app_access",
            "share_nfs_clients",
            "share_snapshot_schedules",
            "shares",
            "system_update_settings",
            "system_update_state",
            "vm_disks",
            "vm_drives",
            "vm_images",
            "vm_locations",
            "vm_nics",
            "vms",
            "web_server"
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
            schema_before: Some(51),
            schema_after: 51,
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

/// The migrations of the package, up to `version`.
fn migrations_up_to(version: i64) -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.split('_').next().and_then(|n| n.parse::<i64>().ok()).is_some_and(|n| n <= version)
        })
        .map(|name| {
            let sql = fs::read_to_string(dir.join(&name)).unwrap();
            (name, sql)
        })
        .collect();
    files.sort();
    files
}

/// The VMs of the proof of concept: their ISO's copies become the library's
/// images, of the same ids; their disk, card and a drive the devices.
#[tokio::test]
async fn the_vms_of_before_carry_on_with_their_isos_in_the_library() {
    use zendiloos_vms::application::{VmImageRepository, VmLocationRepository, VmRepository};
    use zendiloos_vms::domain::{BootDevice, DiskBus, ImageOrigin, Network, NicModel};

    let env = Env::new();
    let before = migrations_up_to(49);
    let pairs: Vec<(&str, &str)> = before.iter().map(|(name, sql)| (name.as_str(), sql.as_str())).collect();
    let v49 = env.migrator("v49", &pairs).await;
    let (db, _) = Database::open_with(&env.db(), &env.backups(), &v49, SystemTime::now()).await.unwrap();
    for (id, name, os, bridge, share, path, created, ready) in [
        ("0a1b2c3d4e5f", "web", "linux", None, Some("media"), Some("isos/debian.iso"), 1, 1),
        ("00000000000b", "Büro-PC", "windows", Some("br0"), Some("media"), Some("win/Win11.iso"), 2, 1),
        ("00000000000c", "lab", "linux", None, Some("other"), Some("DEBIAN.ISO"), 3, 1),
        ("00000000000d", "plain", "other", None, None, None, 4, 1),
        ("00000000000e", "half", "linux", None, Some("media"), Some("x.iso"), 5, 0),
    ] {
        sqlx::query(
            "INSERT INTO vms (id, name, os, cpus, memory_mib, firmware, disk_gib, bridge, autostart, iso_share,
                              iso_path, pool, volume, created_at, ready)
             VALUES (?, ?, ?, 2, 2048, 'uefi', 20, ?, 0, ?, ?, 'tank', 1, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(os)
        .bind(bridge)
        .bind(share)
        .bind(path)
        .bind(created)
        .bind(ready)
        .execute(db.pool())
        .await
        .unwrap();
    }
    db.close().await;

    let (db, report) = open(&env).await;
    assert!(report.applied.contains(&50), "{:?}", report.applied);
    let images = super::SqliteVmImages::new(db.clone()).list().await.unwrap();
    let names: Vec<(&str, &str)> =
        images.iter().map(|image| (image.id.as_str(), image.name.as_str())).collect();
    assert_eq!(
        names,
        [("0a1b2c3d4e5f", "debian.iso"), ("00000000000b", "Win11.iso"), ("00000000000c", "DEBIAN.ISO (2)")],
        "one image per ISO a VM had, named after its file, numbered where names meet"
    );
    assert!(images.iter().all(|image| image.ready && image.bytes.is_none()));
    assert_eq!(images[1].origin, ImageOrigin::Share { share: "media".into(), path: "win/Win11.iso".into() });

    let locations = super::SqliteVmLocations::new(db.clone()).list().await.unwrap();
    assert_eq!(locations, [zendiloos_vms::domain::Location { pool: "tank".into(), volume: 1 }]);
    let vms = super::SqliteVms::new(db).list().await.unwrap();
    assert_eq!(vms.len(), 5);
    let web = &vms[0].settings;
    assert_eq!((web.disks[0].slot, web.disks[0].size_gib, web.disks[0].bus), (0, 20, DiskBus::Virtio));
    assert_eq!(web.disks[0].location, locations[0], "on the pool the VM was on");
    assert_eq!(web.nics[0].model, NicModel::Virtio);
    assert_eq!(web.nics[0].mac, "52:54:00:3d:4e:5f");
    assert_eq!(web.drives[0].image.as_deref(), Some("0a1b2c3d4e5f"));
    assert_eq!(web.boot, [BootDevice::Drive(0), BootDevice::Disk(0)]);
    let office = &vms[1].settings;
    assert_eq!((office.disks[0].bus, office.nics[0].model), (DiskBus::Sata, NicModel::E1000e));
    assert_eq!(office.nics[0].network, Network::Bridge("br0".into()));
    let plain = &vms[3].settings;
    assert_eq!(plain.drives[0].image, None, "an empty drive, for an image later");
    assert_eq!(plain.boot, [BootDevice::Disk(0), BootDevice::Drive(0)]);
    assert_eq!(vms[4].settings.drives[0].image, None, "a VM never made keeps no image");
}

/// The VMs of package 1: each disk on the location of its VM, which
/// becomes the first, cards through the NAS or on their bridge, and the
/// location of the first VM made first.
#[tokio::test]
async fn the_vms_of_one_location_keep_their_disks_and_cards_where_they_were() {
    use zendiloos_vms::application::{VmLocationRepository, VmRepository};
    use zendiloos_vms::domain::{Location, Network};

    let env = Env::new();
    let before = migrations_up_to(50);
    let pairs: Vec<(&str, &str)> = before.iter().map(|(name, sql)| (name.as_str(), sql.as_str())).collect();
    let v50 = env.migrator("v50", &pairs).await;
    let (db, _) = Database::open_with(&env.db(), &env.backups(), &v50, SystemTime::now()).await.unwrap();
    for (id, name, pool, volume, created) in [
        ("0a1b2c3d4e5f", "web", "cold", 2, 5),
        ("00000000000b", "mail", "tank", 1, 9),
        ("00000000000c", "lab", "cold", 2, 7),
    ] {
        sqlx::query(
            "INSERT INTO vms (id, name, os, cpus, memory_mib, firmware, autostart, boot, pool, volume, created_at, ready)
             VALUES (?, ?, 'linux', 1, 256, 'uefi', 0, 'disk:0', ?, ?, ?, 1)",
        )
        .bind(id)
        .bind(name)
        .bind(pool)
        .bind(volume)
        .bind(created)
        .execute(db.pool())
        .await
        .unwrap();
        for slot in [0, 3] {
            sqlx::query("INSERT INTO vm_disks (vm, slot, size_gib, bus) VALUES (?, ?, 4, 'virtio')")
                .bind(id)
                .bind(slot)
                .execute(db.pool())
                .await
                .unwrap();
        }
        let bridge = (name == "mail").then_some("br0");
        sqlx::query("INSERT INTO vm_nics (vm, slot, model, bridge, mac) VALUES (?, 0, 'virtio', ?, ?)")
            .bind(id)
            .bind(bridge)
            .bind(format!("52:54:00:00:00:{}", &id[10..]))
            .execute(db.pool())
            .await
            .unwrap();
    }
    db.close().await;

    let (db, report) = open(&env).await;
    assert_eq!(report.applied, [51]);
    let cold = Location { pool: "cold".into(), volume: 2 };
    let tank = Location { pool: "tank".into(), volume: 1 };
    let locations = super::SqliteVmLocations::new(db.clone()).list().await.unwrap();
    assert_eq!(locations, [cold.clone(), tank.clone()], "the first VM's first");
    let vms = super::SqliteVms::new(db).list().await.unwrap();
    let web = &vms.iter().find(|vm| vm.settings.name == "web").unwrap().settings;
    assert!(web.disks.len() == 2 && web.disks.iter().all(|disk| disk.location == cold));
    assert_eq!(web.nics[0].network, Network::User);
    assert_eq!(web.nics[0].mac, "52:54:00:00:00:5f");
    let mail = &vms.iter().find(|vm| vm.settings.name == "mail").unwrap().settings;
    assert!(mail.disks.iter().all(|disk| disk.location == tank));
    assert_eq!(mail.nics[0].network, Network::Bridge("br0".into()));
}
