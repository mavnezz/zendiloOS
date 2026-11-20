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
//! whose unit still runs is neither started, changed, deleted nor restored.
//!
//! Neither holds the lock of the VMs' changes while its unit runs, which
//! may take hours: other VMs are changed and snapshotted meanwhile. A
//! restore writes down what undoes it before its first write
//! (`zendiloos-<id>.restoring` beside the domain's XML, with the domain's
//! definition from before) and turns the VM's autostart off: one cut off
//! by a restart of the agent or of the machine is undone as the agent
//! starts again, before anything may start the VM, and libvirt never
//! starts half-restored disks with the machine.

use std::path::PathBuf;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::{
    BackupRoot, BackupStem, Response, SharePath, SnapshotLabel, SnapshotNote, VmBackupFile, VmBackupPart,
    VmBackupSettings, VmDiskSlot, VmId, VmSpec,
};
use zendiloos_exec::{ExecError, Limits, Surroundings};

use super::{Autostart, DiskPlan, QUICK, SLOW, Vms, refused, remove_if_there, run, virsh};
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

/// How long a restore's undoing is waited for once its unit stopped.
const UNDONE_WITHIN: Duration = Duration::from_secs(10 * 60);

/// What undoes a restore, written down before it writes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreMarker {
    /// Over the VM there was; else a new one, which goes again.
    pub over: bool,
    /// The snapshot of the disks there were, which they return to.
    pub label: SnapshotLabel,
    /// Those disks' zvols.
    pub kept: Vec<String>,
    /// The zvols the restore makes, which go again.
    pub made: Vec<String>,
    /// Whether the VM started with the machine before.
    pub autostart: bool,
    /// Whether its UEFI variables were to start afresh at its next start.
    pub reset_nvram: bool,
}

/// A restore under way for a VM: nothing else is done with it until this
/// goes.
struct Restoring<'a> {
    vms: &'a Vms,
    id: VmId,
}

