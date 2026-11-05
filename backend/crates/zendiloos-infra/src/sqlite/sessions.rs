use std::time::Duration;

use async_trait::async_trait;
use zendiloos_identity::application::ports::SessionRepository;
use zendiloos_identity::domain::{AccountId, NewSession, Session, SessionId, TokenHash};
use zendiloos_kernel::{PortError, Secret, Timestamp};

use super::Database;

/// [`SessionRepository`] on the `sessions` table.
#[derive(Debug, Clone)]
pub struct SqliteSessions {
    db: Database,
}

impl SqliteSessions {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

/// User agents are sent by the client; store only a bounded prefix.
const MAX_USER_AGENT_CHARS: usize = 256;

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("sessions", error)
}

fn affected(result: sqlx::sqlite::SqliteQueryResult) -> u64 {
    result.rows_affected()
}

struct SessionRow {
    id: i64,
    account_id: i64,
    token_hash: Vec<u8>,
    csrf_token: String,
    created_at: i64,
    last_seen_at: i64,
    expires_at: i64,
}

impl TryFrom<SessionRow> for Session {
    type Error = PortError;

    fn try_from(row: SessionRow) -> Result<Self, PortError> {
        let digest: [u8; 32] = row
            .token_hash
            .try_into()
            .map_err(|_| PortError::new("stored session", "token hash is not 32 bytes"))?;
        Ok(Session {
            id: SessionId::new(row.id),
            account_id: AccountId::new(row.account_id),
            token_hash: TokenHash::new(digest),
            csrf_token: Secret::new(row.csrf_token),
            created_at: Timestamp::from_unix_seconds(row.created_at),
            last_seen_at: Timestamp::from_unix_seconds(row.last_seen_at),
            expires_at: Timestamp::from_unix_seconds(row.expires_at),
        })
    }
}

