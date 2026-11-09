//! Volumes and their backups on the in-memory adapters, with a clock that
//! moves: an hour ahead of UTC, as in Berlin in winter.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobState};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use super::{ContainersError, PlanInput, RestoreTarget};
use crate::domain::{BackupFrequency, BackupOutcome, BackupSchedule, BackupTarget};
use crate::testing::TestContainers;

/// 2026-11-05 00:00:00 UTC: one at night in Berlin.
const MIDNIGHT: i64 = 1_793_836_800;
const HOUR: i64 = 3600;

struct Berlin(Mutex<Timestamp>);

impl Berlin {
    fn set(&self, seconds: i64) {
        *self.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + seconds);
    }
}

impl Clock for Berlin {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap()
    }

    fn utc_offset(&self, _: Timestamp) -> i32 {
        3600
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<AuditEntry>>);

#[async_trait]
impl AuditLog for Audit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry);
        Ok(())
    }
}

impl Audit {
    fn actions(&self) -> Vec<(&'static str, Option<String>, Outcome)> {
        self.0.lock().unwrap().iter().map(|e| (e.action, e.target.clone(), e.outcome)).collect()
    }
}

fn setup() -> (TestContainers, Arc<Audit>, Arc<Berlin>) {
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(Berlin(Mutex::new(Timestamp::from_unix_seconds(MIDNIGHT))));
    (TestContainers::new(audit.clone(), clock.clone()), audit, clock)
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

fn nightly(keep: u32, stop_containers: bool) -> PlanInput {
    PlanInput {
        target: BackupTarget { share_id: 1, folder: "/backups/".into() },
        schedule: BackupSchedule { frequency: BackupFrequency::Daily, hour: 3 },
        keep,
        stop_containers,
    }
}

/// The job `id` once it ended.
async fn finished(t: &TestContainers, id: zendiloos_jobs::JobId) -> Job {
    for _ in 0..400 {
        if let Some(job) = t.jobs.all().into_iter().find(|job| job.id == id && job.finished_at.is_some()) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("job {id:?} did not end");
}

/// The engine's calls about containers.
fn container_calls(t: &TestContainers) -> Vec<String> {
    t.engine.calls().into_iter().filter(|c| c.starts_with("stop ") || c.starts_with("start ")).collect()
}

#[tokio::test]
async fn volumes_are_listed_with_who_uses_them_and_how_they_are_backed_up() {
    let (t, _, _) = setup();
    t.containers.set_backup_plan("kuma_data", nightly(7, true), &alice(), &context()).await.unwrap();

    let volumes = t.containers.volumes(&context()).await.unwrap();

    let names: Vec<&str> = volumes.iter().map(|v| v.volume.name.as_str()).collect();
    assert_eq!(names, ["kuma_data", "nfs_media", "orphan", "stack_db"]);
    let kuma = &volumes[0];
    assert_eq!(kuma.volume.project(), Some("kuma"));
    assert_eq!((kuma.users[0].name.as_str(), kuma.users[0].running), ("kuma-1", true));
    let plan = kuma.plan.as_ref().unwrap();
    // Made at one in Berlin: next at three, two UTC.
    assert_eq!(plan.next_at, Timestamp::from_unix_seconds(MIDNIGHT + 2 * HOUR));
    assert_eq!(plan.plan.target.folder, "backups");
    assert!(kuma.last_backup.is_none());
    assert_eq!((volumes[3].users[0].name.as_str(), volumes[3].users[0].running), ("stack-db", false));
    assert!(volumes[2].users.is_empty() && volumes[2].plan.is_none());
    let sizes = t.containers.volume_sizes(&context()).await.unwrap();
    assert_eq!(sizes.len(), 3, "not the one mounted from elsewhere");
}

#[tokio::test]
async fn only_unused_volumes_are_removed_and_their_plans_with_them() {
    let (t, audit, _) = setup();
    t.containers.set_backup_plan("orphan", nightly(7, false), &alice(), &context()).await.unwrap();
    for used in ["kuma_data", "stack_db"] {
        let refused = t.containers.remove_volume(used, &alice(), &context()).await;
        assert!(
            matches!(&refused, Err(ContainersError::Refused(why)) if why.contains("used by")),
            "{refused:?}"
        );
    }
    assert!(matches!(
        t.containers.remove_volume("ghost", &alice(), &context()).await,
        Err(ContainersError::NotFound)
    ));

    t.containers.remove_volume("orphan", &alice(), &context()).await.unwrap();
    assert!(t.engine.calls().contains(&"volume rm orphan".to_owned()));
    assert!(t.containers.backup_plan("orphan").await.unwrap().is_none());
    assert!(t.containers.prune_volumes(&alice(), &context()).await.unwrap().contains("reclaimed"));
    let actions = audit.actions();
    assert!(actions.contains(&("containers.volume_remove", Some("orphan".into()), Outcome::Success)));
    assert!(actions.contains(&("containers.volume_prune", Some("unused volumes".into()), Outcome::Success)));
}

#[tokio::test]
async fn plans_are_checked_before_they_are_kept() {
    let (t, audit, _) = setup();
    let refused = t.containers.set_backup_plan("nfs_media", nightly(7, true), &alice(), &context()).await;
    assert!(matches!(refused, Err(ContainersError::Refused(_))), "{refused:?}");
    let ghost = t.containers.set_backup_plan("ghost", nightly(7, true), &alice(), &context()).await;
    assert!(matches!(ghost, Err(ContainersError::NotFound)));
    let bob = Actor { account_id: 2, name: "bob".into() };
    let denied = t.containers.set_backup_plan("orphan", nightly(7, true), &bob, &context()).await;
    assert!(matches!(denied, Err(ContainersError::Denied)), "a share bob may not write: {denied:?}");
    let invalid = t.containers.set_backup_plan("orphan", nightly(0, true), &alice(), &context()).await;
    assert!(matches!(invalid, Err(ContainersError::Invalid(_))), "{invalid:?}");
    let outside =
        PlanInput { target: BackupTarget { share_id: 1, folder: "a/../b".into() }, ..nightly(7, true) };
    assert!(matches!(
        t.containers.set_backup_plan("orphan", outside, &alice(), &context()).await,
        Err(ContainersError::Invalid(_))
    ));
    assert!(t.backups.plans.lock().unwrap().is_empty());

    let weekly = PlanInput {
        schedule: BackupSchedule { frequency: BackupFrequency::Weekly { weekday: 6 }, hour: 4 },
        ..nightly(3, false)
    };
    let saved = t.containers.set_backup_plan("orphan", weekly, &alice(), &context()).await.unwrap();
    assert_eq!(saved.plan.keep, 3);
    assert_eq!(t.containers.backup_plan("orphan").await.unwrap(), Some(saved));
    t.containers.delete_backup_plan("orphan", &alice(), &context()).await.unwrap();
    let again = t.containers.delete_backup_plan("orphan", &alice(), &context()).await;
    assert!(matches!(again, Err(ContainersError::NotFound)));
    let actions = audit.actions();
    assert!(actions.contains(&("containers.backup_plan", Some("orphan".into()), Outcome::Success)));
    assert!(actions.contains(&("containers.backup_plan_delete", Some("orphan".into()), Outcome::Success)));
    let none = t.containers.back_up_now("orphan", &alice(), &context()).await;
    assert!(matches!(none, Err(ContainersError::NotFound)), "no plan, no backup");
}

#[tokio::test]
async fn a_backup_stops_the_containers_that_use_the_volume_and_starts_them_again() {
    let (t, audit, _) = setup();
    t.containers.set_backup_plan("kuma_data", nightly(7, true), &alice(), &context()).await.unwrap();

    let job = t.containers.back_up_now("kuma_data", &alice(), &context()).await.unwrap();
    assert_eq!((job.kind.as_str(), job.target.as_deref()), ("containers.volume_backup", Some("kuma_data")));
    let busy = t.containers.back_up_now("kuma_data", &alice(), &context()).await;
    assert!(matches!(busy, Err(ContainersError::VolumeBusy(_))), "one at a time: {busy:?}");
    let job = finished(&t, job.id).await;

    assert_eq!(job.state, JobState::Succeeded, "{job:?}");
    assert_eq!(container_calls(&t), ["stop kuma-1-id", "start kuma-1-id"]);
    assert_eq!(t.archiver.calls(), ["back up kuma_data to docker/backups as kuma_data-20261105-010000"]);
    let runs = t.containers.backups("kuma_data").await.unwrap();
    assert_eq!(runs.len(), 1);
    let written = runs[0].written().unwrap();
    assert_eq!(written.archive.path, "backups/kuma_data/kuma_data-20261105-010000.tar.zst");
    assert_eq!(runs[0].by.as_deref(), Some("alice"));
    assert!(t.backups.owed.lock().unwrap().is_empty(), "started again: owed nothing");
    assert_eq!(runs[0].labels["com.docker.compose.project"], "kuma");
    assert!(audit.actions().contains(&(
        "containers.volume_backup",
        Some("kuma_data".into()),
        Outcome::Success
    )));
    let listed = t.containers.volumes(&context()).await.unwrap();
    assert_eq!(listed[0].last_backup.as_ref().map(|b| b.id), Some(runs[0].id));

    // A volume nothing uses, a plan that leaves them running.
    t.containers.set_backup_plan("stack_db", nightly(7, false), &alice(), &context()).await.unwrap();
    let job = t.containers.back_up_now("stack_db", &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.state, JobState::Succeeded);
    assert_eq!(container_calls(&t).len(), 2, "nothing stopped");
}

#[tokio::test]
async fn a_failed_backup_is_told_and_its_containers_run_again() {
    let (t, audit, _) = setup();
    t.containers.set_backup_plan("kuma_data", nightly(7, true), &alice(), &context()).await.unwrap();
    *t.archiver.fail.lock().unwrap() = Some("No space left on device".into());

    let job = t.containers.back_up_now("kuma_data", &alice(), &context()).await.unwrap();
    let job = finished(&t, job.id).await;

    assert_eq!(job.state, JobState::Failed);
    assert_eq!(container_calls(&t), ["stop kuma-1-id", "start kuma-1-id"], "started again");
    let runs = t.containers.backups("kuma_data").await.unwrap();
    assert!(matches!(&runs[0].outcome, BackupOutcome::Failed(why) if why.contains("No space")), "{runs:?}");
    let told = t.notifications.all();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].code, "docker_backup_failed");
    let params: Vec<(String, String)> = told[0].params.clone().into_iter().collect();
    assert!(params.contains(&("volume".into(), "kuma_data".into())), "{params:?}");
    assert!(audit.actions().contains(&(
        "containers.volume_backup",
        Some("kuma_data".into()),
        Outcome::Failure
    )));

    // A container that will not start again fails the run too; its
    // archive is kept.
    t.engine.stubborn.lock().unwrap().push("kuma-1-id".into());
    let job = t.containers.back_up_now("kuma_data", &alice(), &context()).await.unwrap();
    let job = finished(&t, job.id).await;
    assert_eq!(job.state, JobState::Failed);
    assert!(job.error.unwrap().detail.unwrap().contains("kuma-1"));
    let runs = t.containers.backups("kuma_data").await.unwrap();
    assert!(runs[0].written().is_some(), "{runs:?}");
    assert_eq!(t.notifications.all().len(), 2);
    // Owed a start, tried again every minute: once it starts, owed nothing.
    let owed: Vec<String> = t.backups.owed.lock().unwrap().iter().map(|o| o.container.clone()).collect();
    assert_eq!(owed, ["kuma-1"]);
    assert_eq!(t.containers.start_owed_containers().await, 0, "still stubborn");
    t.engine.stubborn.lock().unwrap().clear();
    assert_eq!(t.containers.start_owed_containers().await, 1);
    assert!(t.backups.owed.lock().unwrap().is_empty());
    assert!(t.archiver.calls().contains(&"end work on kuma_data".to_owned()), "asked first");
}

