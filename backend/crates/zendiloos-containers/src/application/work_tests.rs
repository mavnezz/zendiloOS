//! Backups and restores of volumes and what they hold, on the in-memory
//! adapters with an archiver that can be held: containers they stopped
//! stay stopped until the last one holding them ended, nothing else starts
//! or changes them meanwhile, and they are started again after a restart.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::MemoryJobs;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, PortError, RequestContext, Timestamp};

use super::ports::{EngineError, SharePlace, Steps, VolumeArchiver};
use super::{Containers, ContainersPorts, PlanInput, RestoreTarget};
use crate::domain::{
    BackupFile, BackupFrequency, BackupOutcome, BackupSchedule, BackupTarget, ContainerAction,
    ContainerState, FAILURES_KEPT, WrittenBackup,
};
use crate::testing::{
    FakeArchiver, FakeBackupShares, FakeDockerData, FakeEngine, FakeRegistries, MemoryBackups,
    MemoryProjects, MemoryStorage, MemoryUpdateSettings, container, mounting, volume,
};

/// 2026-11-05 00:00:00 UTC.
const MIDNIGHT: i64 = 1_793_836_800;
const HOUR: i64 = 3600;

struct Fixed(Mutex<Timestamp>);

impl Clock for Fixed {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap()
    }

    fn utc_offset(&self, _: Timestamp) -> i32 {
        0
    }
}

#[derive(Default)]
struct NoAudit;

#[async_trait]
impl AuditLog for NoAudit {
    async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
        Ok(())
    }
}

/// An archiver whose backups of `held` volumes wait for `open`, whose
/// restores never end while `restores_hang`, and which notes whether the
/// container `watched` runs when each backup ends.
struct HeldArchiver {
    inner: FakeArchiver,
    engine: Arc<FakeEngine>,
    held: Vec<String>,
    open: Notify,
    /// Per held volume, released one by one; `open` releases all.
    gates: Mutex<std::collections::BTreeMap<String, Arc<Notify>>>,
    entered: Mutex<Vec<String>>,
    restores_hang: bool,
    watched: String,
    /// (volume, whether `watched` ran as the archive was finished).
    seen: Mutex<Vec<(String, bool)>>,
}

impl HeldArchiver {
    fn running(&self) -> bool {
        self.engine
            .containers
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.name == self.watched && c.state == ContainerState::Running)
    }
}

