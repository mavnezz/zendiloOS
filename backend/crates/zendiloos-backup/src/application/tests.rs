use std::sync::Arc;

use zendiloos_jobs::{Job, JobState};
use zendiloos_kernel::{Actor, RequestContext, Secret, Timestamp, WorkHold};

use super::*;
use crate::domain::{
    Existing, Filters, Frequency, Mode, NasPlace, Outcome, Place, RemotePlace, Schedule, Server, Settings,
    TaskSettings, Transport, Trigger, version_name,
};
use crate::testing::{DISK, Fakes, HostCall, NOW, STICK, backups, folder};

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

fn share(id: i64, path: &str) -> Place {
    Place::Nas(NasPlace::Share { share_id: id, path: path.into() })
}

fn local(name: &str) -> TaskSettings {
    TaskSettings {
        name: name.into(),
        source: share(1, "albums"),
        target: share(2, ""),
        mode: Mode::Versioned { keep_last: Some(2), keep_days: None },
        schedule: None,
        on_plug: false,
        filters: Filters::default(),
        bwlimit_kib: None,
        notify_success: false,
    }
}

fn daemon(host: &str, user: &str) -> Server {
    Server {
        transport: Transport::Daemon { module: "nas".into() },
        host: host.into(),
        port: 873,
        user: user.into(),
    }
}

fn push(name: &str, server: Server) -> TaskSettings {
    TaskSettings { target: Place::Remote(RemotePlace { server, path: "photos".into() }), ..local(name) }
}

/// The job once its work ended.
async fn finished(fakes: &Fakes, job: &Job) -> Job {
    for _ in 0..1000 {
        if let Some(found) = fakes.jobs.all().into_iter().find(|j| j.id == job.id)
            && found.state != JobState::Running
        {
            return found;
        }
        tokio::task::yield_now().await;
    }
    panic!("the job did not end");
}

fn codes(fakes: &Fakes) -> Vec<String> {
    fakes.notifications.all().into_iter().map(|n| n.code).collect()
}

#[tokio::test]
async fn the_plugin_installs_once() {
    let (backups, fakes) = backups();
    assert!(matches!(backups.install(&alice(), &context()).await, Err(BackupError::Installed)));
    *fakes.host.installed.lock().unwrap() = false;
    assert!(matches!(
        backups.create(local("a"), None, &alice(), &context()).await,
        Err(BackupError::NotInstalled)
    ));
    let job = backups.install(&alice(), &context()).await.unwrap();
    assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);
    assert!(backups.plugin(&context()).await.unwrap().facts.rsync);
    assert!(fakes.audit.actions().contains(&("backup.install", zendiloos_kernel::Outcome::Success)));
}

#[tokio::test]
async fn the_key_pair_is_made_once_when_asked_for() {
    let (backups, fakes) = backups();
    let key = backups.public_key(&alice(), &context()).await.unwrap();
    assert!(key.starts_with("ssh-ed25519 "));
    assert_eq!(backups.public_key(&alice(), &context()).await.unwrap(), key);
    assert_eq!(fakes.host.calls().iter().filter(|c| **c == HostCall::MakeKey).count(), 1);
    let keys = backups.host_keys("nas2.lan", 22, &context()).await.unwrap();
    assert!(keys[0].fingerprint.starts_with("SHA256:"));
    assert!(backups.host_keys("-oProxyCommand=x", 22, &context()).await.is_err());
    assert!(matches!(backups.host_keys("silent.lan", 22, &context()).await, Err(BackupError::Refused(_))));
}

#[tokio::test]
async fn a_task_is_made_with_its_folder_and_names_are_unique() {
    let (backups, _) = backups();
    let made = backups.create(local(" Albums nightly "), None, &alice(), &context()).await.unwrap();
    assert_eq!((made.task.name(), made.task.folder.as_str()), ("Albums nightly", "Albums nightly"));
    assert!(made.reachable && !made.running && made.last_run.is_none() && made.next_at.is_none());
    assert!(matches!(
        backups.create(local("albums NIGHTLY"), None, &alice(), &context()).await,
        Err(BackupError::Invalid(_))
    ));
    // Another name, but the same folder in the same target.
    let mut same_folder = local("Albums nightly/");
    same_folder.name = "Albums nightly.".into();
    assert!(matches!(
        backups.create(same_folder, None, &alice(), &context()).await,
        Err(BackupError::FolderTaken(_))
    ));
    let mut gone = local("gone");
    gone.target = share(9, "");
    assert!(matches!(backups.create(gone, None, &alice(), &context()).await, Err(BackupError::ShareMissing)));
    // Renamed, its folder stays.
    let renamed = backups.update(made.task.id, local("Albums"), None, &alice(), &context()).await.unwrap();
    assert_eq!((renamed.task.name(), renamed.task.folder.as_str()), ("Albums", "Albums nightly"));
}

