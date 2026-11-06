use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, AuditLog, PortError, Timestamp};
use zendiloos_system::application::ports::AuditReader;
use zendiloos_system::domain::{AuditQuery, AuditRecord};

use super::Database;

/// [`AuditLog`] and [`AuditReader`] on the `audit_log` table.
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
            "INSERT INTO audit_log
                 (at, account_id, actor, client_ip, action, target, outcome, request_id, details)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            at,
            entry.account_id,
            entry.actor,
            client_ip,
            entry.action,
            entry.target,
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

#[async_trait]
impl AuditReader for SqliteAuditLog {
    async fn read(&self, query: &AuditQuery) -> Result<Vec<AuditRecord>, PortError> {
        let limit = i64::from(query.limit);
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", at, actor, client_ip, action, target, outcome, request_id, details
               FROM audit_log WHERE (?1 IS NULL OR id < ?1) AND (?3 IS NULL OR action = ?3)
                   AND (?4 IS NULL OR substr(action, 1, length(?4)) = ?4)
               ORDER BY id DESC LIMIT ?2"#,
            query.before,
            limit,
            query.action,
            query.prefix
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(|error| PortError::new("audit log", error))?;
        Ok(rows
            .into_iter()
            .map(|row| AuditRecord {
                id: row.id,
                at: Timestamp::from_unix_seconds(row.at),
                actor: row.actor,
                client_ip: row.client_ip,
                action: row.action,
                target: row.target,
                outcome: row.outcome,
                reason: row
                    .details
                    .and_then(|details| serde_json::from_str::<serde_json::Value>(&details).ok())
                    .and_then(|details| details.get("reason")?.as_str().map(str::to_owned)),
                request_id: row.request_id,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::{Actor, Outcome, RequestContext};

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

    #[tokio::test]
    async fn reads_the_newest_entries_first_in_pages() {
        let (_dir, db) = temporary_database().await;
        let audit = SqliteAuditLog::new(db.clone());
        for at in 1..=5 {
            let entry = AuditEntry::new(
                Timestamp::from_unix_seconds(at),
                "identity.account_create",
                Outcome::Success,
                &RequestContext::default(),
            )
            .actor("alice")
            .target(format!("user{at}"));
            audit.record(entry).await.unwrap();
        }

        let first =
            audit.read(&AuditQuery { limit: 2, before: None, action: None, prefix: None }).await.unwrap();
        let targets: Vec<_> = first.iter().map(|r| r.target.clone().unwrap()).collect();
        assert_eq!(targets, ["user5", "user4"]);
        let next = audit
            .read(&AuditQuery { limit: 10, before: Some(first[1].id), action: None, prefix: None })
            .await
            .unwrap();
        let targets: Vec<_> = next.iter().map(|r| r.target.clone().unwrap()).collect();
        assert_eq!(targets, ["user3", "user2", "user1"]);
        assert_eq!(next[0].actor.as_deref(), Some("alice"));
        assert_eq!(next[0].reason, None);
        let logins =
            AuditQuery { limit: 10, before: None, action: Some("identity.login".to_owned()), prefix: None };
        assert!(audit.read(&logins).await.unwrap().is_empty(), "only that action");
        let identity =
            AuditQuery { limit: 10, before: None, action: None, prefix: Some("identity.".to_owned()) };
        assert_eq!(audit.read(&identity).await.unwrap().len(), 5, "every action that starts so");
        let other =
            AuditQuery { limit: 10, before: None, action: None, prefix: Some("identity_".to_owned()) };
        assert!(audit.read(&other).await.unwrap().is_empty(), "no wildcards");
        db.close().await;
    }
}