#[async_trait]
impl VolumeArchiver for HeldArchiver {
    async fn back_up(
        &self,
        volume: &str,
        place: &SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<WrittenBackup, EngineError> {
        self.entered.lock().unwrap().push(volume.to_owned());
        if self.held.iter().any(|v| v == volume) {
            let gate = self.gates.lock().unwrap().entry(volume.to_owned()).or_default().clone();
            tokio::select! {
                () = self.open.notified() => {}
                () = gate.notified() => {}
            }
        }
        let running = self.running();
        self.seen.lock().unwrap().push((volume.to_owned(), running));
        self.inner.back_up(volume, place, folder, stem, steps, context).await
    }

    async fn restore(
        &self,
        volume: &str,
        place: &SharePlace,
        written: &WrittenBackup,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Option<String>, EngineError> {
        if self.restores_hang {
            std::future::pending::<()>().await;
        }
        self.inner.restore(volume, place, written, steps, context).await
    }

    async fn remove(
        &self,
        place: &SharePlace,
        file: &BackupFile,
        context: &RequestContext,
    ) -> Result<bool, EngineError> {
        self.inner.remove(place, file, context).await
    }

    async fn end_work(&self, volume: &str, context: &RequestContext) -> Result<bool, EngineError> {
        self.inner.end_work(volume, context).await
    }
}

struct World {
    engine: Arc<FakeEngine>,
    backups: Arc<MemoryBackups>,
    jobs: Arc<MemoryJobs>,
    clock: Arc<Fixed>,
}

impl World {
    fn new() -> Self {
        Self {
            engine: Arc::new(FakeEngine::default()),
            backups: Arc::new(MemoryBackups::default()),
            jobs: Arc::new(MemoryJobs::default()),
            clock: Arc::new(Fixed(Mutex::new(Timestamp::from_unix_seconds(MIDNIGHT)))),
        }
    }

    /// The use cases over this world's engine, runs and jobs: a second
    /// call is zendiloosd after a restart.
    fn containers(&self, archiver: Arc<dyn VolumeArchiver>) -> Containers {
        let notifications = Arc::new(zendiloos_notify::testing::MemoryNotifications::default());
        Containers::new(ContainersPorts {
            engine: self.engine.clone(),
            projects: Arc::new(MemoryProjects::default()),
            storage: Arc::new(MemoryStorage::default()),
            registries: Arc::new(FakeRegistries),
            updates: Arc::new(MemoryUpdateSettings::default()),
            data: Arc::new(FakeDockerData::default()),
            archiver,
            backups: self.backups.clone(),
            backup_shares: Arc::new(FakeBackupShares),
            jobs: Jobs::new(self.jobs.clone(), self.clock.clone()),
            notifier: zendiloos_notify::Notifier::new(notifications, self.clock.clone()),
            audit: Arc::new(NoAudit),
            clock: self.clock.clone(),
        })
    }

    fn state(&self, name: &str) -> ContainerState {
        self.engine.containers.lock().unwrap().iter().find(|c| c.name == name).unwrap().state
    }

    async fn until(&self, what: impl Fn() -> bool) {
        for _ in 0..400 {
            if what() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("never came");
    }

    fn jobs_running(&self) -> usize {
        self.jobs.all().iter().filter(|j| j.finished_at.is_none()).count()
    }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn nightly(stop_containers: bool) -> PlanInput {
    PlanInput {
        target: BackupTarget { share_id: 1, folder: "backups".into() },
        schedule: BackupSchedule { frequency: BackupFrequency::Daily, hour: 3 },
        keep: 7,
        stop_containers,
    }
}

fn held(world: &World, held: &[&str], restores_hang: bool, watched: &str) -> Arc<HeldArchiver> {
    Arc::new(HeldArchiver {
        inner: FakeArchiver::default(),
        engine: world.engine.clone(),
        held: held.iter().map(|v| (*v).to_owned()).collect(),
        open: Notify::new(),
        gates: Mutex::default(),
        entered: Mutex::default(),
        restores_hang,
        watched: watched.to_owned(),
        seen: Mutex::new(Vec::new()),
    })
}

/// A backup of `kuma_data` that succeeded, to restore.
async fn one_backup(world: &World) -> i64 {
    let plain = world.containers(Arc::new(FakeArchiver::default()));
    plain.set_backup_plan("kuma_data", nightly(false), &alice(), &RequestContext::default()).await.unwrap();
    plain.back_up_now("kuma_data", &alice(), &RequestContext::default()).await.unwrap();
    world.until(|| world.jobs_running() == 0).await;
    let runs = world.backups.runs.lock().unwrap().clone();
    let run = runs.iter().find(|r| matches!(r.outcome, BackupOutcome::Succeeded(_))).unwrap();
    run.id
}

/// Should zendiloosd stop while the agent restores, the containers the
/// restore stopped are started again when it is back.
#[tokio::test]
async fn containers_stopped_for_a_restore_are_started_again_after_a_restart() {
    let world = World::new();
    let id = one_backup(&world).await;
    assert_eq!(world.state("kuma-1"), ContainerState::Running);

    let before = world.containers(held(&world, &[], true, "kuma-1"));
    before
        .restore_backup(id, RestoreTarget::OwnName, true, &alice(), &RequestContext::default())
        .await
        .unwrap();
    world.until(|| world.state("kuma-1") == ContainerState::Exited).await;

    // zendiloosd stops (crash, restart) while the agent restores; it comes up again.
    drop(before);
    let after = world.containers(Arc::new(FakeArchiver::default()));
    after.recover_backups().await;

    assert_eq!(
        world.state("kuma-1"),
        ContainerState::Running,
        "the container stopped for the restore stays stopped after zendiloosd's restart"
    );
}

/// After a power cut mid-backup, zendiloosd may start before Docker (which
/// waits for its pool): starting the containers fails, and they stay owed
/// a start until the minute's look finds Docker up.
#[tokio::test]
async fn containers_stopped_for_a_backup_are_started_once_docker_is_up() {
    let world = World::new();
    let archiver = held(&world, &["kuma_data"], false, "kuma-1");
    let before = world.containers(archiver);
    before.set_backup_plan("kuma_data", nightly(true), &alice(), &RequestContext::default()).await.unwrap();
    before.back_up_now("kuma_data", &alice(), &RequestContext::default()).await.unwrap();
    world.until(|| world.state("kuma-1") == ContainerState::Exited).await;
    drop(before);

    // Booting: Docker is not up yet, starting fails.
    world.engine.stubborn.lock().unwrap().push("kuma-1-id".into());
    let after = world.containers(Arc::new(FakeArchiver::default()));
    assert_eq!(after.recover_backups().await, 1);
    assert_eq!(world.state("kuma-1"), ContainerState::Exited);
    // Docker is up now.
    world.engine.stubborn.lock().unwrap().clear();
    assert_eq!(after.start_owed_containers().await, 1);

    assert_eq!(world.state("kuma-1"), ContainerState::Running);
    assert!(world.backups.owed.lock().unwrap().is_empty(), "owed nothing once it started");
}

/// Two volumes of one container on the same schedule, containers stopped:
/// one backup at a time, and the container stays stopped from the first's
/// start to the second's end.
#[tokio::test]
async fn a_container_of_two_volumes_stays_stopped_until_both_are_backed_up() {
    let world = World::new();
    {
        let mut containers = world.engine.containers.lock().unwrap();
        containers.push(mounting(mounting(container("app", Some("app"), "app", true), "app_data"), "app_db"));
        let mut volumes = world.engine.volumes.lock().unwrap();
        volumes.push(volume("app_data", Some("app")));
        volumes.push(volume("app_db", Some("app")));
    }
    let archiver = held(&world, &["app_data", "app_db"], false, "app");
    let containers = world.containers(archiver.clone());
    for name in ["app_data", "app_db"] {
        containers.set_backup_plan(name, nightly(true), &alice(), &RequestContext::default()).await.unwrap();
    }
    *world.clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 3 * HOUR);

    let jobs = containers.run_due_backups().await;
    assert_eq!(jobs.len(), 2);
    // One at a time: the first is written, the second waits for its turn.
    world.until(|| archiver.entered.lock().unwrap().len() == 1).await;
    assert_eq!(world.state("app"), ContainerState::Exited, "stopped for the first");
    let gate = |v: &str| archiver.gates.lock().unwrap().entry(v.to_owned()).or_default().clone();
    let first = archiver.entered.lock().unwrap()[0].clone();
    gate(&first).notify_one();
    world.until(|| archiver.entered.lock().unwrap().len() == 2).await;
    assert_eq!(world.state("app"), ContainerState::Exited, "still stopped for the second");
    let second = archiver.entered.lock().unwrap()[1].clone();
    gate(&second).notify_one();
    world.until(|| world.jobs_running() == 0).await;

    let seen = archiver.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|(_, ran)| !ran), "app ran while a volume of it was archived: {seen:?}");
    assert_eq!(world.state("app"), ContainerState::Running, "started once both are done");
    let calls: Vec<String> = world.engine.calls().into_iter().filter(|c| c.ends_with(" app-id")).collect();
    assert_eq!(calls, ["stop app-id", "start app-id"], "never started in between");
}

/// Failed runs are remembered ten at a time, also while a plan keeps
/// failing.
#[tokio::test]
async fn failures_beyond_ten_are_forgotten_while_a_plan_keeps_failing() {
    let world = World::new();
    let archiver = Arc::new(FakeArchiver::default());
    let containers = world.containers(archiver.clone());
    containers.set_backup_plan("orphan", nightly(false), &alice(), &RequestContext::default()).await.unwrap();
    for n in 0..12 {
        *archiver.fail.lock().unwrap() = Some("no space left on device".into());
        *world.clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + n * 24 * HOUR);
        containers.back_up_now("orphan", &alice(), &RequestContext::default()).await.unwrap();
        world.until(|| world.jobs_running() == 0).await;
    }
    let failed: Vec<i64> = world
        .backups
        .runs
        .lock()
        .unwrap()
        .iter()
        .filter(|r| matches!(r.outcome, BackupOutcome::Failed(_)))
        .map(|r| r.id)
        .collect();
    assert_eq!(failed.len(), FAILURES_KEPT, "{failed:?}");
    assert_eq!(failed.iter().min(), Some(&3), "the newest ten");
}