#[tokio::test]
async fn retention_removes_only_the_recorded_archives_beyond_the_plans_number() {
    let (t, audit, clock) = setup();
    t.containers.set_backup_plan("orphan", nightly(2, true), &alice(), &context()).await.unwrap();
    for hour in 1..=3 {
        clock.set(hour * HOUR);
        let job = t.containers.back_up_now("orphan", &alice(), &context()).await.unwrap();
        assert_eq!(finished(&t, job.id).await.state, JobState::Succeeded);
    }

    let removed: Vec<String> = t.archiver.calls().into_iter().filter(|c| c.starts_with("remove")).collect();
    assert_eq!(
        removed,
        [
            "remove docker/backups/orphan/orphan-20261105-020000.tar.zst",
            "remove docker/backups/orphan/orphan-20261105-020000.json"
        ]
    );
    let kept: Vec<String> = t
        .containers
        .backups("orphan")
        .await
        .unwrap()
        .iter()
        .map(|b| b.written().unwrap().archive.path.clone())
        .collect();
    assert_eq!(
        kept,
        ["backups/orphan/orphan-20261105-040000.tar.zst", "backups/orphan/orphan-20261105-030000.tar.zst"]
    );
    assert!(audit.actions().iter().any(|(action, target, _)| *action == "containers.volume_backup_remove"
        && target.as_deref() == Some("backups/orphan/orphan-20261105-020000.tar.zst")));
}

