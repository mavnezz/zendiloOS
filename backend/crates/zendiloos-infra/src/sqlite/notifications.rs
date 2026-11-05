use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_notify::{Level, Marks, NewNotification, Notification, NotificationRepository};

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

    async fn recent(&self, after: i64, limit: u32) -> Result<Vec<Notification>, PortError> {
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", at, level, code, params FROM notifications
               WHERE id > ? ORDER BY id DESC LIMIT ?"#,
            after,
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

    async fn prune(&self, keep: u32) -> Result<(), PortError> {
        let keep = i64::from(keep);
        sqlx::query!(
            "DELETE FROM notifications WHERE id <= (SELECT id FROM notifications ORDER BY id DESC LIMIT 1 OFFSET ?)",
            keep
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn marks(&self, account_id: i64) -> Result<Marks, PortError> {
        let row = sqlx::query!(
            "SELECT read_up_to, cleared_up_to FROM notification_marks WHERE account_id = ?",
            account_id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(row.map_or_else(Marks::default, |row| Marks {
            read_up_to: row.read_up_to,
            cleared_up_to: row.cleared_up_to,
        }))
    }

    async fn advance_marks(&self, account_id: i64, to: Marks) -> Result<(), PortError> {
        sqlx::query!(
            "INSERT INTO notification_marks (account_id, read_up_to, cleared_up_to) VALUES (?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET
                 read_up_to = max(read_up_to, excluded.read_up_to),
                 cleared_up_to = max(cleared_up_to, excluded.cleared_up_to)",
            account_id,
            to.read_up_to,
            to.cleared_up_to
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
    use crate::sqlite::tests::{insert_account, temporary_database};

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

        let recent = repository.recent(0, 10).await.unwrap();
        assert_eq!(
            recent.iter().map(|n| n.code.as_str()).collect::<Vec<_>>(),
            ["disk_health", "scrub_finished"]
        );
        assert_eq!(recent[0].level, Level::Error);
        assert_eq!(recent[0].params["health"], "failing");
        assert_eq!(repository.recent(recent[1].id, 10).await.unwrap(), recent[..1]);
        db.close().await;
    }

    #[tokio::test]
    async fn prunes_all_but_the_most_recent() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteNotifications::new(db.clone());
        let at = Timestamp::from_unix_seconds(10);
        for _ in 0..5 {
            repository.create(at, NewNotification::new(Level::Info, "scrub_finished")).await.unwrap();
        }

        repository.prune(2).await.unwrap();
        repository.prune(10).await.unwrap();

        let ids: Vec<i64> = repository.recent(0, 10).await.unwrap().iter().map(|n| n.id).collect();
        assert_eq!(ids, [5, 4]);
        db.close().await;
    }

    #[tokio::test]
    async fn marks_only_advance_and_go_with_the_account() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteNotifications::new(db.clone());
        let alice = insert_account(db.pool(), "alice", "admin").await.unwrap();
        assert_eq!(repository.marks(alice).await.unwrap(), Marks::default());

        repository.advance_marks(alice, Marks { read_up_to: 7, cleared_up_to: 3 }).await.unwrap();
        repository.advance_marks(alice, Marks { read_up_to: 5, cleared_up_to: 4 }).await.unwrap();
        assert_eq!(repository.marks(alice).await.unwrap(), Marks { read_up_to: 7, cleared_up_to: 4 });

        sqlx::query("DELETE FROM accounts WHERE id = ?").bind(alice).execute(db.pool()).await.unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification_marks").fetch_one(db.pool()).await.unwrap();
        assert_eq!(left, 0);
        db.close().await;
    }
}
