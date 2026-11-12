//! Backups of a VM into a share or onto a removable drive, and restores as
//! a new VM or over the VM a backup came from. Each runs as a job with its
//! progress, can be canceled (the agent then stops its unit), and keeps
//! the VM from being changed, deleted or snapshotted on schedule
//! meanwhile. A backup is recorded once written; it stays recorded when its
//! VM goes, to be restored as a new one.

use std::sync::{Arc, PoisonError};

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, Outcome, RequestContext};

use super::{Busy, HostError, JobSteps, Progress, Vms, VmsError, job_error, outcome};
use crate::domain::{
    self, BackupPlace, BackupTarget, Location, Network, NewVmBackup, PowerState, RestoreAs, Vm, VmBackup,
    VmSettings,
};

/// A backup, and whether the VM it came from is still there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmBackupView {
    pub backup: VmBackup,
    pub vm_exists: bool,
}

/// What a canceled job says.
fn canceled() -> JobError {
    JobError::new("canceled", Some("canceled by an administrator".to_owned()))
}

/// The steps a job's host work reports, forwarded to the job in order;
/// awaiting the handle waits until all are.
fn forward(progress: &Progress) -> (JobSteps, tokio::task::JoinHandle<()>) {
    let (sender, mut reported) = tokio::sync::mpsc::unbounded_channel::<(String, Option<u8>)>();
    let progress = progress.clone();
    let forwarded = tokio::spawn(async move {
        while let Some((step, percent)) = reported.recv().await {
            progress.step(&step, percent.map(|p| f32::from(p.min(100)) / 100.0)).await;
        }
    });
    (JobSteps(sender), forwarded)
}

impl Vms {
    /// Every backup, newest first.
    pub async fn backups(&self) -> Result<Vec<VmBackupView>, VmsError> {
        let vms = self.ports.repository.list().await?;
        Ok(self
            .ports
            .backups
            .list()
            .await?
            .into_iter()
            .map(|backup| VmBackupView { vm_exists: vms.iter().any(|vm| vm.id == backup.vm), backup })
            .collect())
    }

    async fn backup(&self, id: i64) -> Result<VmBackup, VmsError> {
        self.ports.backups.find(id).await?.ok_or(VmsError::NotFound)
    }

    /// The backup with its share where it is now, renamed or moved since;
    /// `None` for a share that is gone.
    async fn located(&self, mut backup: VmBackup) -> Result<Option<VmBackup>, VmsError> {
        if let BackupPlace::Share { share_id, .. } = backup.place {
            let Some(share) = self.ports.shares.locate(share_id).await? else { return Ok(None) };
            backup.place = BackupPlace::Share { share_id, share: share.name, volume: share.volume };
        }
        Ok(Some(backup))
    }

    /// Where a backup is to go, found.
    async fn place(&self, target: &BackupTarget) -> Result<BackupPlace, VmsError> {
        match target {
            BackupTarget::Share { share_id } => {
                let share = self.ports.shares.locate(*share_id).await?.ok_or(VmsError::NotFound)?;
                Ok(BackupPlace::Share { share_id: *share_id, share: share.name, volume: share.volume })
            }
            BackupTarget::Drive { drive } => {
                let fine = !drive.is_empty()
                    && drive.len() <= 64
                    && drive.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
                if !fine {
                    return Err(domain::Invalid("there is no such drive".into()).into());
                }
                Ok(BackupPlace::Drive { drive: drive.clone() })
            }
        }
    }