#[tokio::test]
async fn a_daemons_password_stays_with_that_daemon() {
    let (backups, fakes) = backups();
    let made = backups
        .create(
            push("offsite", daemon("nas2.lan", "zendi")),
            Some(Secret::new("s3cret")),
            &alice(),
            &context(),
        )
        .await
        .unwrap();
    assert!(made.password_set);
    let id = made.task.id;
    // Changed otherwise, without a password typed: it stays.
    let mut renamed = push("offsite 2", daemon("NAS2.lan", "zendi"));
    renamed.bwlimit_kib = Some(1000);
    assert!(backups.update(id, renamed, None, &alice(), &context()).await.unwrap().password_set);
    // The server is checked with it, for the same daemon only.
    let place = RemotePlace { server: daemon("nas2.lan", "zendi"), path: "photos".into() };
    backups.check_server(&place, None, Some(id), &context()).await.unwrap();
    let other = RemotePlace { server: daemon("evil.lan", "zendi"), path: "photos".into() };
    backups.check_server(&other, None, Some(id), &context()).await.unwrap();
    assert_eq!(*fakes.host.passwords.lock().unwrap(), [Some("s3cret".to_owned()), None]);
    // Another server: it goes.
    let moved =
        backups.update(id, push("offsite 2", daemon("nas3.lan", "zendi")), None, &alice(), &context()).await;
    assert!(!moved.unwrap().password_set);
    let down = RemotePlace { server: daemon("down.lan", "zendi"), path: String::new() };
    assert!(matches!(
        backups.check_server(&down, None, None, &context()).await,
        Err(BackupError::HostFailed(_))
    ));
}

#[tokio::test]
async fn a_versioned_run_is_logged_and_retention_keeps_its_number() {
    let (backups, fakes) = backups();
    let id = backups.create(local("albums"), None, &alice(), &context()).await.unwrap().task.id;
    let folder_at = Location::Share { volume: 2, share: "backups".into(), path: "albums".into() };
    // Two versions from before, and one cut off.
    let day = 24 * 3600;
    fakes.host.put(
        &folder_at,
        vec![
            folder(&version_name(Timestamp::from_unix_seconds(NOW - 2 * day))),
            folder(&version_name(Timestamp::from_unix_seconds(NOW - day))),
            folder(&format!("{}.incomplete", version_name(Timestamp::from_unix_seconds(NOW - 3600)))),
        ],
    );
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);

    let plan = fakes.host.runs().pop().unwrap();
    assert_eq!(plan.target, folder_at);
    assert_eq!(plan.source, Location::Share { volume: 1, share: "photos".into(), path: "albums".into() });
    assert_eq!((plan.mode, plan.stamp.as_str()), (RunMode::Versioned, "2026-11-11_070000Z"));
    // Keeps two: the oldest goes, and the version cut off.
    let pruned = fakes.host.calls().into_iter().find_map(|call| match call {
        HostCall::Prune(at, names) => Some((at, names)),
        _ => None,
    });
    assert_eq!(
        pruned,
        Some((
            folder_at,
            vec![
                version_name(Timestamp::from_unix_seconds(NOW - 2 * day)),
                format!("{}.incomplete", version_name(Timestamp::from_unix_seconds(NOW - 3600)))
            ]
        ))
    );
    let run = &backups.runs(Some(id), 10).await.unwrap()[0];
    assert_eq!(
        (run.outcome, run.trigger, run.by.as_deref()),
        (Outcome::Succeeded, Trigger::Manual, Some("alice"))
    );
    assert_eq!((run.files, run.version.as_deref(), run.transfers), (3, Some("2026-11-11_070000Z"), 3));
    let (_, transfers) = backups.transfers(run.id, 0, 10).await.unwrap();
    assert_eq!(transfers[0].path, "a.jpg");
    assert!(codes(&fakes).is_empty(), "success is told only when asked for");
    let view = backups.task(id, &context()).await.unwrap();
    assert_eq!(view.last_run.map(|run| run.outcome), Some(Outcome::Succeeded));
    let versions = backups.versions(id, &context()).await.unwrap();
    assert_eq!(versions.versions.len(), 2);
    assert!(versions.versions[0] > versions.versions[1], "newest first");
}

