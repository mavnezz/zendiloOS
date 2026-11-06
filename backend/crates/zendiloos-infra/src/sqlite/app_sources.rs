use async_trait::async_trait;
use zendiloos_apps::application::ports::SourceRepository;
use zendiloos_apps::domain::{Refresh, Source};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`SourceRepository`] on the `app_sources` and `app_icons` tables.
#[derive(Debug, Clone)]
pub struct SqliteAppSources {
    db: Database,
}

impl SqliteAppSources {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("app sources", error)
}

#[async_trait]
impl SourceRepository for SqliteAppSources {
    async fn list(&self) -> Result<Vec<Source>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", index_address, official AS "official: bool", added_at, refreshed_at,
                      refresh_error, skipped
               FROM app_sources ORDER BY official DESC, added_at, id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(|row| Source {
                id: row.id,
                index: row.index_address,
                official: row.official,
                added_at: Timestamp::from_unix_seconds(row.added_at),
                refreshed: row.refreshed_at.map(|at| Refresh {
                    at: Timestamp::from_unix_seconds(at),
                    error: row.refresh_error,
                    skipped: usize::try_from(row.skipped).unwrap_or(0),
                }),
            })
            .collect())
    }

    async fn add(&self, index: &str, at: Timestamp) -> Result<Option<Source>, PortError> {
        let at_seconds = at.unix_seconds();
        let added = sqlx::query!(
            "INSERT INTO app_sources (index_address, added_at) VALUES (?, ?) ON CONFLICT (index_address) DO NOTHING",
            index,
            at_seconds
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        if added.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(Source {
            id: added.last_insert_rowid(),
            index: index.to_owned(),
            official: false,
            added_at: at,
            refreshed: None,
        }))
    }

    async fn remove(&self, id: i64) -> Result<bool, PortError> {
        let removed = sqlx::query!("DELETE FROM app_sources WHERE id = ? AND official = 0", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(removed.rows_affected() > 0)
    }

    async fn index(&self, id: i64) -> Result<Option<String>, PortError> {
        let row = sqlx::query!("SELECT index_text FROM app_sources WHERE id = ?", id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(row.and_then(|row| row.index_text))
    }

    async fn refreshed(&self, id: i64, refresh: &Refresh, index: Option<&str>) -> Result<(), PortError> {
        let (at, skipped) = (refresh.at.unix_seconds(), i64::try_from(refresh.skipped).unwrap_or(i64::MAX));
        sqlx::query!(
            "UPDATE app_sources SET refreshed_at = ?, refresh_error = ?, skipped = ?,
                                    index_text = COALESCE(?, index_text)
             WHERE id = ?",
            at,
            refresh.error,
            skipped,
            index,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn icon(&self, id: i64, app: &str, digest: &str) -> Result<Option<Vec<u8>>, PortError> {
        let row = sqlx::query!(
            "SELECT png FROM app_icons WHERE source_id = ? AND app_id = ? AND digest = ?",
            id,
            app,
            digest
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(row.map(|row| row.png))
    }

    async fn store_icon(&self, id: i64, app: &str, digest: &str, png: &[u8]) -> Result<(), PortError> {
        sqlx::query!(
            "INSERT INTO app_icons (source_id, app_id, digest, png) VALUES (?, ?, ?, ?)
             ON CONFLICT (source_id, app_id) DO UPDATE SET digest = excluded.digest, png = excluded.png",
            id,
            app,
            digest,
            png
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn the_community_catalog_is_there_and_others_come_and_go() {
        let (_dir, db) = temporary_database().await;
        let sources = SqliteAppSources::new(db.clone());
        let at = Timestamp::from_unix_seconds(1_793_836_800);

        let official = sources.list().await.unwrap().remove(0);
        assert!(official.official);
        assert!(official.index.starts_with("https://mavnezz.github.io/"));
        assert!(!sources.remove(official.id).await.unwrap(), "built in");

        let added = sources.add("https://friend.github.io/apps/v1/index.json", at).await.unwrap().unwrap();
        assert!(sources.add("https://friend.github.io/apps/v1/index.json", at).await.unwrap().is_none());
        let fine = Refresh { at, error: None, skipped: 1 };
        sources.refreshed(added.id, &fine, Some("{\"format\":1}")).await.unwrap();
        let failed = Refresh { at, error: Some("refused".into()), skipped: 0 };
        sources.refreshed(added.id, &failed, None).await.unwrap();
        assert_eq!(sources.index(added.id).await.unwrap().as_deref(), Some("{\"format\":1}"), "kept");
        assert_eq!(sources.list().await.unwrap()[1].refreshed, Some(failed));

        sources.store_icon(added.id, "jellyfin", "sha256:a", b"png-a").await.unwrap();
        assert_eq!(
            sources.icon(added.id, "jellyfin", "sha256:a").await.unwrap().as_deref(),
            Some(&b"png-a"[..])
        );
        assert_eq!(sources.icon(added.id, "jellyfin", "sha256:b").await.unwrap(), None, "a new version");
        assert!(sources.remove(added.id).await.unwrap());
        assert_eq!(sources.list().await.unwrap().len(), 1);
        db.close().await;
    }
}
