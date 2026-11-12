//! zendiloOS's network settings as last confirmed: its systemd-networkd
//! files' contents.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_network::application::{KeptSettings, RestoredSettings, SettingsStore};
use zendiloos_network::domain::NetworkdFile;

use super::Database;

/// [`SettingsStore`] on `network_settings` and `network_setting_files`.
#[derive(Debug, Clone)]
pub struct SqliteNetworkSettings {
    db: Database,
}

impl SqliteNetworkSettings {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// At the first start after a restore, before the files are looked at:
    /// the settings the restored database keeps are the backed-up
    /// machine's, so they become the restored ones (replacing any earlier
    /// restore's) and this machine keeps none, which the next look at the
    /// files takes as they are. Nothing kept, nothing changes: done twice,
    /// the second time leaves the first's.
    pub async fn set_restored_aside(&self, at: Timestamp) -> Result<bool, PortError> {
        let at = at.unix_seconds();
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let kept = sqlx::query_scalar!("SELECT count(*) FROM network_settings")
            .fetch_one(&mut *transaction)
            .await
            .map_err(db_error)?;
        if kept == 0 {
            return Ok(false);
        }
        for statement in [
            sqlx::query!("DELETE FROM network_restored_files"),
            sqlx::query!("DELETE FROM network_restored"),
            sqlx::query!(
                "INSERT INTO network_restored_files (name, text) SELECT name, text FROM network_setting_files"
            ),
        ] {
            statement.execute(&mut *transaction).await.map_err(db_error)?;
        }
        sqlx::query!("INSERT INTO network_restored (id, restored_at) VALUES (1, ?)", at)
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for statement in
            [sqlx::query!("DELETE FROM network_setting_files"), sqlx::query!("DELETE FROM network_settings")]
        {
            statement.execute(&mut *transaction).await.map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)?;
        Ok(true)
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("network settings", error)
}

#[async_trait]
impl SettingsStore for SqliteNetworkSettings {
    async fn kept(&self) -> Result<Option<KeptSettings>, PortError> {
        let pool = self.db.pool();
        let Some(row) = sqlx::query!("SELECT kept_at, said FROM network_settings WHERE id = 1")
            .fetch_optional(pool)
            .await
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let files = sqlx::query_as!(
            NetworkdFile,
            r#"SELECT name AS "name!", text FROM network_setting_files ORDER BY name"#
        )
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        Ok(Some(KeptSettings { files, kept_at: Timestamp::from_unix_seconds(row.kept_at), said: row.said }))
    }

    async fn keep(&self, files: &[NetworkdFile], at: Timestamp) -> Result<(), PortError> {
        let at = at.unix_seconds();
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM network_setting_files")
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for file in files {
            sqlx::query!(
                "INSERT INTO network_setting_files (name, text) VALUES (?, ?)",
                file.name,
                file.text
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        sqlx::query!(
            "INSERT INTO network_settings (id, kept_at, said) VALUES (1, ?, NULL)
             ON CONFLICT (id) DO UPDATE SET kept_at = excluded.kept_at, said = NULL",
            at
        )
        .execute(&mut *transaction)
        .await
        .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)
    }

    async fn note_said(&self, said: Option<&str>) -> Result<(), PortError> {
        sqlx::query!("UPDATE network_settings SET said = ? WHERE id = 1", said)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn restored(&self) -> Result<Option<RestoredSettings>, PortError> {
        let pool = self.db.pool();
        let Some(at) = sqlx::query_scalar!("SELECT restored_at FROM network_restored WHERE id = 1")
            .fetch_optional(pool)
            .await
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let files = sqlx::query_as!(
            NetworkdFile,
            r#"SELECT name AS "name!", text FROM network_restored_files ORDER BY name"#
        )
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
        Ok((!files.is_empty())
            .then(|| RestoredSettings { files, restored_at: Timestamp::from_unix_seconds(at) }))
    }

    async fn forget_restored(&self) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        for statement in
            [sqlx::query!("DELETE FROM network_restored_files"), sqlx::query!("DELETE FROM network_restored")]
        {
            statement.execute(&mut *transaction).await.map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn file(name: &str, text: &str) -> NetworkdFile {
        NetworkdFile { name: format!("05-zendiloos-{name}"), text: text.into() }
    }

    #[tokio::test]
    async fn the_files_are_kept_whole_as_confirmed_and_what_was_said_with_them() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteNetworkSettings::new(db.clone());
        assert_eq!(store.kept().await.unwrap(), None, "nothing kept yet");
        store.note_said(Some("before anything")).await.unwrap();
        assert_eq!(store.kept().await.unwrap(), None);

        let first =
            [file("enp1s0.network", "[Match]\nName=enp1s0\n"), file("br0.netdev", "[NetDev]\nName=br0\n")];
        store.keep(&first, Timestamp::from_unix_seconds(100)).await.unwrap();
        store.note_said(Some("interface enp1s0 changed")).await.unwrap();
        let kept = store.kept().await.unwrap().unwrap();
        assert_eq!(kept.files, [first[1].clone(), first[0].clone()], "by name");
        assert_eq!(kept.kept_at, Timestamp::from_unix_seconds(100));
        assert_eq!(kept.said.as_deref(), Some("interface enp1s0 changed"));

        // Kept anew: the files replaced, nothing said of them yet.
        store.keep(&[file("enp2s0.network", "x")], Timestamp::from_unix_seconds(200)).await.unwrap();
        let kept = store.kept().await.unwrap().unwrap();
        assert_eq!((kept.files.len(), kept.said), (1, None));
        // No files at all is something kept too.
        store.keep(&[], Timestamp::from_unix_seconds(300)).await.unwrap();
        assert!(store.kept().await.unwrap().unwrap().files.is_empty());
        // Only zendiloOS's files.
        assert!(
            store
                .keep(
                    &[NetworkdFile { name: "10-netplan.network".into(), text: String::new() }],
                    Timestamp::from_unix_seconds(1)
                )
                .await
                .is_err()
        );
        db.close().await;
    }

    #[tokio::test]
    async fn a_restores_settings_are_set_aside_once_and_this_machine_keeps_none() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteNetworkSettings::new(db.clone());
        assert!(!store.set_restored_aside(Timestamp::from_unix_seconds(5)).await.unwrap(), "nothing kept");
        assert_eq!(store.restored().await.unwrap(), None);

        let backed_up = [file("enp3s0.network", "[Match]\nName=enp3s0\n")];
        store.keep(&backed_up, Timestamp::from_unix_seconds(100)).await.unwrap();
        assert!(store.set_restored_aside(Timestamp::from_unix_seconds(200)).await.unwrap());
        assert_eq!(store.kept().await.unwrap(), None, "the next look keeps this machine's files");
        let restored = store.restored().await.unwrap().unwrap();
        assert_eq!(
            (restored.files, restored.restored_at),
            (backed_up.to_vec(), Timestamp::from_unix_seconds(200))
        );

        // The start repeated: nothing kept, the restored ones stay.
        assert!(!store.set_restored_aside(Timestamp::from_unix_seconds(300)).await.unwrap());
        assert_eq!(store.restored().await.unwrap().unwrap().restored_at, Timestamp::from_unix_seconds(200));
        store.forget_restored().await.unwrap();
        assert_eq!(store.restored().await.unwrap(), None);
        db.close().await;
    }
}
