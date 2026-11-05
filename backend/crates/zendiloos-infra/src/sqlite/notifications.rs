use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notification, NotificationRepository};

use super::Database;

/// [`NotificationRepository`] on the `notifications` table.
#[derive(Debug, Clone)]
pub struct SqliteNotifications {
    db: Database,
}

impl SqliteNotifications {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("notifications", error)
}

#[async_trait]
impl NotificationRepository for SqliteNotifications {
    async fn create(&self, at: Timestamp, new: NewNotification) -> Result<Notification, PortError> {
        let (seconds, level) = (at.unix_seconds(), new.level.as_str());
        let params = serde_json::to_string(&new.params).map_err(|e| PortError::new("notifications", e))?;
        let id = sqlx::query_scalar!(
            r#"INSERT INTO notifications (at, level, code, params) VALUES (?, ?, ?, ?) RETURNING id AS "id!""#,
            seconds,
            level,
            new.code,
            params
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(Notification { id, at, level: new.level, code: new.code, params: new.params })
    }

    async fn recent(&self, limit: u32) -> Result<Vec<Notification>, PortError> {
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", at, level, code, params FROM notifications ORDER BY id DESC LIMIT ?"#,
            limit
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let level = Level::parse(&row.level)
                    .ok_or_else(|| PortError::new("stored notification", format!("level {:?}", row.level)))?;
                let params: BTreeMap<String, String> = serde_json::from_str(&row.params)
                    .map_err(|e| PortError::new("stored notification", e))?;
                Ok(Notification {
                    id: row.id,
                    at: Timestamp::from_unix_seconds(row.at),
                    level,
                    code: row.code,
                    params,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn stores_notifications_with_parameters() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteNotifications::new(db.clone());
        let at = Timestamp::from_unix_seconds(10);
        repository
            .create(at, NewNotification::new(Level::Info, "scrub_finished").with("pool", "tank"))
            .await
            .unwrap();
        let second =
            NewNotification::new(Level::Error, "disk_health").with("disk", "sdd").with("health", "failing");
        repository.create(at, second).await.unwrap();

        let recent = repository.recent(10).await.unwrap();
        assert_eq!(
            recent.iter().map(|n| n.code.as_str()).collect::<Vec<_>>(),
            ["disk_health", "scrub_finished"]
        );
        assert_eq!(recent[0].level, Level::Error);
        assert_eq!(recent[0].params["health"], "failing");
        db.close().await;
    }
}