#[tokio::test]
async fn plans_run_at_their_local_hour_and_catch_up_once() {
    let (t, _, clock) = setup();
    // Made at one in Berlin, for three.
    t.containers.set_backup_plan("orphan", nightly(7, false), &alice(), &context()).await.unwrap();
    clock.set(HOUR + 59 * 60);
    assert!(t.containers.run_due_backups().await.is_empty(), "at two to three in Berlin");
    clock.set(2 * HOUR);
    let jobs = t.containers.run_due_backups().await;
    assert_eq!(jobs.len(), 1);
    finished(&t, jobs[0].id).await;
    assert!(t.containers.run_due_backups().await.is_empty(), "once");
    // Off the next night until noon: once at noon, then not again that day.
    clock.set(DAY + 11 * HOUR);
    let caught_up = t.containers.run_due_backups().await;
    assert_eq!(caught_up.len(), 1);
    finished(&t, caught_up[0].id).await;
    clock.set(DAY + 20 * HOUR);
    assert!(t.containers.run_due_backups().await.is_empty());
    let runs = t.containers.backups("orphan").await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].by, None, "the schedule's");

    // A plan whose volume is gone fails when it is due, and is told.
    t.containers.set_backup_plan("stack_db", nightly(7, false), &alice(), &context()).await.unwrap();
    t.engine.volumes.lock().unwrap().retain(|v| v.name != "stack_db");
    clock.set(2 * DAY + 2 * HOUR);
    for job in t.containers.run_due_backups().await {
        finished(&t, job.id).await;
    }
    let told: Vec<String> = t.notifications.all().iter().map(|n| n.params["volume"].clone()).collect();
    assert_eq!(told, ["stack_db"]);
}

