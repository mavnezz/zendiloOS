use async_trait::async_trait;
use zendiloos_kernel::{PortError, Secret, Timestamp};
use zendiloos_recovery::application::{Operators, PlanRepository};
use zendiloos_recovery::domain::{Frequency, LastRun, Plan, Schedule, Target};

use super::Database;

/// [`PlanRepository`] on `configuration_backup`, one row; none yet is the
/// default plan (off).
#[derive(Debug, Clone)]
pub struct SqliteConfigurationPlans {
    db: Database,
}

impl SqliteConfigurationPlans {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("configuration backup", error)
}

fn stored(what: &'static str) -> PortError {
    PortError::new("stored configuration backup", what)
}

#[async_trait]
impl PlanRepository for SqliteConfigurationPlans {
    async fn get(&self) -> Result<Plan, PortError> {
        let row = sqlx::query!(
            r#"SELECT enabled AS "enabled: bool", frequency, weekday, hour, minute, target_kind, share_id, drive,
                      folder, keep, passphrase, account_id, since, last_at, last_ok AS "last_ok: bool", last_file,
                      last_error
               FROM configuration_backup WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(Plan::default()) };
        let small = |value: i64, what| u8::try_from(value).map_err(|_| stored(what));
        let frequency = match (row.frequency.as_str(), row.weekday) {
            ("daily", _) => Frequency::Daily,
            ("weekly", Some(day)) => Frequency::Weekly { weekday: small(day, "weekday")? },
            _ => return Err(stored("frequency")),
        };
        let target = match (row.target_kind.as_deref(), row.share_id, row.drive) {
            (Some("share"), Some(share_id), _) => Some(Target::Share { share_id, folder: row.folder }),
            (Some("drive"), _, Some(drive)) => Some(Target::Drive { drive, folder: row.folder }),
            _ => None,
        };
        let last = row.last_at.map(|at| LastRun {
            at: Timestamp::from_unix_seconds(at),
            succeeded: row.last_ok.unwrap_or(false),
            file: row.last_file,
            error: row.last_error,
        });
        Ok(Plan {
            enabled: row.enabled,
            schedule: Schedule {
                frequency,
                hour: small(row.hour, "hour")?,
                minute: small(row.minute, "minute")?,
            },
            target,
            keep: small(row.keep, "keep")?,
            account_id: row.account_id,
            passphrase: row.passphrase.map(Secret::new),
            since: Timestamp::from_unix_seconds(row.since),
            last,
        })
    }

    async fn save(&self, plan: &Plan) -> Result<(), PortError> {
        let (frequency, weekday) = match plan.schedule.frequency {
            Frequency::Daily => ("daily", None),
            Frequency::Weekly { weekday } => ("weekly", Some(i64::from(weekday))),
        };
        let (kind, share_id, drive, folder) = match &plan.target {
            Some(Target::Share { share_id, folder }) => {
                (Some("share"), Some(*share_id), None, folder.as_str())
            }
            Some(Target::Drive { drive, folder }) => {
                (Some("drive"), None, Some(drive.as_str()), folder.as_str())
            }
            None => (None, None, None, ""),
        };
        let (hour, minute, keep) =
            (i64::from(plan.schedule.hour), i64::from(plan.schedule.minute), i64::from(plan.keep));
        let passphrase = plan.passphrase.as_ref().map(Secret::expose);
        let since = plan.since.unix_seconds();
        sqlx::query!(
            "INSERT INTO configuration_backup
                 (id, enabled, frequency, weekday, hour, minute, target_kind, share_id, drive, folder, keep,
                  passphrase, account_id, since)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 enabled = excluded.enabled, frequency = excluded.frequency, weekday = excluded.weekday,
                 hour = excluded.hour, minute = excluded.minute, target_kind = excluded.target_kind,
                 share_id = excluded.share_id, drive = excluded.drive, folder = excluded.folder,
                 keep = excluded.keep, passphrase = excluded.passphrase, account_id = excluded.account_id,
                 since = excluded.since",
            plan.enabled,
            frequency,
            weekday,
            hour,
            minute,
            kind,
            share_id,
            drive,
            folder,
            keep,
            passphrase,
            plan.account_id,
            since,
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn record(&self, run: &LastRun) -> Result<(), PortError> {
        let at = run.at.unix_seconds();
        sqlx::query!(
            "UPDATE configuration_backup SET last_at = ?, last_ok = ?, last_file = ?, last_error = ? WHERE id = 1",
            at,
            run.succeeded,
            run.file,
            run.error,
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

/// [`Operators`] on `accounts`: an administrator is an account with the
/// role `admin` that is not disabled.
#[derive(Debug, Clone)]
pub struct SqliteOperators {
    db: Database,
}

impl SqliteOperators {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Operators for SqliteOperators {
    async fn name(&self, account_id: i64) -> Result<Option<String>, PortError> {
        sqlx::query_scalar!("SELECT name FROM accounts WHERE id = ?", account_id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)
    }

    async fn administrator(&self, account_id: i64) -> Result<Option<String>, PortError> {
        sqlx::query_scalar!(
            "SELECT name FROM accounts WHERE id = ? AND role = 'admin' AND disabled = 0",
            account_id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Secret;

    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn the_plan_is_kept_with_its_last_run() {
        let (_dir, db) = temporary_database().await;
        let plans = SqliteConfigurationPlans::new(db.clone());
        assert!(!plans.get().await.unwrap().enabled, "off until set");
        sqlx::query("INSERT INTO accounts (name, role, password_hash, created_at, updated_at) VALUES ('alice', 'admin', 'x', 0, 0)")
            .execute(db.pool())
            .await
            .unwrap();
        let plan = Plan {
            enabled: true,
            schedule: Schedule { frequency: Frequency::Weekly { weekday: 6 }, hour: 4, minute: 30 },
            target: Some(Target::Drive { drive: "1A2B-3C4D".into(), folder: "backups/nas".into() }),
            keep: 9,
            account_id: Some(1),
            passphrase: Some(Secret::new("correct horse battery")),
            since: Timestamp::from_unix_seconds(1_800_000_000),
            last: None,
        };
        plans.save(&plan).await.unwrap();
        plans
            .record(&LastRun {
                at: Timestamp::from_unix_seconds(1_800_000_100),
                succeeded: false,
                file: None,
                error: Some("No space left on device".into()),
            })
            .await
            .unwrap();
        plans.save(&Plan { keep: 3, ..plan.clone() }).await.unwrap();

        let kept = plans.get().await.unwrap();
        assert_eq!((kept.enabled, kept.schedule, kept.keep), (true, plan.schedule, 3));
        assert_eq!(kept.target, plan.target);
        assert_eq!(kept.passphrase.unwrap().expose(), "correct horse battery");
        assert_eq!(kept.last.unwrap().error.as_deref(), Some("No space left on device"), "kept by a save");

        let operators = SqliteOperators::new(db.clone());
        assert_eq!(operators.administrator(1).await.unwrap().as_deref(), Some("alice"));
        sqlx::query("UPDATE accounts SET disabled = 1").execute(db.pool()).await.unwrap();
        assert_eq!(operators.administrator(1).await.unwrap(), None);
        assert_eq!(operators.name(1).await.unwrap().as_deref(), Some("alice"));
        assert_eq!(operators.name(7).await.unwrap(), None);
    }
}