    /// The VM's work begins: it is busy, and a cancel reaches the job.
    fn begin_work(&self, id: &str, what: &'static str) -> Result<(Busy, Arc<tokio::sync::Notify>), VmsError> {
        let busy = self.take_busy(id, what)?;
        let cancel = Arc::new(tokio::sync::Notify::new());
        self.work.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned(), Arc::clone(&cancel));
        Ok((busy, cancel))
    }

    fn end_of_work(&self, id: &str) {
        self.work.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
    }

    /// Cancels the backup or restore the VM is in: its job ends as
    /// canceled, and the agent stops its unit.
    pub async fn cancel_work(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        if !domain::is_vm_id(id) {
            return Err(VmsError::NotFound);
        }
        let cancel = self.work.lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned();
        let Some(cancel) = cancel else {
            return Err(VmsError::Refused("no backup or restore of the virtual machine runs".into()));
        };
        cancel.notify_one();
        let name = self.ports.repository.find(id).await?.map_or_else(|| id.to_owned(), |vm| vm.settings.name);
        self.audit("vms.backup_cancel", Outcome::Success, &name, actor, context).await;
        Ok(())
    }

    /// Backs the VM up in a job: a snapshot of its disks taken (it may
    /// run; the backup is then as consistent as that snapshot), each disk
    /// read from it, its UEFI variables and TPM, its settings; recorded once
    /// written.
    pub async fn back_up(
        &self,
        id: &str,
        target: BackupTarget,
        folder: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, VmsError> {
        let vm = self.ready(id).await?;
        let folder = domain::backup_folder(folder)?;
        let place = self.place(&target).await?;
        let (busy, cancel) = self.begin_work(&vm.id, "backed up")?;
        let this = self.clone();
        let (who, context_owned) = (actor.clone(), context.clone());
        let started = self
            .ports
            .jobs
            .start("vms.backup", Some(vm.settings.name.clone()), Some(actor), move |progress| async move {
                let _busy = busy;
                progress.step("snapshotting", Some(0.0)).await;
                let now = this.ports.clock.now();
                let taken = this.ports.host.snapshots(&context_owned).await.unwrap_or_default();
                let mine: Vec<_> = taken.into_iter().filter(|snapshot| snapshot.vm == vm.id).collect();
                let label = this.free_label(&mine);
                let stem = domain::backup_stem(now);
                let (steps, forwarded) = forward(&progress);
                let host = &this.ports.host;
                let written = tokio::select! {
                    written = host.back_up(&vm, &place, &folder, &stem, &label, &steps, &context_owned) => Some(written),
                    () = cancel.notified() => None,
                };
                drop(steps);
                let _ = forwarded.await;
                let done = match written {
                    None => {
                        if let Err(error) = host.end_work(&vm.id, &context_owned).await {
                            tracing::warn!(%error, vm = vm.settings.name, "vms: the canceled backup's unit may still run");
                        }
                        Err(canceled())
                    }
                    Some(Err(error)) => Err(job_error(error)),
                    Some(Ok(written)) => {
                        let backup = NewVmBackup {
                            vm: vm.id.clone(),
                            vm_name: vm.settings.name.clone(),
                            place: place.clone(),
                            folder: written.folder,
                            taken_at: now,
                            consistency: written.consistency,
                            files: written.files,
                            settings: vm.settings.clone(),
                        };
                        this.ports.backups.create(&backup).await.map(|_| ()).map_err(|error| {
                            JobError::new("agent_failed", Some(format!("written, but not recorded: {error}")))
                        })
                    }
                };
                this.end_of_work(&vm.id);
                this.audit("vms.backup", outcome(&done), &vm.settings.name, &who, &context_owned).await;
                done
            })
            .await;
        started.map_err(|error| {
            self.end_of_work(id);
            error.into()
        })
    }

    /// The settings a backup's VM gets back: what it had, as the machine
    /// can have it now. Images no longer in the library leave their drives
    /// empty, cards on networks it no longer has go through the NAS, and
    /// the host's devices are not given again.
    fn restored_settings(
        &self,
        backup: &VmBackup,
        images: &[String],
        limits: &domain::HostLimits,
    ) -> VmSettings {
        let mut settings = backup.settings.clone();
        for drive in &mut settings.drives {
            if drive.image.as_ref().is_some_and(|image| !images.contains(image)) {
                drive.image = None;
            }
        }
        for nic in &mut settings.nics {
            let there = match &nic.network {
                Network::User => true,
                Network::Bridge(bridge) => limits.bridges.contains(bridge),
                Network::Direct(card) => limits.cards.contains(card),
                Network::Internal(bridge) => limits.internal.contains(bridge),
            };
            if !there {
                nic.network = Network::User;
            }
        }
        settings.usb.clear();
        settings.pci.clear();
        settings
    }

    /// Restores a backup in a job: as a new VM, or over the VM it came
    /// from, which must be shut off (its name typed again); a disk it has
    /// that the backup has not goes then. Each file is checked against the
    /// backup's record while it is read; should anything fail, a new VM
    /// goes again, and the old one's disks return to what they were.
    pub async fn restore(
        &self,
        backup_id: i64,
        how: RestoreAs,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(String, Job), VmsError> {
        let backup = self.located(self.backup(backup_id).await?).await?.ok_or(VmsError::NotFound)?;
        let (facts, library) = self.storage(context).await?;
        let _library = self.library.lock().await;
        let images = self.ready_images().await?;
        let mut settings = self.restored_settings(&backup, &images, &facts.limits);
        let image_ids: Vec<&str> = images.iter().map(String::as_str).collect();
        let existing = self.ports.repository.list().await?;
        let (vm, over, remove) = match how {
            RestoreAs::New { name, pool } => {
                let taken: Vec<&str> = existing.iter().map(|vm| vm.settings.name.as_str()).collect();
                let macs: std::collections::BTreeSet<String> = existing
                    .iter()
                    .flat_map(|vm| vm.settings.nics.iter().map(|nic| nic.mac.clone()))
                    .collect();
                settings.name = name;
                for nic in &mut settings.nics {
                    nic.mac.clear();
                }
                for disk in &mut settings.disks {
                    disk.location = Location::named(&pool);
                }
                settings.place_disks(None, &facts.limits.locations)?;
                settings.give_macs(&[], &macs, || self.ports.ids.new_mac());
                settings.name = settings.check(&facts.limits, &taken, &image_ids, None)?;
                let vm = Vm {
                    id: self.new_id().await?,
                    settings,
                    created_at: self.ports.clock.now(),
                    ready: false,
                };
                if !self.ports.repository.create(&vm).await? {
                    return Err(VmsError::NameTaken);
                }
                (vm, false, Vec::new())
            }
            RestoreAs::Over { confirm } => {
                let old = self.ready(&backup.vm).await?;
                if check_confirm(&confirm) != Some(old.settings.name.clone()) {
                    return Err(VmsError::ConfirmMismatch);
                }
                let others: Vec<&Vm> = existing.iter().filter(|vm| vm.id != old.id).collect();
                let taken: Vec<&str> = others.iter().map(|vm| vm.settings.name.as_str()).collect();
                let macs: std::collections::BTreeSet<String> =
                    others.iter().flat_map(|vm| vm.settings.nics.iter().map(|nic| nic.mac.clone())).collect();
                settings.name = old.settings.name.clone();
                settings.autostart = old.settings.autostart;
                // A disk the VM has keeps its place and size if larger.
                for disk in &mut settings.disks {
                    match old.settings.disks.iter().find(|had| had.slot == disk.slot) {
                        Some(had) => {
                            disk.location = had.location.clone();
                            disk.size_gib = disk.size_gib.max(had.size_gib);
                        }
                        None => disk.location = Location::named(""),
                    }
                }
                settings.place_disks(Some(&old.settings), &facts.limits.locations)?;
                // Its cards' addresses as they were, unless another VM took one since.
                for nic in &mut settings.nics {
                    if macs.contains(&nic.mac) {
                        nic.mac.clear();
                    }
                }
                settings.give_macs(&[], &macs, || self.ports.ids.new_mac());
                settings.check(&facts.limits, &taken, &image_ids, Some(&old.settings))?;
                let remove = settings.removed_disks(&old.settings);
                (Vm { settings, ..old }, true, remove)
            }
        };
        if over {
            let states = self.ports.host.states(context).await?;
            if states.get(&vm.id) != Some(&PowerState::ShutOff) {
                return Err(VmsError::Running);
            }
        }
        drop(_library);
        let (busy, cancel) = match self.begin_work(&vm.id, "restored") {
            Ok(work) => work,
            Err(error) => {
                if !over {
                    let _ = self.ports.repository.delete(&vm.id).await;
                }
                return Err(error);
            }
        };
        let this = self.clone();
        let (who, context_owned, restored) = (actor.clone(), context.clone(), vm.clone());
        let started = self
            .ports
            .jobs
            .start("vms.restore", Some(vm.settings.name.clone()), Some(actor), move |progress| async move {
                let _busy = busy;
                progress.step("restoring", Some(0.0)).await;
                let label = {
                    let taken = this.ports.host.snapshots(&context_owned).await.unwrap_or_default();
                    let mine: Vec<_> = taken.into_iter().filter(|snapshot| snapshot.vm == restored.id).collect();
                    this.free_label(&mine)
                };
                let (steps, forwarded) = forward(&progress);
                let host = &this.ports.host;
                let ran = tokio::select! {
                    ran = host.restore(&restored, library.volume, &backup, over, &remove, &label, &steps, &context_owned) => Some(ran),
                    () = cancel.notified() => None,
                };
                drop(steps);
                let _ = forwarded.await;
                let done: Result<(), JobError> = match ran {
                    Some(Ok(())) => {
                        let recorded = if over {
                            this.ports.repository.update(&restored).await.map(|_| ())
                        } else {
                            this.ports.repository.set_ready(&restored.id).await
                        };
                        recorded.map_err(HostError::Port).map_err(job_error)
                    }
                    Some(Err(error)) => Err(job_error(error)),
                    None => {
                        if let Err(error) = host.end_work(&restored.id, &context_owned).await {
                            tracing::warn!(%error, vm = restored.settings.name, "vms: the canceled restore's unit may still run");
                        }
                        Err(canceled())
                    }
                };
                if done.is_err() && !over {
                    this.undo(&restored, &context_owned).await;
                }
                this.end_of_work(&restored.id);
                let target = format!("{} ({})", restored.settings.name, backup.vm_name);
                this.audit("vms.restore", outcome(&done), &target, &who, &context_owned).await;
                done
            })
            .await;
        match started {
            Ok(job) => Ok((vm.id, job)),
            Err(error) => {
                self.end_of_work(&vm.id);
                if !over {
                    let _ = self.ports.repository.delete(&vm.id).await;
                }
                Err(error.into())
            }
        }
    }

    /// Removes a backup: its files, then its record; only the record if its
    /// share is gone.
    pub async fn delete_backup(
        &self,
        backup_id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        let backup = self.backup(backup_id).await?;
        // With its share gone, so are its files: only the record is left.
        let removed = match self.located(backup.clone()).await? {
            Some(located) => self.ports.host.remove_backup(&located, context).await,
            None => Ok(()),
        };
        self.audit("vms.backup_delete", outcome(&removed), &backup.vm_name, actor, context).await;
        removed?;
        self.ports.backups.delete(backup.id).await?;
        Ok(())
    }
}

/// A name typed again, as names are kept.
fn check_confirm(confirm: &str) -> Option<String> {
    domain::check_name(confirm).ok()
}