const DAY: i64 = 24 * HOUR;

async fn backed_up(t: &TestContainers, volume: &str) -> i64 {
    t.containers.set_backup_plan(volume, nightly(7, false), &alice(), &context()).await.unwrap();
    let job = t.containers.back_up_now(volume, &alice(), &context()).await.unwrap();
    assert_eq!(finished(t, job.id).await.state, JobState::Succeeded);
    t.containers.backups(volume).await.unwrap()[0].id
}

#[tokio::test]
async fn a_backup_is_restored_under_its_own_name_with_its_users_stopped_meanwhile() {
    let (t, audit, _) = setup();
    let id = backed_up(&t, "kuma_data").await;

    let in_use = t.containers.restore_backup(id, RestoreTarget::OwnName, false, &alice(), &context()).await;
    assert!(
        matches!(&in_use, Err(ContainersError::VolumeInUse(names)) if names == &["kuma-1"]),
        "{in_use:?}"
    );
    let job =
        t.containers.restore_backup(id, RestoreTarget::OwnName, true, &alice(), &context()).await.unwrap();
    assert_eq!(job.kind, "containers.volume_restore");
    assert_eq!(finished(&t, job.id).await.state, JobState::Succeeded);
    assert_eq!(container_calls(&t), ["stop kuma-1-id", "start kuma-1-id"]);
    assert_eq!(
        t.archiver.calls().last().map(String::as_str),
        Some("restore kuma_data from docker/backups/kuma_data/kuma_data-20261105-010000.tar.zst abab")
    );
    assert!(audit.actions().iter().any(|(action, target, outcome)| *action == "containers.volume_restore"
        && target.as_deref().is_some_and(|t| t.starts_with("kuma_data ← "))
        && *outcome == Outcome::Success));

    // Gone meanwhile: made again with the labels the manifest names.
    t.engine.volumes.lock().unwrap().retain(|v| v.name != "kuma_data");
    t.engine.containers.lock().unwrap().retain(|c| c.name != "kuma-1");
    let job =
        t.containers.restore_backup(id, RestoreTarget::OwnName, false, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.state, JobState::Succeeded);
    assert!(t.engine.calls().contains(&"volume create kuma_data com.docker.compose.project".to_owned()));
}

