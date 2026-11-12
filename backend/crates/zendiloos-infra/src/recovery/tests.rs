use std::sync::Arc;

use zendiloos_agent_proto::{AgentTransport, PoolName, Request, ShareName as ProtoShare, VolumeNumber};
use zendiloos_kernel::{Actor, AuditEntry, Outcome, RequestContext};
use zendiloos_recovery::application::{BackupFolders, Restart, RestoreError, Restorer, Snapshots};
use zendiloos_recovery::domain::{Passphrase, Target};
use zendiloos_sim::SimulatedAgent;

use super::*;
use crate::SystemClock;
use crate::agent::AgentFiles;
use crate::sqlite::tests::temporary_database;

fn passphrase() -> Passphrase {
    Passphrase::new("correct horse battery").unwrap()
}

fn audit() -> AuditEntry {
    AuditEntry::new(
        Timestamp::from_unix_seconds(1_800_000_000),
        "configuration.restore",
        Outcome::Success,
        &RequestContext::default(),
    )
    .by(&Actor { account_id: 9, name: "alice".into() })
}

#[tokio::test]
async fn a_backup_is_restored_on_another_installation_with_its_database_staged() {
    let (_dir, db) = temporary_database().await;
    let state = tempfile::tempdir().unwrap();
    let agent: Arc<dyn AgentTransport> = Arc::new(SimulatedAgent::new());
    let snapshots = ConfigurationSnapshots::new(
        db.clone(),
        agent.clone(),
        state.path().to_owned(),
        "0.1.0",
        Arc::new(SystemClock),
    );

    let archive = snapshots.create(&passphrase(), &RequestContext::default()).await.unwrap();

    assert!(archive.name.starts_with("zendiloos-config-zendiloos-"), "{}", archive.name);
    assert_eq!(archive.manifest.schema, Database::supported_schema());
    assert!(fs::read_dir(state.path().join("configuration")).unwrap().next().is_none(), "no copy left");

    let fresh = tempfile::tempdir().unwrap();
    let restorer = ConfigurationRestorer::new(agent, fresh.path().to_owned());
    let wrong = restorer
        .restore(
            archive.bytes.clone(),
            &Passphrase::new("another passphrase").unwrap(),
            audit(),
            &RequestContext::default(),
        )
        .await;
    assert!(matches!(wrong, Err(RestoreError::WrongPassphrase)), "{wrong:?}");

    let restored =
        restorer.restore(archive.bytes, &passphrase(), audit(), &RequestContext::default()).await.unwrap();

    assert_eq!(restored.manifest.host_name, "zendiloos");
    assert_eq!(restored.https_port, None, "the web server's settings never set");
    assert!(restored.warnings.is_empty(), "{:?}", restored.warnings);
    let staged = fresh.path().join("restore/zendiloos.db");
    let leftovers: Vec<_> = fs::read_dir(fresh.path().join("restore")).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "only the staged database");

    // The next start puts it in place, the database before kept aside.
    let live = fresh.path().join("zendiloos.db");
    fs::write(&live, b"before").unwrap();
    fs::write(fresh.path().join("zendiloos.db-wal"), b"wal").unwrap();
    let before = stage_restored_database(fresh.path(), &live, &fresh.path().join("backup")).unwrap().unwrap();
    assert!(!staged.exists());
    assert_eq!(fs::read(&before).unwrap(), b"before");
    assert_eq!(fs::read(format!("{}-wal", before.display())).unwrap(), b"wal");
    assert!(!fresh.path().join("zendiloos.db-wal").exists());
    assert!(restore_to_follow_up(fresh.path()), "the start follows the restore up");
    assert_eq!(stage_restored_database(fresh.path(), &live, &fresh.path().join("backup")).unwrap(), None);
    restore_followed_up(fresh.path()).unwrap();
    assert!(!restore_to_follow_up(fresh.path()));
    restore_followed_up(fresh.path()).unwrap();
    let (restored_db, report) = Database::open(&live, &fresh.path().join("backup")).await.unwrap();
    assert!(report.applied.is_empty());
    let actions: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT action, actor, target FROM audit_log")
            .fetch_all(restored_db.pool())
            .await
            .unwrap();
    assert_eq!(
        actions,
        [("configuration.restore".to_owned(), Some("alice".to_owned()), Some("zendiloos".to_owned()))],
        "written into the restored database"
    );
}

#[tokio::test]
async fn a_database_newer_than_this_release_is_refused_and_nothing_is_staged() {
    let (_dir, db) = temporary_database().await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
         VALUES (9999, 'from the future', CURRENT_TIMESTAMP, 1, x'00', 0)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let state = tempfile::tempdir().unwrap();
    let agent: Arc<dyn AgentTransport> = Arc::new(SimulatedAgent::new());
    let snapshots = ConfigurationSnapshots::new(
        db,
        agent.clone(),
        state.path().to_owned(),
        "9.0.0",
        Arc::new(SystemClock),
    );
    let archive = snapshots.create(&passphrase(), &RequestContext::default()).await.unwrap();
    assert_eq!(archive.manifest.schema, 9999);

    let fresh = tempfile::tempdir().unwrap();
    let refused = ConfigurationRestorer::new(agent, fresh.path().to_owned())
        .restore(archive.bytes, &passphrase(), audit(), &RequestContext::default())
        .await;

    assert!(matches!(refused, Err(RestoreError::TooNew { found: 9999, .. })), "{refused:?}");
    assert!(fs::read_dir(fresh.path().join("restore")).unwrap().next().is_none());
}

