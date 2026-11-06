//! Newer images: asked of the registries every few hours, taken by hand or
//! on a schedule. Projects are updated as a whole (pulled and brought up);
//! apps of catalogs are the App Center's, containers outside a project are
//! only pulled for: they would have to be made anew by whoever made them.

use std::collections::BTreeMap;
use std::sync::PoisonError;

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, AuditEntry, Outcome, RequestContext};

use super::projects::Deployment;
use super::{Containers, ContainersError, job_error};
use crate::domain::{ImageCheck, Origin, RegistryImage, UpdateReport, UpdateSettings, UpdateState, is_newer};

impl Containers {
    pub async fn update_settings(&self) -> Result<UpdateSettings, ContainersError> {
        Ok(self.ports.updates.settings().await?)
    }

    pub async fn set_update_settings(
        &self,
        settings: UpdateSettings,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<UpdateSettings, ContainersError> {
        let settings = settings.checked().map_err(|e| ContainersError::Invalid(e.to_string()))?;
        self.ports.updates.save(&settings).await?;
        self.audit("containers.updates_settings", Outcome::Success, "image updates", actor, context).await;
        Ok(settings)
    }

    /// The newest check, if there was one since zendiloosd started.
    pub fn update_report(&self) -> Option<UpdateReport> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Asks the registries about every image the containers run by tag.
    pub async fn check_updates(&self, context: &RequestContext) -> Result<UpdateReport, ContainersError> {
        let containers = self.ports.engine.containers(context).await?;
        let images = self.ports.engine.images(context).await?;
        let mut used: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        for container in &containers {
            let entry =
                used.entry(container.image.clone()).or_insert_with(|| (container.image_id.clone(), vec![]));
            entry.1.push(container.name.clone());
        }
        let mut checks = Vec::new();
        for (reference, (image_id, mut names)) in used {
            // Pinned by digest: nothing newer can come under it.
            let Some(image) = RegistryImage::parse(&reference) else { continue };
            names.sort();
            let local =
                images.iter().find(|i| i.id == image_id).map(|i| i.digests.clone()).unwrap_or_default();
            let state = match self.ports.registries.digest(&image).await {
                Ok(remote) => is_newer(&image, &remote, &local),
                Err(reason) => UpdateState::Unknown(reason),
            };
            checks.push(ImageCheck { reference, state, containers: names });
        }
        let report = UpdateReport { checked_at: self.now(), images: checks };
        *self.report.lock().unwrap_or_else(PoisonError::into_inner) = Some(report.clone());
        Ok(report)
    }

    /// Checks, then updates every project with a newer image, in a job;
    /// none if there is nothing to take. `actor`: none on a schedule.
    pub async fn apply_updates(
        &self,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) -> Result<Option<Job>, ContainersError> {
        let report = self.check_updates(context).await?;
        let containers = self.ports.engine.containers(context).await?;
        let newer: Vec<String> = report.available().flat_map(|image| image.containers.clone()).collect();
        // An administrator's project comes up as they confirmed it, never
        // from its file in the share.
        let projects: Vec<(String, Deployment)> = self
            .projects(context)
            .await?
            .into_iter()
            .filter(|p| p.containers.iter().any(|c| newer.contains(&c.name)))
            .filter_map(|p| match p.origin {
                Origin::Own(own) => Some((p.name, Deployment::of(&own))),
                Origin::Foreign { .. } => Some((p.name, Deployment::Foreign)),
                Origin::App { .. } => None,
            })
            .collect();
        // Images of containers outside any project: pulled only.
        let pulls: Vec<String> = report
            .available()
            .filter(|image| containers.iter().any(|c| c.project.is_none() && c.image == image.reference))
            .map(|image| image.reference.clone())
            .collect();
        if projects.is_empty() && pulls.is_empty() {
            return Ok(None);
        }
        let this = self.clone();
        let (who, context) = (actor.cloned(), context.clone());
        let target = format!("{} projects", projects.len());
        let job = self
            .ports
            .jobs
            .start("containers.update", Some(target), actor, move |progress| async move {
                let engine = &this.ports.engine;
                let steps = (projects.len() + pulls.len()).max(1) as f32;
                let mut failed: Option<ContainersError> = None;
                for (done, (name, how)) in projects.iter().enumerate() {
                    progress.step(name, Some(done as f32 / steps)).await;
                    let outcome = how.run(&this, name, true, &context).await;
                    this.record_update(name, outcome.is_ok(), who.as_ref(), &context).await;
                    if let Err(error) = outcome {
                        failed.get_or_insert(error.into());
                    }
                }
                for reference in &pulls {
                    let outcome = engine.pull_image(reference, &context).await;
                    this.record_update(reference, outcome.is_ok(), who.as_ref(), &context).await;
                    if let Err(error) = outcome {
                        failed.get_or_insert(error.into());
                    }
                }
                if let Err(error) = this.check_updates(&context).await {
                    tracing::warn!(%error, "containers: cannot check for newer images after updating");
                }
                failed.map_or(Ok(()), |error| Err(job_error(&error)))
            })
            .await?;
        Ok(Some(job))
    }

    async fn record_update(&self, target: &str, done: bool, actor: Option<&Actor>, context: &RequestContext) {
        let outcome = if done { Outcome::Success } else { Outcome::Failure };
        let mut entry =
            AuditEntry::new(self.now(), "containers.update", outcome, context).target(target.to_owned());
        if let Some(actor) = actor {
            entry = entry.by(actor);
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, "containers: cannot write the audit log");
        }
    }

    /// Run every minute: updates when the schedule says so, by the
    /// machine's local time.
    pub async fn run_schedule(&self) -> Result<Option<Job>, ContainersError> {
        let now = self.now();
        let local = now.unix_seconds() + i64::from(self.ports.clock.utc_offset(now));
        // 1 January 1970 was a Thursday, the fourth day from Monday.
        let weekday = usize::try_from((local.div_euclid(86_400) + 3).rem_euclid(7)).unwrap_or(0);
        let minute = u16::try_from(local.rem_euclid(86_400) / 60).unwrap_or(0);
        self.scheduled(weekday, minute).await
    }

    /// Run every few hours: asks the registries, if that is wanted.
    pub async fn check_if_wanted(&self) -> Result<(), ContainersError> {
        if self.ports.updates.settings().await?.check {
            self.check_updates(&RequestContext::default()).await?;
        }
        Ok(())
    }

    /// With the machine's local weekday (0 is Monday) and minute of the
    /// day: updates when the schedule says so.
    pub async fn scheduled(&self, weekday: usize, minute: u16) -> Result<Option<Job>, ContainersError> {
        let settings = self.ports.updates.settings().await?;
        match settings.schedule {
            Some(schedule) if settings.check && schedule.due(weekday, minute) => {
                self.apply_updates(None, &RequestContext::default()).await
            }
            _ => Ok(None),
        }
    }
}
