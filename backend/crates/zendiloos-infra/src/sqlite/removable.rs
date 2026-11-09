use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::PortError;
use zendiloos_storage::application::ports::EjectedDrives;

use super::Database;

/// [`EjectedDrives`] on the `removable_ejected` table.
#[derive(Debug, Clone)]
pub struct SqliteEjectedDrives {
    db: Database,
}

impl SqliteEjectedDrives {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn ejected_error(error: sqlx::Error) -> PortError {
    PortError::new("ejected drives", error)
}

#[async_trait]
impl EjectedDrives for SqliteEjectedDrives {
    async fn ejected(&self) -> Result<BTreeMap<String, Option<String>>, PortError> {
        let rows = sqlx::query!("SELECT id, plugged_in FROM removable_ejected")
            .fetch_all(self.db.pool())
            .await
            .map_err(ejected_error)?;
        Ok(rows.into_iter().map(|row| (row.id, row.plugged_in)).collect())
    }

    async fn eject(&self, id: &str, plugged_in: Option<&str>) -> Result<(), PortError> {
        sqlx::query!(
            "INSERT INTO removable_ejected (id, plugged_in) VALUES (?, ?) \
             ON CONFLICT (id) DO UPDATE SET plugged_in = excluded.plugged_in",
            id,
            plugged_in
        )
        .execute(self.db.pool())
        .await
        .map_err(ejected_error)?;
        Ok(())
    }

    async fn forget(&self, id: &str) -> Result<(), PortError> {
        sqlx::query!("DELETE FROM removable_ejected WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(ejected_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn an_ejected_drive_is_kept_with_its_plugging_in_until_forgotten() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteEjectedDrives::new(db.clone());
        assert!(store.ejected().await.unwrap().is_empty());

        store.eject("1A2B-3C4D", Some("boot/7")).await.unwrap();
        store.eject("9F", None).await.unwrap();
        // Ejected again after it was plugged in anew.
        store.eject("1A2B-3C4D", Some("boot/9")).await.unwrap();

        let ejected = store.ejected().await.unwrap();
        let expected =
            BTreeMap::from([("1A2B-3C4D".to_owned(), Some("boot/9".to_owned())), ("9F".to_owned(), None)]);
        assert_eq!(ejected, expected);
        store.forget("1A2B-3C4D").await.unwrap();
        store.forget("never ejected").await.unwrap();
        assert_eq!(store.ejected().await.unwrap().into_keys().collect::<Vec<_>>(), ["9F"]);
        db.close().await;
    }
}
