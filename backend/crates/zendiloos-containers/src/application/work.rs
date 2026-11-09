//! Backups and restores of volumes as the rest of this context sees them
//! (docs/architecture.md, 4.9): one at a time on the NAS, each holding the
//! containers it keeps stopped, which nothing else starts, stops, brings
//! up anew or removes meanwhile. Containers they stopped are owed a start,
//! noted durably: started once nothing holds them any more and the agent
//! said the work ended, also after a restart, and tried again every minute
//! until they run (Docker may not be up yet after a power cut).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use zendiloos_jobs::Progress;
use zendiloos_kernel::RequestContext;

use super::ports::Steps;
use super::{Containers, ContainersError};
use crate::domain::{Container, ContainerAction, Holds, Unclaimable, users};

/// The holds, and a word to whoever waits on them whenever they change.
#[derive(Default)]
pub(super) struct Work {
    holds: Mutex<Holds>,
    changed: Notify,
}

impl Work {
    pub(super) fn holds(&self) -> MutexGuard<'_, Holds> {
        self.holds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the holds and tells those waiting.
    pub(super) fn change<T>(&self, change: impl FnOnce(&mut Holds) -> T) -> T {
        let done = change(&mut self.holds());
        self.changed.notify_waiters();
        done
    }

    /// Waits until `ready` says so of the holds; it is asked again whenever
    /// they change.
    async fn until(&self, mut ready: impl FnMut(&mut Holds) -> bool) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if ready(&mut self.holds()) {
                return;
            }
            changed.await;
        }
    }
}

impl From<Unclaimable> for ContainersError {
    fn from(error: Unclaimable) -> Self {
        match error {
            Unclaimable::Busy(volume) => Self::VolumeBusy(volume),
            Unclaimable::Moving => Self::Refused(error.to_string()),
        }
    }
}

/// The volumes `container` mounts.
fn volumes_of(container: &Container) -> Vec<&str> {
    container.mounts.iter().filter(|m| m.kind == "volume").map(|m| m.source.as_str()).collect()
}

impl Containers {
    /// Claims `volume` for a backup or restore: it holds the containers
    /// that use it and run (if `stop_running`), and those another backup or
    /// restore stopped, until it ends. `stops`: the volume's containers must
    /// not run while it works.
    pub(super) async fn claim(
        &self,
        volume: &str,
        stops: bool,
        stop_running: bool,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let containers = if stops { self.ports.engine.containers(context).await? } else { Vec::new() };
        self.work.change(|holds| {
            let held: Vec<String> = users(volume, &containers)
                .into_iter()
                .filter(|user| (user.running && stop_running) || holds.is_stopped(&user.name))
                .map(|user| user.name)
                .collect();
            holds.claim(volume, stops, held)
        })?;
        Ok(())
    }

    /// Ends `volume`'s claim without starting anything: it never ran.
    pub(super) fn unclaim(&self, volume: &str) {
        self.work.change(|holds| holds.release(volume));
    }

    /// Waits for `volume`'s turn: one backup or restore at a time.
    pub(super) async fn turn(&self, volume: &str, steps: &dyn Steps) {
        if self.work.change(|holds| holds.take_turn(volume)) {
            return;
        }
        steps.step("waiting", None);
        self.work.until(|holds| holds.take_turn(volume)).await;
    }

