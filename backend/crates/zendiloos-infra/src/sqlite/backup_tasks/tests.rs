use super::*;
use crate::sqlite::tests::temporary_database;

fn ssh_push() -> TaskSettings {
    TaskSettings {
        name: "Photos offsite".into(),
        source: Place::Nas(NasPlace::Share { share_id: 1, path: "albums".into() }),
        target: Place::Remote(RemotePlace {
            server: Server {
                transport: Transport::Ssh {
                    host_key:
                        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
                            .into(),
                    fingerprint: "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU".into(),
                },
                host: "nas2.lan".into(),
                port: 2222,
                user: "backup".into(),
            },
            path: "/srv/backups".into(),
        }),
        mode: Mode::Versioned { keep_last: Some(30), keep_days: Some(90) },
        schedule: Some(Schedule { frequency: Frequency::Weekly { weekday: 6 }, hour: 3, minute: 15 }),
        on_plug: false,
        filters: Filters { excludes: vec!["*.tmp".into(), "@eaDir".into()], hidden: false },
        bwlimit_kib: Some(2048),
        notify_success: true,
    }
}

fn new(settings: TaskSettings) -> NewTask {
    NewTask {
        folder: settings.name.clone(),
        settings,
        created_at: Timestamp::from_unix_seconds(100),
        last_plug: None,
    }
}

#[tokio::test]
async fn tasks_round_trip_and_names_are_unique_without_case() {
    let (_dir, db) = temporary_database().await;
    let tasks = SqliteBackupTasks::new(db);
    let id = tasks.create(&new(ssh_push()), None).await.unwrap().unwrap();
    let stored = tasks.find(id).await.unwrap().unwrap();
    assert_eq!(stored.settings, ssh_push());
    assert_eq!(
        (stored.folder.as_str(), stored.since, stored.created_at.unix_seconds()),
        ("Photos offsite", Timestamp::from_unix_seconds(100), 100)
    );
    let mut same = ssh_push();
    same.name = "PHOTOS OFFSITE".into();
    assert_eq!(tasks.create(&new(same), None).await.unwrap(), None);

    // A pull from a daemon into a drive, a mirror, by hand.
    let pull = TaskSettings {
        name: "Web server".into(),
        source: Place::Remote(RemotePlace {
            server: Server {
                transport: Transport::Daemon { module: "www".into() },
                host: "web.lan".into(),
                port: 873,
                user: "nas".into(),
            },
            path: "html".into(),
        }),
        target: Place::Nas(NasPlace::Drive { drive: "0c5f8a2e".into(), path: "servers".into() }),
        mode: Mode::Mirror { keep_changed_days: Some(14) },
        schedule: None,
        on_plug: true,
        filters: Filters::default(),
        bwlimit_kib: None,
        notify_success: false,
    };
    let pulled = tasks.create(&new(pull.clone()), Some(&Secret::new("s3cret"))).await.unwrap().unwrap();
    assert_eq!(tasks.find(pulled).await.unwrap().unwrap().settings, pull);
    assert_eq!(tasks.password(pulled).await.unwrap().unwrap().expose(), "s3cret");
    assert!(tasks.password(id).await.unwrap().is_none());
    assert_eq!(tasks.list().await.unwrap().len(), 2);

    let mut task = tasks.find(pulled).await.unwrap().unwrap();
    task.settings.name = "Web".into();
    task.last_plug = Some("boot:1".into());
    assert!(tasks.update(&task, PasswordChange::Keep).await.unwrap());
    assert_eq!(tasks.password(pulled).await.unwrap().unwrap().expose(), "s3cret");
    assert!(tasks.update(&task, PasswordChange::Set(Secret::new("other"))).await.unwrap());
    assert_eq!(tasks.password(pulled).await.unwrap().unwrap().expose(), "other");
    assert!(tasks.update(&task, PasswordChange::Clear).await.unwrap());
    assert!(tasks.password(pulled).await.unwrap().is_none());
    task.settings.name = "photos OFFSITE".into();
    assert!(!tasks.update(&task, PasswordChange::Keep).await.unwrap(), "name taken");

    tasks.noted(id, Timestamp::from_unix_seconds(500)).await.unwrap();
    tasks.plugged(id, Some("boot:2")).await.unwrap();
    let noted = tasks.find(id).await.unwrap().unwrap();
    assert_eq!((noted.since.unix_seconds(), noted.last_plug.as_deref()), (500, Some("boot:2")));
    assert!(tasks.delete(id).await.unwrap());
    assert!(!tasks.delete(id).await.unwrap());
}

