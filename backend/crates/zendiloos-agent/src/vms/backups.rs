//! Backups of a VM into a share or onto a removable drive, and restores
//! (docs/architecture.md, 4.13). The agent checks what is asked, takes the
//! snapshot a backup reads, makes its devices show (`snapdev=visible`) and
//! runs the work in a transient unit of its own (this binary as
//! `back-up-vm`, `restore-vm` or `remove-vm-backup`, see
//! [`crate::vm_backup_unit`]), outside its sandbox: the whole system read
//! only but the share or drive (a backup) or libvirt's folders of the UEFI
//! variables and TPM states (a restore), no device but the zvols, read
//! only for a backup, and only the capabilities its work needs.
//!
//! Each unit has a name of its own (`zendiloos-vm-backup-<id>`,
//! `zendiloos-vm-restore-<id>`), so systemd refuses a second one for the
//! same VM, and the agent answers only once it ended: should the client
//! hang up (zendiloosd cancels the job) or the agent be asked to stop, it
//! stops the unit and waits for it first, as for Docker's volumes. A VM
//! whose unit still runs is neither changed, nor deleted, nor restored.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use zendiloos_agent_proto::{
    BackupRoot, BackupStem, Response, SharePath, SnapshotLabel, SnapshotNote, VmBackupFile, VmBackupPart,
    VmBackupSettings, VmDiskSlot, VmId, VmSpec,
};
use zendiloos_exec::{ExecError, Limits, Surroundings};

use super::{DiskPlan, QUICK, SLOW, Vms, refused, run};
use crate::operations::OperationError;
use crate::vm_backup_unit::{
    BackupDisk, RestoreFile, VmBackupDone, VmBackupJob, VmBackupWork, VmRemoveJob, VmRestoreJob,
};
use crate::volume_archive::{UnitLine, encode};

/// What a backup or restore of a large VM may take in all; zendiloosd
/// waits a little longer.
const WORK_FOR: Duration = Duration::from_secs(12 * 3600);
/// What a unit leaves of that, to be stopped and answered.
const STOP_MARGIN: Duration = Duration::from_secs(10 * 60);
/// `systemctl stop`, which waits for the unit to end.
const STOPPING: Limits = Limits { timeout: Duration::from_secs(3 * 60), max_stdout: 64 * 1024 };
/// How long a snapshot's device is waited for once udev settled.
const DEVICE_WITHIN: Duration = Duration::from_secs(20);
/// A backup reads the disks' snapshots and libvirt's files whoever owns
/// them, and writes the share or drive whoever owns its folders.
const BACKUP_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE";
/// A restore reads the share and writes the zvols and libvirt's files.
const RESTORE_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE CAP_FOWNER CAP_CHOWN";
/// The account swtpm runs as, as libvirt's `swtpm_user` has it by default.
const SWTPM_USER: &str = "tss";
/// Removing a backup's files from a share or drive.
const REMOVE_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE";

/// The unit a backup or restore of `id` runs in.
fn unit_name(work: VmBackupWork, id: &VmId) -> String {
    let word = match work {
        VmBackupWork::BackUp => "backup",
        VmBackupWork::Restore => "restore",
        VmBackupWork::Remove => "remove",
    };
    format!("zendiloos-vm-{word}-{}.service", id.as_str())
}

/// What a unit's step is called for the interface.
fn step_name(step: &str) -> &'static str {
    match step {
        "backing_up" => "backing_up",
        "restoring" => "restoring",
        _ => "working",
    }
}

/// A unit to run: what, where it may write, which devices it may open.
struct Unit {
    work: VmBackupWork,
    name: String,
    writable: Vec<PathBuf>,
    capabilities: &'static str,
    /// `r` or `rw` for the zvols' devices; none: no device.
    zvols: Option<&'static str>,
    description: String,
    until: Instant,
}