/// A plan noted while the clock was a year ahead runs at its next time
/// once the clock is right, not a year later; and the run then is never
/// removed by its own retention.
#[tokio::test]
async fn a_plan_noted_ahead_of_the_clock_runs_when_it_says_and_keeps_its_archive() {
    let world = World::new();
    let archiver = Arc::new(FakeArchiver::default());
    let containers = world.containers(archiver.clone());
    let keep_one = PlanInput { keep: 1, ..nightly(false) };
    *world.clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 365 * 24 * HOUR);
    containers
        .set_backup_plan("orphan", keep_one.clone(), &alice(), &RequestContext::default())
        .await
        .unwrap();
    containers.back_up_now("orphan", &alice(), &RequestContext::default()).await.unwrap();
    world.until(|| world.jobs_running() == 0).await;
    let ahead = Timestamp::from_unix_seconds(MIDNIGHT + 365 * 24 * HOUR);
    super::ports::BackupRepository::ran(world.backups.as_ref(), "orphan", ahead).await.unwrap();

    // The clock is put right: at noon on the 5th, then at three on the 6th.
    *world.clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 12 * HOUR);
    assert!(containers.run_due_backups().await.is_empty());
    let since = world.backups.plans.lock().unwrap()[0].since;
    assert_eq!(since, Timestamp::from_unix_seconds(MIDNIGHT + 12 * HOUR), "noted anew");
    *world.clock.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + 27 * HOUR);
    assert_eq!(containers.run_due_backups().await.len(), 1);
    world.until(|| world.jobs_running() == 0).await;

    let runs = world.backups.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(
        runs[0].started_at,
        Timestamp::from_unix_seconds(MIDNIGHT + 27 * HOUR),
        "the one just written"
    );
}

