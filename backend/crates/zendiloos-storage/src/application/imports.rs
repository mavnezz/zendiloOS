//! Pools of another installation (docs/architecture.md, 4.15): the pools
//! the configuration names that are not imported, imported when an
//! administrator asks; the mounts of every pool it names made sure of at
//! the start, as after a restore on a machine installed afresh.
//!
//! Never by itself: ZFS imports a pool without `-f` only if this machine
//! (its host id, which a restore brings back) used it last, and importing
//! a pool another machine still uses would corrupt it. Forced, it is asked
//! for by typing the pool's name.

use std::sync::Arc;

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::application::StorageError;
use crate::application::commands::Context;
use crate::application::ports::{KnownPools, PoolImport, VolumeObserver};
use crate::domain::{self, AbsentPools, PoolName, VolumeNumber};

/// What the administrator asks to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub name: String,
    pub id: u64,
    /// For a pool the configuration does not name: the volume it becomes;
    /// none, the lowest free.
    pub volume: Option<u16>,
    /// Though another machine used it last.
    pub force: bool,
    /// The pool's name, typed again when forced.
    pub confirm: Option<String>,
}

/// What the start found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Volumes whose mounts were made anew.
    pub mounted: Vec<VolumeNumber>,
    /// Pools the configuration names that are not imported.
    pub absent: Vec<String>,
}

/// Importing pools and keeping the mounts of those the configuration
/// names in place.
#[derive(Clone)]
pub struct PoolImports {
    context: Arc<Context>,
    known: Arc<dyn KnownPools>,
    observer: Arc<dyn VolumeObserver>,
    notifier: Notifier,
}

impl PoolImports {
    pub(crate) fn new(
        context: Arc<Context>,
        known: Arc<dyn KnownPools>,
        observer: Arc<dyn VolumeObserver>,
        notifier: Notifier,
    ) -> Self {
        Self { context, known, observer, notifier }
    }

    /// The pools the configuration names that are not imported, found on
    /// the disks or missing, and the others `zpool import` finds.
    pub async fn absent(&self, request: &RequestContext) -> Result<AbsentPools, StorageError> {
        let known = self.known.known().await?;
        let imported = self.context.zfs.pools(request).await?;
        let found = self.context.admin.importable(request).await?;
        Ok(domain::absent_pools(&known, &imported, &found))
    }

