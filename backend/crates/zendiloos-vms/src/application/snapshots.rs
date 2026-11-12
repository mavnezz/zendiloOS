//! Snapshots of a VM's disks: taken by hand or on schedules, listed,
//! rolled back while it is shut off (to an older one only once the
//! administrator agreed that those after it go, as `zfs rollback -r`
//! destroys them), and deleted.

use std::cmp::Reverse;
use std::time::Duration;

use zendiloos_kernel::{Actor, RequestContext};

use super::{Vms, VmsError, outcome};
use crate::domain::{self, PowerState, SnapshotSchedule, Vm, VmSnapshot};

/// How often the schedules are looked at.
pub const SNAPSHOTS_EVERY: Duration = Duration::from_secs(5 * 60);

/// A VM's snapshots, newest first, and its schedules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSnapshots {
    pub snapshots: Vec<VmSnapshot>,
    pub schedules: Vec<SnapshotSchedule>,
}

/// Those of `all` that are `vm`'s, newest first.
fn of_vm(vm: &str, all: Vec<VmSnapshot>) -> Vec<VmSnapshot> {
    let mut mine: Vec<VmSnapshot> = all.into_iter().filter(|snapshot| snapshot.vm == vm).collect();
    mine.sort_by_key(|snapshot| Reverse(snapshot.taken_at));
    mine
}

impl Vms {
    pub async fn snapshots(&self, id: &str, context: &RequestContext) -> Result<VmSnapshots, VmsError> {
        let vm = self.ready(id).await?;
        let snapshots = of_vm(&vm.id, self.ports.host.snapshots(context).await?);
        Ok(VmSnapshots { snapshots, schedules: self.ports.schedules.list(&vm.id).await? })
    }

    /// A label for now that the VM's snapshots do not have yet: two in the
    /// same second take the next.
    pub(super) fn free_label(&self, taken: &[VmSnapshot]) -> String {
        let mut at = self.ports.clock.now();
        loop {
            let label = domain::label_at(at);
            if !taken.iter().any(|snapshot| snapshot.label == label) {
                return label;
            }
            at = zendiloos_kernel::Timestamp::from_unix_seconds(at.unix_seconds() + 1);
        }
    }

