use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_storage::application::ports::SelfTestPlanStore;
use zendiloos_storage::domain::{ScheduledSelfTest, SelfTestFrequency, SelfTestKind, SelfTestPlan};

use super::Database;

/// [`SelfTestPlanStore`] on the `self_test_plans` tables.
#[derive(Debug, Clone)]
pub struct SqliteSelfTestPlans {
    db: Database,
}

impl SqliteSelfTestPlans {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("self-test plans", error)
}

#[async_trait]
impl SelfTestPlanStore for SqliteSelfTestPlans {
    async fn plans(&self) -> Result<Vec<ScheduledSelfTest>, PortError> {
        let rows = sqlx::query!("SELECT kind, frequency, hour, last_run FROM self_test_plans")
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        let disks = sqlx::query!("SELECT kind, serial FROM self_test_plan_disks ORDER BY rowid")
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        let mut plans: Vec<ScheduledSelfTest> = rows
            .into_iter()
            .filter_map(|row| {
                Some(ScheduledSelfTest {
                    plan: SelfTestPlan {
                        kind: SelfTestKind::parse(&row.kind)?,
                        frequency: SelfTestFrequency::parse(&row.frequency)?,
                        hour: u8::try_from(row.hour).ok()?,
                        disks: disks
                            .iter()
                            .filter(|d| d.kind == row.kind)
                            .map(|d| d.serial.clone())
                            .collect(),
                    },
                    last_run: row.last_run.map(Timestamp::from_unix_seconds),
                })
            })
            .collect();
        plans.sort_by_key(|scheduled| scheduled.plan.kind);
        Ok(plans)
    }

    async fn save(&self, plans: &[SelfTestPlan]) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let kept: Vec<&str> = plans.iter().map(|plan| plan.kind.as_str()).collect();
        for kind in
            SelfTestKind::ALL.map(SelfTestKind::as_str).into_iter().filter(|kind| !kept.contains(kind))
        {
            sqlx::query!("DELETE FROM self_test_plans WHERE kind = ?", kind)
                .execute(&mut *transaction)
                .await
                .map_err(db_error)?;
        }
        for plan in plans {
            let (kind, frequency, hour) = (plan.kind.as_str(), plan.frequency.as_str(), i64::from(plan.hour));
            sqlx::query!(
                "INSERT INTO self_test_plans (kind, frequency, hour) VALUES (?, ?, ?) \
                 ON CONFLICT (kind) DO UPDATE SET frequency = excluded.frequency, hour = excluded.hour",
                kind,
                frequency,
                hour
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
            sqlx::query!("DELETE FROM self_test_plan_disks WHERE kind = ?", kind)
                .execute(&mut *transaction)
                .await
                .map_err(db_error)?;
            for serial in &plan.disks {
                sqlx::query!("INSERT INTO self_test_plan_disks (kind, serial) VALUES (?, ?)", kind, serial)
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_error)?;
            }
        }
        transaction.commit().await.map_err(db_error)
    }

    async fn ran(&self, kind: SelfTestKind, at: Timestamp) -> Result<(), PortError> {
        let (kind, at) = (kind.as_str(), at.unix_seconds());
        sqlx::query!("UPDATE self_test_plans SET last_run = ? WHERE kind = ?", at, kind)
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

    fn plan(kind: SelfTestKind, frequency: SelfTestFrequency, hour: u8, disks: &[&str]) -> SelfTestPlan {
        SelfTestPlan { kind, frequency, hour, disks: disks.iter().map(|d| (*d).to_owned()).collect() }
    }

    #[tokio::test]
    async fn plans_keep_their_last_run_while_they_stay() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteSelfTestPlans::new(db.clone());
        let short = plan(SelfTestKind::Short, SelfTestFrequency::Daily, 2, &[]);
        let long = plan(SelfTestKind::Long, SelfTestFrequency::Monthly, 3, &["B2", "A1"]);
        store.save(&[short.clone(), long.clone()]).await.unwrap();
        let at = Timestamp::from_unix_seconds(1_793_836_800);
        store.ran(SelfTestKind::Long, at).await.unwrap();

        let plans = store.plans().await.unwrap();
        assert_eq!(plans[0], ScheduledSelfTest { plan: short, last_run: None });
        assert_eq!(
            plans[1],
            ScheduledSelfTest { plan: long, last_run: Some(at) },
            "disks in the order given"
        );

        // A new hour and other disks for the long test; the short one goes.
        let moved = plan(SelfTestKind::Long, SelfTestFrequency::Weekly, 4, &["C3"]);
        store.save(std::slice::from_ref(&moved)).await.unwrap();
        assert_eq!(store.plans().await.unwrap(), [ScheduledSelfTest { plan: moved, last_run: Some(at) }]);

        store.save(&[]).await.unwrap();
        assert!(store.plans().await.unwrap().is_empty());
        db.close().await;
    }
}
