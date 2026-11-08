//! The disks the NAS has seen, by serial number, and what the owner noted
//! about each.

use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext};

use super::StorageError;
use super::ports::{BlockDeviceSource, DiskInventoryStore, SeenDisk};
use crate::domain::{self, Disk, InventoryDetails, InventoryRecord};

/// A disk of the inventory and, while it is present, its kernel name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    pub record: InventoryRecord,
    pub disk: Option<String>,
}

#[derive(Clone)]
pub struct DiskInventory {
    devices: Arc<dyn BlockDeviceSource>,
    store: Arc<dyn DiskInventoryStore>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl DiskInventory {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        store: Arc<dyn DiskInventoryStore>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { devices, store, audit, clock }
    }

    /// Notes the disks present now; call regularly. Disks without a serial
    /// number cannot be told apart and are not kept.
    pub async fn observe(&self, context: &RequestContext) -> Result<Vec<Disk>, StorageError> {
        let disks = domain::disks(&self.devices.block_devices(context).await?, &[]);
        let seen: Vec<SeenDisk> = disks
            .iter()
            .filter_map(|disk| {
                Some(SeenDisk {
                    serial: disk.serial.clone()?,
                    model: disk.model.clone(),
                    size_bytes: disk.size_bytes,
                })
            })
            .collect();
        self.store.seen(&seen, self.clock.now()).await?;
        Ok(disks)
    }

    /// Every disk seen: those present first, by kernel name, then the
    /// others, the most recently seen first.
    pub async fn list(&self, context: &RequestContext) -> Result<Vec<InventoryEntry>, StorageError> {
        let disks = self.observe(context).await?;
        let mut entries: Vec<InventoryEntry> = self
            .store
            .records()
            .await?
            .into_iter()
            .map(|record| {
                let disk = disks.iter().find(|d| d.serial.as_deref() == Some(record.serial.as_str()));
                InventoryEntry { disk: disk.map(|d| d.name.clone()), record }
            })
            .collect();
        entries.sort_by(|a, b| match (&a.disk, &b.disk) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.record.last_seen.cmp(&a.record.last_seen),
        });
        Ok(entries)
    }

    pub async fn update(
        &self,
        serial: &str,
        details: InventoryDetails,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<InventoryEntry, StorageError> {
        let found = self.store.set_details(serial, &details).await;
        self.record("storage.inventory_update", serial, found.is_ok(), actor, context).await;
        if !found? {
            return Err(StorageError::InventoryNotFound);
        }
        self.list(context)
            .await?
            .into_iter()
            .find(|entry| entry.record.serial == serial)
            .ok_or(StorageError::InventoryNotFound)
    }

    /// Forgets a disk that is no longer present.
    pub async fn remove(
        &self,
        serial: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), StorageError> {
        let present = self.observe(context).await?.iter().any(|disk| disk.serial.as_deref() == Some(serial));
        if present {
            return Err(StorageError::DiskPresent);
        }
        let removed = self.store.remove(serial).await;
        self.record("storage.inventory_remove", serial, removed.is_ok(), actor, context).await;
        if removed? { Ok(()) } else { Err(StorageError::InventoryNotFound) }
    }

    async fn record(
        &self,
        action: &'static str,
        serial: &str,
        ok: bool,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let outcome = if ok { Outcome::Success } else { Outcome::Failure };
        let entry = AuditEntry::new(self.clock.now(), action, outcome, context).by(actor).target(serial);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::domain::CalendarDate;
    use crate::testing::use_cases;

    fn admin() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn every_disk_seen_is_kept_by_serial_number() {
        let (use_cases, fake, audit) = use_cases();
        let inventory = &use_cases.inventory;
        let request = RequestContext::default();
        let entries = inventory.list(&request).await.unwrap();
        let listed: Vec<_> = entries.iter().map(|e| (e.record.serial.as_str(), e.disk.as_deref())).collect();
        // The system disk has no serial number here.
        assert_eq!(
            listed,
            [
                ("T1", Some("sda")),
                ("T2", Some("sdb")),
                ("T3", Some("sdc")),
                ("T4", Some("sdd")),
                ("T5", Some("sde"))
            ]
        );
        assert_eq!(entries[0].record.model.as_deref(), Some("TEST HDD"));
        assert_eq!(entries[0].record.first_seen, Timestamp::from_unix_seconds(1_793_880_000));

        let details =
            InventoryDetails::new(Some("2024-03-01"), Some("2027-03-01"), Some("Shop"), Some("bay 1"))
                .unwrap();
        let updated = inventory.update("T1", details.clone(), &admin(), &request).await.unwrap();
        assert_eq!(updated.record.details.purchased, CalendarDate::parse("2024-03-01"));
        assert_eq!(updated.disk.as_deref(), Some("sda"));
        assert_eq!(audit.entries().last().unwrap().action, "storage.inventory_update");
        assert!(matches!(
            inventory.update("X9", details, &admin(), &request).await,
            Err(StorageError::InventoryNotFound)
        ));

        // sdd swapped for another disk: the old one is kept, not present.
        *fake.sdd_serial.lock().unwrap() = "T9".to_owned();
        let entries = inventory.list(&request).await.unwrap();
        let last = entries.last().unwrap();
        assert_eq!((last.record.serial.as_str(), last.disk.as_deref()), ("T4", None));
        assert!(entries.iter().any(|e| e.record.serial == "T9" && e.disk.as_deref() == Some("sdd")));

        assert!(matches!(inventory.remove("T1", &admin(), &request).await, Err(StorageError::DiskPresent)));
        inventory.remove("T4", &admin(), &request).await.unwrap();
        assert_eq!(audit.entries().last().unwrap().action, "storage.inventory_remove");
        assert!(!inventory.list(&request).await.unwrap().iter().any(|e| e.record.serial == "T4"));
        assert!(matches!(
            inventory.remove("T4", &admin(), &request).await,
            Err(StorageError::InventoryNotFound)
        ));
    }

    #[tokio::test]
    async fn disks_no_longer_present_come_last_the_newest_first() {
        let (use_cases, fake, _) = use_cases();
        let inventory = &use_cases.inventory;
        let request = RequestContext::default();
        fake.remember("OLD1", 1_700_000_000);
        fake.remember("OLD2", 1_750_000_000);
        let serials: Vec<_> =
            inventory.list(&request).await.unwrap().into_iter().map(|e| e.record.serial).collect();
        assert_eq!(serials[5..], ["OLD2", "OLD1"]);
    }
}
