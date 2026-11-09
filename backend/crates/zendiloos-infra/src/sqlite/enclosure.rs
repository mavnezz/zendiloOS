use std::collections::BTreeSet;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_storage::application::ports::{DiskInventoryStore, EnclosureStore, SeenDisk};
use zendiloos_storage::domain::{
    Binding, CalendarDate, EnclosureChoice, InventoryDetails, InventoryRecord, Layout, Numbering,
    Orientation, Override, Slot, SlotForm, SlotGroup,
};

use super::Database;

/// [`EnclosureStore`] on the `enclosure` tables.
#[derive(Debug, Clone)]
pub struct SqliteEnclosure {
    db: Database,
}

impl SqliteEnclosure {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("enclosure", error)
}

fn invalid(what: &str) -> PortError {
    PortError::new("enclosure", format!("the database holds an invalid {what}"))
}

fn binding(kind: Option<&str>, value: Option<String>) -> Result<Option<Binding>, PortError> {
    match (kind, value) {
        (None, _) => Ok(None),
        (Some("port"), Some(port)) => Ok(Some(Binding::Port(port))),
        (Some("serial"), Some(serial)) => Ok(Some(Binding::Serial(serial))),
        _ => Err(invalid("binding")),
    }
}

fn binding_columns(binding: Option<&Binding>) -> (Option<&'static str>, Option<&str>) {
    match binding {
        None => (None, None),
        Some(Binding::Port(port)) => (Some("port"), Some(port)),
        Some(Binding::Serial(serial)) => (Some("serial"), Some(serial)),
    }
}

#[async_trait]
impl EnclosureStore for SqliteEnclosure {
    async fn chosen(&self) -> Result<Option<EnclosureChoice>, PortError> {
        let pool = self.db.pool();
        let Some(row) = sqlx::query!("SELECT model FROM enclosure WHERE id = 1")
            .fetch_optional(pool)
            .await
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let bindings = sqlx::query!("SELECT slot, kind, value FROM enclosure_bindings ORDER BY slot")
            .fetch_all(pool)
            .await
            .map_err(db_error)?;
        if let Some(model) = row.model {
            let overrides = bindings
                .into_iter()
                .map(|b| Ok(Override { binding: binding(b.kind.as_deref(), b.value)?, slot: b.slot }))
                .collect::<Result<_, PortError>>()?;
            return Ok(Some(EnclosureChoice::Model { model, overrides }));
        }
        let groups = sqlx::query!(
            "SELECT id, name, row_count, column_count, orientation, form, numbering \
             FROM enclosure_groups ORDER BY position"
        )
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        let slots =
            sqlx::query!("SELECT group_id, id, label FROM enclosure_slots ORDER BY group_id, position")
                .fetch_all(pool)
                .await
                .map_err(db_error)?;
        let mut layout = Vec::new();
        for group in groups {
            let mut members = Vec::new();
            for slot in slots.iter().filter(|slot| slot.group_id == group.id) {
                let bound = bindings.iter().find(|b| b.slot == slot.id);
                members.push(Slot {
                    id: slot.id.clone(),
                    label: slot.label.clone(),
                    binding: binding(
                        bound.and_then(|b| b.kind.as_deref()),
                        bound.and_then(|b| b.value.clone()),
                    )?,
                });
            }
            layout.push(SlotGroup {
                rows: u8::try_from(group.row_count).map_err(|_| invalid("group"))?,
                columns: u8::try_from(group.column_count).map_err(|_| invalid("group"))?,
                orientation: Orientation::parse(&group.orientation).ok_or_else(|| invalid("orientation"))?,
                form: SlotForm::parse(&group.form).ok_or_else(|| invalid("form"))?,
                numbering: Numbering::parse(&group.numbering).ok_or_else(|| invalid("numbering"))?,
                id: group.id,
                name: group.name,
                slots: members,
            });
        }
        let layout = Layout::new(layout).map_err(|e| PortError::new("enclosure", e))?;
        Ok(Some(EnclosureChoice::Custom(layout)))
    }

