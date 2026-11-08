use async_trait::async_trait;
use zendiloos_identity::application::ports::TwoFactorRepository;
use zendiloos_identity::domain::{AccountId, RecoveryHash, TotpSecret, TwoFactor};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`TwoFactorRepository`] on the `account_two_factor` table.
#[derive(Debug, Clone)]
pub struct SqliteTwoFactor {
    db: Database,
}

impl SqliteTwoFactor {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("two-factor", error)
}

fn secret(text: Option<&str>) -> Result<Option<TotpSecret>, PortError> {
    text.map(|text| {
        TotpSecret::from_base32(text)
            .ok_or_else(|| PortError::new("two-factor", "a stored secret is not base32"))
    })
    .transpose()
}

fn step(value: Option<i64>) -> Result<Option<u64>, PortError> {
    value
        .map(|v| u64::try_from(v).map_err(|_| PortError::new("two-factor", "a stored time step is negative")))
        .transpose()
}

#[async_trait]
impl TwoFactorRepository for SqliteTwoFactor {
    async fn find(&self, account: AccountId) -> Result<Option<TwoFactor>, PortError> {
        let id = account.get();
        let Some(row) = sqlx::query!(
            "SELECT secret, pending_secret, last_step, recovery_codes, enabled_at
             FROM account_two_factor WHERE account_id = ?",
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        else {
            return Ok(None);
        };
        Ok(Some(TwoFactor {
            account_id: account,
            secret: secret(row.secret.as_deref())?,
            pending: secret(row.pending_secret.as_deref())?,
            last_step: step(row.last_step)?,
            recovery: row.recovery_codes.lines().filter(|l| !l.is_empty()).map(RecoveryHash::new).collect(),
            enabled_at: row.enabled_at.map(Timestamp::from_unix_seconds),
        }))
    }

    async fn save(&self, two_factor: &TwoFactor) -> Result<(), PortError> {
        let id = two_factor.account_id.get();
        let secret = two_factor.secret.as_ref().map(TotpSecret::to_base32);
        let pending = two_factor.pending.as_ref().map(TotpSecret::to_base32);
        let last_step = two_factor
            .last_step
            .map(i64::try_from)
            .transpose()
            .map_err(|_| PortError::new("two-factor", "the time step is out of range"))?;
        let recovery: Vec<&str> = two_factor.recovery.iter().map(RecoveryHash::as_str).collect();
        let recovery = recovery.join("\n");
        let enabled_at = two_factor.enabled_at.map(Timestamp::unix_seconds);
        sqlx::query!(
            "INSERT INTO account_two_factor (account_id, secret, pending_secret, last_step, recovery_codes, enabled_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET secret = excluded.secret,
                pending_secret = excluded.pending_secret, last_step = excluded.last_step,
                recovery_codes = excluded.recovery_codes, enabled_at = excluded.enabled_at",
            id,
            secret,
            pending,
            last_step,
            recovery,
            enabled_at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn delete(&self, account: AccountId) -> Result<(), PortError> {
        let id = account.get();
        sqlx::query!("DELETE FROM account_two_factor WHERE account_id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn enabled(&self) -> Result<Vec<AccountId>, PortError> {
        let ids = sqlx::query_scalar!(
            "SELECT account_id FROM account_two_factor WHERE secret IS NOT NULL ORDER BY account_id"
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(ids.into_iter().map(AccountId::new).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::{insert_account, temporary_database};

    #[tokio::test]
    async fn keeps_each_accounts_second_factor() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteTwoFactor::new(db.clone());
        let alice = AccountId::new(insert_account(db.pool(), "alice", "admin").await.unwrap());
        let bob = AccountId::new(insert_account(db.pool(), "bob", "user").await.unwrap());
        assert_eq!(repository.find(alice).await.unwrap(), None);

        let mut setting_up = TwoFactor::new(alice);
        setting_up.pending = Some(TotpSecret::from_bytes([7; 20]));
        repository.save(&setting_up).await.unwrap();
        assert_eq!(repository.find(alice).await.unwrap(), Some(setting_up));
        assert!(repository.enabled().await.unwrap().is_empty(), "not on while being set up");

        let on = TwoFactor {
            account_id: alice,
            secret: Some(TotpSecret::from_bytes([7; 20])),
            pending: None,
            last_step: Some(56_666_666),
            recovery: vec![RecoveryHash::new("aa11"), RecoveryHash::new("bb22")],
            enabled_at: Some(Timestamp::from_unix_seconds(1_700_000_000)),
        };
        repository.save(&on).await.unwrap();
        let mut bobs = TwoFactor::new(bob);
        bobs.pending = Some(TotpSecret::from_bytes([9; 20]));
        repository.save(&bobs).await.unwrap();

        assert_eq!(repository.find(alice).await.unwrap(), Some(on.clone()));
        assert_eq!(repository.enabled().await.unwrap(), vec![alice]);
        let spent = TwoFactor { recovery: vec![], ..on };
        repository.save(&spent).await.unwrap();
        assert_eq!(repository.find(alice).await.unwrap().unwrap().recovery, vec![]);

        repository.delete(alice).await.unwrap();
        assert_eq!(repository.find(alice).await.unwrap(), None);
        assert_eq!(repository.find(bob).await.unwrap(), Some(bobs));
        db.close().await;
    }

    #[tokio::test]
    async fn goes_with_its_account() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteTwoFactor::new(db.clone());
        let alice = insert_account(db.pool(), "alice", "admin").await.unwrap();
        let mut on = TwoFactor::new(AccountId::new(alice));
        on.secret = Some(TotpSecret::from_bytes([1; 20]));
        on.enabled_at = Some(Timestamp::from_unix_seconds(1_700_000_000));
        repository.save(&on).await.unwrap();

        sqlx::query("DELETE FROM accounts WHERE id = ?").bind(alice).execute(db.pool()).await.unwrap();

        assert_eq!(repository.find(AccountId::new(alice)).await.unwrap(), None);
        db.close().await;
    }

    #[tokio::test]
    async fn the_database_refuses_a_secret_without_its_time() {
        let (_dir, db) = temporary_database().await;
        let alice = insert_account(db.pool(), "alice", "admin").await.unwrap();

        let stored = sqlx::query(
            "INSERT INTO account_two_factor (account_id, secret) VALUES (?, 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ')",
        )
        .bind(alice)
        .execute(db.pool())
        .await;

        assert!(stored.is_err());
        db.close().await;
    }
}
