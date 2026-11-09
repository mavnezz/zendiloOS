//! Recorded backups as a whole, on the in-memory adapters: listed by
//! volume also once the volume is gone, restored from there, removed by
//! hand; plans of gone volumes paused and resumed; backups found again in
//! a folder recorded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobState};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use super::ports::{FoundArchive, ScannedArchives};
use super::{ContainersError, PlanInput, RestoreTarget};
use crate::domain::{
    BackupFile, BackupFrequency, BackupOutcome, BackupSchedule, BackupTarget, WrittenBackup,
};
use crate::testing::TestContainers;

/// 2026-11-05 00:00:00 UTC: one at night in Berlin.
const MIDNIGHT: i64 = 1_793_836_800;
const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

struct Berlin(Mutex<Timestamp>);

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
    fn has(&self, action: &str, outcome: Outcome) -> bool {
        self.0.lock().unwrap().iter().any(|e| e.action == action && e.outcome == outcome)
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

fn nightly(keep: u32) -> PlanInput {
    PlanInput {
        target: BackupTarget { share_id: 1, folder: "backups".into() },
        schedule: BackupSchedule { frequency: BackupFrequency::Daily, hour: 3 },
        keep,
        stop_containers: false,
    }
}

async fn finished(t: &TestContainers, id: zendiloos_jobs::JobId) -> Job {
    for _ in 0..400 {
        if let Some(job) = t.jobs.all().into_iter().find(|job| job.id == id && job.finished_at.is_some()) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("job {id:?} did not end");
}

async fn backed_up(t: &TestContainers, volume: &str) -> i64 {
    t.containers.set_backup_plan(volume, nightly(7), &alice(), &context()).await.unwrap();
    let job = t.containers.back_up_now(volume, &alice(), &context()).await.unwrap();
    assert_eq!(finished(t, job.id).await.state, JobState::Succeeded);
    t.containers.backups(volume).await.unwrap()[0].id
}

/// The volume goes outside zendiloOS (`docker compose down -v`).
fn gone(t: &TestContainers, volume: &str) {
    t.engine.volumes.lock().unwrap().retain(|v| v.name != volume);
}

#[tokio::test]
async fn the_backups_of_a_gone_volume_are_listed_and_restored_under_its_name() {
    let (t, _, _) = setup();
    let id = backed_up(&t, "orphan").await;
    gone(&t, "orphan");

    let groups = t.containers.backup_groups(&context()).await.unwrap();
    let names: Vec<(&str, bool)> = groups.iter().map(|g| (g.volume.as_str(), g.exists)).collect();
    assert_eq!(names, [("orphan", false)]);
    assert_eq!(groups[0].backups.iter().map(|b| b.id).collect::<Vec<_>>(), [id]);
    assert!(groups[0].plan.is_some());

    let job =
        t.containers.restore_backup(id, RestoreTarget::OwnName, false, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.state, JobState::Succeeded);
    assert!(t.engine.calls().iter().any(|c| c.starts_with("volume create orphan")));
    let groups = t.containers.backup_groups(&context()).await.unwrap();
    assert!(groups[0].exists, "made again");
}

#[tokio::test]
async fn a_plan_whose_volume_is_gone_pauses_and_runs_again_only_once_resumed() {
    let (t, audit, clock) = setup();
    t.containers.set_backup_plan("orphan", nightly(7), &alice(), &context()).await.unwrap();
    gone(&t, "orphan");

    *clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 2 * HOUR);
    assert!(t.containers.run_due_backups().await.is_empty());
    let plan = t.containers.backup_plan("orphan").await.unwrap().unwrap().plan;
    assert!(plan.paused);
    assert!(t.notifications.all().is_empty(), "no failure told");
    *clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + DAY + 2 * HOUR);
    assert!(t.containers.run_due_backups().await.is_empty(), "nor the next night");
    let refused = t.containers.back_up_now("orphan", &alice(), &context()).await;
    assert!(matches!(refused, Err(ContainersError::PlanPaused(_))), "{refused:?}");
    let without = t.containers.resume_backup_plan("orphan", &alice(), &context()).await;
    assert!(matches!(without, Err(ContainersError::Refused(_))), "no volume to resume for: {without:?}");

    // A volume of that name is there again: it waits for the administrator.
    t.engine.volumes.lock().unwrap().push(crate::testing::volume("orphan", None));
    *clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 2 * DAY + 2 * HOUR);
    assert!(t.containers.run_due_backups().await.is_empty(), "not resumed by itself");
    let resumed = t.containers.resume_backup_plan("orphan", &alice(), &context()).await.unwrap();
    assert!(!resumed.plan.paused);
    assert!(audit.has("containers.backup_plan_resume", Outcome::Success));
    assert!(t.containers.run_due_backups().await.is_empty(), "from now on, not at once");
    *clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 3 * DAY + 2 * HOUR);
    assert_eq!(t.containers.run_due_backups().await.len(), 1);

    // Set anew, a paused plan runs again too.
    t.backups.plans.lock().unwrap()[0].paused = true;
    let saved = t.containers.set_backup_plan("orphan", nightly(3), &alice(), &context()).await.unwrap();
    assert!(!saved.plan.paused);
}

