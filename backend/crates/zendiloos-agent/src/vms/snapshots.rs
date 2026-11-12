//! Snapshots of a VM's disks (docs/architecture.md, 4.13): ZFS snapshots
//! `<pool>/zendiloos.vms/<id>-disk<N>@<label>` of every disk at once, named
//! as the shares' are (`zdl-YYYY-MM-DD_HH.MM.SS`, UTC), with the schedules
//! they were taken for, a name and how consistent they are as ZFS
//! properties, so that nothing about them is kept anywhere else.
//!
//! `zfs snapshot` takes all snapshots it is given in one transaction, but
//! only within one pool. A running VM's file systems are frozen first by
//! the QEMU guest agent if the guest runs it (`virsh domfsfreeze`); without
//! it the snapshot is crash-consistent, and a VM whose disks are on more
//! than one pool is paused meanwhile, so that its disks' snapshots are of
//! one moment. A rollback is only for a VM that is shut off.

use std::collections::BTreeMap;

use zendiloos_agent_proto::{
    Response, SnapshotFrequency, SnapshotLabel, SnapshotNote, VmConsistency, VmDiskSlot, VmId,
};

use super::{ON, QUICK, SLOW, Vms, refused, run, virsh};
use crate::operations::OperationError;
use crate::pools::{NOTE_PROPERTY, SCHEDULE_PROPERTY};

/// How consistent a snapshot is, as a ZFS property.
pub(crate) const CONSISTENCY_PROPERTY: &str = "zendiloos:consistency";

impl Vms {
    /// The VM's zvols there are, on every location: the slot and the zvol.
    pub(super) async fn vm_zvols(&self, id: &VmId) -> Result<Vec<(VmDiskSlot, String)>, OperationError> {
        let mut found = Vec::new();
        for slot in (0..).map_while(|slot| VmDiskSlot::new(slot).ok()) {
            for zvol in self.zvols_in(id, slot).await? {
                found.push((slot, zvol));
            }
        }
        Ok(found)
    }

    /// Whether `snapshot` (`<zvol>@<label>`) is there.
    async fn has_snapshot(&self, snapshot: &str) -> Result<bool, OperationError> {
        let output = zendiloos_exec::run(
            &self.tools.zfs,
            ["list", "-H", "-o", "name", "-t", "snapshot", snapshot],
            QUICK,
        )
        .await?;
        Ok(output.status.success())
    }

    pub(crate) async fn snapshot(
        &self,
        id: &VmId,
        label: &SnapshotLabel,
        schedules: &[SnapshotFrequency],
        note: Option<&SnapshotNote>,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_defined(id).await?;
        let consistency = self.take_snapshot(id, label, schedules, note).await?;
        Ok(Response::VmSnapshotTaken { consistency })
    }

    /// Takes snapshot `label` of every disk of the VM; how consistent it is.
    pub(super) async fn take_snapshot(
        &self,
        id: &VmId,
        label: &SnapshotLabel,
        schedules: &[SnapshotFrequency],
        note: Option<&SnapshotNote>,
    ) -> Result<VmConsistency, OperationError> {
        let zvols = self.vm_zvols(id).await?;
        if zvols.is_empty() {
            return Err(refused(format!("the virtual machine {} has no disks", id.as_str())));
        }
        for (_, zvol) in &zvols {
            if self.has_snapshot(&format!("{zvol}@{}", label.as_str())).await? {
                return Err(OperationError::Conflict);
            }
        }
        let mut pools: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (_, zvol) in &zvols {
            let pool = zvol.split('/').next().unwrap_or_default();
            pools.entry(pool).or_default().push(format!("{zvol}@{}", label.as_str()));
        }
        let domain = id.domain();
        let state = self.domain_state(id).await?.unwrap_or_default();
        // QEMU runs: paused, its disks are as at a power cut too.
        let on = ON.contains(&state.as_str());
        let running = on && state != "paused";
        let frozen = running
            && zendiloos_exec::run(&self.tools.virsh, virsh(&["domfsfreeze", &domain]), QUICK)
                .await
                .is_ok_and(|output| output.status.success());
        let paused = running
            && !frozen
            && pools.len() > 1
            && zendiloos_exec::run(&self.tools.virsh, virsh(&["suspend", &domain]), QUICK)
                .await
                .is_ok_and(|output| output.status.success());
        let consistency = match (on, frozen) {
            (false, _) => VmConsistency::Off,
            (true, true) => VmConsistency::Frozen,
            (true, false) => VmConsistency::Crash,
        };
        let mut properties =
            vec!["-o".to_owned(), format!("{CONSISTENCY_PROPERTY}={}", consistency.as_str())];
        if !schedules.is_empty() {
            let names: Vec<&str> = schedules.iter().map(|frequency| frequency.as_str()).collect();
            properties.extend(["-o".to_owned(), format!("{SCHEDULE_PROPERTY}={}", names.join(","))]);
        }
        if let Some(note) = note {
            properties.extend(["-o".to_owned(), format!("{NOTE_PROPERTY}={}", note.as_str())]);
        }
        let mut taken: Vec<String> = Vec::new();
        let mut done = Ok(());
        for snapshots in pools.values() {
            let mut args: Vec<&str> = vec!["snapshot"];
            args.extend(properties.iter().map(String::as_str));
            args.extend(snapshots.iter().map(String::as_str));
            done = run(&self.tools.zfs, &args, SLOW).await;
            if done.is_err() {
                break;
            }
            taken.extend(snapshots.iter().cloned());
        }
        // Whatever came of it, the guest goes on.
        if frozen {
            let thawed = zendiloos_exec::run(&self.tools.virsh, virsh(&["domfsthaw", &domain]), QUICK).await;
            if !thawed.is_ok_and(|output| output.status.success()) {
                tracing::warn!(domain, "vms: the guest's file systems may still be frozen");
            }
        }
        if paused {
            let resumed = zendiloos_exec::run(&self.tools.virsh, virsh(&["resume", &domain]), QUICK).await;
            if !resumed.is_ok_and(|output| output.status.success()) {
                tracing::warn!(domain, "vms: the virtual machine paused for its snapshot did not resume");
            }
        }
        if let Err(error) = done {
            // Of one moment, or none: those of other pools go again.
            for snapshot in taken {
                let _ = run(&self.tools.zfs, &["destroy", &snapshot], SLOW).await;
            }
            return Err(error);
        }
        Ok(consistency)
    }

