//! The recorded backups of volumes as a whole (docs/architecture.md, 4.9):
//! listed by volume, also of volumes that are gone, removed by hand, plans
//! of gone volumes resumed, and backups found again in a folder recorded,
//! for when the database lost them.

use std::collections::{BTreeMap, BTreeSet};

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};

use super::backups::{JobSteps, PlanView};
use super::ports::Steps;
use super::{Containers, ContainersError, job_error};
use crate::domain::{Backup, BackupOutcome, is_share_folder};

/// The recorded backups of one volume, and its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGroup {
    pub volume: String,
    /// A volume of this name is there now.
    pub exists: bool,
    pub plan: Option<PlanView>,
    /// The newest first.
    pub backups: Vec<Backup>,
}

/// What an import recorded, passed over and refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub recorded: Vec<String>,
    pub known: usize,
    pub refused: Vec<(String, String)>,
}

impl Containers {
    /// Every volume with recorded backups or a plan, by name, also those
    /// gone, with their backups.
    pub async fn backup_groups(&self, context: &RequestContext) -> Result<Vec<BackupGroup>, ContainersError> {
        let volumes: BTreeSet<String> =
            self.ports.engine.volumes(context).await?.into_iter().map(|volume| volume.name).collect();
        let plans = self.ports.backups.plans().await?;
        let mut groups: BTreeMap<String, BackupGroup> = BTreeMap::new();
        let group = |volume: &str| BackupGroup {
            volume: volume.to_owned(),
            exists: volumes.contains(volume),
            plan: None,
            backups: Vec::new(),
        };
        for backup in self.ports.backups.backups(None).await? {
            groups.entry(backup.volume.clone()).or_insert_with(|| group(&backup.volume)).backups.push(backup);
        }
        for plan in plans {
            let volume = plan.volume.clone();
            groups.entry(volume.clone()).or_insert_with(|| group(&volume)).plan = Some(self.plan_view(plan));
        }
        Ok(groups.into_values().collect())
    }

    /// Removes backup `id`: its archive and manifest, each only while it is
    /// still the file written, and its record; a run that failed only
    /// loses its record. Not while it runs, nor while any backup or restore
    /// is under way (one may be reading it).
    pub async fn remove_backup(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let backup = self.ports.backups.backup(id).await?.ok_or(ContainersError::NotFound)?;
        if backup.outcome == BackupOutcome::Running {
            return Err(ContainersError::VolumeBusy(backup.volume));
        }
        self.no_volume_work()?;
        let target =
            backup.written().map_or_else(|| backup.volume.clone(), |written| written.archive.path.clone());
        let done = async {
            if let Some(written) = backup.written() {
                // The share went, and the files with it.
                if let Some(place) = self.ports.backup_shares.locate(backup.share_id).await? {
                    self.remove_files(&place, written, context).await?;
                }
            }
            Ok(self.ports.backups.forget(id).await?)
        }
        .await;
        self.audited("containers.volume_backup_remove", &target, actor, context, done).await
    }

    /// Resumes the paused plan of `volume`, once a volume of the name is
    /// there again: runs are due from now on, and what the plan keeps
    /// counts its new backups and the old ones alike.
    pub async fn resume_backup_plan(
        &self,
        volume: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<PlanView, ContainersError> {
        let mut plan = self.backup_plan(volume).await?.ok_or(ContainersError::NotFound)?.plan;
        let found = self.find_volume(volume, context).await.map_err(|error| match error {
            ContainersError::NotFound => {
                ContainersError::Refused(format!("there is no volume {volume} to back up"))
            }
            other => other,
        })?;
        if !found.backed_up_here() {
            return Err(ContainersError::Refused(format!("{volume} is not backed up here")));
        }
        let now = self.now();
        let done = self.ports.backups.pause_plan(volume, false, now).await.map_err(Into::into);
        self.audited("containers.backup_plan_resume", volume, actor, context, done).await?;
        (plan.paused, plan.since) = (false, now);
        Ok(self.plan_view(plan))
    }

    /// Records the backups found in `folder` of share `share_id`, a share
    /// `actor` may write, in a job: archive and manifest pairs only root
    /// may write, the archive's SHA-256 the manifest's. Those recorded
    /// already are passed over; what is refused is listed with why on the
    /// job. They belong to no plan, and retention never removes them.
    pub async fn import_backups(
        &self,
        share_id: i64,
        folder: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let folder = folder.trim_matches('/').to_owned();
        if !is_share_folder(&folder) {
            return Err(ContainersError::Invalid(format!("{folder:?} is no folder of a share")));
        }
        let places = self.ports.storage.places(actor).await?;
        if !places.iter().any(|place| place.share_id == share_id) {
            return Err(ContainersError::Denied);
        }
        let place = self.ports.backup_shares.locate(share_id).await?.ok_or(ContainersError::NotFound)?;
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let target = format!("{}/{folder}", place.share).trim_end_matches('/').to_owned();
        let job = self
            .ports
            .jobs
            .start(
                "containers.volume_import",
                Some(target.clone()),
                Some(actor),
                move |progress| async move {
                    let steps = JobSteps::new(progress);
                    let done = this.import_now(share_id, &place, &folder, &steps, &context).await;
                    if let Ok(imported) = &done {
                        for path in &imported.recorded {
                            steps.note(&format!("Recorded {path}"));
                        }
                        for (path, why) in &imported.refused {
                            steps.note(&format!("Refused {path}: {why}"));
                        }
                        steps.note(&format!(
                            "{} recorded, {} recorded before, {} refused",
                            imported.recorded.len(),
                            imported.known,
                            imported.refused.len()
                        ));
                    }
                    let done =
                        this.audited("containers.volume_backup_import", &target, &who, &context, done).await;
                    done.map(drop).map_err(|error| job_error(&error))
                },
            )
            .await?;
        Ok(job)
    }

    async fn import_now(
        &self,
        share_id: i64,
        place: &super::ports::SharePlace,
        folder: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Imported, ContainersError> {
        let inside = |path: &str| folder.is_empty() || path.starts_with(&format!("{folder}/"));
        let known: Vec<String> = self
            .ports
            .backups
            .backups(None)
            .await?
            .into_iter()
            .filter(|backup| backup.share_id == share_id)
            .filter_map(|backup| backup.written().map(|written| written.archive.path.clone()))
            .filter(|path| inside(path))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();
        steps.step("scanning", Some(0));
        let scanned = self.ports.archiver.scan(place, folder, &known, steps, context).await?;
        let mut imported = Imported { known: known.len(), refused: scanned.refused, ..Imported::default() };
        for found in scanned.found {
            // Recorded meanwhile (by a backup into the folder, or another
            // import): passed over too.
            if !self.ports.backups.import(share_id, &found).await? {
                imported.known += 1;
                continue;
            }
            imported.recorded.push(found.written.archive.path);
        }
        Ok(imported)
    }
}