#[tokio::test]
async fn one_run_per_task_and_a_stopped_run_is_not_a_failure() {
    let (backups, fakes) = backups();
    let id = backups.create(local("albums"), None, &alice(), &context()).await.unwrap().task.id;
    let gate = fakes.host.gate.lock().await;
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(backups.run_now(id, &alice(), &context()).await, Err(BackupError::Running)));
    assert!(matches!(backups.delete(id, &alice(), &context()).await, Err(BackupError::Running)));
    assert!(matches!(
        backups
            .restore(
                id,
                RestoreRequest { version: None, paths: vec![], to: None, existing: Existing::Skip },
                &alice(),
                &context()
            )
            .await,
        Err(BackupError::Running)
    ));
    assert!(backups.task(id, &context()).await.unwrap().running);
    backups.stop(id, &alice(), &context()).await.unwrap();
    drop(gate);
    let ended = finished(&fakes, &job).await;
    assert_eq!(ended.error.map(|e| e.code).as_deref(), Some("stopped"));
    assert_eq!(fakes.runs.all()[0].outcome, Outcome::Stopped);
    assert!(codes(&fakes).is_empty(), "a stop is not told");
    assert!(matches!(backups.stop(id, &alice(), &context()).await, Err(BackupError::NotRunning)));
}

#[tokio::test]
async fn failures_and_partial_runs_are_told_and_success_when_asked() {
    let (backups, fakes) = backups();
    let mut settings = local("albums");
    settings.notify_success = true;
    let id = backups.create(settings, None, &alice(), &context()).await.unwrap().task.id;
    *fakes.host.fail_next.lock().unwrap() =
        Some(HostError::HostKeyChanged("ED25519 key for nas2.lan".into()));
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&fakes, &job).await.error.unwrap().code, "host_key_changed");
    *fakes.host.partial_next.lock().unwrap() = Some("file has vanished: a.tmp".into());
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    finished(&fakes, &job).await;
    assert_eq!(codes(&fakes), ["backup_failed", "backup_partial", "backup_succeeded"]);
    let outcomes: Vec<Outcome> = fakes.runs.all().iter().map(|run| run.outcome).collect();
    assert_eq!(outcomes, [Outcome::Failed, Outcome::Partial, Outcome::Succeeded]);
    assert!(fakes.runs.all()[0].error.as_deref().unwrap().contains("host key changed"));
}

#[tokio::test]
async fn schedules_come_due_once_and_wait_for_an_update() {
    let (backups, fakes) = backups();
    let mut settings = local("albums");
    settings.schedule = Some(Schedule { frequency: Frequency::Daily, hour: 9, minute: 0 });
    let id = backups.create(settings, None, &alice(), &context()).await.unwrap().task.id;
    let view = backups.task(id, &context()).await.unwrap();
    assert_eq!(view.next_at, Some(Timestamp::from_unix_seconds(NOW + 2 * 3600)));
    assert!(backups.run_due().await.is_empty(), "not yet");

    fakes.clock.set(NOW + 2 * 3600 + 5 * 60);
    struct Updating;
    #[async_trait::async_trait]
    impl WorkHold for Updating {
        async fn holding(&self) -> Option<String> {
            Some("system.update".into())
        }
    }
    let held = backups.clone();
    let jobs = zendiloos_jobs::Jobs::new(fakes.jobs.clone(), fakes.clock.clone());
    jobs.hold_while(Arc::new(Updating));
    let (on_hold, _) = {
        let ports = BackupPorts {
            tasks: fakes.tasks.clone(),
            runs: fakes.runs.clone(),
            settings: fakes.settings.clone(),
            host: fakes.host.clone(),
            shares: fakes.shares.clone(),
            drives: fakes.drives.clone(),
            jobs,
            notifier: zendiloos_notify::Notifier::new(fakes.notifications.clone(), fakes.clock.clone()),
            audit: fakes.audit.clone(),
            clock: fakes.clock.clone(),
        };
        (Backups::new(ports), ())
    };
    assert!(on_hold.run_due().await.is_empty(), "nothing starts while an update installs");

    let started = held.run_due().await;
    assert_eq!(started.len(), 1, "due, and started");
    finished(&fakes, &started[0]).await;
    assert!(backups.run_due().await.is_empty(), "once");
    assert_eq!(fakes.runs.all()[0].trigger, Trigger::Schedule);
    fakes.clock.set(NOW + 26 * 3600 + 5 * 60);
    assert_eq!(backups.run_due().await.len(), 1, "the next day");
}