impl Vms {
    /// A backup's root: a share mounted, or a removable drive mounted at
    /// its place.
    fn place_root(&self, root: &BackupRoot) -> Result<PathBuf, OperationError> {
        match root {
            BackupRoot::Share { location } => {
                if self.host.share_mounts {
                    zendiloos_fileops::ShareRoot::open(
                        &self.host.volumes,
                        location.volume,
                        &location.share,
                        true,
                    )
                    .map_err(|e| {
                        refused(format!("the share {} cannot be used: {e}", location.share.as_str()))
                    })?;
                }
                Ok(self
                    .host
                    .volumes
                    .join(format!("volume{}", location.volume.get()))
                    .join(location.share.as_str()))
            }
            BackupRoot::Drive { drive } => {
                zendiloos_fileops::ShareRoot::open_removable(
                    &self.host.removable,
                    drive,
                    self.host.share_mounts,
                )
                .map_err(|e| refused(format!("the drive {drive} cannot be used: {e}")))?;
                Ok(self.host.removable.join(drive.as_str()))
            }
        }
    }

    /// The units of `id`'s work that run (starting, running or stopping).
    async fn running_units(&self, id: &VmId) -> Result<Vec<String>, OperationError> {
        let mut running = Vec::new();
        for work in [VmBackupWork::BackUp, VmBackupWork::Restore] {
            let name = unit_name(work, id);
            let args = ["show", "--property=ActiveState", "--value", name.as_str()];
            let output = zendiloos_exec::run(&self.tools.systemctl, args, QUICK).await?;
            if !output.status.success() {
                return Err(OperationError::Failed(format!(
                    "systemd does not say whether the VM's backup runs: {}",
                    output.stderr.trim()
                )));
            }
            let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !matches!(state.as_str(), "inactive" | "failed" | "") {
                running.push(name);
            }
        }
        Ok(running)
    }

    /// Refused while a backup or restore of the VM still runs.
    pub(super) async fn not_busy(&self, id: &VmId) -> Result<(), OperationError> {
        let running = self.running_units(id).await?;
        if running.is_empty() {
            return Ok(());
        }
        Err(OperationError::Busy(format!(
            "a backup or restore of the virtual machine {} still runs ({})",
            id.as_str(),
            running.join(", ")
        )))
    }

    async fn stop_unit(&self, name: &str) {
        match zendiloos_exec::run(&self.tools.systemctl, ["stop", name], STOPPING).await {
            Ok(output) if output.status.success() => tracing::info!(unit = name, "a VM's unit was stopped"),
            Ok(output) => {
                tracing::warn!(unit = name, complaint = %output.stderr.trim(), "cannot stop a VM's unit")
            }
            Err(error) => tracing::warn!(unit = name, %error, "cannot stop a VM's unit"),
        }
    }