/// A restore under the volume's own name that fails leaves no empty volume
/// it made behind; old contents it could not remove are told on the job.
#[tokio::test]
async fn a_failed_restore_removes_the_volume_it_made_and_leftovers_are_told() {
    let (t, _, _) = setup();
    let id = backed_up(&t, "orphan").await;
    t.engine.volumes.lock().unwrap().retain(|v| v.name != "orphan");
    *t.archiver.fail.lock().unwrap() = Some("its SHA-256 differs".into());

    let job =
        t.containers.restore_backup(id, RestoreTarget::OwnName, false, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.state, JobState::Failed);

    let calls = t.engine.calls();
    assert!(calls.iter().any(|c| c.starts_with("volume create orphan")), "{calls:?}");
    assert!(calls.contains(&"volume rm orphan".to_owned()), "{calls:?}");
    assert!(!t.engine.volumes.lock().unwrap().iter().any(|v| v.name == "orphan"));

    // Restored, with old contents left beside the data.
    *t.archiver.left_over.lock().unwrap() = Some("_data.zendiloos-restore-0123456789abcdef".into());
    let job = t
        .containers
        .restore_backup(id, RestoreTarget::Existing("stack_db".into()), false, &alice(), &context())
        .await
        .unwrap();
    let job = finished(&t, job.id).await;
    assert_eq!(job.state, JobState::Succeeded);
    assert!(
        job.output.iter().any(|line| line.contains("_data.zendiloos-restore-0123456789abcdef")),
        "{job:?}"
    );
}

#[tokio::test]
async fn a_backup_is_restored_into_another_volume_and_only_one_that_succeeded() {
    let (t, _, _) = setup();
    let id = backed_up(&t, "kuma_data").await;

    let job = t
        .containers
        .restore_backup(id, RestoreTarget::Existing("orphan".into()), false, &alice(), &context())
        .await;
    assert_eq!(finished(&t, job.unwrap().id).await.state, JobState::Succeeded);
    assert!(t.archiver.calls().last().unwrap().starts_with("restore orphan from "));
    for (target, wanted) in [("ghost", "not found"), ("nfs_media", "refused")] {
        let refused = t
            .containers
            .restore_backup(id, RestoreTarget::Existing(target.into()), true, &alice(), &context())
            .await;
        let matched = match refused {
            Err(ContainersError::NotFound) => wanted == "not found",
            Err(ContainersError::Refused(_)) => wanted == "refused",
            _ => false,
        };
        assert!(matched, "{target}");
    }
    assert!(matches!(
        t.containers.restore_backup(999, RestoreTarget::OwnName, true, &alice(), &context()).await,
        Err(ContainersError::NotFound)
    ));

    // A run that failed has no archive to restore.
    *t.archiver.fail.lock().unwrap() = Some("broken".into());
    let job = t.containers.back_up_now("kuma_data", &alice(), &context()).await.unwrap();
    finished(&t, job.id).await;
    let failed = t.containers.backups("kuma_data").await.unwrap()[0].id;
    let refused =
        t.containers.restore_backup(failed, RestoreTarget::OwnName, true, &alice(), &context()).await;
    assert!(matches!(refused, Err(ContainersError::Refused(_))), "{refused:?}");

    // A restore the agent refuses (an archive changed in the share) starts
    // the containers again all the same.
    *t.archiver.fail.lock().unwrap() = Some("its SHA-256 differs".into());
    let job =
        t.containers.restore_backup(id, RestoreTarget::OwnName, true, &alice(), &context()).await.unwrap();
    let job = finished(&t, job.id).await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.error.unwrap().code, "refused");
    let calls = container_calls(&t);
    assert_eq!(&calls[calls.len() - 2..], ["stop kuma-1-id", "start kuma-1-id"]);
}

#[tokio::test]
async fn after_a_restart_runs_cut_off_fail_and_their_containers_start_again() {
    let (t, _, _) = setup();
    let id = {
        use super::ports::{BackupRepository, NewBackup};
        let backups = &t.backups;
        let new = NewBackup {
            volume: "kuma_data".into(),
            share_id: 1,
            started_at: Timestamp::from_unix_seconds(MIDNIGHT),
            by: None,
        };
        let id = backups.start(&new).await.unwrap();
        backups.owe_start("kuma-1", "kuma_data", new.started_at).await.unwrap();
        id
    };
    {
        use super::ports::DockerEngine;
        t.engine
            .control_container("kuma-1-id", crate::domain::ContainerAction::Stop, &context())
            .await
            .unwrap();
    }
    // The agent cannot say yet whether the work on kuma_data ended.
    t.archiver.unanswering.store(true, std::sync::atomic::Ordering::Relaxed);

    assert_eq!(t.containers.recover_backups().await, 1);
    assert_eq!(container_calls(&t), ["stop kuma-1-id"], "not started under work that may run on");
    t.archiver.unanswering.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(t.containers.start_owed_containers().await, 1);

    assert_eq!(container_calls(&t), ["stop kuma-1-id", "start kuma-1-id"]);
    let run = t.containers.backups("kuma_data").await.unwrap().into_iter().find(|b| b.id == id).unwrap();
    assert!(matches!(run.outcome, BackupOutcome::Failed(_)), "{run:?}");
    assert!(t.backups.owed.lock().unwrap().is_empty());
    assert_eq!(t.notifications.all()[0].code, "docker_backup_failed");
    assert_eq!(t.containers.recover_backups().await, 0);
}