    /// Stops the containers that use `volume` and run, each noted as owed a
    /// start first and held by `volume`'s claim; those it stopped.
    pub(super) async fn stop_users(
        &self,
        volume: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Vec<String>, ContainersError> {
        let running: Vec<Container> = self
            .ports
            .engine
            .containers(context)
            .await?
            .into_iter()
            .filter(|c| c.state.is_up() && volumes_of(c).contains(&volume))
            .collect();
        if running.is_empty() {
            return Ok(Vec::new());
        }
        steps.step("stopping", None);
        let mut stopped = Vec::new();
        for container in running {
            self.ports.backups.owe_start(&container.name, volume, self.now()).await?;
            self.work.change(|holds| holds.stopped(volume, &container.name));
            self.ports.engine.control_container(&container.id, ContainerAction::Stop, context).await?;
            stopped.push(container.name);
        }
        Ok(stopped)
    }

    /// `volume`'s backup or restore ended: its claim goes, and the
    /// containers no other one holds start again if they are owed a start.
    /// `ended`: the agent answered, so its work ended; otherwise it is asked
    /// to end it first, and if it cannot say so, nothing starts until the
    /// minute's look ([`Containers::start_owed_containers`]).
    pub(super) async fn finish(
        &self,
        volume: &str,
        ended: bool,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let free = self.work.change(|holds| holds.release(volume));
        if !ended && let Err(error) = self.ports.archiver.end_work(volume, context).await {
            tracing::warn!(%error, volume, "containers: the agent does not say the work on the volume ended");
            return Err(ContainersError::Failed(format!(
                "the agent does not say the work on {volume} ended; its containers start once it does: {error}"
            )));
        }
        let owed: BTreeSet<String> =
            self.ports.backups.owed_starts().await?.into_iter().map(|owed| owed.container).collect();
        let starting: Vec<String> = free.into_iter().filter(|name| owed.contains(name)).collect();
        if starting.is_empty() {
            return Ok(());
        }
        steps.step("starting", None);
        let failed = self.start_owed(&starting, context).await;
        if failed.is_empty() {
            Ok(())
        } else {
            Err(ContainersError::Failed(format!("could not start again: {}", failed.join(", "))))
        }
    }

    /// Starts each container named, owed a start; one that started, or is
    /// gone, is owed nothing any more. Those that would not start.
    async fn start_owed(&self, names: &[String], context: &RequestContext) -> Vec<String> {
        let mut failed = Vec::new();
        for name in names {
            let started = match self.find_container(name, context).await {
                Ok(container) => self
                    .ports
                    .engine
                    .control_container(&container.id, ContainerAction::Start, context)
                    .await
                    .map_err(ContainersError::from),
                // Removed meanwhile: nothing to start.
                Err(ContainersError::NotFound) => Ok(()),
                Err(error) => Err(error),
            };
            match started {
                Ok(()) => {
                    if let Err(error) = self.ports.backups.started(name).await {
                        tracing::warn!(%error, container = name, "containers: cannot note a start owed as done");
                    }
                    self.work.change(|holds| holds.started(name));
                }
                Err(error) => {
                    tracing::warn!(%error, container = name, "containers: cannot start a container again");
                    failed.push(name.clone());
                }
            }
        }
        failed
    }

    /// Starts the containers owed a start that nothing holds any more: a
    /// start that failed (Docker not up yet after a power cut), or one a
    /// restart of zendiloosd cut off. Only once the agent said nothing works
    /// on their volumes any more, stopping what a restart left. Run at
    /// startup and every minute; how many started.
    pub async fn start_owed_containers(&self) -> usize {
        let owed = match self.ports.backups.owed_starts().await {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(%error, "containers: cannot read the starts owed");
                return 0;
            }
        };
        let mut volumes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for start in owed {
            volumes.entry(start.container).or_default().insert(start.volume);
        }
        // Each container only once nothing holds it and none of its volumes
        // is claimed; each volume asked about once.
        volumes.retain(|container, of| {
            let holds = self.work.holds();
            !holds.is_held(container) && !of.iter().any(|volume| holds.is_claimed(volume))
        });
        let context = RequestContext::default();
        let mut ended: BTreeMap<String, bool> = BTreeMap::new();
        let mut startable = Vec::new();
        for (container, of) in volumes {
            let mut all = true;
            for volume in of {
                if !ended.contains_key(&volume) {
                    let answer = self.ports.archiver.end_work(&volume, &context).await;
                    if let Err(error) = &answer {
                        tracing::info!(%error, volume, "containers: the agent does not say the volume's work ended");
                    }
                    ended.insert(volume.clone(), answer.is_ok());
                }
                all &= ended[&volume];
            }
            if all {
                startable.push(container);
            }
        }
        if startable.is_empty() {
            return 0;
        }
        let failed = self.start_owed(&startable, &context).await;
        startable.len() - failed.len()
    }

    /// The containers of `containers` a backup or restore holds now.
    fn blocked(&self, containers: &[&Container]) -> Vec<String> {
        let holds = self.work.holds();
        containers.iter().filter(|c| holds.blocks(&c.name, &volumes_of(c))).map(|c| c.name.clone()).collect()
    }

    /// Refused while a backup or restore holds `container`.
    pub(super) fn not_held(&self, container: &Container) -> Result<(), ContainersError> {
        let blocked = self.blocked(&[container]);
        if blocked.is_empty() { Ok(()) } else { Err(ContainersError::Held(blocked)) }
    }

    /// Refused while a backup or restore holds a container of `project`.
    pub(super) async fn project_not_held(
        &self,
        project: &str,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let containers = self.ports.engine.containers(context).await?;
        let of: Vec<&Container> =
            containers.iter().filter(|c| c.project.as_deref() == Some(project)).collect();
        let blocked = self.blocked(&of);
        if blocked.is_empty() { Ok(()) } else { Err(ContainersError::Held(blocked)) }
    }

    /// Waits until no backup or restore holds a container of `project`:
    /// bringing it up would start them. Looked at again whenever the holds
    /// change, and every minute.
    pub(super) async fn until_free(
        &self,
        project: &str,
        progress: &Progress,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let mut told = false;
        loop {
            let changed = self.work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.project_not_held(project, context).await.is_ok() {
                return Ok(());
            }
            if !told {
                progress.step("waiting", None).await;
                told = true;
            }
            tokio::select! {
                () = changed => {}
                () = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    }

    /// Refused while a volume is claimed for a backup or restore.
    pub(super) fn no_volume_work(&self) -> Result<(), ContainersError> {
        match self.work.holds().any() {
            Some(volume) => Err(ContainersError::VolumeBusy(volume.to_owned())),
            None => Ok(()),
        }
    }

    /// Docker's data starts moving: refused while a volume is claimed;
    /// none is claimed until it ends.
    pub(super) fn begin_move(&self) -> Result<(), ContainersError> {
        Ok(self.work.change(Holds::begin_move)?)
    }

    pub(super) fn end_move(&self) {
        self.work.change(Holds::end_move);
    }
}
