use async_trait::async_trait;
use zendiloos_containers::application::ports::UpdateSettingsRepository;
use zendiloos_containers::domain::{UpdateSchedule, UpdateSettings};
use zendiloos_kernel::PortError;

use super::Database;

/// [`UpdateSettingsRepository`] on `docker_update_settings`.
#[derive(Debug, Clone)]
pub struct SqliteUpdateSettings {
    db: Database,
}

impl SqliteUpdateSettings {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("image update settings", error)
}

#[async_trait]
impl UpdateSettingsRepository for SqliteUpdateSettings {
    async fn settings(&self) -> Result<UpdateSettings, PortError> {
        let row = sqlx::query!(
            r#"SELECT check_images AS "check_images: bool", weekdays, minute FROM docker_update_settings WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(UpdateSettings::default()) };
        let schedule = match (row.weekdays, row.minute) {
            (Some(weekdays), Some(minute)) => Some(UpdateSchedule {
                days: std::array::from_fn(|day| weekdays & (1 << day) != 0),
                minute: u16::try_from(minute)
                    .map_err(|_| PortError::new("stored update settings", "minute"))?,
            }),
            _ => None,
        };
        Ok(UpdateSettings { check: row.check_images, schedule })
    }

    async fn save(&self, settings: &UpdateSettings) -> Result<(), PortError> {
        let weekdays = settings.schedule.map(|s| {
            s.days.iter().enumerate().filter(|(_, on)| **on).fold(0_i64, |bits, (day, _)| bits | (1 << day))
        });
        let minute = settings.schedule.map(|s| i64::from(s.minute));
        sqlx::query!(
            "INSERT INTO docker_update_settings (id, check_images, weekdays, minute) VALUES (1, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET check_images = excluded.check_images, weekdays = excluded.weekdays,
                 minute = excluded.minute",
            settings.check,
            weekdays,
            minute
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
    async fn keeps_the_schedule() {
        let (_dir, db) = temporary_database().await;
        let settings = SqliteUpdateSettings::new(db);
        assert_eq!(settings.settings().await.unwrap(), UpdateSettings::default());
        let weekly = UpdateSettings {
            check: true,
            schedule: Some(UpdateSchedule {
                days: [true, false, false, false, false, false, true],
                minute: 340,
            }),
        };
        settings.save(&weekly).await.unwrap();
        assert_eq!(settings.settings().await.unwrap(), weekly);
        let off = UpdateSettings { check: false, schedule: None };
        settings.save(&off).await.unwrap();
        assert_eq!(settings.settings().await.unwrap(), off);
    }
}
