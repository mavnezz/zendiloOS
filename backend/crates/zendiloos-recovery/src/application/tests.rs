use std::sync::atomic::Ordering;

use zendiloos_jobs::{Job, JobState};
use zendiloos_kernel::{Actor, Outcome, RequestContext};

use super::*;
use crate::domain::{Frequency, Schedule, Target};
use crate::testing::{Fakes, NOW, recovery};

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

fn share() -> Target {
    Target::Share { share_id: 3, folder: "zendiloOS".into() }
}

fn change(enabled: bool, passphrase: Option<&str>) -> PlanChange {
    PlanChange {
        enabled,
        schedule: Schedule { frequency: Frequency::Daily, hour: 3, minute: 0 },
        target: Some(share()),
        keep: 2,
        passphrase: passphrase.map(str::to_owned),
    }
}

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

#[tokio::test]
async fn a_download_is_encrypted_with_the_passphrase_and_audited() {
    let (recovery, fakes) = recovery();

    let archive = recovery.download("correct horse battery", &alice(), &context()).await.unwrap();

    assert_eq!(archive.bytes, b"backup:correct horse battery");
    assert!(archive.name.starts_with("zendiloos-config-nas-"));
    assert_eq!(fakes.audit.actions(), [("configuration.download", Outcome::Success)]);
    assert!(matches!(
        recovery.download("short", &alice(), &context()).await,
        Err(RecoveryError::Passphrase(_))
    ));
    *fakes.snapshots.fail.lock().unwrap() = Some("disk full".into());
    assert!(matches!(
        recovery.download("correct horse battery", &alice(), &context()).await,
        Err(RecoveryError::Port(_))
    ));
    assert_eq!(fakes.audit.actions()[1], ("configuration.download", Outcome::Failure));
}

#[tokio::test]
async fn a_plan_keeps_its_passphrase_and_never_shows_it() {
    let (recovery, fakes) = recovery();
    assert!(matches!(
        recovery.set_plan(change(true, None), &alice(), &context()).await,
        Err(RecoveryError::Plan(InvalidPlan::NoPassphrase))
    ));

    let plan =
        recovery.set_plan(change(true, Some("correct horse battery")), &alice(), &context()).await.unwrap();

    assert!(plan.enabled && plan.passphrase_set);
    assert_eq!(plan.account.as_deref(), Some("alice"));
    assert_eq!(
        plan.next_at,
        Some(zendiloos_kernel::Timestamp::from_unix_seconds(1_800_068_400)),
        "03:00 tomorrow"
    );
    // Changed without one: kept.
    let plan =
        recovery.set_plan(PlanChange { keep: 5, ..change(true, None) }, &alice(), &context()).await.unwrap();
    assert!(plan.passphrase_set && plan.keep == 5);
    assert_eq!(fakes.plans.0.lock().unwrap().passphrase.as_ref().unwrap().expose(), "correct horse battery");
    assert!(matches!(
        recovery.set_plan(change(true, Some("short")), &alice(), &context()).await,
        Err(RecoveryError::Passphrase(_))
    ));
    assert_eq!(
        fakes.audit.actions().iter().filter(|(a, _)| *a == "configuration.backup_settings").count(),
        2
    );
}

#[tokio::test]
async fn a_scheduled_backup_goes_into_its_folder_once_and_the_oldest_beyond_those_kept_go() {
    let (recovery, fakes) = recovery();
    recovery.set_plan(change(true, Some("correct horse battery")), &alice(), &context()).await.unwrap();
    assert!(recovery.run_due().await.is_none(), "not before its time");

    for day in 1..=3 {
        fakes.clock.0.store(NOW + day * 86_400, Ordering::SeqCst);
        let job = recovery.run_due().await.expect("due");
        assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);
        assert!(recovery.run_due().await.is_none(), "once");
    }

    let names = fakes.folders.files.lock().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(names.len(), 2, "two kept: {names:?}");
    assert!(names.iter().all(|name| name.starts_with("share3:zendiloOS/zendiloos-config-nas-")));
    assert_eq!(fakes.folders.writers.lock().unwrap()[..], ["alice", "alice", "alice"]);
    let plan = recovery.plan().await.unwrap();
    let last = plan.last.unwrap();
    assert!(last.succeeded && last.file.is_some() && last.error.is_none());
}