#[tokio::test]
async fn runs_keep_their_transfer_list_and_go_with_their_task_or_their_days() {
    let (_dir, db) = temporary_database().await;
    let tasks = SqliteBackupTasks::new(db.clone());
    let runs = SqliteBackupRuns::new(db);
    let task = tasks.create(&new(ssh_push()), None).await.unwrap().unwrap();
    let start = |at| NewRun {
        task_id: task,
        trigger: Trigger::Schedule,
        by: None,
        started_at: Timestamp::from_unix_seconds(at),
        job_id: Some(4),
    };
    let first = runs.start(&start(10)).await.unwrap();
    let second = runs
        .start(&NewRun { by: Some("alice".into()), trigger: Trigger::Manual, ..start(20) })
        .await
        .unwrap();
    runs.finish(
        first,
        &RunEnd {
            finished_at: Timestamp::from_unix_seconds(15),
            outcome: Outcome::Partial,
            files: 2,
            bytes: 300,
            version: Some("2026-11-11_030000Z".into()),
            error: Some("file has vanished".into()),
            transfers: vec![
                Transfer { change: ">f+++++++++".into(), path: "a.jpg".into() },
                Transfer { change: "*deleting".into(), path: "old.txt".into() },
            ],
            transfers_total: 7,
        },
    )
    .await
    .unwrap();
    let listed = runs.list(Some(task), 10).await.unwrap();
    assert_eq!(listed.iter().map(|run| run.id).collect::<Vec<_>>(), [second, first], "newest first");
    let done = runs.find(first).await.unwrap().unwrap();
    assert_eq!((done.outcome, done.files, done.transfers, done.job_id), (Outcome::Partial, 2, 7, Some(4)));
    assert_eq!(done.version.as_deref(), Some("2026-11-11_030000Z"));
    assert_eq!(listed[0].by.as_deref(), Some("alice"));
    let transfers = runs.transfers(first, 1, 10).await.unwrap();
    assert_eq!(transfers, [Transfer { change: "*deleting".into(), path: "old.txt".into() }]);
    assert_eq!(runs.list(None, 1).await.unwrap().len(), 1);

    assert_eq!(runs.interrupt_running(Timestamp::from_unix_seconds(30)).await.unwrap(), 1);
    assert_eq!(runs.find(second).await.unwrap().unwrap().outcome, Outcome::Interrupted);
    assert_eq!(runs.forget_before(Timestamp::from_unix_seconds(18)).await.unwrap(), 1);
    assert!(runs.find(first).await.unwrap().is_none());
    assert!(runs.transfers(first, 0, 10).await.unwrap().is_empty(), "with its transfer list");
    tasks.delete(task).await.unwrap();
    assert!(runs.find(second).await.unwrap().is_none(), "with its task");
}

#[tokio::test]
async fn settings_are_the_defaults_until_saved() {
    let (_dir, db) = temporary_database().await;
    let settings = SqliteBackupSettings::new(db);
    assert_eq!(settings.get().await.unwrap(), Settings::default());
    settings.set(Settings { log_days: 30, bwlimit_kib: 1024 }).await.unwrap();
    settings.set(Settings { log_days: 31, bwlimit_kib: 0 }).await.unwrap();
    assert_eq!(settings.get().await.unwrap(), Settings { log_days: 31, bwlimit_kib: 0 });
}