    /// Takes a snapshot of all the VM's disks now; schedules never delete
    /// it. While it runs it is crash-consistent unless the guest agent
    /// froze its file systems.
    pub async fn take_snapshot(
        &self,
        id: &str,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmSnapshot, VmsError> {
        let vm = self.ready(id).await?;
        let name = domain::snapshot_name(name)?;
        let taken = of_vm(&vm.id, self.ports.host.snapshots(context).await?);
        let label = self.free_label(&taken);
        let done = self.ports.host.snapshot(&vm.id, &label, &[], name.as_deref(), context).await;
        self.audit("vms.snapshot", outcome(&done), &format!("{}@{label}", vm.settings.name), actor, context)
            .await;
        let consistency = done?;
        let now = of_vm(&vm.id, self.ports.host.snapshots(context).await.unwrap_or_default());
        Ok(now.into_iter().find(|snapshot| snapshot.label == label).unwrap_or(VmSnapshot {
            vm: vm.id.clone(),
            taken_at: domain::label_time(&label).unwrap_or_else(|| self.ports.clock.now()),
            label,
            name,
            schedules: Vec::new(),
            consistency,
            disks: vm.settings.disks.iter().map(|disk| disk.slot).collect(),
            used_bytes: 0,
            referenced_bytes: 0,
        }))
    }

    /// The snapshot `label` of the VM, found.
    async fn snapshot_of(
        &self,
        vm: &Vm,
        label: &str,
        context: &RequestContext,
    ) -> Result<(VmSnapshot, Vec<VmSnapshot>), VmsError> {
        if !domain::is_label(label) {
            return Err(VmsError::NotFound);
        }
        let all = of_vm(&vm.id, self.ports.host.snapshots(context).await?);
        let found = all.iter().find(|snapshot| snapshot.label == label).cloned().ok_or(VmsError::NotFound)?;
        Ok((found, all))
    }

    pub async fn delete_snapshot(
        &self,
        id: &str,
        label: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        let vm = self.ready(id).await?;
        self.snapshot_of(&vm, label, context).await?;
        let done = self.ports.host.destroy_snapshot(&vm.id, label, context).await;
        self.audit(
            "vms.snapshot_delete",
            outcome(&done),
            &format!("{}@{label}", vm.settings.name),
            actor,
            context,
        )
        .await;
        Ok(done?)
    }

    /// Returns the VM's disks to snapshot `label`; it must be shut off.
    /// Snapshots taken after it go with it, so unless `newer` says the
    /// administrator agreed, a rollback to any but the newest is refused,
    /// naming them. A disk added since stays as it is. The labels that went.
    pub async fn rollback_snapshot(
        &self,
        id: &str,
        label: &str,
        newer: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<String>, VmsError> {
        let vm = self.ready(id).await?;
        let _busy = self.take_busy(&vm.id, "rolled back")?;
        let (target, all) = self.snapshot_of(&vm, label, context).await?;
        let later: Vec<String> = all
            .iter()
            .filter(|snapshot| snapshot.taken_at > target.taken_at)
            .map(|snapshot| snapshot.label.clone())
            .collect();
        if !later.is_empty() && !newer {
            return Err(VmsError::NewerSnapshots(later));
        }
        let states = self.ports.host.states(context).await?;
        if states.get(&vm.id) != Some(&PowerState::ShutOff) {
            return Err(VmsError::Running);
        }
        let done = self.ports.host.rollback_snapshot(&vm.id, label, !later.is_empty(), context).await;
        self.audit("vms.rollback", outcome(&done), &format!("{}@{label}", vm.settings.name), actor, context)
            .await;
        done?;
        Ok(later)
    }

    /// Replaces how often the VM is snapshotted and how many to keep.
    /// Snapshots of a schedule that goes stay until deleted by hand.
    pub async fn set_snapshot_schedules(
        &self,
        id: &str,
        schedules: Vec<SnapshotSchedule>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<SnapshotSchedule>, VmsError> {
        let vm = self.ready(id).await?;
        let schedules = domain::schedules(schedules)?;
        let done = self.ports.schedules.set(&vm.id, &schedules).await;
        self.audit("vms.snapshot_schedules", outcome(&done), &vm.settings.name, actor, context).await;
        done?;
        Ok(schedules)
    }

    /// Takes the snapshots that are due and lets go of those the schedules
    /// no longer keep; called every few minutes. A VM being changed,
    /// backed up or restored waits for the next look.
    pub async fn take_due_snapshots(&self) {
        let context = RequestContext::default();
        let all = match self.ports.schedules.all().await {
            Ok(all) if !all.is_empty() => all,
            Ok(_) => return,
            Err(error) => return tracing::warn!(%error, "vms: cannot read the snapshots' schedules"),
        };
        let snapshots = match self.ports.host.snapshots(&context).await {
            Ok(snapshots) => snapshots,
            Err(error) => return tracing::warn!(%error, "vms: cannot read the snapshots"),
        };
        for (id, schedules) in all {
            let Ok(Some(vm)) = self.ports.repository.find(&id).await else { continue };
            if !vm.ready {
                continue;
            }
            let Ok(_busy) = self.take_busy(&vm.id, "snapshotted") else { continue };
            self.keep_up(&vm, &schedules, of_vm(&vm.id, snapshots.clone()), &context).await;
        }
    }

    async fn keep_up(
        &self,
        vm: &Vm,
        schedules: &[SnapshotSchedule],
        mut taken: Vec<VmSnapshot>,
        context: &RequestContext,
    ) {
        let now = self.ports.clock.now();
        let due = domain::due(schedules, &taken, now);
        if !due.is_empty() {
            let label = self.free_label(&taken);
            match self.ports.host.snapshot(&vm.id, &label, &due, None, context).await {
                Ok(consistency) => {
                    tracing::info!(vm = vm.settings.name, label, "vms: a snapshot taken on schedule");
                    taken.push(VmSnapshot {
                        vm: vm.id.clone(),
                        taken_at: now,
                        label,
                        name: None,
                        schedules: due,
                        consistency,
                        disks: Vec::new(),
                        used_bytes: 0,
                        referenced_bytes: 0,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, vm = vm.settings.name, "vms: cannot take a snapshot on schedule");
                    return;
                }
            }
        }
        for old in domain::prunable(schedules, &taken) {
            if let Err(error) = self.ports.host.destroy_snapshot(&vm.id, &old.label, context).await {
                tracing::warn!(%error, vm = vm.settings.name, label = old.label, "vms: cannot let a snapshot go");
            }
        }
    }
}