#[tokio::test]
async fn a_backup_that_fails_is_told_and_noted() {
    let (recovery, fakes) = recovery();
    recovery.set_plan(change(true, Some("correct horse battery")), &alice(), &context()).await.unwrap();
    *fakes.folders.fail.lock().unwrap() = true;

    let job = recovery.run_now(&alice(), &context()).await.unwrap();

    let job = finished(&fakes, &job).await;
    assert_eq!(job.state, JobState::Failed);
    let codes: Vec<String> = fakes.notifications.all().into_iter().map(|n| n.code).collect();
    assert_eq!(codes, ["configuration_backup_failed"]);
    let last = recovery.plan().await.unwrap().last.unwrap();
    assert!(!last.succeeded && last.error.unwrap().contains("No space left"));
}

#[tokio::test]
async fn a_plan_whose_administrator_is_one_no_longer_fails() {
    let (recovery, fakes) = recovery();
    recovery.set_plan(change(true, Some("correct horse battery")), &alice(), &context()).await.unwrap();
    fakes.operators.0.lock().unwrap().insert(1, ("alice".into(), false));

    let job = recovery.run_now(&alice(), &context()).await.unwrap();

    assert_eq!(finished(&fakes, &job).await.state, JobState::Failed);
    assert!(fakes.folders.files.lock().unwrap().is_empty());
}

#[tokio::test]
async fn nothing_runs_without_a_plan() {
    let (recovery, _) = recovery();
    assert!(matches!(recovery.run_now(&alice(), &context()).await, Err(RecoveryError::NotPlanned)));
    assert!(recovery.run_due().await.is_none());
}

#[tokio::test]
async fn a_restore_checks_the_backup_then_restarts_with_it() {
    let (recovery, fakes) = recovery();
    let (admin, request) = (alice(), context());
    let restore = |archive: &'static str, passphrase: &'static str| {
        recovery.restore(archive.as_bytes().to_vec(), passphrase, Some(&admin), &request)
    };

    assert!(matches!(
        restore("backup:other passphrase", "correct horse battery").await,
        Err(RecoveryError::WrongPassphrase)
    ));
    assert!(matches!(
        restore("too-new:correct horse battery", "correct horse battery").await,
        Err(RecoveryError::TooNew { found: 56, supported: 55 })
    ));
    assert!(matches!(
        restore("format:correct horse battery", "correct horse battery").await,
        Err(RecoveryError::UnsupportedFormat(2))
    ));
    assert!(matches!(restore("garbage", "correct horse battery").await, Err(RecoveryError::NotABackup(_))));
    assert_eq!(fakes.restart.0.load(Ordering::SeqCst), 0);

    let restored = restore("backup:correct horse battery", "correct horse battery").await.unwrap();

    assert_eq!(restored.manifest.host_name, "nas");
    assert_eq!(fakes.restart.0.load(Ordering::SeqCst), 1);
    let entries = fakes.restorer.audit.lock().unwrap();
    assert_eq!((entries[0].action, entries[0].actor.as_deref()), ("configuration.restore", Some("alice")));
    let actions = fakes.audit.actions();
    assert_eq!(actions.iter().filter(|a| **a == ("configuration.restore", Outcome::Failure)).count(), 4);
    assert_eq!(actions.last(), Some(&("configuration.restore", Outcome::Success)));
}

#[tokio::test]
async fn no_restore_while_jobs_run() {
    let (recovery, fakes) = recovery();
    fakes.work.0.lock().unwrap().push("storage.create_pool".into());

    let refused = recovery
        .restore(b"backup:correct horse battery".to_vec(), "correct horse battery", None, &context())
        .await;

    assert!(matches!(refused, Err(RecoveryError::JobsRunning(named)) if named == ["storage.create_pool"]));
    assert!(fakes.restorer.restored.lock().unwrap().is_empty());
}