    /// As the agent starts: units of backups and restores nobody waits for
    /// any more (systemd keeps their files while they run) are stopped.
    pub(crate) async fn recover(&self) {
        let units: Vec<String> = std::fs::read_dir(&self.paths.transient)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| {
                        ["zendiloos-vm-backup-", "zendiloos-vm-restore-", "zendiloos-vm-remove-"]
                            .iter()
                            .any(|prefix| name.starts_with(prefix))
                            && name.ends_with(".service")
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for name in units {
            tracing::warn!(unit = name, "a VM's unit nobody waits for any more is stopped");
            self.stop_unit(&name).await;
        }
    }

    /// Stops whatever still works on the VM's backup or restore, and gives
    /// its snapshots' devices back; answers once nothing runs.
    pub(crate) async fn end_work(&self, id: &VmId) -> Result<Response, OperationError> {
        let running = self.running_units(id).await?;
        for name in &running {
            self.stop_unit(name).await;
        }
        let still = self.running_units(id).await?;
        if !still.is_empty() {
            return Err(OperationError::Failed(format!("{} still runs", still.join(", "))));
        }
        for (_, zvol) in self.vm_zvols(id).await.unwrap_or_default() {
            let _ = run(&self.tools.zfs, &["inherit", "snapdev", &zvol], QUICK).await;
        }
        Ok(Response::VmWorkEnded { stopped: !running.is_empty() })
    }

    /// Runs `job` in the unit `unit` says; what it did, as it says.
    async fn run_unit<T: DeserializeOwned>(
        &self,
        unit: Unit,
        job: &impl Serialize,
    ) -> Result<T, OperationError> {
        let left = unit.until.saturating_duration_since(Instant::now());
        let Some(runs_for) = left.checked_sub(STOP_MARGIN).filter(|time| *time >= Duration::from_secs(60))
        else {
            return Err(OperationError::Failed("no time is left for the work".into()));
        };
        let mut args: Vec<String> = [
            format!("--unit={}", unit.name),
            "--wait".into(),
            "--pipe".into(),
            "--collect".into(),
            "--quiet".into(),
            format!("--description=zendiloOS: {}", unit.description),
        ]
        .into();
        args.extend(crate::volumes::SANDBOX.iter().map(|property| (*property).to_owned()));
        if let Some(access) = unit.zvols {
            // The host's devices, of which it may open the zvols alone.
            args.extend([
                "--property=PrivateDevices=no".to_owned(),
                "--property=DevicePolicy=closed".to_owned(),
                format!("--property=DeviceAllow=block-zvol {access}"),
            ]);
        }
        for path in &unit.writable {
            args.push(format!("--property=ReadWritePaths=-{}", path.display()));
        }
        args.extend([
            format!("--property=CapabilityBoundingSet={}", unit.capabilities),
            format!("--property=RuntimeMaxSec={}", runs_for.as_secs()),
            "--property=TimeoutStopSec=60".into(),
            self.host.agent.to_string_lossy().into_owned(),
            unit.work.command().into(),
            encode(job),
        ]);
        let mut said: Option<UnitLine> = None;
        let mut parts = String::new();
        let ran = {
            let mut heard = |line: &str| match serde_json::from_str::<UnitLine>(line) {
                Ok(UnitLine::Progress { step, percent }) => {
                    crate::progress::step(step_name(&step), Some(percent))
                }
                Ok(UnitLine::Part(part)) => parts.push_str(&part),
                Ok(other) => said = Some(other),
                Err(_) => tracing::debug!(line, "a VM's unit wrote"),
            };
            let limits = Limits { timeout: left, max_stdout: 1024 * 1024 };
            let surroundings = Surroundings::default();
            let running =
                zendiloos_exec::run_lines(&self.tools.systemd_run, &args, &surroundings, limits, &mut heard);
            tokio::select! {
                ran = running => Some(ran),
                () = crate::waiting::given_up() => None,
            }
        };
        let output = match ran {
            Some(Ok(output)) => output,
            Some(Err(ExecError::Timeout { .. })) => {
                self.stop_unit(&unit.name).await;
                return Err(OperationError::Failed(format!("{} took too long and was stopped", unit.name)));
            }
            Some(Err(error)) => return Err(error.into()),
            None => {
                self.stop_unit(&unit.name).await;
                return Err(OperationError::Failed(format!(
                    "{} was stopped: it was canceled, or the agent stops",
                    unit.name
                )));
            }
        };
        let unreadable =
            |e: serde_json::Error| OperationError::Failed(format!("the unit's answer cannot be read: {e}"));
        match said {
            Some(UnitLine::Done(_)) if output.status.success() && !parts.is_empty() => {
                serde_json::from_str(&parts).map_err(unreadable)
            }
            Some(UnitLine::Done(value)) if output.status.success() => {
                serde_json::from_value(value).map_err(unreadable)
            }
            Some(UnitLine::Failed { message, refused: true }) => Err(OperationError::Refused(message)),
            Some(UnitLine::Failed { message, .. }) => Err(OperationError::Failed(message)),
            _ => {
                let tail =
                    output.stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
                Err(OperationError::Failed(format!("the unit ended with {}: {tail}", output.status)))
            }
        }
    }

    /// `<nvram>/zendiloos-<id>_VARS.fd`, as libvirt names a domain's copy.
    fn nvram_file(&self, id: &VmId) -> PathBuf {
        self.paths.nvram.join(format!("{}_VARS.fd", id.domain()))
    }

    /// `<swtpm>/<uuid>/tpm2/tpm2-00.permall`, where swtpm keeps the state
    /// of the domain's TPM.
    async fn tpm_file(&self, id: &VmId) -> Result<PathBuf, OperationError> {
        let uuid = self.domain_uuid(id).await?;
        Ok(self.paths.swtpm.join(uuid).join("tpm2").join(VmBackupPart::Tpm.file_name()))
    }

    /// The device of `zvol`'s snapshot `label`, once udev made it.
    async fn snapshot_device(&self, zvol: &str, label: &SnapshotLabel) -> Result<PathBuf, OperationError> {
        let device = self.paths.zvols.join(format!("{zvol}@{}", label.as_str()));
        let _ = zendiloos_exec::run(
            &self.tools.udevadm,
            ["settle", "--timeout=30"],
            Limits { timeout: Duration::from_secs(40), max_stdout: 64 * 1024 },
        )
        .await;
        let until = Instant::now() + DEVICE_WITHIN;
        while !device.exists() {
            if Instant::now() > until {
                return Err(OperationError::Failed(format!("{} did not show", device.display())));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(device)
    }

    /// Backs the VM up into `<folder>/<id>/<stem>` beneath `root`, from a
    /// snapshot `label` of its disks that goes again afterwards.
    pub(crate) async fn back_up(
        &self,
        id: &VmId,
        root: &BackupRoot,
        folder: &SharePath,
        stem: &BackupStem,
        label: &SnapshotLabel,
        settings: &VmBackupSettings,
    ) -> Result<Response, OperationError> {
        let too_long = |_| refused("the folder's path is too long");
        let target =
            folder.join(id.as_str()).and_then(|folder| folder.join(stem.as_str())).map_err(too_long)?;
        let place = self.place_root(root)?;
        let (consistency, zvols) = {
            let _serial = crate::waiting::lock(&self.serial).await?;
            self.require_defined(id).await?;
            self.not_busy(id).await?;
            let note = SnapshotNote::parse("Backup").ok();
            let consistency = self.take_snapshot(id, label, &[], note.as_ref()).await?;
            (consistency, self.vm_zvols(id).await?)
        };
        let outcome = async {
            let mut disks = Vec::new();
            for (slot, zvol) in &zvols {
                run(&self.tools.zfs, &["set", "snapdev=visible", zvol], QUICK).await?;
                let volsize =
                    self.volsize(zvol).await?.ok_or_else(|| refused(format!("there is no zvol {zvol}")))?;
                disks.push(BackupDisk {
                    slot: *slot,
                    device: self.snapshot_device(zvol, label).await?,
                    volsize,
                });
            }
            let tpm = self.tpm_file(id).await.ok();
            let job = VmBackupJob {
                root: place.clone(),
                folder: target.clone(),
                vm: id.as_str().to_owned(),
                disks,
                nvram: Some(self.nvram_file(id)),
                tpm,
                settings: settings.as_str().to_owned(),
                consistency,
            };
            let unit = Unit {
                work: VmBackupWork::BackUp,
                name: unit_name(VmBackupWork::BackUp, id),
                writable: vec![place.clone()],
                capabilities: BACKUP_CAPABILITIES,
                zvols: Some("r"),
                description: format!("back up the virtual machine {}", id.as_str()),
                until: Instant::now() + WORK_FOR,
            };
            self.run_unit::<VmBackupDone>(unit, &job).await
        }
        .await;
        // The snapshot was for the backup alone.
        for (_, zvol) in &zvols {
            let _ = run(&self.tools.zfs, &["inherit", "snapdev", zvol], QUICK).await;
        }
        {
            let _serial = crate::waiting::lock(&self.serial).await?;
            if let Err(error) = self.destroy_snapshots(id, label).await {
                tracing::warn!(%error, vm = id.as_str(), "vms: the backup's snapshot stays");
            }
        }
        let done = outcome?;
        Ok(Response::VmBackedUp { folder: target, files: done.files, manifest: done.manifest, consistency })
    }

    /// Restores the backup's `files` into the VM `spec` describes: a new
    /// one defined first, one there (`over`) returned to snapshot `label`
    /// of its disks should anything fail.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn restore(
        &self,
        spec: &VmSpec,
        root: &BackupRoot,
        folder: &SharePath,
        files: &[VmBackupFile],
        over: bool,
        remove: &[VmDiskSlot],
        label: &SnapshotLabel,
    ) -> Result<Response, OperationError> {
        let place = self.place_root(root)?;
        for file in files {
            if let VmBackupPart::Disk { slot, volsize } = file.part {
                let disk = spec.disks().iter().find(|disk| disk.slot == slot);
                if disk.is_none_or(|disk| disk.size.bytes() < volsize) {
                    return Err(refused(format!(
                        "disk {} of the backup has no place of its size",
                        slot.get()
                    )));
                }
            }
        }
        if remove.iter().any(|slot| spec.disks().iter().any(|disk| disk.slot == *slot)) {
            return Err(refused("a disk is both restored and removed"));
        }
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        self.locations_of(spec).await?;
        self.check_devices(spec).await?;
        self.not_busy(&spec.id).await?;
        // What existed before, to return to; what was made, to destroy.
        let mut kept: Vec<String> = Vec::new();
        let mut made: Vec<String> = Vec::new();
        if over {
            self.require_shut_off(&spec.id).await?;
            if !self.vm_zvols(&spec.id).await?.is_empty() {
                self.take_snapshot(
                    &spec.id,
                    label,
                    &[],
                    SnapshotNote::parse("Before a restore").ok().as_ref(),
                )
                .await?;
            }
            kept = self.vm_zvols(&spec.id).await?.into_iter().map(|(_, zvol)| zvol).collect();
            for disk in spec.disks() {
                let zvol = Self::disk_zvol(spec, disk);
                let plan = match self.volsize(&zvol).await? {
                    None => DiskPlan::Make,
                    Some(size) if size < disk.size.bytes() => DiskPlan::Grow,
                    Some(_) => DiskPlan::Keep,
                };
                let done = match plan {
                    DiskPlan::Keep => Ok(()),
                    DiskPlan::Make => self.make_disk(spec, disk).await.map(|()| made.push(zvol.clone())),
                    DiskPlan::Grow => {
                        run(&self.tools.zfs, &["set", &format!("volsize={}G", disk.size.gib()), &zvol], SLOW)
                            .await
                    }
                };
                if let Err(error) = done {
                    self.undo_restore(label, &kept, &made).await;
                    return Err(error);
                }
            }
        } else {
            if self.defined(&spec.id).await? {
                return Err(OperationError::Conflict);
            }
            for disk in spec.disks() {
                let zvol = Self::disk_zvol(spec, disk);
                if let Err(error) = self.make_disk(spec, disk).await {
                    self.destroy_disks(&made).await;
                    return Err(error);
                }
                made.push(zvol);
            }
            if let Err(error) = self.define_domain(spec, false).await {
                self.destroy_disks(&made).await;
                return Err(error);
            }
        }
        let mut targets = Vec::new();
        for file in files {
            let target = match file.part {
                VmBackupPart::Disk { slot, .. } => {
                    let disk = spec
                        .disks()
                        .iter()
                        .find(|disk| disk.slot == slot)
                        .ok_or_else(|| refused("no disk"))?;
                    self.paths.zvols.join(Self::disk_zvol(spec, disk))
                }
                VmBackupPart::Nvram => self.nvram_file(&spec.id),
                VmBackupPart::Tpm if spec.tpm() => match self.tpm_file(&spec.id).await {
                    Ok(path) => path,
                    Err(_) if over => continue,
                    Err(error) => return Err(error),
                },
                VmBackupPart::Tpm => continue,
            };
            // swtpm runs as its own account (`tss`), which must own its state.
            let owner = (file.part == VmBackupPart::Tpm)
                .then(|| std::fs::read_to_string(&self.paths.passwd).ok())
                .flatten()
                .and_then(|passwd| crate::passwd::lookup(&passwd, SWTPM_USER));
            targets.push(RestoreFile { file: file.clone(), target, owner });
        }
        let job = VmRestoreJob { root: place.clone(), folder: folder.clone(), files: targets };
        let unit = Unit {
            work: VmBackupWork::Restore,
            name: unit_name(VmBackupWork::Restore, &spec.id),
            writable: vec![self.paths.nvram.clone(), self.paths.swtpm.clone()],
            capabilities: RESTORE_CAPABILITIES,
            zvols: Some("rw"),
            description: format!("restore the virtual machine {}", spec.id.as_str()),
            until: Instant::now() + WORK_FOR,
        };
        let restored: Result<serde_json::Value, OperationError> = self.run_unit(unit, &job).await;
        if let Err(error) = restored {
            if over {
                self.undo_restore(label, &kept, &made).await;
            }
            // A new VM goes as zendiloosd removes what its making left.
            return Err(error);
        }
        if over {
            self.define_domain(spec, true).await?;
            if files.iter().any(|file| file.part == VmBackupPart::Nvram) {
                let _ = std::fs::remove_file(self.reset_marker(&spec.id));
            }
            for slot in remove {
                for zvol in self.zvols_in(&spec.id, *slot).await? {
                    run(&self.tools.zfs, &["destroy", "-r", &zvol], SLOW).await?;
                }
            }
            if !kept.is_empty()
                && let Err(error) = self.destroy_snapshots(&spec.id, label).await
            {
                tracing::warn!(%error, vm = spec.id.as_str(), "vms: the snapshot from before the restore stays");
            }
        }
        Ok(Response::Done)
    }

    /// A restore over a VM that failed: its disks back to the snapshot
    /// from before, what it made gone.
    async fn undo_restore(&self, label: &SnapshotLabel, kept: &[String], made: &[String]) {
        for zvol in kept {
            let snapshot = format!("{zvol}@{}", label.as_str());
            if let Err(error) = run(&self.tools.zfs, &["rollback", &snapshot], SLOW).await {
                tracing::error!(%error, zvol, "vms: a disk did not return to what it was before the restore");
            }
            let _ = run(&self.tools.zfs, &["destroy", &snapshot], SLOW).await;
        }
        self.destroy_disks(made).await;
    }

    /// Removes a backup's files and its folder once empty, in a unit.
    pub(crate) async fn remove_backup(
        &self,
        root: &BackupRoot,
        folder: &SharePath,
        files: &[VmBackupPart],
    ) -> Result<Response, OperationError> {
        let place = self.place_root(root)?;
        let mut random = [0u8; 8];
        rustix::rand::getrandom(&mut random, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| OperationError::Failed(format!("no random name: {e}")))?;
        let unit = Unit {
            work: VmBackupWork::Remove,
            name: format!("zendiloos-vm-remove-{:016x}.service", u64::from_le_bytes(random)),
            writable: vec![place.clone()],
            capabilities: REMOVE_CAPABILITIES,
            zvols: None,
            description: "remove a backup of a virtual machine".to_owned(),
            until: Instant::now() + Duration::from_secs(30 * 60),
        };
        let job = VmRemoveJob { root: place, folder: folder.clone(), files: files.to_vec() };
        let _removed: bool = self.run_unit(unit, &job).await?;
        Ok(Response::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_vm_has_a_unit_of_its_own_for_its_backup_and_its_restore() {
        let id = VmId::parse("0a1b2c3d4e5f").unwrap();
        assert_eq!(unit_name(VmBackupWork::BackUp, &id), "zendiloos-vm-backup-0a1b2c3d4e5f.service");
        assert_eq!(unit_name(VmBackupWork::Restore, &id), "zendiloos-vm-restore-0a1b2c3d4e5f.service");
        assert_eq!(step_name("restoring"), "restoring");
        assert_eq!(step_name("other"), "working");
    }
}