    async fn save(&self, choice: Option<&EnclosureChoice>) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        for statement in [
            "DELETE FROM enclosure_bindings",
            "DELETE FROM enclosure_slots",
            "DELETE FROM enclosure_groups",
            "DELETE FROM enclosure",
        ] {
            sqlx::query(statement).execute(&mut *transaction).await.map_err(db_error)?;
        }
        let bind = |slot: &str, binding: Option<&Binding>| {
            let (kind, value) = binding_columns(binding);
            sqlx::query!(
                "INSERT INTO enclosure_bindings (slot, kind, value) VALUES (?, ?, ?)",
                slot,
                kind,
                value
            )
        };
        match choice {
            None => {}
            Some(EnclosureChoice::Model { model, overrides }) => {
                sqlx::query!("INSERT INTO enclosure (id, model) VALUES (1, ?)", model)
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
                for change in overrides {
                    bind(&change.slot, change.binding.as_ref())
                        .execute(&mut *transaction)
                        .await
                        .map_err(db_error)?;
                }
            }
            Some(EnclosureChoice::Custom(layout)) => {
                sqlx::query!("INSERT INTO enclosure (id, model) VALUES (1, NULL)")
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
                for (position, group) in (0i64..).zip(layout.groups()) {
                    let (rows, columns) = (i64::from(group.rows), i64::from(group.columns));
                    let (orientation, form, numbering) =
                        (group.orientation.as_str(), group.form.as_str(), group.numbering.as_str());
                    sqlx::query!(
                        "INSERT INTO enclosure_groups \
                         (position, id, name, row_count, column_count, orientation, form, numbering) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                        position,
                        group.id,
                        group.name,
                        rows,
                        columns,
                        orientation,
                        form,
                        numbering
                    )
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
                    for (position, slot) in (0i64..).zip(&group.slots) {
                        sqlx::query!(
                            "INSERT INTO enclosure_slots (group_id, position, id, label) VALUES (?, ?, ?, ?)",
                            group.id,
                            position,
                            slot.id,
                            slot.label
                        )
                        .execute(&mut *transaction)
                        .await
                        .map_err(db_error)?;
                        if slot.binding.is_some() {
                            bind(&slot.id, slot.binding.as_ref())
                                .execute(&mut *transaction)
                                .await
                                .map_err(db_error)?;
                        }
                    }
                }
            }
        }
        transaction.commit().await.map_err(db_error)
    }
}

/// [`DiskInventoryStore`] on the `disk_inventory` table.
#[derive(Debug, Clone)]
pub struct SqliteDiskInventory {
    db: Database,
}

impl SqliteDiskInventory {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn inventory_error(error: sqlx::Error) -> PortError {
    PortError::new("disk inventory", error)
}

/// How far behind the last time seen may lag, so a look every minute
/// does not write every minute.
const LAST_SEEN_STEP_SECONDS: i64 = 3600;

#[async_trait]
impl DiskInventoryStore for SqliteDiskInventory {
    async fn records(&self) -> Result<Vec<InventoryRecord>, PortError> {
        let rows = sqlx::query!(
            "SELECT serial, model, size_bytes, purchased, warranty_until, vendor, note, first_seen, last_seen \
             FROM disk_inventory ORDER BY serial"
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(inventory_error)?;
        Ok(rows
            .into_iter()
            .map(|row| InventoryRecord {
                serial: row.serial,
                model: row.model,
                size_bytes: u64::try_from(row.size_bytes).unwrap_or(0),
                details: InventoryDetails {
                    purchased: row.purchased.as_deref().and_then(CalendarDate::parse),
                    warranty_until: row.warranty_until.as_deref().and_then(CalendarDate::parse),
                    vendor: row.vendor,
                    note: row.note,
                },
                first_seen: Timestamp::from_unix_seconds(row.first_seen),
                last_seen: Timestamp::from_unix_seconds(row.last_seen),
            })
            .collect())
    }

    async fn seen(&self, disks: &[SeenDisk], at: Timestamp) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(inventory_error)?;
        let (at, step) = (at.unix_seconds(), LAST_SEEN_STEP_SECONDS);
        // The last time seen never goes back: not for a clock set back (an
        // RTC before NTP), nor for a model written at once.
        for disk in disks {
            let size = i64::try_from(disk.size_bytes).unwrap_or(i64::MAX);
            sqlx::query!(
                "INSERT INTO disk_inventory (serial, model, size_bytes, first_seen, last_seen) VALUES (?, ?, ?, ?, ?) \
                 ON CONFLICT (serial) DO UPDATE SET \
                 model = excluded.model, size_bytes = excluded.size_bytes, \
                 last_seen = MAX(disk_inventory.last_seen, excluded.last_seen) \
                 WHERE disk_inventory.last_seen <= excluded.last_seen - ? \
                 OR disk_inventory.model IS NOT excluded.model OR disk_inventory.size_bytes != excluded.size_bytes",
                disk.serial,
                disk.model,
                size,
                at,
                at,
                step
            )
            .execute(&mut *transaction)
            .await
            .map_err(inventory_error)?;
        }
        transaction.commit().await.map_err(inventory_error)
    }

