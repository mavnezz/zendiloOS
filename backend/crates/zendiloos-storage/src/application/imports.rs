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
use zendiloos_kernel::{Actor, Outcome, RequestContext};
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
    /// For a pool the configuration does not name: the name it is imported
    /// under, which must be free; asked for where its own is taken
    /// ([`AbsentPools::names_taken`]).
    pub rename: Option<String>,
}

/// What the start found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Volumes whose mounts were made anew.
    pub mounted: Vec<VolumeNumber>,
    /// Pools the configuration names that are not imported.
    pub absent: Vec<String>,
    /// Pools imported under a name the configuration names, but another
    /// pool than the one it knows by its GUID; told, and offered to take
    /// as the configuration's ([`PoolImports::claim`]).
    pub not_the_configurations: Vec<String>,
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
            Some(_) if asked.rename.is_some() => {
                return Err(StorageError::Refused(
                    "the configuration's pool is imported under its own name".into(),
                ));
            }
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
        // Adopted under a name taken, it would be taken for that pool (the
        // configuration's GUID kept anew, its shares on another pool).
        let rename = match asked.rename.as_deref().map(str::trim) {
            Some(rename) => {
                let rename = PoolName::parse(rename).map_err(domain::PlanError::from)?;
                if absent.names_taken.iter().any(|taken| taken == rename.as_str()) {
                    return Err(StorageError::Plan(domain::PlanError::NameTaken));
                }
                Some(rename)
            }
            None if adopt && absent.names_taken.contains(&found.name) => {
                return Err(StorageError::Plan(domain::PlanError::NameTaken));
            }
            None => None,
        };
        let import = PoolImport {
            name: name.clone(),
            id: found.id,
            volume,
            adopt,
            force: found.other_host,
            rename: rename.clone(),
        };
        let name = rename.unwrap_or(name);
        let observer = Arc::clone(&self.observer);
        let known = Arc::clone(&self.known);
        self.context
            .job(
                "storage.import_pool",
                found.name.clone(),
                actor,
                request,
                move |admin, steps, request| async move {
                    steps.step("importing_pool", None);
                    admin.import_pool(&import, steps.as_ref(), &request).await?;
                    remember(known.as_ref(), name.as_str(), import.id).await;
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
    /// configuration, or is another pool of the name (its GUID not the one
    /// kept), is left alone; the GUID of one the configuration kept none of
    /// is kept.
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
                    "storage: the pool imported is not the one the configuration names"
                );
                if matches!(mismatch, domain::Mismatch::Guid { .. }) {
                    reconciled.not_the_configurations.push(pool.name.clone());
                }
                continue;
            }
            if pool.guid.is_none()
                && let Ok(guid) = found.guid.parse()
            {
                remember(self.known.as_ref(), &pool.name, guid).await;
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
        if !reconciled.not_the_configurations.is_empty() {
            self.notifier
                .notify(
                    NewNotification::new(Level::Warning, "pools_not_the_configurations")
                        .with("count", reconciled.not_the_configurations.len().to_string())
                        .with("names", reconciled.not_the_configurations.join(", ")),
                )
                .await;
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

impl PoolImports {
    /// Takes the pool imported as `name` as the configuration's pool of
    /// that name, though it is another than the one it knows by its GUID
    /// ([`AbsentPools::not_the_configurations`]): its GUID is kept anew and
    /// its mounts are made, as for a pool imported. Only the same volume,
    /// and with `confirm` naming it again. Audited.
    pub async fn claim(
        &self,
        name: &str,
        confirm: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), StorageError> {
        if confirm.trim() != name {
            return Err(StorageError::ConfirmationMismatch);
        }
        let absent = self.absent(request).await?;
        let other = absent
            .not_the_configurations
            .into_iter()
            .find(|other| other.known.name == name)
            .ok_or(StorageError::PoolNotFound)?;
        let imported = self.context.zfs.pools(request).await?;
        let pool = imported.iter().find(|pool| pool.name == name).ok_or(StorageError::PoolNotFound)?;
        if pool.volume != Some(other.known.volume) {
            return Err(StorageError::Refused(format!(
                "{name} is not volume {}, which the configuration names",
                other.known.volume.get()
            )));
        }
        let guid = other.guid.parse().map_err(|_| StorageError::Refused(format!("{name} has no GUID")))?;
        let claimed = async {
            self.known.remember_guid(name, guid).await?;
            let pool_name = PoolName::parse(name).map_err(domain::PlanError::from)?;
            self.context
                .admin
                .mount_volume(&pool_name, other.known.volume, &other.known.shares, request)
                .await?;
            Ok::<(), StorageError>(())
        }
        .await;
        let outcome = if claimed.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.context.audit("storage.claim_pool", outcome, name, actor, request).await;
        claimed?;
        self.observer.volume_back(other.known.volume, request).await;
        Ok(())
    }
}

/// Keeps a pool's GUID, never in place of another the configuration knows
/// for that name (its pool is that one, not this); a failure is only
/// logged: the pool is matched by its name alone until it is kept.
pub(crate) async fn remember(known: &dyn KnownPools, name: &str, guid: u64) {
    let kept = match known.known().await {
        Ok(pools) => pools.into_iter().find(|pool| pool.name == name).and_then(|pool| pool.guid),
        Err(error) => {
            tracing::warn!(pool = name, %error, "storage: cannot keep the pool's GUID");
            return;
        }
    };
    if kept.is_some_and(|kept| kept != guid) {
        tracing::warn!(
            pool = name,
            "storage: the configuration knows another pool of the name; its GUID is kept"
        );
        return;
    }
    if let Err(error) = known.remember_guid(name, guid).await {
        tracing::warn!(pool = name, %error, "storage: cannot keep the pool's GUID");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
    use zendiloos_jobs::{JobState, Jobs};

    use super::*;
    use crate::application::StorageUseCases;
    use crate::domain::{ImportablePool, KnownPool, PlanError};
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
            guid: None,
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
        ImportRequest { name: name.into(), id, volume: None, force: false, confirm: None, rename: None }
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
        assert_eq!(*s.fake.guids.lock().unwrap(), [("tank".to_owned(), Some(77))], "its GUID kept");
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

    /// A configuration from before GUIDs were kept gets the GUID of its
    /// pool imported; one that knows another GUID leaves the pool of the
    /// name alone: after a restore, it may be another pool of that name.
    #[tokio::test]
    async fn the_start_keeps_the_guid_of_a_pool_imported_and_leaves_another_of_its_name_alone() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().push(known("tank", 1, &["photos"]));
        s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(*s.fake.guids.lock().unwrap(), [("tank".to_owned(), Some(9_846_106_214_133_009_000))]);
        assert_eq!(s.fake.calls(), ["mount tank 1 photos"]);

        s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(s.fake.guids.lock().unwrap().len(), 1, "kept once");

        s.fake.known.lock().unwrap()[0].guid = Some(1);
        let reconciled = s.storage.imports.reconcile(&request).await.unwrap();
        assert_eq!(s.fake.calls().len(), 2, "another pool called tank gets no mounts");
        assert_eq!(reconciled.not_the_configurations, ["tank"], "told");
    }

    /// The configuration knows tank by another GUID than the tank imported
    /// (made here before the configuration was restored): offered, and on
    /// a confirmation taken as the configuration's, its GUID kept anew.
    #[tokio::test]
    async fn another_pool_of_the_configurations_name_is_taken_as_its_own_when_confirmed() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().push(KnownPool { guid: Some(1), ..known("tank", 1, &["photos"]) });

        let absent = s.storage.imports.absent(&request).await.unwrap();
        let other: Vec<(&str, &str)> = absent
            .not_the_configurations
            .iter()
            .map(|other| (other.known.name.as_str(), other.guid.as_str()))
            .collect();
        assert_eq!(other, [("tank", "9846106214133009000")]);

        let mistyped = s.storage.imports.claim("tank", "tnak", &alice(), &request).await;
        assert!(matches!(mistyped, Err(StorageError::ConfirmationMismatch)), "{mistyped:?}");
        let unknown = s.storage.imports.claim("cold", "cold", &alice(), &request).await;
        assert!(matches!(unknown, Err(StorageError::PoolNotFound)), "{unknown:?}");
        assert!(s.fake.calls().is_empty());

        s.storage.imports.claim("tank", "tank", &alice(), &request).await.unwrap();
        assert_eq!(*s.fake.guids.lock().unwrap(), [("tank".to_owned(), Some(9_846_106_214_133_009_000))]);
        assert_eq!(s.fake.calls(), ["mount tank 1 photos"]);
        assert_eq!(*s.fake.volumes_back.lock().unwrap(), [1], "Samba and NFS follow");
        let entry = s.audit.entries().pop().unwrap();
        assert_eq!((entry.action, entry.target.as_deref()), ("storage.claim_pool", Some("tank")));
        assert!(s.storage.imports.absent(&request).await.unwrap().not_the_configurations.is_empty());

        // One that is another volume is not the configuration's either.
        s.fake.known.lock().unwrap()[0] = KnownPool { guid: Some(1), ..known("tank", 2, &[]) };
        let other_volume = s.storage.imports.claim("tank", "tank", &alice(), &request).await;
        assert!(matches!(other_volume, Err(StorageError::Refused(_))), "{other_volume:?}");
    }

    /// The disks hold two pools called tank: the configuration's is the
    /// one with its GUID; the other is offered for adopting only.
    #[tokio::test]
    async fn of_two_pools_of_a_name_the_one_with_the_guid_is_imported_as_the_configurations() {
        let s = setup();
        let request = RequestContext::default();
        *s.fake.tank_absent.lock().unwrap() = true;
        s.fake.known.lock().unwrap().push(KnownPool { guid: Some(77), ..known("tank", 1, &["photos"]) });
        s.fake.importable.lock().unwrap().extend([found("tank", 66, false), found("tank", 77, false)]);

        let absent = s.storage.imports.absent(&request).await.unwrap();
        assert_eq!(absent.known.iter().map(|k| k.found.id).collect::<Vec<_>>(), [77]);
        assert_eq!(absent.foreign, [found("tank", 66, false)]);
        assert!(absent.names_taken.contains(&"tank".to_owned()), "the other one needs a name of its own");

        // Under the configuration's name it would take that pool's GUID,
        // and the configuration's pool would be foreign for good.
        let same = s.storage.imports.import(ask("tank", 66), &alice(), &request).await;
        assert!(matches!(same, Err(StorageError::Plan(PlanError::NameTaken))), "{same:?}");
        for taken in ["tank", "-x"] {
            let renamed = ImportRequest { rename: Some(taken.into()), ..ask("tank", 66) };
            let refused = s.storage.imports.import(renamed, &alice(), &request).await;
            assert!(refused.is_err(), "{taken}: {refused:?}");
        }
        assert!(s.fake.calls().is_empty());

        let renamed = ImportRequest { rename: Some("tank2".into()), ..ask("tank", 66) };
        let job = s.storage.imports.import(renamed, &alice(), &request).await.unwrap();
        assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
        assert_eq!(
            s.fake.calls(),
            ["import tank 66 2 adopt=true force=false as tank2", "mount tank2 2 "],
            "adopted under a name of its own, as another volume"
        );
        assert_eq!(*s.fake.guids.lock().unwrap(), [("tank2".to_owned(), Some(66))]);
        assert_eq!(s.fake.known.lock().unwrap()[0].guid, Some(77), "the configuration's pool stays its own");
    }

    /// A new pool never takes the name of one the configuration names that
    /// is not imported: the configuration's would be another pool's then.
    #[tokio::test]
    async fn a_new_pool_never_takes_the_name_of_a_pool_not_imported() {
        let s = setup();
        let request = RequestContext::default();
        s.fake.known.lock().unwrap().push(KnownPool { guid: Some(77), ..known("cold", 2, &[]) });
        let planned = s
            .storage
            .plan_pool
            .execute(
                "cold",
                crate::domain::PoolLayout::Mirror,
                vec!["sdd".into(), "sde".into()],
                &alice(),
                &request,
            )
            .await;
        assert!(matches!(planned, Err(StorageError::Plan(PlanError::NameTaken))), "{planned:?}");
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