    /// Imports a pool, as a job: a pool the configuration names as its
    /// volume, another (adopted) as the volume asked for or the lowest
    /// free; then its mounts are made, and Samba and NFS follow.
    pub async fn import(
        &self,
        asked: ImportRequest,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, StorageError> {
        let absent = self.absent(request).await?;
        let known = absent.known.iter().find(|k| k.found.name == asked.name && k.found.id == asked.id);
        let (found, volume, adopt, shares) = match known {
            Some(known) => (known.found.clone(), known.known.volume, false, known.known.shares.clone()),
            None => {
                let found = absent
                    .foreign
                    .iter()
                    .find(|found| found.name == asked.name && found.id == asked.id)
                    .ok_or(StorageError::PoolNotFound)?;
                let volume = match asked.volume {
                    Some(number) => VolumeNumber::new(number).ok_or(StorageError::VolumeTaken)?,
                    None => {
                        absent.free_volume.ok_or(StorageError::Plan(domain::PlanError::NoVolumeNumber))?
                    }
                };
                let known = self.known.known().await?;
                let imported = self.context.zfs.pools(request).await?;
                if domain::used_volumes(&known, &imported).contains(&volume) {
                    return Err(StorageError::VolumeTaken);
                }
                (found.clone(), volume, true, Vec::new())
            }
        };
        if !found.can_import {
            return Err(StorageError::Refused(
                found
                    .action
                    .clone()
                    .unwrap_or_else(|| format!("ZFS cannot import {}: its devices are missing", found.name)),
            ));
        }
        if found.other_host {
            if !asked.force {
                return Err(StorageError::OtherHost);
            }
            if asked.confirm.as_deref().map(str::trim) != Some(found.name.as_str()) {
                return Err(StorageError::ConfirmationMismatch);
            }
        }
        let name = PoolName::parse(&found.name).map_err(|_| {
            StorageError::Refused(format!("{} is not a name zendiloOS gives pools", found.name))
        })?;
        let import = PoolImport { name: name.clone(), id: found.id, volume, adopt, force: found.other_host };
        let observer = Arc::clone(&self.observer);
        self.context
            .job(
                "storage.import_pool",
                found.name.clone(),
                actor,
                request,
                move |admin, steps, request| async move {
                    steps.step("importing_pool", None);
                    admin.import_pool(&import, steps.as_ref(), &request).await?;
                    steps.step("mounting", None);
                    let mounted = admin.mount_volume(&name, volume, &shares, &request).await?;
                    if !mounted.missing.is_empty() {
                        tracing::warn!(
                            pool = name.as_str(),
                            missing = mounted.missing.join(", "),
                            "storage: shares the configuration names are not on the pool imported"
                        );
                    }
                    observer.volume_back(volume, &request).await;
                    Ok(())
                },
            )
            .await
    }

    /// At the start: the mounts of every pool the configuration names that
    /// is imported, made where they are missing (a restore on a machine
    /// installed afresh, a unit removed by hand), and Samba and NFS told;
    /// the pools not imported told once as a notification, to be imported
    /// on the Storage page. A pool that says another volume number than the
    /// configuration is left alone.
    pub async fn reconcile(&self, request: &RequestContext) -> Result<Reconciled, StorageError> {
        let known = self.known.known().await?;
        let imported = self.context.zfs.pools(request).await?;
        let mut reconciled = Reconciled::default();
        for pool in &known {
            let Some(found) = imported.iter().find(|imported| imported.name == pool.name) else {
                reconciled.absent.push(pool.name.clone());
                continue;
            };
            if let Some(mismatch) = domain::mismatch(pool, found) {
                tracing::warn!(
                    pool = pool.name,
                    ?mismatch,
                    "storage: the pool is not the volume the configuration names"
                );
                continue;
            }
            let Ok(name) = PoolName::parse(&pool.name) else { continue };
            match self.context.admin.mount_volume(&name, pool.volume, &pool.shares, request).await {
                Ok(mounted) => {
                    if !mounted.missing.is_empty() {
                        tracing::warn!(
                            pool = pool.name,
                            missing = mounted.missing.join(", "),
                            "storage: shares the configuration names are not on their pool"
                        );
                    }
                    if mounted.changed {
                        tracing::info!(
                            pool = pool.name,
                            volume = pool.volume.get(),
                            "storage: the volume's mounts made again"
                        );
                        reconciled.mounted.push(pool.volume);
                        self.observer.volume_back(pool.volume, request).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(pool = pool.name, %error, "storage: cannot make sure of the volume's mounts");
                }
            }
        }
        if !reconciled.absent.is_empty() {
            tracing::warn!(
                pools = reconciled.absent.join(", "),
                "storage: pools the configuration names are not imported"
            );
            self.notifier
                .notify(
                    NewNotification::new(Level::Warning, "pools_not_imported")
                        .with("count", reconciled.absent.len().to_string())
                        .with("names", reconciled.absent.join(", ")),
                )
                .await;
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_jobs::{JobState, Jobs};

    use super::*;
    use crate::application::StorageUseCases;
    use crate::domain::{ImportablePool, KnownPool};
    use crate::testing::{FakeStorage, RecordingAudit};

    struct Setup {
        storage: StorageUseCases,
        fake: Arc<FakeStorage>,
        jobs: Jobs,
        audit: Arc<RecordingAudit>,
    }

    fn setup() -> Setup {
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let audit = Arc::new(RecordingAudit::default());
        let (ports, fake) = FakeStorage::ports(jobs.clone(), audit.clone(), Arc::new(FixedClock));
        Setup { storage: StorageUseCases::new(ports), fake, jobs, audit }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    fn known(name: &str, volume: u16, shares: &[&str]) -> KnownPool {
        KnownPool {
            name: name.into(),
            volume: VolumeNumber::new(volume).unwrap(),
            shares: shares.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    fn found(name: &str, id: u64, other_host: bool) -> ImportablePool {
        ImportablePool {
            name: name.into(),
            id,
            state: "ONLINE".into(),
            other_host,
            can_import: true,
            status: other_host.then(|| "The pool was last accessed by another system.".into()),
            action: None,
            devices: vec![("ata-TEST_HDD_T3-part1".into(), "ONLINE".into())],
        }
    }

    async fn finished(jobs: &Jobs, job: Job) -> Job {
        let mut events = jobs.subscribe();
        loop {
            if let Some(current) = jobs.find(job.id).await.unwrap()
                && current.state != JobState::Running
            {
                return current;
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        }
    }

    fn ask(name: &str, id: u64) -> ImportRequest {
        ImportRequest { name: name.into(), id, volume: None, force: false, confirm: None }
    }

    #[tokio::test]
    async fn a_pool_the_configuration_names_is_imported_as_its_volume_and_mounted() {
        let s = setup();
        let request = RequestContext::default();
        *s.fake.tank_absent.lock().unwrap() = true;
        s.fake.known.lock().unwrap().extend([known("tank", 1, &["photos", "media"]), known("cold", 3, &[])]);
        s.fake.importable.lock().unwrap().extend([found("tank", 77, false), found("stray", 88, false)]);

        let absent = s.storage.imports.absent(&request).await.unwrap();
        assert_eq!(absent.known.len(), 1);
        assert_eq!(absent.known[0].known.volume.get(), 1);
        assert_eq!(absent.missing, [known("cold", 3, &[])], "no disk holds it");
        assert_eq!(absent.foreign, [found("stray", 88, false)]);
        assert_eq!(absent.free_volume.map(VolumeNumber::get), Some(2), "1 and 3 are the configuration's");

        let unknown = s.storage.imports.import(ask("tank", 1), &alice(), &request).await;
        assert!(matches!(unknown, Err(StorageError::PoolNotFound)), "the identifier counts: {unknown:?}");
        let job = s.storage.imports.import(ask("tank", 77), &alice(), &request).await.unwrap();
        assert_eq!(job.kind, "storage.import_pool");
        let done = finished(&s.jobs, job).await;
        assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
        assert_eq!(s.fake.calls(), ["import tank 77 1 adopt=false force=false", "mount tank 1 photos,media"]);
        assert_eq!(*s.fake.volumes_back.lock().unwrap(), [1], "Samba and NFS follow");
        let entry = s.audit.entries().pop().unwrap();
        assert_eq!((entry.action, entry.target.as_deref()), ("storage.import_pool", Some("tank")));
    }

    #[tokio::test]
    async fn a_pool_another_machine_used_last_is_imported_only_by_force_and_its_name() {
        let s = setup();
        let request = RequestContext::default();
        *s.fake.tank_absent.lock().unwrap() = true;
        s.fake.known.lock().unwrap().push(known("tank", 1, &[]));
        s.fake.importable.lock().unwrap().push(found("tank", 77, true));

        let plain = s.storage.imports.import(ask("tank", 77), &alice(), &request).await;
        assert!(matches!(plain, Err(StorageError::OtherHost)), "{plain:?}");
        let forced = ImportRequest { force: true, confirm: Some("tnak".into()), ..ask("tank", 77) };
        let mistyped = s.storage.imports.import(forced, &alice(), &request).await;
        assert!(matches!(mistyped, Err(StorageError::ConfirmationMismatch)), "{mistyped:?}");
        assert!(s.fake.calls().is_empty());
        let forced = ImportRequest { force: true, confirm: Some("tank".into()), ..ask("tank", 77) };
        let job = s.storage.imports.import(forced, &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.fake.calls()[0], "import tank 77 1 adopt=false force=true");
    }

    #[tokio::test]
    async fn a_pool_the_configuration_does_not_name_is_adopted_as_a_free_volume() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().push(known("cold", 2, &[]));
        s.fake.importable.lock().unwrap().push(found("stray", 88, false));
        let mut broken = found("broken", 99, false);
        broken.can_import = false;
        broken.action = Some("The pool cannot be imported. Attach the missing devices and try again.".into());
        s.fake.importable.lock().unwrap().push(broken);

        for taken in [1, 2] {
            let asked = ImportRequest { volume: Some(taken), ..ask("stray", 88) };
            let refused = s.storage.imports.import(asked, &alice(), &request).await;
            assert!(matches!(refused, Err(StorageError::VolumeTaken)), "volume {taken}: {refused:?}");
        }
        let odd = ImportRequest { volume: Some(1000), ..ask("stray", 88) };
        assert!(matches!(
            s.storage.imports.import(odd, &alice(), &request).await,
            Err(StorageError::VolumeTaken)
        ));
        let missing = s.storage.imports.import(ask("broken", 99), &alice(), &request).await;
        assert!(
            matches!(missing, Err(StorageError::Refused(ref why)) if why.contains("missing devices")),
            "{missing:?}"
        );

        let job = s.storage.imports.import(ask("stray", 88), &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.fake.calls(), ["import stray 88 3 adopt=true force=false", "mount stray 3 "]);
        let asked = ImportRequest { volume: Some(7), ..ask("stray", 88) };
        s.fake.importable.lock().unwrap().push(found("stray", 88, false));
        let job = s.storage.imports.import(asked, &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(s.fake.calls()[2], "import stray 88 7 adopt=true force=false");
    }

    #[tokio::test]
    async fn the_start_makes_the_mounts_of_the_pools_imported_and_tells_of_those_that_are_not() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().extend([known("tank", 1, &["photos"]), known("cold", 2, &[])]);
        let unchanged = s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(unchanged.mounted, []);
        assert_eq!(unchanged.absent, ["cold"]);
        assert_eq!(s.fake.calls(), ["mount tank 1 photos"]);
        assert!(s.fake.volumes_back.lock().unwrap().is_empty(), "nothing came back");

        *s.fake.mounts_change.lock().unwrap() = true;
        let made = s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(made.mounted, [VolumeNumber::new(1).unwrap()]);
        assert_eq!(*s.fake.volumes_back.lock().unwrap(), [1]);

        // The configuration says another volume than the pool: left alone.
        s.fake.known.lock().unwrap()[0].volume = VolumeNumber::new(5).unwrap();
        s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(s.fake.calls().len(), 2);
    }

    #[tokio::test]
    async fn a_new_pool_never_takes_the_volume_of_a_pool_not_imported() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().push(known("cold", 2, &[]));
        let planned = s
            .storage
            .plan_pool
            .execute(
                "media",
                crate::domain::PoolLayout::Mirror,
                vec!["sdd".into(), "sde".into()],
                &alice(),
                &request,
            )
            .await
            .unwrap();
        assert_eq!(planned.plan.volume.get(), 3, "tank is 1, cold's 2 is kept for it");
    }
}
