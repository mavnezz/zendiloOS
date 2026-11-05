use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, AuditLog, PortError};

use super::Database;

/// [`AuditLog`] on the `audit_log` table.
#[derive(Debug, Clone)]
pub struct SqliteAuditLog {
    db: Database,
}

impl SqliteAuditLog {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AuditLog for SqliteAuditLog {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        let at = entry.at.unix_seconds();
        let client_ip = entry.client_ip.map(|ip| ip.to_string());
        let outcome = entry.outcome.as_str();
        let details = entry.reason.map(|reason| serde_json::json!({ "reason": reason }).to_string());
        sqlx::query!(
            "INSERT INTO audit_log (at, account_id, actor, client_ip, action, outcome, request_id, details)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            at,
            entry.account_id,
            entry.actor,
            client_ip,
            entry.action,
            outcome,
            entry.request_id,
            details
        )
        .execute(self.db.pool())
        .await
        .map_err(|error| PortError::new("audit log", error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::{Actor, Outcome, RequestContext, Timestamp};

    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn records_entries_with_the_reason_as_json() {
        let (_dir, db) = temporary_database().await;
        let request = RequestContext {
            client_ip: Some("192.0.2.1".parse().unwrap()),
            user_agent: None,
            request_id: Some("req-1".to_owned()),
        };
        let entry =
            AuditEntry::new(Timestamp::from_unix_seconds(5), "identity.login", Outcome::Failure, &request)
                .by(&Actor { account_id: 3, name: "alice".to_owned() })
                .reason("wrong_password");

        SqliteAuditLog::new(db.clone()).record(entry).await.unwrap();

        let row: (i64, Option<i64>, String, String, String, String, String, String) = sqlx::query_as(
            "SELECT at, account_id, actor, client_ip, action, outcome, request_id, details FROM audit_log",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                5,
                Some(3),
                "alice".to_owned(),
                "192.0.2.1".to_owned(),
                "identity.login".to_owned(),
                "failure".to_owned(),
                "req-1".to_owned(),
                r#"{"reason":"wrong_password"}"#.to_owned()
            )
        );
        db.close().await;
    }
}