#[async_trait]
impl SessionRepository for SqliteSessions {
    async fn create(&self, session: NewSession) -> Result<SessionId, PortError> {
        let token_hash = &session.token_hash.as_bytes()[..];
        let account_id = session.account_id.get();
        let csrf_token = session.csrf_token.expose();
        let client_ip = session.client_ip.map(|ip| ip.to_string());
        let user_agent =
            session.user_agent.map(|ua| ua.chars().take(MAX_USER_AGENT_CHARS).collect::<String>());
        let (created_at, expires_at) = (session.created_at.unix_seconds(), session.expires_at.unix_seconds());
        let id = sqlx::query_scalar!(
            r#"INSERT INTO sessions
                   (token_hash, account_id, csrf_token, client_ip, user_agent, created_at, last_seen_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)
               RETURNING id AS "id!""#,
            token_hash,
            account_id,
            csrf_token,
            client_ip,
            user_agent,
            created_at,
            expires_at
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(SessionId::new(id))
    }

    async fn find_by_token_hash(&self, hash: &TokenHash) -> Result<Option<Session>, PortError> {
        let hash = &hash.as_bytes()[..];
        sqlx::query_as!(
            SessionRow,
            "SELECT id, account_id, token_hash, csrf_token, created_at, last_seen_at, expires_at
             FROM sessions WHERE token_hash = ?",
            hash
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        .map(Session::try_from)
        .transpose()
    }

    async fn touch(&self, id: SessionId, at: Timestamp) -> Result<(), PortError> {
        let (id, at) = (id.get(), at.unix_seconds());
        sqlx::query!("UPDATE sessions SET last_seen_at = ? WHERE id = ?", at, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn delete(&self, id: SessionId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("DELETE FROM sessions WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn delete_others(&self, account: AccountId, keep: SessionId) -> Result<u64, PortError> {
        let (account, keep) = (account.get(), keep.get());
        sqlx::query!("DELETE FROM sessions WHERE account_id = ? AND id <> ?", account, keep)
            .execute(self.db.pool())
            .await
            .map(affected)
            .map_err(db_error)
    }

    async fn delete_all(&self, account: AccountId) -> Result<u64, PortError> {
        let account = account.get();
        sqlx::query!("DELETE FROM sessions WHERE account_id = ?", account)
            .execute(self.db.pool())
            .await
            .map(affected)
            .map_err(db_error)
    }

    async fn delete_inactive(&self, now: Timestamp, idle_timeout: Duration) -> Result<u64, PortError> {
        // The complement of Session::is_active.
        let idle_since = now.saturating_sub(idle_timeout).unix_seconds();
        let now = now.unix_seconds();
        sqlx::query!("DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?", now, idle_since)
            .execute(self.db.pool())
            .await
            .map(affected)
            .map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_identity::application::ports::AccountRepository;
    use zendiloos_identity::domain::{AccountName, NewAccount, PasswordHash, Role};

    use super::*;
    use crate::sqlite::SqliteAccounts;
    use crate::sqlite::tests::temporary_database;

    async fn account(db: &Database) -> AccountId {
        let new = NewAccount {
            name: AccountName::parse("alice").unwrap(),
            full_name: None,
            role: Role::Admin,
            password_hash: PasswordHash::new("hash"),
            password_change_required: false,
            created_at: Timestamp::from_unix_seconds(0),
        };
        SqliteAccounts::new(db.clone()).create_first_admin(new).await.unwrap().unwrap().id
    }

    fn new_session(account_id: AccountId, hash_byte: u8, created: i64) -> NewSession {
        NewSession {
            account_id,
            token_hash: TokenHash::new([hash_byte; 32]),
            csrf_token: Secret::new(format!("csrf-{hash_byte}")),
            client_ip: Some("192.0.2.1".parse().unwrap()),
            user_agent: Some("x".repeat(1000)),
            created_at: Timestamp::from_unix_seconds(created),
            expires_at: Timestamp::from_unix_seconds(created + 3600),
        }
    }

    #[tokio::test]
    async fn stores_and_finds_sessions_by_token_hash() {
        let (_dir, db) = temporary_database().await;
        let sessions = SqliteSessions::new(db.clone());
        let alice = account(&db).await;

        let id = sessions.create(new_session(alice, 7, 1000)).await.unwrap();
        sessions.touch(id, Timestamp::from_unix_seconds(1100)).await.unwrap();

        let found = sessions.find_by_token_hash(&TokenHash::new([7; 32])).await.unwrap().unwrap();
        assert_eq!((found.id, found.account_id), (id, alice));
        assert_eq!(found.csrf_token.expose(), "csrf-7");
        assert_eq!(found.last_seen_at, Timestamp::from_unix_seconds(1100));
        assert_eq!(found.expires_at, Timestamp::from_unix_seconds(4600));
        assert!(sessions.find_by_token_hash(&TokenHash::new([8; 32])).await.unwrap().is_none());

        let user_agent: String =
            sqlx::query_scalar("SELECT user_agent FROM sessions").fetch_one(db.pool()).await.unwrap();
        assert_eq!(user_agent.len(), 256, "user agents are truncated");
        db.close().await;
    }

    #[tokio::test]
    async fn deletes_other_and_inactive_sessions() {
        let (_dir, db) = temporary_database().await;
        let sessions = SqliteSessions::new(db.clone());
        let alice = account(&db).await;
        let first = sessions.create(new_session(alice, 1, 1000)).await.unwrap();
        sessions.create(new_session(alice, 2, 1000)).await.unwrap();
        let third = sessions.create(new_session(alice, 3, 2000)).await.unwrap();

        assert_eq!(sessions.delete_others(alice, third).await.unwrap(), 2);
        assert!(sessions.find_by_token_hash(&TokenHash::new([1; 32])).await.unwrap().is_none());

        let idle = Duration::from_secs(1800);
        assert_eq!(sessions.delete_inactive(Timestamp::from_unix_seconds(3799), idle).await.unwrap(), 0);
        assert_eq!(sessions.delete_inactive(Timestamp::from_unix_seconds(3800), idle).await.unwrap(), 1);
        sessions.delete(first).await.unwrap();
        db.close().await;
    }
}
