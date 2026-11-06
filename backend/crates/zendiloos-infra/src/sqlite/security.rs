use async_trait::async_trait;
use zendiloos_identity::application::ports::SecurityRepository;
use zendiloos_identity::domain::{AddressRange, AutoBlock, BlockedAddress, FailureLimit, SecuritySettings};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`SecurityRepository`] on `security_settings`, `allowed_addresses` and
/// `blocked_addresses`.
#[derive(Debug, Clone)]
pub struct SqliteSecurity {
    db: Database,
}

impl SqliteSecurity {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("security settings", error)
}

fn stored(what: &str) -> PortError {
    PortError::new("stored security settings", what.to_owned())
}

fn small(value: i64, what: &str) -> Result<u16, PortError> {
    u16::try_from(value).map_err(|_| stored(what))
}

fn limit(failures: Option<i64>, window: Option<i64>) -> Result<Option<FailureLimit>, PortError> {
    match (failures, window) {
        (Some(failures), Some(window)) => Ok(Some(FailureLimit {
            failures: small(failures, "failures")?,
            window_minutes: small(window, "window")?,
        })),
        _ => Ok(None),
    }
}

fn range(text: &str) -> Result<AddressRange, PortError> {
    AddressRange::parse(text).map_err(|e| PortError::new("stored address", e))
}