/// A container a restore stopped is not started meanwhile, by hand or by
/// a project's redeploy, nor removed.
#[tokio::test]
async fn a_container_of_a_volume_being_restored_is_left_alone() {
    let world = World::new();
    let id = one_backup(&world).await;
    let containers = world.containers(held(&world, &[], true, "kuma-1"));
    containers
        .restore_backup(id, RestoreTarget::OwnName, true, &alice(), &RequestContext::default())
        .await
        .unwrap();
    world.until(|| world.state("kuma-1") == ContainerState::Exited).await;

    for action in [ContainerAction::Start, ContainerAction::Remove] {
        let refused =
            containers.control_container("kuma-1", action, &alice(), &RequestContext::default()).await;
        assert!(
            matches!(&refused, Err(super::ContainersError::Held(names)) if names == &["kuma-1"]),
            "{action:?} while its volume is being restored: {refused:?}"
        );
    }
    let start = super::ports::ProjectAction::Start;
    let project = containers.control_project("kuma", start, &alice(), &RequestContext::default()).await;
    assert!(matches!(project, Err(super::ContainersError::Held(_))), "{project:?}");
    assert_eq!(world.state("kuma-1"), ContainerState::Exited);
}

/// Docker's data does not move while a volume is restored: the unit would
/// write on into the old dataset once its copy was sent.
#[tokio::test]
async fn dockers_data_is_not_moved_while_a_volume_is_restored() {
    let world = World::new();
    let id = one_backup(&world).await;
    let containers = world.containers(held(&world, &[], true, "kuma-1"));
    containers
        .restore_backup(id, RestoreTarget::OwnName, true, &alice(), &RequestContext::default())
        .await
        .unwrap();
    world.until(|| world.state("kuma-1") == ContainerState::Exited).await;

    let location = containers.data_location(&RequestContext::default()).await.unwrap();
    let Some(pool) = location.targets.first().map(|t| t.pool.clone()) else {
        panic!("the fake offers no other pool: {location:?}")
    };
    let moved = containers.move_data(&pool, &alice(), &RequestContext::default()).await;
    assert!(moved.is_err(), "Docker's data moves while kuma_data is being restored");
}

/// While Docker's data moves, no volume is backed up or restored.
#[tokio::test]
async fn no_volume_is_backed_up_while_dockers_data_moves() {
    struct Moving(tokio::sync::Notify);

    #[async_trait]
    impl super::ports::DockerData for Moving {
        async fn places(
            &self,
            context: &RequestContext,
        ) -> Result<(Option<super::ports::DataPlace>, Vec<super::ports::DataPlace>), PortError> {
            FakeDockerData::default().places(context).await
        }

        async fn move_to(
            &self,
            _: &super::ports::DataPlace,
            _: &super::ports::DataPlace,
            _: &RequestContext,
        ) -> Result<(), EngineError> {
            self.0.notified().await;
            Ok(())
        }
    }

    let world = World::new();
    let mut containers = world.containers(Arc::new(FakeArchiver::default()));
    let moving = Arc::new(Moving(tokio::sync::Notify::new()));
    containers.ports.data = moving.clone();
    containers.set_backup_plan("orphan", nightly(false), &alice(), &RequestContext::default()).await.unwrap();
    containers.move_data("backup", &alice(), &RequestContext::default()).await.unwrap();

    let refused = containers.back_up_now("orphan", &alice(), &RequestContext::default()).await;
    assert!(matches!(refused, Err(super::ContainersError::Refused(_))), "{refused:?}");
    moving.0.notify_one();
    world.until(|| world.jobs_running() == 0).await;
    containers.back_up_now("orphan", &alice(), &RequestContext::default()).await.unwrap();
    world.until(|| world.jobs_running() == 0).await;
}