impl Drop for Restoring<'_> {
    fn drop(&mut self) {
        self.vms.restoring.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.id);
    }
}

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
        self.not_restoring_now(id)?;
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

    /// Refused while a restore of the VM is under way.
    fn not_restoring_now(&self, id: &VmId) -> Result<(), OperationError> {
        if self.restoring.lock().unwrap_or_else(PoisonError::into_inner).contains(id) {
            return Err(OperationError::Busy(format!(
                "a restore of the virtual machine {} is under way",
                id.as_str()
            )));
        }
        Ok(())
    }

    /// Refused while a restore of the VM is under way; one cut off before
    /// and not undone yet is undone first. With the lock held.
    pub(super) async fn not_restoring(&self, id: &VmId) -> Result<(), OperationError> {
        self.not_restoring_now(id)?;
        self.undo_cut_off(id).await
    }

    /// Undoes a restore of the VM that was cut off, if one was and is not
    /// undone yet; refused while that fails.
    async fn undo_cut_off(&self, id: &VmId) -> Result<(), OperationError> {
        let Some(marker) = self.read_restore_marker(id) else { return Ok(()) };
        tracing::warn!(vm = id.as_str(), "vms: a restore that was cut off is undone");
        self.undo_restore(id, &marker).await.map_err(|error| {
            refused(format!(
                "a restore of the virtual machine {} was cut off and cannot be undone yet: {error}",
                id.as_str()
            ))
        })
    }

    /// Nothing works on the VM's disks but what asks: refused while a
    /// backup or restore of it runs, and a restore cut off is undone first.
    /// With the lock held.
    pub(super) async fn settled(&self, id: &VmId) -> Result<(), OperationError> {
        self.not_busy(id).await?;
        self.undo_cut_off(id).await
    }

    /// `zendiloos-<id>.restoring`: a restore under way, or cut off.
    pub(super) fn restore_marker(&self, id: &VmId) -> PathBuf {
        self.paths.domains.join(format!("{}.restoring", id.domain()))
    }

    /// The domain's definition from before a restore over it.
    pub(super) fn before_restore_file(&self, id: &VmId) -> PathBuf {
        self.paths.domains.join(format!("{}.xml.before-restore", id.domain()))
    }

    fn read_restore_marker(&self, id: &VmId) -> Option<RestoreMarker> {
        let path = self.restore_marker(id);
        let text = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&text) {
            Ok(marker) => Some(marker),
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "vms: a restore's marker cannot be read");
                None
            }
        }
    }

    /// The restore is not to be undone any more: it is done, or the VM goes.
    pub(super) fn forget_restore(&self, id: &VmId) -> Result<(), OperationError> {
        remove_if_there(&self.restore_marker(id))?;
        remove_if_there(&self.before_restore_file(id))
    }

    /// Undoes a restore that failed or was cut off: a new VM goes with its
    /// disks; one restored over gets its disks back as they were before
    /// (rolled back to the snapshot then, those made destroyed), its
    /// definition from before and its autostart. A disk gone since is
    /// nothing to return. Its UEFI variables and TPM are as they were
    /// unless the unit finished writing them. Once all of it is done, the
    /// marker goes; until then the VM is refused.
    async fn undo_restore(&self, id: &VmId, marker: &RestoreMarker) -> Result<(), OperationError> {
        if !marker.over {
            self.remove_vm(id).await?;
            return self.forget_restore(id);
        }
        let mut failed: Vec<String> = Vec::new();
        let mut rolled_back = Vec::new();
        for zvol in &marker.kept {
            if !self.exists(zvol).await? {
                continue;
            }
            let snapshot = format!("{zvol}@{}", marker.label.as_str());
            match run(&self.tools.zfs, &["rollback", &snapshot], SLOW).await {
                Ok(()) => rolled_back.push(snapshot),
                Err(error) => failed.push(format!("{zvol} did not return: {error}")),
            }
        }
        for zvol in &marker.made {
            if self.exists(zvol).await?
                && let Err(error) = run(&self.tools.zfs, &["destroy", "-r", zvol], SLOW).await
            {
                failed.push(format!("{zvol} stays: {error}"));
            }
        }
        let before = self.before_restore_file(id);
        if before.exists() {
            let path = before.to_string_lossy().into_owned();
            match run(&self.tools.virsh, &virsh(&["define", "--validate", &path]), QUICK).await {
                Ok(()) => {
                    if let Err(error) = std::fs::rename(&before, self.domain_file(id)) {
                        failed.push(format!("the definition from before is not kept: {error}"));
                    }
                }
                Err(error) => {
                    failed.push(format!("libvirt did not take the definition from before: {error}"))
                }
            }
        }
        let reset = self.reset_marker(id);
        let marked =
            if marker.reset_nvram { crate::durable::replace(&reset, b"") } else { remove_if_there(&reset) };
        if let Err(error) = marked {
            failed.push(error.to_string());
        }
        if marker.autostart {
            let domain = id.domain();
            if let Err(error) = run(&self.tools.virsh, &virsh(&["autostart", &domain]), QUICK).await {
                failed.push(format!("its autostart is off: {error}"));
            }
        }
        if !failed.is_empty() {
            // The snapshot from before stays, to return to on the next try.
            return Err(OperationError::Failed(failed.join("; ")));
        }
        self.forget_restore(id)?;
        for snapshot in rolled_back {
            let _ = run(&self.tools.zfs, &["destroy", &snapshot], SLOW).await;
        }
        Ok(())
    }

    /// A restore that failed is undone, or said to be not undone yet.
    async fn give_up_restore(&self, id: &VmId, marker: &RestoreMarker) {
        if let Err(error) = self.undo_restore(id, marker).await {
            tracing::error!(
                %error,
                vm = id.as_str(),
                "vms: a restore that failed is not undone; it is tried again before the VM is used"
            );
        }
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
        // Restores cut off: undone before anything may start their VMs.
        let cut_off: Vec<VmId> = std::fs::read_dir(&self.paths.domains)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        VmId::of_domain(name.strip_suffix(".restoring")?)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if cut_off.is_empty() || !self.installed() {
            return;
        }
        let _serial = self.serial.lock().await;
        for id in cut_off {
            let Some(marker) = self.read_restore_marker(&id) else { continue };
            match self.undo_restore(&id, &marker).await {
                Ok(()) => {
                    tracing::warn!(vm = id.as_str(), over = marker.over, "vms: a restore cut off is undone")
                }
                Err(error) => tracing::error!(
                    %error,
                    vm = id.as_str(),
                    "vms: a restore cut off is not undone yet; it is tried again before the VM is used"
                ),
            }
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
        // A restore whose unit stopped undoes what it did: until it is done
        // the VM is not to be removed or used.
        let until = Instant::now() + UNDONE_WITHIN;
        while self.restoring.lock().unwrap_or_else(PoisonError::into_inner).contains(id) {
            if Instant::now() > until {
                return Err(OperationError::Failed("the restore is still being undone".into()));
            }
            crate::waiting::wait(tokio::time::sleep(Duration::from_millis(200))).await?;
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
        self.device_of(&format!("{zvol}@{}", label.as_str())).await
    }

    /// The device of a zvol or a zvol's snapshot (`/dev/zvol/<name>`), once
    /// udev made it: it shows a moment after the zvol was made, or after its
    /// snapshots were made to show.
    async fn device_of(&self, name: &str) -> Result<PathBuf, OperationError> {
        let device = self.paths.zvols.join(name);
        if device.exists() {
            return Ok(device);
        }
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

    /// The zvols a backup reads: one for each disk the VM is recorded with
    /// (`disks`, as zendiloosd records it), refused if one has none or two.
    /// A zvol of the VM's that the record has not is refused while its
    /// domain names it, as the backup would lack a disk the VM uses; one it
    /// does not name (a disk whose removal failed) is left out.
    async fn backed_up_zvols(
        &self,
        id: &VmId,
        disks: &[VmDiskSlot],
    ) -> Result<Vec<(VmDiskSlot, String)>, OperationError> {
        let found = self.vm_zvols(id).await?;
        let mut zvols = Vec::new();
        for slot in disks {
            let mut of_slot = found.iter().filter(|(found, _)| found == slot);
            match (of_slot.next(), of_slot.next()) {
                (Some((_, zvol)), None) => zvols.push((*slot, zvol.clone())),
                (None, _) => {
                    return Err(refused(format!("disk {} of the virtual machine has no zvol", slot.get())));
                }
                (Some(_), Some(_)) => {
                    return Err(refused(format!(
                        "disk {} of the virtual machine has zvols on more than one location",
                        slot.get()
                    )));
                }
            }
        }
        let domain = std::fs::read_to_string(self.domain_file(id)).unwrap_or_default();
        for (slot, zvol) in &found {
            if disks.contains(slot) {
                continue;
            }
            let device = self.paths.zvols.join(zvol);
            if domain.contains(&format!("<source dev='{}'/>", device.display())) {
                return Err(refused(format!(
                    "the virtual machine's domain uses {zvol}, of which zendiloOS has no disk: save its \
                     settings once, which defines it anew"
                )));
            }
            tracing::warn!(
                vm = id.as_str(),
                zvol,
                "vms: a zvol of the VM's that it does not use is not backed up"
            );
        }
        Ok(zvols)
    }

    /// Backs the VM up into `<folder>/<id>/<stem>` beneath `root`, from a
    /// snapshot `label` of its disks that goes again afterwards: the disks
    /// `disks` its record has.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn back_up(
        &self,
        id: &VmId,
        root: &BackupRoot,
        folder: &SharePath,
        stem: &BackupStem,
        label: &SnapshotLabel,
        settings: &VmBackupSettings,
        disks: &[VmDiskSlot],
    ) -> Result<Response, OperationError> {
        let too_long = |_| refused("the folder's path is too long");
        let target =
            folder.join(id.as_str()).and_then(|folder| folder.join(stem.as_str())).map_err(too_long)?;
        let place = self.place_root(root)?;
        let (consistency, zvols) = {
            let _serial = crate::waiting::lock(&self.serial).await?;
            self.require_defined(id).await?;
            self.settled(id).await?;
            let zvols = self.backed_up_zvols(id, disks).await?;
            let note = SnapshotNote::parse("Backup").ok();
            let consistency = self.take_snapshot(id, label, &[], note.as_ref()).await?;
            (consistency, zvols)
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
            // Also once the client is gone: a step of the work, not a wait for it.
            let _serial = self.serial.lock().await;
            if let Err(error) = self.destroy_snapshots(id, label).await {
                tracing::warn!(%error, vm = id.as_str(), "vms: the backup's snapshot stays");
            }
        }
        let done = outcome?;
        Ok(Response::VmBackedUp { folder: target, files: done.files, manifest: done.manifest, consistency })
    }

    /// Restores the backup's `files` into the VM `spec` describes: a new
    /// one defined first, one there (`over`) returned to snapshot `label`
    /// of its disks and to its definition from before should anything
    /// fail. The lock is not held while the unit writes; the VM is busy
    /// meanwhile.
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
        let (job, marker, _restoring) = {
            let _serial = crate::waiting::lock(&self.serial).await?;
            self.require_installed()?;
            self.locations_of(spec).await?;
            self.check_devices(spec).await?;
            self.settled(&spec.id).await?;
            let marker = if over {
                self.prepare_over(spec, label).await?
            } else {
                self.prepare_new(spec, label).await?
            };
            let job = match self.restore_job(spec, &place, folder, files, over).await {
                Ok(job) => job,
                Err(error) => {
                    self.give_up_restore(&spec.id, &marker).await;
                    return Err(error);
                }
            };
            self.restoring.lock().unwrap_or_else(PoisonError::into_inner).insert(spec.id.clone());
            (job, marker, Restoring { vms: self, id: spec.id.clone() })
        };
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
        // Also once the client is gone: a step of the work, not a wait for it.
        let _serial = self.serial.lock().await;
        if let Err(error) = restored {
            self.give_up_restore(&spec.id, &marker).await;
            return Err(error);
        }
        self.finish_restore(spec, files, remove, &marker).await?;
        Ok(Response::Done)
    }

    /// Writes down what undoes a restore, and over a VM its definition from
    /// before.
    fn write_restore_marker(&self, id: &VmId, marker: &RestoreMarker) -> Result<(), OperationError> {
        if marker.over {
            let file = self.domain_file(id);
            match std::fs::read(&file) {
                Ok(xml) => crate::durable::replace(&self.before_restore_file(id), &xml)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tracing::warn!(vm = id.as_str(), "vms: the domain's definition from before is not kept");
                }
                Err(error) => return Err(OperationError::Read(file, error)),
            }
        }
        let json = serde_json::to_vec_pretty(marker)
            .map_err(|e| OperationError::Failed(format!("the restore's marker: {e}")))?;
        crate::durable::replace(&self.restore_marker(id), &json)
    }

    /// Before a restore over the VM writes: a snapshot of its disks to
    /// return to, what undoes the restore written down, its autostart off,
    /// its disks made or grown, the backup's domain defined. Undone should
    /// any of it fail.
    async fn prepare_over(
        &self,
        spec: &VmSpec,
        label: &SnapshotLabel,
    ) -> Result<RestoreMarker, OperationError> {
        self.require_shut_off(&spec.id).await?;
        let had = self.vm_zvols(&spec.id).await?;
        if !had.is_empty() {
            let note = SnapshotNote::parse("Before a restore").ok();
            self.take_snapshot(&spec.id, label, &[], note.as_ref()).await?;
        }
        let mut plan = Vec::new();
        for disk in spec.disks() {
            let zvol = Self::disk_zvol(spec, disk);
            let step = match self.volsize(&zvol).await? {
                None => DiskPlan::Make,
                Some(size) if size < disk.size.bytes() => DiskPlan::Grow,
                Some(_) => DiskPlan::Keep,
            };
            plan.push((disk, zvol, step));
        }
        let marker = RestoreMarker {
            over: true,
            label: label.clone(),
            kept: had.into_iter().map(|(_, zvol)| zvol).collect(),
            made: plan
                .iter()
                .filter(|(_, _, step)| matches!(step, DiskPlan::Make))
                .map(|(_, zvol, _)| zvol.clone())
                .collect(),
            autostart: spec.autostart,
            reset_nvram: self.reset_marker(&spec.id).exists(),
        };
        if let Err(error) = self.write_restore_marker(&spec.id, &marker) {
            if !marker.kept.is_empty() {
                let _ = self.destroy_snapshots(&spec.id, label).await;
            }
            let _ = self.forget_restore(&spec.id);
            return Err(error);
        }
        let prepared = async {
            // libvirt starts no half-restored disks with the machine.
            let domain = spec.id.domain();
            run(&self.tools.virsh, &virsh(&["autostart", "--disable", &domain]), QUICK).await?;
            for (disk, zvol, step) in &plan {
                match step {
                    DiskPlan::Keep => {}
                    DiskPlan::Make => self.make_disk(spec, disk).await?,
                    DiskPlan::Grow => {
                        let size = format!("volsize={}G", disk.size.gib());
                        run(&self.tools.zfs, &["set", &size, zvol], SLOW).await?;
                    }
                }
            }
            // Before anything is written: what libvirt refuses writes nothing.
            self.define_domain(spec, true, Autostart::Leave).await
        }
        .await;
        if let Err(error) = prepared {
            self.give_up_restore(&spec.id, &marker).await;
            return Err(error);
        }
        Ok(marker)
    }

    /// Before a restore as a new VM writes: what undoes it written down,
    /// its disks made, its domain defined, not started with the machine
    /// yet. Undone should any of it fail.
    async fn prepare_new(
        &self,
        spec: &VmSpec,
        label: &SnapshotLabel,
    ) -> Result<RestoreMarker, OperationError> {
        if self.defined(&spec.id).await? {
            return Err(OperationError::Conflict);
        }
        let marker = RestoreMarker {
            over: false,
            label: label.clone(),
            kept: Vec::new(),
            made: spec.disks().iter().map(|disk| Self::disk_zvol(spec, disk)).collect(),
            autostart: false,
            reset_nvram: false,
        };
        self.write_restore_marker(&spec.id, &marker)?;
        let prepared = async {
            for disk in spec.disks() {
                self.make_disk(spec, disk).await?;
            }
            self.define_domain(spec, false, Autostart::Leave).await
        }
        .await;
        if let Err(error) = prepared {
            self.give_up_restore(&spec.id, &marker).await;
            return Err(error);
        }
        Ok(marker)
    }

    /// What the restore's unit is asked: each file of the backup and where
    /// it goes, the disks' devices once udev made them.
    async fn restore_job(
        &self,
        spec: &VmSpec,
        place: &std::path::Path,
        folder: &SharePath,
        files: &[VmBackupFile],
        over: bool,
    ) -> Result<VmRestoreJob, OperationError> {
        let mut targets = Vec::new();
        for file in files {
            let target = match file.part {
                VmBackupPart::Disk { slot, .. } => {
                    let disk = spec
                        .disks()
                        .iter()
                        .find(|disk| disk.slot == slot)
                        .ok_or_else(|| refused("no disk"))?;
                    self.device_of(&Self::disk_zvol(spec, disk)).await?
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
        Ok(VmRestoreJob { root: place.to_path_buf(), folder: folder.clone(), files: targets })
    }

    /// Once the unit wrote it all, the restore holds: it is not undone any
    /// more, and what does not work out after (autostart, a disk that would
    /// go, the snapshot from before) is only said. Should even the marker
    /// not go, the restore is undone, as it would be at the next start.
    async fn finish_restore(
        &self,
        spec: &VmSpec,
        files: &[VmBackupFile],
        remove: &[VmDiskSlot],
        marker: &RestoreMarker,
    ) -> Result<(), OperationError> {
        let vm = spec.id.as_str();
        if files.iter().any(|file| file.part == VmBackupPart::Nvram)
            && let Err(error) = remove_if_there(&self.reset_marker(&spec.id))
        {
            tracing::warn!(%error, vm, "vms: the restored UEFI variables start afresh at the next start");
        }
        if let Err(error) = self.forget_restore(&spec.id) {
            self.give_up_restore(&spec.id, marker).await;
            return Err(error);
        }
        if spec.autostart {
            let domain = spec.id.domain();
            if let Err(error) = run(&self.tools.virsh, &virsh(&["autostart", &domain]), QUICK).await {
                tracing::warn!(%error, vm, "vms: the restored VM does not start with the machine");
            }
        }
        self.destroy_removed(&spec.id, remove).await;
        if !marker.kept.is_empty()
            && let Err(error) = self.destroy_snapshots(&spec.id, &marker.label).await
        {
            tracing::warn!(%error, vm, "vms: the snapshot from before the restore stays");
        }
        Ok(())
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