#[tokio::test]
async fn plugging_in_its_drive_starts_a_task() {
    let (backups, fakes) = backups();
    fakes.drives.plug(DISK, "ext4", "boot1:100");
    let mut settings = local("to disk");
    settings.target = Place::Nas(NasPlace::Drive { drive: DISK.into(), path: "nas".into() });
    settings.on_plug = true;
    let id = backups.create(settings, None, &alice(), &context()).await.unwrap().task.id;
    assert!(backups.run_due().await.is_empty(), "plugged in when it was made: not a new plugging in");

    fakes.drives.pull(DISK);
    assert!(backups.run_due().await.is_empty());
    assert!(!backups.task(id, &context()).await.unwrap().reachable);
    fakes.drives.plug(DISK, "ext4", "boot1:200");
    let started = backups.run_due().await;
    assert_eq!(started.len(), 1);
    finished(&fakes, &started[0]).await;
    assert!(backups.run_due().await.is_empty(), "once per plugging in");
    let plan = fakes.host.runs().pop().unwrap();
    assert_eq!(plan.target, Location::Drive { drive: DISK.into(), path: "nas/to disk".into() });
    assert_eq!(fakes.runs.all()[0].trigger, Trigger::Plug);
}

#[tokio::test]
async fn a_drive_without_links_takes_mirrors_only_and_a_missing_one_is_told() {
    let (backups, fakes) = backups();
    fakes.drives.plug(STICK, "vfat", "boot1:1");
    let mut versions = local("stick");
    versions.target = Place::Nas(NasPlace::Drive { drive: STICK.into(), path: String::new() });
    assert!(matches!(
        backups.create(versions.clone(), None, &alice(), &context()).await,
        Err(BackupError::Invalid(_))
    ));
    let mut mirror = versions;
    mirror.mode = Mode::Mirror { keep_changed_days: Some(7) };
    mirror.schedule = Some(Schedule { frequency: Frequency::Hourly, hour: 0, minute: 0 });
    let id = backups.create(mirror, None, &alice(), &context()).await.unwrap().task.id;
    fakes.drives.pull(STICK);
    assert!(matches!(backups.run_now(id, &alice(), &context()).await, Err(BackupError::DriveMissing)));
    // On schedule, without "when plugged in": a failed run, told.
    fakes.clock.set(NOW + 3600);
    assert!(backups.run_due().await.is_empty());
    assert_eq!(fakes.runs.all()[0].outcome, Outcome::Failed);
    assert_eq!(codes(&fakes), ["backup_failed"]);
}

#[tokio::test]
async fn a_mirror_prunes_kept_changes_by_age() {
    let (backups, fakes) = backups();
    let mut settings = local("mirror");
    settings.mode = Mode::Mirror { keep_changed_days: Some(7) };
    let id = backups.create(settings, None, &alice(), &context()).await.unwrap().task.id;
    let folder_at = Location::Share { volume: 2, share: "backups".into(), path: "mirror".into() };
    let day = 24 * 3600;
    fakes.host.put(
        &folder_at.join("removed"),
        vec![
            folder(&version_name(Timestamp::from_unix_seconds(NOW - 10 * day))),
            folder(&version_name(Timestamp::from_unix_seconds(NOW - day))),
        ],
    );
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    finished(&fakes, &job).await;
    assert_eq!(fakes.host.runs()[0].mode, RunMode::Mirror { keep_changed: true });
    assert!(fakes.host.calls().contains(&HostCall::Prune(
        folder_at.clone(),
        vec![format!("removed/{}", version_name(Timestamp::from_unix_seconds(NOW - 10 * day)))]
    )));
    let versions = backups.versions(id, &context()).await.unwrap();
    assert!(versions.current && versions.versions.is_empty());
}