#[tokio::test]
async fn a_backup_is_removed_by_hand_its_files_and_its_record() {
    let (t, audit, _) = setup();
    let id = backed_up(&t, "orphan").await;
    *t.archiver.fail.lock().unwrap() = Some("no space left".into());
    let job = t.containers.back_up_now("orphan", &alice(), &context()).await.unwrap();
    finished(&t, job.id).await;
    let failed = t.containers.backups("orphan").await.unwrap()[0].id;
    let outcome = t.backups.runs.lock().unwrap()[0].outcome.clone();
    t.backups.runs.lock().unwrap()[0].outcome = BackupOutcome::Running;
    let running = t.containers.remove_backup(id, &alice(), &context()).await;
    assert!(matches!(running, Err(ContainersError::VolumeBusy(_))), "not while it runs: {running:?}");
    t.backups.runs.lock().unwrap()[0].outcome = outcome;

    t.containers.remove_backup(id, &alice(), &context()).await.unwrap();
    let removed: Vec<String> = t.archiver.calls().into_iter().filter(|c| c.starts_with("remove")).collect();
    assert_eq!(
        removed,
        [
            "remove docker/backups/orphan/orphan-20261105-010000.tar.zst",
            "remove docker/backups/orphan/orphan-20261105-010000.json"
        ]
    );
    t.containers.remove_backup(failed, &alice(), &context()).await.unwrap();
    assert!(t.containers.backups("orphan").await.unwrap().is_empty());
    assert!(audit.has("containers.volume_backup_remove", Outcome::Success));
    assert!(matches!(
        t.containers.remove_backup(id, &alice(), &context()).await,
        Err(ContainersError::NotFound)
    ));
}

fn found(volume: &str, name: &str, inode: u64) -> FoundArchive {
    let file =
        |path: String, inode: u64| BackupFile { path, bytes: 100, device: 9, inode, modified: Some(1) };
    FoundArchive {
        volume: volume.to_owned(),
        backed_up_at: Timestamp::from_unix_seconds(MIDNIGHT - DAY),
        written: WrittenBackup {
            archive: file(format!("old/{volume}/{name}.tar.zst"), inode),
            manifest: file(format!("old/{volume}/{name}.json"), inode + 1),
            sha256: "cd".repeat(32),
            data_bytes: 4096,
            entries: Some(3),
            manifest_json:
                r#"{"volume":"lost","driver":"local","labels":{"com.docker.compose.project":"lost"}}"#.into(),
        },
    }
}

#[tokio::test]
async fn backups_found_again_in_a_folder_are_recorded_once_and_never_retained() {
    let (t, audit, clock) = setup();
    *t.archiver.found.lock().unwrap() = ScannedArchives {
        found: vec![
            found("lost", "lost-20261104-030000", 100),
            found("orphan", "orphan-20261104-030000", 200),
        ],
        refused: vec![(
            "old/lost/lost-20261103-030000.tar.zst".into(),
            "the archive: writable by others".into(),
        )],
    };

    let job = t.containers.import_backups(1, "/old/", &alice(), &context()).await.unwrap();
    let job = finished(&t, job.id).await;
    assert_eq!((job.kind.as_str(), job.state), ("containers.volume_import", JobState::Succeeded));
    assert!(
        job.output.contains(&"Recorded old/lost/lost-20261104-030000.tar.zst".to_owned()),
        "{:?}",
        job.output
    );
    assert!(
        job.output.contains(
            &"Refused old/lost/lost-20261103-030000.tar.zst: the archive: writable by others".to_owned()
        ),
        "{:?}",
        job.output
    );
    assert!(
        job.output.last().unwrap().starts_with("2 recorded, 0 recorded before, 1 refused"),
        "{:?}",
        job.output
    );
    assert!(t.archiver.calls().contains(&"scan docker/old knowing 0".to_owned()));
    assert!(audit.has("containers.volume_backup_import", Outcome::Success));
    let lost = t.containers.backups("lost").await.unwrap();
    assert_eq!(lost.len(), 1);
    assert!(lost[0].imported && lost[0].by.is_none());
    assert_eq!(lost[0].labels["com.docker.compose.project"], "lost");
    assert_eq!(lost[0].started_at, Timestamp::from_unix_seconds(MIDNIGHT - DAY));
    let groups = t.containers.backup_groups(&context()).await.unwrap();
    assert!(groups.iter().any(|g| g.volume == "lost" && !g.exists && g.plan.is_none()));

    // Again: what is recorded is passed over.
    let job = t.containers.import_backups(1, "old", &alice(), &context()).await.unwrap();
    let job = finished(&t, job.id).await;
    assert!(job.output.last().unwrap().starts_with("0 recorded, 2 recorded before"), "{:?}", job.output);
    assert!(t.archiver.calls().contains(&"scan docker/old knowing 2".to_owned()));

    // Of no plan: a plan keeping one removes none of them.
    t.containers.set_backup_plan("orphan", nightly(1), &alice(), &context()).await.unwrap();
    for hour in 1..=2 {
        *clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + hour * HOUR);
        let job = t.containers.back_up_now("orphan", &alice(), &context()).await.unwrap();
        finished(&t, job.id).await;
    }
    let orphan = t.containers.backups("orphan").await.unwrap();
    assert_eq!(orphan.iter().filter(|b| b.imported).count(), 1, "{orphan:?}");
    assert_eq!(orphan.iter().filter(|b| !b.imported && b.written().is_some()).count(), 1);
    assert!(orphan.iter().all(|b| !matches!(b.outcome, BackupOutcome::Failed(_))));

    let bob = Actor { account_id: 2, name: "bob".into() };
    assert!(matches!(
        t.containers.import_backups(1, "old", &bob, &context()).await,
        Err(ContainersError::Denied)
    ));
    assert!(matches!(
        t.containers.import_backups(1, "a/../b", &alice(), &context()).await,
        Err(ContainersError::Invalid(_))
    ));
}