    async fn set_details(&self, serial: &str, details: &InventoryDetails) -> Result<bool, PortError> {
        let purchased = details.purchased.map(|date| date.to_string());
        let until = details.warranty_until.map(|date| date.to_string());
        let changed = sqlx::query!(
            "UPDATE disk_inventory SET purchased = ?, warranty_until = ?, vendor = ?, note = ? WHERE serial = ?",
            purchased,
            until,
            details.vendor,
            details.note,
            serial
        )
        .execute(self.db.pool())
        .await
        .map_err(inventory_error)?;
        Ok(changed.rows_affected() > 0)
    }

    async fn remove(&self, serial: &str) -> Result<bool, PortError> {
        let removed = sqlx::query!("DELETE FROM disk_inventory WHERE serial = ?", serial)
            .execute(self.db.pool())
            .await
            .map_err(inventory_error)?;
        Ok(removed.rows_affected() > 0)
    }

    async fn shared_serials(&self) -> Result<BTreeSet<String>, PortError> {
        let rows = sqlx::query_scalar!("SELECT serial FROM disk_shared_serials")
            .fetch_all(self.db.pool())
            .await
            .map_err(inventory_error)?;
        Ok(rows.into_iter().collect())
    }

    async fn note_shared(&self, serials: &BTreeSet<String>) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(inventory_error)?;
        for serial in serials {
            sqlx::query!(
                "INSERT INTO disk_shared_serials (serial) VALUES (?) ON CONFLICT DO NOTHING",
                serial
            )
            .execute(&mut *transaction)
            .await
            .map_err(inventory_error)?;
        }
        transaction.commit().await.map_err(inventory_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::testing::test_catalog;

    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn a_model_with_its_overrides_or_an_own_layout_is_kept() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteEnclosure::new(db.clone());
        assert_eq!(store.chosen().await.unwrap(), None);

        let model = EnclosureChoice::Model {
            model: "test-nas4".into(),
            overrides: vec![
                Override { slot: "bay1".into(), binding: None },
                Override { slot: "m2-2".into(), binding: Some(Binding::Serial("S 2".into())) },
            ],
        };
        store.save(Some(&model)).await.unwrap();
        assert_eq!(store.chosen().await.unwrap(), Some(model));

        let mut groups = test_catalog()[0].layout.groups().to_vec();
        groups[0].slots[1].binding = Some(Binding::Serial("T2".into()));
        groups[1].slots[1].binding = None;
        groups[1].name = "Unten".into();
        let own = EnclosureChoice::Custom(Layout::new(groups).unwrap());
        store.save(Some(&own)).await.unwrap();
        assert_eq!(store.chosen().await.unwrap(), Some(own), "groups, slots and bindings in their order");

        store.save(None).await.unwrap();
        assert_eq!(store.chosen().await.unwrap(), None);
        db.close().await;
    }

    #[tokio::test]
    async fn what_the_database_cannot_hold_is_an_error_not_a_guess() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteEnclosure::new(db.clone());
        sqlx::query("INSERT INTO enclosure (id, model) VALUES (1, NULL)").execute(db.pool()).await.unwrap();
        assert!(store.chosen().await.is_err(), "a layout of no groups");
        assert!(binding(Some("port"), None).is_err());
        assert!(binding(Some("wwn"), Some("x".into())).is_err());
        db.close().await;
    }

    fn seen(serial: &str, model: &str) -> SeenDisk {
        SeenDisk { serial: serial.into(), model: Some(model.into()), size_bytes: 4_000_787_030_016 }
    }

    #[tokio::test]
    async fn disks_are_remembered_with_what_the_owner_noted() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteDiskInventory::new(db.clone());
        let at = |seconds| Timestamp::from_unix_seconds(seconds);
        store.seen(&[seen("A1", "HDD"), seen("B2", "HDD")], at(1_000_000)).await.unwrap();
        // A minute later nothing is written; an hour later it is.
        store.seen(&[seen("A1", "HDD")], at(1_000_060)).await.unwrap();
        assert_eq!(store.records().await.unwrap()[0].last_seen, at(1_000_000));
        store.seen(&[seen("A1", "HDD")], at(1_003_600)).await.unwrap();
        // A changed model is written at once.
        store.seen(&[seen("B2", "HDD v2")], at(1_000_120)).await.unwrap();
        let records = store.records().await.unwrap();
        assert_eq!((records[0].first_seen, records[0].last_seen), (at(1_000_000), at(1_003_600)));
        assert_eq!(records[1].model.as_deref(), Some("HDD v2"));
        assert_eq!(records[1].last_seen, at(1_000_120));
        assert_eq!(records[1].size_bytes, 4_000_787_030_016);