/// Unused volumes are not pruned while one of them is backed up or
/// restored: its folder would go mid-way.
#[tokio::test]
async fn unused_volumes_are_not_pruned_while_one_is_backed_up() {
    let world = World::new();
    let archiver = held(&world, &["orphan"], false, "kuma-1");
    let containers = world.containers(archiver.clone());
    containers.set_backup_plan("orphan", nightly(false), &alice(), &RequestContext::default()).await.unwrap();
    containers.back_up_now("orphan", &alice(), &RequestContext::default()).await.unwrap();
    assert!(containers.remove_volume("orphan", &alice(), &RequestContext::default()).await.is_err());

    let pruned = containers.prune_volumes(&alice(), &RequestContext::default()).await;
    archiver.open.notify_one();
    assert!(pruned.is_err(), "pruned while orphan was being backed up: {pruned:?}");
}

/// The fake archiver, whose backups wait for `go`.
struct SlowArchiver {
    inner: FakeArchiver,
    started: Notify,
    go: Notify,
}

#[async_trait]
impl VolumeArchiver for SlowArchiver {
    async fn back_up(
        &self,
        volume: &str,
        place: &SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<WrittenBackup, EngineError> {
        self.started.notify_one();
        self.go.notified().await;
        self.inner.back_up(volume, place, folder, stem, steps, context).await
    }

    async fn restore(
        &self,
        volume: &str,
        place: &SharePlace,
        written: &WrittenBackup,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Option<String>, EngineError> {
        self.inner.restore(volume, place, written, steps, context).await
    }

    async fn remove(
        &self,
        place: &SharePlace,
        file: &BackupFile,
        context: &RequestContext,
    ) -> Result<bool, EngineError> {
        self.inner.remove(place, file, context).await
    }

    async fn end_work(&self, volume: &str, context: &RequestContext) -> Result<bool, EngineError> {
        self.inner.end_work(volume, context).await
    }
}

/// An image update (on schedule or by hand) brings a project up anew: it
/// waits while a backup keeps a container of it stopped, and runs after.
#[tokio::test]
async fn an_image_update_waits_for_the_backup_that_stopped_its_containers() {
    let world = World::new();
    // `stack` (made elsewhere): `stack-web` (nginx, which the registry has
    // newer) and `stack-db`, running, on the volume `stack_db`.
    for c in world.engine.containers.lock().unwrap().iter_mut().filter(|c| c.name == "stack-db") {
        c.state = ContainerState::Running;
    }
    let archiver =
        Arc::new(SlowArchiver { inner: FakeArchiver::default(), started: Notify::new(), go: Notify::new() });
    let containers = world.containers(archiver.clone());
    let context = RequestContext::default();
    containers.set_backup_plan("stack_db", nightly(true), &alice(), &context).await.unwrap();
    let running = |name: &str| world.state(name) == ContainerState::Running;

    // The backup stops stack-db and reads the volume...
    let backup = containers.back_up_now("stack_db", &alice(), &context).await.unwrap();
    archiver.started.notified().await;
    assert!(!running("stack-db"), "the backup stopped it");

    // ...and the image update comes meanwhile.
    let update = containers.apply_updates(None, &context).await.unwrap().expect("nginx is newer");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let deployed =
        || world.engine.calls().iter().any(|call| call.starts_with("project ") && call.ends_with(" stack"));
    assert!(!running("stack-db") && !deployed(), "stack is not brought up while its volume is archived");
    let job = world.jobs.all().into_iter().find(|job| job.id == update.id).unwrap();
    assert_eq!((job.finished_at, job.step.as_deref()), (None, Some("waiting")));

    archiver.go.notify_one();
    world.until(|| world.jobs_running() == 0).await;
    assert!(world.jobs.all().iter().all(|job| job.error.is_none()), "{:?}", world.jobs.all());
    assert!(deployed(), "updated once the backup ended");
    assert!(running("stack-db"));
    let _ = backup;
}