#[async_trait]
impl SecurityRepository for SqliteSecurity {
    async fn settings(&self) -> Result<SecuritySettings, PortError> {
        let row = sqlx::query!(
            "SELECT idle_minutes, lock_failures, lock_window_minutes, block_failures, block_window_minutes,
                    block_release_days
             FROM security_settings WHERE id = 1"
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let allowed = sqlx::query_scalar!("SELECT address FROM allowed_addresses ORDER BY rowid")
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?
            .iter()
            .map(|text| range(text))
            .collect::<Result<Vec<_>, _>>()?;
        let Some(row) = row else { return Ok(SecuritySettings { allowed, ..SecuritySettings::default() }) };
        let auto_block = limit(row.block_failures, row.block_window_minutes)?
            .map(|limit| {
                let release_after_days =
                    row.block_release_days.map(|days| small(days, "release")).transpose()?;
                Ok::<_, PortError>(AutoBlock { limit, release_after_days })
            })
            .transpose()?;
        Ok(SecuritySettings {
            idle_minutes: small(row.idle_minutes, "idle minutes")?,
            account_lock: limit(row.lock_failures, row.lock_window_minutes)?,
            auto_block,
            allowed,
        })
    }

    async fn save_settings(&self, settings: &SecuritySettings) -> Result<(), PortError> {
        let idle = i64::from(settings.idle_minutes);
        let lock = settings.account_lock;
        let (lock_failures, lock_window) =
            (lock.map(|l| i64::from(l.failures)), lock.map(|l| i64::from(l.window_minutes)));
        let block = settings.auto_block;
        let (block_failures, block_window, release) = (
            block.map(|b| i64::from(b.limit.failures)),
            block.map(|b| i64::from(b.limit.window_minutes)),
            block.and_then(|b| b.release_after_days).map(i64::from),
        );
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!(
            "INSERT INTO security_settings (id, idle_minutes, lock_failures, lock_window_minutes, block_failures,
                 block_window_minutes, block_release_days)
             VALUES (1, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET idle_minutes = excluded.idle_minutes,
                 lock_failures = excluded.lock_failures, lock_window_minutes = excluded.lock_window_minutes,
                 block_failures = excluded.block_failures, block_window_minutes = excluded.block_window_minutes,
                 block_release_days = excluded.block_release_days",
            idle,
            lock_failures,
            lock_window,
            block_failures,
            block_window,
            release
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query!("DELETE FROM allowed_addresses").execute(&mut *tx).await.map_err(db_error)?;
        for range in &settings.allowed {
            let text = range.to_string();
            sqlx::query!("INSERT INTO allowed_addresses (address) VALUES (?)", text)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)
    }

    async fn blocked(&self) -> Result<Vec<BlockedAddress>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT address, blocked_at, until, automatic AS "automatic: bool"
               FROM blocked_addresses ORDER BY blocked_at DESC, address"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(BlockedAddress {
                    range: range(&row.address)?,
                    blocked_at: Timestamp::from_unix_seconds(row.blocked_at),
                    until: row.until.map(Timestamp::from_unix_seconds),
                    automatic: row.automatic,
                })
            })
            .collect()
    }

    async fn block(&self, block: &BlockedAddress) -> Result<(), PortError> {
        let address = block.range.to_string();
        let (blocked_at, until) = (block.blocked_at.unix_seconds(), block.until.map(Timestamp::unix_seconds));
        sqlx::query!(
            "INSERT INTO blocked_addresses (address, blocked_at, until, automatic) VALUES (?, ?, ?, ?)
             ON CONFLICT (address) DO UPDATE SET blocked_at = excluded.blocked_at, until = excluded.until,
                 automatic = excluded.automatic",
            address,
            blocked_at,
            until,
            block.automatic
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn unblock(&self, range: &AddressRange) -> Result<bool, PortError> {
        let address = range.to_string();
        let result = sqlx::query!("DELETE FROM blocked_addresses WHERE address = ?", address)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_expired(&self, now: Timestamp) -> Result<u64, PortError> {
        let now = now.unix_seconds();
        let result =
            sqlx::query!("DELETE FROM blocked_addresses WHERE until IS NOT NULL AND until <= ?", now)
                .execute(self.db.pool())
                .await
                .map_err(db_error)?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    #[tokio::test]
    async fn keeps_the_settings_and_the_allowed_addresses_in_order() {
        let (_dir, db) = temporary_database().await;
        let security = SqliteSecurity::new(db);
        assert_eq!(security.settings().await.unwrap(), SecuritySettings::default());
        let settings = SecuritySettings {
            idle_minutes: 60,
            account_lock: None,
            auto_block: Some(AutoBlock {
                limit: FailureLimit { failures: 6, window_minutes: 10 },
                release_after_days: None,
            }),
            allowed: vec![
                AddressRange::parse("192.168.1.0/24").unwrap(),
                AddressRange::parse("fd00::1").unwrap(),
            ],
        };

        security.save_settings(&settings).await.unwrap();
        assert_eq!(security.settings().await.unwrap(), settings);
        let fewer = SecuritySettings { allowed: vec![settings.allowed[1]], ..SecuritySettings::default() };
        security.save_settings(&fewer).await.unwrap();
        assert_eq!(security.settings().await.unwrap(), fewer);
    }

    #[tokio::test]
    async fn blocks_until_lifted_or_over() {
        let (_dir, db) = temporary_database().await;
        let security = SqliteSecurity::new(db);
        let block = |text: &str, blocked_at: i64, until: Option<i64>| BlockedAddress {
            range: AddressRange::parse(text).unwrap(),
            blocked_at: at(blocked_at),
            until: until.map(at),
            automatic: until.is_some(),
        };
        security.block(&block("192.0.2.7", 10, Some(100))).await.unwrap();
        security.block(&block("198.51.100.0/24", 20, None)).await.unwrap();
        security.block(&block("192.0.2.7", 30, Some(200))).await.unwrap();

        let stored = security.blocked().await.unwrap();
        assert_eq!(stored, [block("192.0.2.7", 30, Some(200)), block("198.51.100.0/24", 20, None)]);
        assert_eq!(security.delete_expired(at(200)).await.unwrap(), 1);
        assert!(security.unblock(&AddressRange::parse("198.51.100.0/24").unwrap()).await.unwrap());
        assert!(!security.unblock(&AddressRange::parse("198.51.100.0/24").unwrap()).await.unwrap());
        assert!(security.blocked().await.unwrap().is_empty());
    }
}