/// Lets each volume's backup through once told to, and says when it came.
struct GatedArchiver {
    inner: Arc<crate::testing::FakeArchiver>,
    gates: std::collections::HashMap<String, (tokio::sync::Notify, tokio::sync::Notify)>,
}

#[async_trait]
impl super::ports::VolumeArchiver for GatedArchiver {
    async fn back_up(
        &self,
        volume: &str,
        place: &super::ports::SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn super::ports::Steps,
        context: &RequestContext,
    ) -> Result<crate::domain::WrittenBackup, super::ports::EngineError> {
        let (entered, release) = &self.gates[volume];
        entered.notify_one();
        release.notified().await;
        self.inner.back_up(volume, place, folder, stem, steps, context).await
    }

    async fn restore(
        &self,
        volume: &str,
        place: &super::ports::SharePlace,
        written: &crate::domain::WrittenBackup,
        steps: &dyn super::ports::Steps,
        context: &RequestContext,
    ) -> Result<Option<String>, super::ports::EngineError> {
        self.inner.restore(volume, place, written, steps, context).await
    }

    async fn remove(
        &self,
        place: &super::ports::SharePlace,
        file: &crate::domain::BackupFile,
        context: &RequestContext,
    ) -> Result<bool, super::ports::EngineError> {
        self.inner.remove(place, file, context).await
    }

    async fn end_work(
        &self,
        volume: &str,
        context: &RequestContext,
    ) -> Result<bool, super::ports::EngineError> {
        self.inner.end_work(volume, context).await
    }
}

/// Two volumes of one container, both backed up with it stopped: the
/// second waits for its turn, and the container runs again only once the
/// second is done, never while it is read.
#[tokio::test]
async fn a_shared_container_stays_stopped_until_the_last_backup_holding_it_ended() {
    let (mut t, _, _) = setup();
    {
        let mut all = t.engine.containers.lock().unwrap();
        let c = all.iter_mut().find(|c| c.name == "kuma-1").unwrap();
        *c = crate::testing::mounting(c.clone(), "orphan");
    }
    let gated = Arc::new(GatedArchiver {
        inner: t.archiver.clone(),
        gates: ["kuma_data", "orphan"]
            .into_iter()
            .map(|s| (s.to_owned(), (tokio::sync::Notify::new(), tokio::sync::Notify::new())))
            .collect(),
    });
    t.containers.ports.archiver = gated.clone();
    for volume in ["kuma_data", "orphan"] {
        t.containers.set_backup_plan(volume, nightly(7, true), &alice(), &context()).await.unwrap();
    }
    let up = |t: &TestContainers| {
        t.engine.containers.lock().unwrap().iter().any(|c| c.name == "kuma-1" && c.state.is_up())
    };
    let a = t.containers.back_up_now("kuma_data", &alice(), &context()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), gated.gates["kuma_data"].0.notified()).await.unwrap();
    assert!(!up(&t));
    let b = t.containers.back_up_now("orphan", &alice(), &context()).await.unwrap();
    let waiting = tokio::time::timeout(Duration::from_millis(200), gated.gates["orphan"].0.notified()).await;
    assert!(waiting.is_err(), "one at a time: orphan waits for its turn");

    gated.gates["kuma_data"].1.notify_one();
    assert_eq!(finished(&t, a.id).await.state, JobState::Succeeded);
    tokio::time::timeout(Duration::from_secs(5), gated.gates["orphan"].0.notified()).await.unwrap();
    assert!(!up(&t), "kuma-1 stays stopped while orphan is read");
    gated.gates["orphan"].1.notify_one();
    assert_eq!(finished(&t, b.id).await.state, JobState::Succeeded);
    assert!(up(&t));
    assert_eq!(container_calls(&t), ["stop kuma-1-id", "start kuma-1-id"], "never started in between");
}