#[tokio::test]
async fn restores_go_back_or_elsewhere() {
    let (backups, fakes) = backups();
    let id = backups.create(local("albums"), None, &alice(), &context()).await.unwrap().task.id;
    let version = Timestamp::from_unix_seconds(NOW - 3600);
    let request = RestoreRequest {
        version: Some(version),
        paths: vec!["2024/summer".into()],
        to: None,
        existing: Existing::Skip,
    };
    let job = backups.restore(id, request, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);
    let restored = fakes.host.calls().into_iter().find_map(|call| match call {
        HostCall::Restore(plan) => Some(plan),
        _ => None,
    });
    let plan = restored.unwrap();
    assert_eq!(
        plan.from,
        Location::Share {
            volume: 2,
            share: "backups".into(),
            path: format!("albums/{}", version_name(version))
        }
    );
    assert_eq!(plan.to, Location::Share { volume: 1, share: "photos".into(), path: "albums".into() });
    assert_eq!(plan.paths, ["2024/summer"]);

    let elsewhere = RestoreRequest {
        version: None,
        paths: vec![],
        to: Some(NasPlace::Share { share_id: 2, path: "restored".into() }),
        existing: Existing::Replace,
    };
    let job = backups.restore(id, elsewhere, &alice(), &context()).await.unwrap();
    finished(&fakes, &job).await;
    for bad in [vec!["../etc".to_owned()], vec![String::new()]] {
        let request = RestoreRequest { version: None, paths: bad, to: None, existing: Existing::Skip };
        assert!(matches!(
            backups.restore(id, request, &alice(), &context()).await,
            Err(BackupError::Invalid(_))
        ));
    }
    // A server's backup goes into a folder named.
    let mut pull = local("server");
    pull.source = Place::Remote(RemotePlace { server: daemon("www.lan", "backup"), path: "/srv".into() });
    let pulled = backups.create(pull, None, &alice(), &context()).await.unwrap().task.id;
    let request = RestoreRequest { version: None, paths: vec![], to: None, existing: Existing::Skip };
    assert!(matches!(
        backups.restore(pulled, request, &alice(), &context()).await,
        Err(BackupError::Invalid(_))
    ));
    assert!(fakes.audit.actions().contains(&("backup.restore", zendiloos_kernel::Outcome::Success)));
}

#[tokio::test]
async fn browsing_a_version_lists_its_folder() {
    let (backups, fakes) = backups();
    let id = backups.create(local("albums"), None, &alice(), &context()).await.unwrap().task.id;
    let at = Location::Share { volume: 2, share: "backups".into(), path: "albums/current/2024".into() };
    fakes.host.put(&at, vec![folder("."), folder("summer")]);
    let entries = backups.browse(id, None, "2024", &context()).await.unwrap();
    assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["summer"]);
    assert!(backups.browse(id, None, "../x", &context()).await.is_err());
}

#[tokio::test]
async fn the_log_is_kept_for_its_days_and_runs_cut_off_are_marked() {
    let (backups, fakes) = backups();
    let id = backups.create(local("albums"), None, &alice(), &context()).await.unwrap().task.id;
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    finished(&fakes, &job).await;
    fakes
        .runs
        .start(&NewRun {
            task_id: id,
            trigger: Trigger::Manual,
            by: None,
            started_at: Timestamp::from_unix_seconds(NOW),
            job_id: None,
        })
        .await
        .unwrap();
    assert_eq!(backups.recover().await, 1);
    assert_eq!(fakes.runs.all()[1].outcome, Outcome::Interrupted);
    assert!(
        backups.set_settings(Settings { log_days: 0, bwlimit_kib: 0 }, &alice(), &context()).await.is_err()
    );
    backups.set_settings(Settings { log_days: 1, bwlimit_kib: 500 }, &alice(), &context()).await.unwrap();
    fakes.clock.set(NOW + 2 * 24 * 3600);
    assert_eq!(backups.purge_log().await, 2);
    // The default bandwidth limit reaches runs that follow it.
    let job = backups.run_now(id, &alice(), &context()).await.unwrap();
    finished(&fakes, &job).await;
    assert_eq!(fakes.host.runs().last().unwrap().bwlimit_kib, 500);
    backups.delete(id, &alice(), &context()).await.unwrap();
    assert!(matches!(backups.task(id, &context()).await, Err(BackupError::NotFound)));
}

#[tokio::test]
async fn readers_of_a_target_share_are_named() {
    let (backups, _) = backups();
    assert_eq!(backups.readers(2).await.unwrap().accounts, ["alice"]);
    assert!(matches!(backups.readers(9).await, Err(BackupError::NotFound)));
}