/// The agent refuses the accounts (a UID is another's here): refused in
/// its words, nothing staged.
#[tokio::test]
async fn a_restore_the_host_refuses_stages_nothing() {
    use zendiloos_agent_proto::{ErrorCode, RequestId, Response, TransportError};

    struct Refusing(SimulatedAgent);

    #[async_trait::async_trait]
    impl AgentTransport for Refusing {
        async fn call(&self, request: Request, id: Option<RequestId>) -> Result<Response, TransportError> {
            match request {
                Request::RestoreAccounts { .. } => Ok(Response::Error {
                    code: ErrorCode::Forbidden,
                    message: "refused: UID 2001 of anna is eve's here".into(),
                }),
                other => self.0.call(other, id).await,
            }
        }
    }

    let (_dir, db) = temporary_database().await;
    let state = tempfile::tempdir().unwrap();
    let source: Arc<dyn AgentTransport> = Arc::new(SimulatedAgent::new());
    let snapshots =
        ConfigurationSnapshots::new(db, source, state.path().to_owned(), "0.1.0", Arc::new(SystemClock));
    let mut archive = snapshots.create(&passphrase(), &RequestContext::default()).await.unwrap();
    // An archive with an account in it, as a machine with people has.
    let opened = archive::read(&archive.bytes, "correct horse battery", &state.path().join("x.db")).unwrap();
    let accounts = br#"{"users":[{"name":"anna","uid":2001}],"groups":[]}"#;
    let manifest = opened.manifest.clone();
    let contents = archive::Contents {
        manifest: &manifest,
        settings: &opened.settings,
        accounts,
        database: &state.path().join("x.db"),
        files: &opened.files,
    };
    archive.bytes = archive::write(&contents, "correct horse battery").unwrap();

    let fresh = tempfile::tempdir().unwrap();
    let refused =
        ConfigurationRestorer::new(Arc::new(Refusing(SimulatedAgent::new())), fresh.path().to_owned())
            .restore(archive.bytes, &passphrase(), audit(), &RequestContext::default())
            .await;

    assert!(matches!(&refused, Err(RestoreError::Refused(why)) if why.contains("eve's")), "{refused:?}");
    assert!(fs::read_dir(fresh.path().join("restore")).unwrap().next().is_none());
}

#[tokio::test]
async fn backups_are_written_into_a_share_and_listed_and_removed() {
    let (_dir, db) = temporary_database().await;
    sqlx::query("INSERT INTO shares (name, pool, volume, recycle_bin, created_at, updated_at) VALUES ('backups', 'tank', 1, 0, 0, 0)")
        .execute(db.pool())
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let agent = Arc::new(SimulatedAgent::new().with_files(root.path()));
    let share = Request::CreateShare {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        name: ProtoShare::parse("backups").unwrap(),
    };
    agent.call(share, None).await.unwrap();
    let folders = AgentBackupFolders::new(Arc::new(AgentFiles::new(agent)), db);
    let target = Target::Share { share_id: 1, folder: "zendiloOS/config".into() };
    let context = RequestContext::default();

    folders
        .write(
            &target,
            "alice",
            "zendiloos-config-nas-20261112T030000Z.tar.zst.age",
            b"one".to_vec(),
            &context,
        )
        .await
        .unwrap();
    folders
        .write(
            &target,
            "alice",
            "zendiloos-config-nas-20261113T030000Z.tar.zst.age",
            b"two".to_vec(),
            &context,
        )
        .await
        .unwrap();

    let folder = root.path().join("volume1/backups/zendiloOS/config");
    assert_eq!(fs::read(folder.join("zendiloos-config-nas-20261113T030000Z.tar.zst.age")).unwrap(), b"two");
    let mut names = folders.names(&target, "alice", &context).await.unwrap();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    folders.remove(&target, "alice", &names[0], &context).await.unwrap();
    assert_eq!(folders.names(&target, "alice", &context).await.unwrap().len(), 1);
    let gone = Target::Share { share_id: 7, folder: String::new() };
    assert!(folders.write(&gone, "alice", "x", vec![], &context).await.is_err());
}

#[tokio::test]
async fn a_restart_comes_once_asked_for() {
    let restart = ProcessRestart::new();
    assert!(!restart.was_requested());
    let waiting = tokio::spawn({
        let restart = restart.clone();
        async move { restart.requested().await }
    });
    tokio::task::yield_now().await;
    restart.restart();
    tokio::time::timeout(Duration::from_secs(10), waiting).await.unwrap().unwrap();
    assert!(restart.was_requested());
}
