use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_containers::application::ports::{BackupRepository, NewBackup};
use zendiloos_containers::domain::{
    Backup, BackupFile, BackupFrequency, BackupOutcome, BackupPlan, BackupSchedule, BackupTarget, OwedStart,
    WrittenBackup,
};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`BackupRepository`] on `docker_backup_plans`, `docker_backups` and
/// `docker_owed_starts`.
#[derive(Debug, Clone)]
pub struct SqliteVolumeBackups {
    db: Database,
}

impl SqliteVolumeBackups {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("volume backups", error)
}

fn stored(what: &'static str) -> PortError {
    PortError::new("stored volume backups", what)
}

/// Device and inode numbers as SQLite keeps them: the same bits.
fn signed(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn unsigned(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

fn bytes(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn frequency(frequency: BackupFrequency) -> (&'static str, Option<i64>) {
    match frequency {
        BackupFrequency::Daily => ("daily", None),
        BackupFrequency::Weekly { weekday } => ("weekly", Some(i64::from(weekday))),
        BackupFrequency::Monthly { day } => ("monthly", Some(i64::from(day))),
    }
}

fn read_frequency(name: &str, day: Option<i64>) -> Result<BackupFrequency, PortError> {
    let day = || day.and_then(|day| u8::try_from(day).ok()).ok_or_else(|| stored("day"));
    Ok(match name {
        "daily" => BackupFrequency::Daily,
        "weekly" => BackupFrequency::Weekly { weekday: day()? },
        "monthly" => BackupFrequency::Monthly { day: day()? },
        _ => return Err(stored("frequency")),
    })
}

/// A file as stored, if all of it is.
fn file(
    path: Option<String>,
    bytes: Option<i64>,
    device: Option<i64>,
    inode: Option<i64>,
) -> Option<BackupFile> {
    Some(BackupFile {
        path: path?,
        bytes: u64::try_from(bytes?).unwrap_or(0),
        device: unsigned(device?),
        inode: unsigned(inode?),
    })
}

/// What a manifest says a volume made again from it gets: its driver and
/// labels.
fn made_from(manifest: Option<&str>) -> (String, BTreeMap<String, String>) {
    let value: Value = manifest.and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null);
    let driver = value["driver"].as_str().unwrap_or("local").to_owned();
    let labels = value["labels"]
        .as_object()
        .map(|labels| labels.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default();
    (driver, labels)
}

/// A row of `docker_backups`.
struct Row {
    id: i64,
    volume: String,
    share_id: i64,
    started_at: i64,
    finished_at: Option<i64>,
    state: String,
    error: Option<String>,
    archive: Option<BackupFile>,
    manifest_file: Option<BackupFile>,
    sha256: Option<String>,
    data_bytes: Option<i64>,
    entries: Option<i64>,
    manifest: Option<String>,
    started_by: Option<String>,
}

impl Row {
    fn backup(self) -> Result<Backup, PortError> {
        let (driver, labels) = made_from(self.manifest.as_deref());
        let outcome = match self.state.as_str() {
            "running" => BackupOutcome::Running,
            "failed" => BackupOutcome::Failed(self.error.unwrap_or_default()),
            "succeeded" => BackupOutcome::Succeeded(WrittenBackup {
                archive: self.archive.ok_or_else(|| stored("archive"))?,
                manifest: self.manifest_file.ok_or_else(|| stored("manifest"))?,
                sha256: self.sha256.ok_or_else(|| stored("sha256"))?,
                data_bytes: u64::try_from(self.data_bytes.unwrap_or(0)).unwrap_or(0),
                entries: self.entries.and_then(|entries| u64::try_from(entries).ok()),
                manifest_json: self.manifest.unwrap_or_default(),
            }),
            _ => return Err(stored("state")),
        };
        Ok(Backup {
            id: self.id,
            volume: self.volume,
            share_id: self.share_id,
            started_at: Timestamp::from_unix_seconds(self.started_at),
            finished_at: self.finished_at.map(Timestamp::from_unix_seconds),
            outcome,
            driver,
            labels,
            by: self.started_by,
        })
    }
}

macro_rules! rows {
    ($query:expr) => {
        $query.into_iter().map(|row| {
            Row {
                id: row.id,
                volume: row.volume,
                share_id: row.share_id,
                started_at: row.started_at,
                finished_at: row.finished_at,
                state: row.state,
                error: row.error,
                archive: file(row.archive_path, row.archive_bytes, row.archive_device, row.archive_inode),
                manifest_file: file(
                    row.manifest_path,
                    row.manifest_bytes,
                    row.manifest_device,
                    row.manifest_inode,
                ),
                sha256: row.sha256,
                data_bytes: row.data_bytes,
                entries: row.entries,
                manifest: row.manifest,
                started_by: row.started_by,
            }
            .backup()
        })
    };
}

#[async_trait]
impl BackupRepository for SqliteVolumeBackups {
    async fn plans(&self) -> Result<Vec<BackupPlan>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT volume, share_id, folder, frequency, day, hour, keep,
                      stop_containers AS "stop_containers: bool", since
               FROM docker_backup_plans ORDER BY volume"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(BackupPlan {
                    volume: row.volume,
                    target: BackupTarget { share_id: row.share_id, folder: row.folder },
                    schedule: BackupSchedule {
                        frequency: read_frequency(&row.frequency, row.day)?,
                        hour: u8::try_from(row.hour).map_err(|_| stored("hour"))?,
                    },
                    keep: u32::try_from(row.keep).map_err(|_| stored("keep"))?,
                    stop_containers: row.stop_containers,
                    since: Timestamp::from_unix_seconds(row.since),
                })
            })
            .collect()
    }

    async fn save_plan(&self, plan: &BackupPlan) -> Result<(), PortError> {
        let (frequency, day) = frequency(plan.schedule.frequency);
        let (hour, keep, since) =
            (i64::from(plan.schedule.hour), i64::from(plan.keep), plan.since.unix_seconds());
        sqlx::query!(
            "INSERT INTO docker_backup_plans
                 (volume, share_id, folder, frequency, day, hour, keep, stop_containers, since)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (volume) DO UPDATE SET
                 share_id = excluded.share_id, folder = excluded.folder, frequency = excluded.frequency,
                 day = excluded.day, hour = excluded.hour, keep = excluded.keep,
                 stop_containers = excluded.stop_containers, since = excluded.since",
            plan.volume,
            plan.target.share_id,
            plan.target.folder,
            frequency,
            day,
            hour,
            keep,
            plan.stop_containers,
            since
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn delete_plan(&self, volume: &str) -> Result<bool, PortError> {
        let result = sqlx::query!("DELETE FROM docker_backup_plans WHERE volume = ?", volume)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    async fn ran(&self, volume: &str, at: Timestamp) -> Result<(), PortError> {
        let at = at.unix_seconds();
        sqlx::query!("UPDATE docker_backup_plans SET since = ? WHERE volume = ?", at, volume)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn start(&self, backup: &NewBackup) -> Result<i64, PortError> {
        let started_at = backup.started_at.unix_seconds();
        let result = sqlx::query!(
            "INSERT INTO docker_backups (volume, share_id, started_at, state, started_by)
             VALUES (?, ?, ?, 'running', ?)",
            backup.volume,
            backup.share_id,
            started_at,
            backup.by
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(result.last_insert_rowid())
    }

    async fn finish(
        &self,
        id: i64,
        at: Timestamp,
        outcome: Result<&WrittenBackup, &str>,
    ) -> Result<(), PortError> {
        let at = at.unix_seconds();
        match outcome {
            Ok(written) => {
                let (archive, manifest) = (&written.archive, &written.manifest);
                let (archive_bytes, archive_device, archive_inode) =
                    (bytes(archive.bytes), signed(archive.device), signed(archive.inode));
                let (manifest_bytes, manifest_device, manifest_inode) =
                    (bytes(manifest.bytes), signed(manifest.device), signed(manifest.inode));
                let data_bytes = bytes(written.data_bytes);
                let entries = written.entries.map(bytes);
                sqlx::query!(
                    "UPDATE docker_backups SET finished_at = ?, state = 'succeeded', error = NULL,
                         archive_path = ?, archive_bytes = ?, archive_device = ?, archive_inode = ?,
                         manifest_path = ?, manifest_bytes = ?, manifest_device = ?, manifest_inode = ?,
                         sha256 = ?, data_bytes = ?, entries = ?, manifest = ?
                     WHERE id = ?",
                    at,
                    archive.path,
                    archive_bytes,
                    archive_device,
                    archive_inode,
                    manifest.path,
                    manifest_bytes,
                    manifest_device,
                    manifest_inode,
                    written.sha256,
                    data_bytes,
                    entries,
                    written.manifest_json,
                    id
                )
                .execute(self.db.pool())
                .await
                .map_err(db_error)?;
            }
            Err(reason) => {
                sqlx::query!(
                    "UPDATE docker_backups SET finished_at = ?, state = 'failed', error = ? WHERE id = ?",
                    at,
                    reason,
                    id
                )
                .execute(self.db.pool())
                .await
                .map_err(db_error)?;
            }
        }
        Ok(())
    }

    async fn backups(&self, volume: Option<&str>) -> Result<Vec<Backup>, PortError> {
        let rows = sqlx::query!(
            "SELECT id, volume, share_id, started_at, finished_at, state, error,
                    archive_path, archive_bytes, archive_device, archive_inode,
                    manifest_path, manifest_bytes, manifest_device, manifest_inode,
                    sha256, data_bytes, entries, manifest, started_by
             FROM docker_backups WHERE ?1 IS NULL OR volume = ?1
             ORDER BY started_at DESC, id DESC",
            volume
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows!(rows).collect()
    }

    async fn backup(&self, id: i64) -> Result<Option<Backup>, PortError> {
        let rows = sqlx::query!(
            "SELECT id, volume, share_id, started_at, finished_at, state, error,
                    archive_path, archive_bytes, archive_device, archive_inode,
                    manifest_path, manifest_bytes, manifest_device, manifest_inode,
                    sha256, data_bytes, entries, manifest, started_by
             FROM docker_backups WHERE id = ?",
            id
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows!(rows).next().transpose()
    }

    async fn forget(&self, id: i64) -> Result<(), PortError> {
        sqlx::query!("DELETE FROM docker_backups WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn interrupted(&self, at: Timestamp) -> Result<Vec<Backup>, PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let rows = sqlx::query!(
            "SELECT id, volume, share_id, started_at, finished_at, state, error,
                    archive_path, archive_bytes, archive_device, archive_inode,
                    manifest_path, manifest_bytes, manifest_device, manifest_inode,
                    sha256, data_bytes, entries, manifest, started_by
             FROM docker_backups WHERE state = 'running' ORDER BY id"
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_error)?;
        let cut: Vec<Backup> = rows!(rows).collect::<Result<_, _>>()?;
        let at = at.unix_seconds();
        sqlx::query!(
            "UPDATE docker_backups SET state = 'failed', finished_at = ?,
                 error = 'zendiloosd stopped while it ran' WHERE state = 'running'",
            at
        )
        .execute(&mut *transaction)
        .await
        .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(cut)
    }

    async fn owed_starts(&self) -> Result<Vec<OwedStart>, PortError> {
        let rows = sqlx::query!(
            "SELECT container, volume, since FROM docker_owed_starts ORDER BY since, container, volume"
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(|row| OwedStart {
                container: row.container,
                volume: row.volume,
                since: Timestamp::from_unix_seconds(row.since),
            })
            .collect())
    }

    async fn owe_start(&self, container: &str, volume: &str, at: Timestamp) -> Result<(), PortError> {
        let since = at.unix_seconds();
        sqlx::query!(
            "INSERT INTO docker_owed_starts (container, volume, since) VALUES (?, ?, ?)
             ON CONFLICT (container, volume) DO NOTHING",
            container,
            volume,
            since
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn started(&self, container: &str) -> Result<(), PortError> {
        sqlx::query!("DELETE FROM docker_owed_starts WHERE container = ?", container)
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

    fn plan(volume: &str, frequency: BackupFrequency) -> BackupPlan {
        BackupPlan {
            volume: volume.into(),
            target: BackupTarget { share_id: 3, folder: "backups/docker".into() },
            schedule: BackupSchedule { frequency, hour: 3 },
            keep: 7,
            stop_containers: true,
            since: Timestamp::from_unix_seconds(1_793_836_800),
        }
    }

    fn written(inode: u64) -> WrittenBackup {
        let file = |name: &str, inode: u64| BackupFile {
            path: format!("backups/docker/db/{name}"),
            bytes: 1234,
            device: u64::MAX - 1,
            inode,
        };
        WrittenBackup {
            archive: file("db-20261108-030000.tar.zst", inode),
            manifest: file("db-20261108-030000.json", inode + 1),
            sha256: "ab".repeat(32),
            data_bytes: 4096,
            entries: Some(12),
            manifest_json:
                r#"{"volume":"db","driver":"local","labels":{"com.docker.compose.project":"tesla"}}"#.into(),
        }
    }

    #[tokio::test]
    async fn plans_are_kept_one_per_volume() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteVolumeBackups::new(db.clone());
        let weekly = plan("db", BackupFrequency::Weekly { weekday: 6 });
        store.save_plan(&weekly).await.unwrap();
        store.save_plan(&plan("cache", BackupFrequency::Daily)).await.unwrap();
        assert_eq!(store.plans().await.unwrap(), [plan("cache", BackupFrequency::Daily), weekly.clone()]);

        let monthly = BackupPlan {
            keep: 30,
            stop_containers: false,
            ..plan("db", BackupFrequency::Monthly { day: 31 })
        };
        store.save_plan(&monthly).await.unwrap();
        let at = Timestamp::from_unix_seconds(1_793_900_000);
        store.ran("db", at).await.unwrap();
        let plans = store.plans().await.unwrap();
        assert_eq!(plans[1], BackupPlan { since: at, ..monthly });
        assert!(store.delete_plan("db").await.unwrap());
        assert!(!store.delete_plan("db").await.unwrap());
        assert_eq!(store.plans().await.unwrap().len(), 1);
        db.close().await;
    }

    #[tokio::test]
    async fn runs_keep_what_they_wrote() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteVolumeBackups::new(db.clone());
        let new = |at: i64, by: Option<&str>| NewBackup {
            volume: "db".into(),
            share_id: 3,
            started_at: Timestamp::from_unix_seconds(at),
            by: by.map(str::to_owned),
        };
        let first = store.start(&new(100, Some("alice"))).await.unwrap();
        store.finish(first, Timestamp::from_unix_seconds(150), Ok(&written(7))).await.unwrap();
        let second = store.start(&new(200, None)).await.unwrap();
        store.finish(second, Timestamp::from_unix_seconds(250), Err("no space left")).await.unwrap();
        let other = store.start(&NewBackup { volume: "cache".into(), ..new(300, None) }).await.unwrap();

        let runs = store.backups(Some("db")).await.unwrap();
        assert_eq!(runs.iter().map(|b| b.id).collect::<Vec<_>>(), [second, first], "the newest first");
        assert_eq!(runs[0].outcome, BackupOutcome::Failed("no space left".into()));
        let done = &runs[1];
        assert_eq!(done.written(), Some(&written(7)), "device and inode as they were");
        assert_eq!(done.by.as_deref(), Some("alice"));
        assert_eq!(
            (done.driver.as_str(), done.labels["com.docker.compose.project"].as_str()),
            ("local", "tesla")
        );
        assert_eq!(done.finished_at, Some(Timestamp::from_unix_seconds(150)));
        assert_eq!(store.backups(None).await.unwrap().len(), 3);
        assert_eq!(store.backup(other).await.unwrap().unwrap().outcome, BackupOutcome::Running);

        // After a restart: the running one failed, as it was told.
        let cut = store.interrupted(Timestamp::from_unix_seconds(400)).await.unwrap();
        assert_eq!((cut.len(), cut[0].id), (1, other));
        let after = store.backup(other).await.unwrap().unwrap();
        assert!(matches!(after.outcome, BackupOutcome::Failed(_)));
        assert!(store.interrupted(Timestamp::from_unix_seconds(500)).await.unwrap().is_empty());

        store.forget(first).await.unwrap();
        assert!(store.backup(first).await.unwrap().is_none());
        db.close().await;
    }

    #[tokio::test]
    async fn starts_owed_are_kept_until_each_container_started() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteVolumeBackups::new(db.clone());
        let at = |seconds: i64| Timestamp::from_unix_seconds(seconds);
        store.owe_start("app", "app_data", at(100)).await.unwrap();
        store.owe_start("app", "app_db", at(200)).await.unwrap();
        store.owe_start("app", "app_data", at(300)).await.unwrap();
        store.owe_start("web", "app_db", at(200)).await.unwrap();
        let owed = store.owed_starts().await.unwrap();
        let seen: Vec<(&str, &str, i64)> =
            owed.iter().map(|o| (o.container.as_str(), o.volume.as_str(), o.since.unix_seconds())).collect();
        assert_eq!(seen, [("app", "app_data", 100), ("app", "app_db", 200), ("web", "app_db", 200)]);
        store.started("app").await.unwrap();
        let owed = store.owed_starts().await.unwrap();
        assert_eq!(owed.iter().map(|o| o.container.as_str()).collect::<Vec<_>>(), ["web"]);
        db.close().await;
    }

    #[test]
    fn what_is_stored_reads_back_or_is_refused() {
        assert_eq!(unsigned(signed(u64::MAX)), u64::MAX);
        assert!(read_frequency("hourly", None).is_err() && read_frequency("weekly", None).is_err());
        assert_eq!(made_from(None), ("local".to_owned(), BTreeMap::new()));
        assert_eq!(file(Some("a".into()), Some(1), None, Some(1)), None);
    }
}
