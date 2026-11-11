//! The virtual machines' settings.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_vms::application::VmRepository;
use zendiloos_vms::domain::{Firmware, IsoOrigin, Network, OsPreset, Vm, VmSettings};

use super::Database;

/// [`VmRepository`] on the `vms` table.
#[derive(Debug, Clone)]
pub struct SqliteVms {
    db: Database,
}

impl SqliteVms {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("virtual machines", error)
}

fn stored(what: String) -> PortError {
    PortError::new("stored virtual machine", what)
}

struct Row {
    id: String,
    name: String,
    os: String,
    cpus: i64,
    memory_mib: i64,
    firmware: String,
    disk_gib: i64,
    bridge: Option<String>,
    autostart: bool,
    iso_share: Option<String>,
    iso_path: Option<String>,
    pool: String,
    volume: i64,
    created_at: i64,
    ready: bool,
}

fn vm(row: Row) -> Result<Vm, PortError> {
    let number = |value: i64, what: &str| stored(format!("{what} {value} of {}", row.id));
    Ok(Vm {
        settings: VmSettings {
            name: row.name,
            os: OsPreset::parse(&row.os).ok_or_else(|| stored(format!("os {}", row.os)))?,
            cpus: u16::try_from(row.cpus).map_err(|_| number(row.cpus, "cpus"))?,
            memory_mib: u32::try_from(row.memory_mib).map_err(|_| number(row.memory_mib, "memory"))?,
            firmware: Firmware::parse(&row.firmware)
                .ok_or_else(|| stored(format!("firmware {}", row.firmware)))?,
            disk_gib: u32::try_from(row.disk_gib).map_err(|_| number(row.disk_gib, "disk"))?,
            network: row.bridge.map_or(Network::User, Network::Bridge),
            autostart: row.autostart,
        },
        iso: row.iso_share.zip(row.iso_path).map(|(share, path)| IsoOrigin { share, path }),
        pool: row.pool,
        volume: u16::try_from(row.volume).map_err(|_| number(row.volume, "volume"))?,
        created_at: Timestamp::from_unix_seconds(row.created_at),
        ready: row.ready,
        id: row.id,
    })
}

#[async_trait]
impl VmRepository for SqliteVms {
    async fn list(&self) -> Result<Vec<Vm>, PortError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", name, os, cpus, memory_mib, firmware, disk_gib, bridge,
                      autostart AS "autostart: bool", iso_share, iso_path, pool, volume, created_at,
                      ready AS "ready: bool"
               FROM vms ORDER BY created_at, id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter().map(vm).collect()
    }

    async fn find(&self, id: &str) -> Result<Option<Vm>, PortError> {
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", name, os, cpus, memory_mib, firmware, disk_gib, bridge,
                      autostart AS "autostart: bool", iso_share, iso_path, pool, volume, created_at,
                      ready AS "ready: bool"
               FROM vms WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(vm).transpose()
    }

    async fn create(&self, vm: &Vm) -> Result<bool, PortError> {
        let settings = &vm.settings;
        let (os, firmware) = (settings.os.as_str(), settings.firmware.as_str());
        let (cpus, memory, disk) =
            (i64::from(settings.cpus), i64::from(settings.memory_mib), i64::from(settings.disk_gib));
        let bridge = match &settings.network {
            Network::User => None,
            Network::Bridge(bridge) => Some(bridge.as_str()),
        };
        let (iso_share, iso_path) = match &vm.iso {
            Some(iso) => (Some(iso.share.as_str()), Some(iso.path.as_str())),
            None => (None, None),
        };
        let (volume, created_at) = (i64::from(vm.volume), vm.created_at.unix_seconds());
        let done = sqlx::query!(
            "INSERT INTO vms (id, name, os, cpus, memory_mib, firmware, disk_gib, bridge, autostart, iso_share,
                              iso_path, pool, volume, created_at, ready)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            vm.id,
            settings.name,
            os,
            cpus,
            memory,
            firmware,
            disk,
            bridge,
            settings.autostart,
            iso_share,
            iso_path,
            vm.pool,
            volume,
            created_at,
            vm.ready
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn set_ready(&self, id: &str) -> Result<(), PortError> {
        sqlx::query!("UPDATE vms SET ready = 1 WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn set_autostart(&self, id: &str, autostart: bool) -> Result<bool, PortError> {
        let done = sqlx::query!("UPDATE vms SET autostart = ? WHERE id = ?", autostart, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM vms WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn vm(id: &str, name: &str) -> Vm {
        Vm {
            id: id.into(),
            settings: VmSettings {
                network: Network::Bridge("br0".into()),
                ..VmSettings::preset(name, OsPreset::Windows)
            },
            iso: Some(IsoOrigin { share: "media".into(), path: "isos/Win11.iso".into() }),
            pool: "tank".into(),
            volume: 1,
            created_at: Timestamp::from_unix_seconds(1_700_000_000),
            ready: false,
        }
    }

    #[tokio::test]
    async fn vms_are_kept_with_their_settings() {
        let (_dir, db) = temporary_database().await;
        let vms = SqliteVms::new(db);
        let first = vm("0a1b2c3d4e5f", "Büro-PC");
        assert!(vms.create(&first).await.unwrap());
        assert!(!vms.create(&vm("0a1b2c3d4e5f", "other")).await.unwrap(), "the id is taken");
        assert!(!vms.create(&vm("00000000000c", "Büro-PC")).await.unwrap(), "the name is taken");
        // SQLite folds ASCII only; the context compares names without case.
        assert!(!vms.create(&vm("00000000000a", "büro-pc")).await.unwrap(), "nor in other letters' case");
        let plain = Vm {
            iso: None,
            settings: VmSettings::preset("Debian", OsPreset::Linux),
            ..vm("00000000000d", "")
        };
        assert!(vms.create(&plain).await.unwrap());

        assert_eq!(vms.find("0a1b2c3d4e5f").await.unwrap(), Some(first.clone()));
        vms.set_ready("0a1b2c3d4e5f").await.unwrap();
        assert!(vms.set_autostart("0a1b2c3d4e5f", true).await.unwrap());
        let found = vms.find("0a1b2c3d4e5f").await.unwrap().unwrap();
        assert!(found.ready && found.settings.autostart);
        assert_eq!(found.settings.network, Network::Bridge("br0".into()));
        assert_eq!(vms.find("00000000000d").await.unwrap().unwrap().settings.network, Network::User);

        assert!(vms.delete("0a1b2c3d4e5f").await.unwrap());
        assert!(!vms.delete("0a1b2c3d4e5f").await.unwrap());
        assert!(vms.list().await.unwrap().iter().all(|vm| vm.id != "0a1b2c3d4e5f"));
    }
}