    /// The snapshots of every location's zvols, for zendiloosd to read.
    pub(crate) async fn snapshots(&self) -> Result<Response, OperationError> {
        let datasets: Vec<String> =
            super::vms_locations(&self.host).iter().map(|location| Self::dataset(&location.pool)).collect();
        if datasets.is_empty() {
            return Ok(Response::VmSnapshots { zfs_list: String::new() });
        }
        let columns = format!(
            "name,creation,used,referenced,{SCHEDULE_PROPERTY},{NOTE_PROPERTY},{CONSISTENCY_PROPERTY}"
        );
        let mut args = vec!["list", "-H", "-p", "-r", "-t", "snapshot", "-o", columns.as_str()];
        args.extend(datasets.iter().map(String::as_str));
        let output = zendiloos_exec::run(&self.tools.zfs, args, QUICK).await?.success(&self.tools.zfs)?;
        let zfs_list = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(Response::VmSnapshots { zfs_list })
    }

    /// Destroys snapshot `label` of every disk of the VM that has it.
    pub(crate) async fn destroy_snapshot(
        &self,
        id: &VmId,
        label: &SnapshotLabel,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.destroy_snapshots(id, label).await?;
        Ok(Response::Done)
    }

    pub(super) async fn destroy_snapshots(
        &self,
        id: &VmId,
        label: &SnapshotLabel,
    ) -> Result<(), OperationError> {
        let mut found = false;
        for (_, zvol) in self.vm_zvols(id).await? {
            let snapshot = format!("{zvol}@{}", label.as_str());
            if self.has_snapshot(&snapshot).await? {
                run(&self.tools.zfs, &["destroy", &snapshot], SLOW).await?;
                found = true;
            }
        }
        if found { Ok(()) } else { Err(refused(format!("there is no snapshot {}", label.as_str()))) }
    }

    /// Returns the VM's disks to snapshot `label`, while it is shut off;
    /// later snapshots stand in the way unless `newer` says they go. Each
    /// disk keeps the size it has now, as disks only grow.
    pub(crate) async fn rollback_snapshot(
        &self,
        id: &VmId,
        label: &SnapshotLabel,
        newer: bool,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_shut_off(id).await?;
        self.not_busy(id).await?;
        let zvols = self.vm_zvols(id).await?;
        let mut with = Vec::new();
        for (_, zvol) in zvols {
            if self.has_snapshot(&format!("{zvol}@{}", label.as_str())).await? {
                with.push(zvol);
            }
        }
        if with.is_empty() {
            return Err(refused(format!("there is no snapshot {}", label.as_str())));
        }
        for zvol in &with {
            let listed = zendiloos_exec::run(
                &self.tools.zfs,
                ["list", "-H", "-o", "name", "-s", "createtxg", "-t", "snapshot", "-d", "1", zvol],
                QUICK,
            )
            .await?
            .success(&self.tools.zfs)?;
            let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
            let after = listed
                .lines()
                .skip_while(|name| name.split_once('@').map(|(_, at)| at) != Some(label.as_str()))
                .skip(1)
                .count();
            if after > 0 && !newer {
                return Err(refused(format!(
                    "{zvol} has {after} snapshots after {}: they go with the rollback only if that is confirmed",
                    label.as_str()
                )));
            }
        }
        for zvol in &with {
            let size = self.volsize(zvol).await?;
            let snapshot = format!("{zvol}@{}", label.as_str());
            let args: &[&str] = if newer { &["rollback", "-r", &snapshot] } else { &["rollback", &snapshot] };
            run(&self.tools.zfs, args, SLOW).await?;
            if let (Some(before), Some(now)) = (size, self.volsize(zvol).await?)
                && now < before
            {
                run(&self.tools.zfs, &["set", &format!("volsize={before}"), zvol], SLOW).await?;
            }
        }
        Ok(Response::Done)
    }

    /// Refused unless libvirt says the domain is shut off.
    pub(super) async fn require_shut_off(&self, id: &VmId) -> Result<(), OperationError> {
        self.require_installed()?;
        match self.domain_state(id).await? {
            None => Err(refused(format!("there is no virtual machine {}", id.as_str()))),
            Some(state) if state != "shut off" => {
                Err(refused("the virtual machine is not shut off: shut it down first"))
            }
            Some(_) => Ok(()),
        }
    }
}