        let details =
            InventoryDetails::new(Some("2024-03-01"), Some("2027-03-01"), Some("Shop"), Some("A")).unwrap();
        assert!(store.set_details("A1", &details).await.unwrap());
        assert!(!store.set_details("Z9", &details).await.unwrap());
        assert_eq!(store.records().await.unwrap()[0].details, details);
        assert!(store.set_details("A1", &InventoryDetails::default()).await.unwrap());
        assert_eq!(store.records().await.unwrap()[0].details, InventoryDetails::default());

        assert!(store.remove("B2").await.unwrap());
        assert!(!store.remove("B2").await.unwrap());
        assert_eq!(store.records().await.unwrap().len(), 1);
        db.close().await;
    }

    #[tokio::test]
    async fn the_last_time_a_disk_was_seen_never_goes_back() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteDiskInventory::new(db.clone());
        let at = |seconds| Timestamp::from_unix_seconds(seconds);
        store.seen(&[seen("A1", "HDD")], at(2_000_000)).await.unwrap();

        // The clock set back by a day (an RTC before NTP), and the model
        // changed, which is written at once.
        store.seen(&[seen("A1", "HDD v2")], at(1_913_600)).await.unwrap();
        store.seen(&[seen("A1", "HDD v2")], at(1_000_000)).await.unwrap();

        let record = store.records().await.unwrap().remove(0);
        assert_eq!((record.model.as_deref(), record.last_seen), (Some("HDD v2"), at(2_000_000)));
        store.seen(&[seen("A1", "HDD v2")], at(2_003_600)).await.unwrap();
        assert_eq!(store.records().await.unwrap()[0].last_seen, at(2_003_600));
        db.close().await;
    }

    #[tokio::test]
    async fn serial_numbers_seen_shared_are_kept_for_good() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteDiskInventory::new(db.clone());
        assert!(store.shared_serials().await.unwrap().is_empty());

        store.note_shared(&BTreeSet::from(["BRIDGE1".to_owned()])).await.unwrap();
        store.note_shared(&BTreeSet::from(["BRIDGE1".to_owned(), "BRIDGE2".to_owned()])).await.unwrap();

        let shared = store.shared_serials().await.unwrap();
        assert_eq!(shared, BTreeSet::from(["BRIDGE1".to_owned(), "BRIDGE2".to_owned()]));
        db.close().await;
    }

    /// Before, the inventory kept the disks of a USB dock by serial number
    /// and port while both were there, and by the serial number alone
    /// while one was. The update knows those serial numbers as shared, and
    /// drops the record by the serial number alone unless the owner noted
    /// something on it.
    #[tokio::test]
    async fn the_update_learns_shared_serials_from_the_inventory_kept_before() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("before.db").display());
        let options = <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str(&url)
            .unwrap()
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
        let migrator = sqlx::migrate!("./migrations");
        for migration in migrator.iter().filter(|m| m.version < 46) {
            sqlx::raw_sql(migration.sql.clone()).execute(&pool).await.unwrap();
        }
        sqlx::raw_sql(
            "INSERT INTO disk_inventory (serial, size_bytes, note, first_seen, last_seen) VALUES
                 ('BRIDGE1', 1, NULL, 0, 0),
                 ('BRIDGE1@pci-usb-0:2:0', 1, NULL, 0, 0),
                 ('BRIDGE1@pci-usb-0:2:1', 1, NULL, 0, 0),
                 ('DOCK2', 1, 'the one in bay 2', 0, 0),
                 ('DOCK2@pci-usb-0:3:0', 1, NULL, 0, 0),
                 ('DOCK2@pci-usb-0:3:1', 1, NULL, 0, 0),
                 ('WD-1', 1, NULL, 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let update = migrator.iter().find(|m| m.version == 46).unwrap();
        sqlx::raw_sql(update.sql.clone()).execute(&pool).await.unwrap();

        let shared: Vec<String> =
            sqlx::query_scalar("SELECT serial FROM disk_shared_serials ORDER BY serial")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(shared, ["BRIDGE1", "DOCK2"]);
        let kept: Vec<String> = sqlx::query_scalar("SELECT serial FROM disk_inventory ORDER BY serial")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            kept,
            [
                "BRIDGE1@pci-usb-0:2:0",
                "BRIDGE1@pci-usb-0:2:1",
                "DOCK2",
                "DOCK2@pci-usb-0:3:0",
                "DOCK2@pci-usb-0:3:1",
                "WD-1"
            ],
            "the record by the serial number alone goes, unless something was noted on it"
        );
        pool.close().await;
    }
}
