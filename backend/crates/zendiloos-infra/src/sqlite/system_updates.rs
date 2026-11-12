use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_updates::application::ports::UpdateStore;
use zendiloos_updates::domain::{
    Asset, BootChoice, BootReason, Channel, CheckResult, HostReport, PackageUpdate, Release, RunState,
    UpdateRun, UpdateSettings,
};

use super::Database;

/// [`UpdateStore`] on `system_update_settings` and `system_update_state`;
/// the last check is kept as JSON.
#[derive(Debug, Clone)]
pub struct SqliteSystemUpdates {
    db: Database,
}

impl SqliteSystemUpdates {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    async fn value(&self, topic: &str) -> Result<Option<String>, PortError> {
        let row = sqlx::query!("SELECT value FROM system_update_state WHERE topic = ?", topic)
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(row.map(|row| row.value))
    }

    async fn set_value(&self, topic: &str, value: &str) -> Result<(), PortError> {
        sqlx::query!(
            "INSERT INTO system_update_state (topic, value) VALUES (?, ?)
             ON CONFLICT (topic) DO UPDATE SET value = excluded.value",
            topic,
            value
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("system updates", error)
}

#[derive(Serialize, Deserialize)]
struct StoredCheck {
    checked_at: i64,
    release: Option<StoredRelease>,
    release_error: Option<String>,
    host: Option<StoredReport>,
    host_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredRelease {
    tag: String,
    version: String,
    channel: String,
    published_at: Option<i64>,
    notes: String,
    package: StoredAsset,
    sums: StoredAsset,
    signature: StoredAsset,
}

#[derive(Serialize, Deserialize)]
struct StoredAsset {
    name: String,
    address: String,
    size: u64,
}

#[derive(Serialize, Deserialize)]
struct StoredReport {
    packages: Vec<StoredPackage>,
    running_kernel: String,
    newest_kernel: Option<String>,
    zfs_ready: bool,
    #[serde(default)]
    interrupted: bool,
    /// Kept from 0.5.0 on.
    #[serde(default)]
    boot: Option<StoredBoot>,
}

#[derive(Serialize, Deserialize)]
struct StoredBoot {
    kernel: Option<String>,
    reason: String,
    detail: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredPackage {
    name: String,
    from: Option<String>,
    to: String,
    security: bool,
}

impl From<&Asset> for StoredAsset {
    fn from(asset: &Asset) -> Self {
        Self { name: asset.name.clone(), address: asset.address.clone(), size: asset.size }
    }
}

impl From<StoredAsset> for Asset {
    fn from(asset: StoredAsset) -> Self {
        Self { name: asset.name, address: asset.address, size: asset.size }
    }
}

impl From<&HostReport> for StoredReport {
    fn from(host: &HostReport) -> Self {
        Self {
            packages: host
                .packages
                .iter()
                .map(|p| StoredPackage {
                    name: p.name.clone(),
                    from: p.from.clone(),
                    to: p.to.clone(),
                    security: p.security,
                })
                .collect(),
            running_kernel: host.running_kernel.clone(),
            newest_kernel: host.newest_kernel.clone(),
            zfs_ready: host.zfs_ready,
            interrupted: host.interrupted,
            boot: host.boot.as_ref().map(|boot| StoredBoot {
                kernel: boot.kernel.clone(),
                reason: boot.reason.as_str().to_owned(),
                detail: boot.detail.clone(),
            }),
        }
    }
}

impl From<StoredReport> for HostReport {
    fn from(host: StoredReport) -> Self {
        Self {
            packages: host
                .packages
                .into_iter()
                .map(|p| PackageUpdate { name: p.name, from: p.from, to: p.to, security: p.security })
                .collect(),
            running_kernel: host.running_kernel,
            newest_kernel: host.newest_kernel,
            zfs_ready: host.zfs_ready,
            interrupted: host.interrupted,
            // A reason this build does not know (written by a newer one) is
            // as good as none.
            boot: host.boot.and_then(|boot| {
                Some(BootChoice {
                    kernel: boot.kernel,
                    reason: BootReason::parse(&boot.reason)?,
                    detail: boot.detail,
                })
            }),
        }
    }
}

/// The last update run that installed, as `run` (once it finished).
#[derive(Serialize, Deserialize)]
struct StoredRun {
    id: String,
    /// It finished an update that was cut off.
    #[serde(default)]
    finishes: bool,
    state: String,
    step: String,
    started_at: i64,
    finished_at: Option<i64>,
    error: Option<String>,
    zendiloos: Option<String>,
    report: Option<StoredReport>,
}

impl From<&UpdateRun> for StoredRun {
    fn from(run: &UpdateRun) -> Self {
        let state = match run.state {
            RunState::Running => "running",
            RunState::Succeeded => "succeeded",
            RunState::Failed => "failed",
        };
        Self {
            id: run.id.clone(),
            finishes: run.finishes,
            state: state.to_owned(),
            step: run.step.clone(),
            started_at: run.started_at.unix_seconds(),
            finished_at: run.finished_at.map(Timestamp::unix_seconds),
            error: run.error.clone(),
            zendiloos: run.zendiloos.clone(),
            report: run.report.as_ref().map(StoredReport::from),
        }
    }
}

impl TryFrom<StoredRun> for UpdateRun {
    type Error = PortError;

    fn try_from(stored: StoredRun) -> Result<Self, PortError> {
        let state = match stored.state.as_str() {
            "running" => RunState::Running,
            "succeeded" => RunState::Succeeded,
            "failed" => RunState::Failed,
            other => return Err(PortError::new("stored update run", format!("state {other:?}"))),
        };
        Ok(Self {
            id: stored.id,
            installs: true,
            finishes: stored.finishes,
            state,
            step: stored.step,
            started_at: Timestamp::from_unix_seconds(stored.started_at),
            finished_at: stored.finished_at.map(Timestamp::from_unix_seconds),
            error: stored.error,
            zendiloos: stored.zendiloos,
            report: stored.report.map(HostReport::from),
        })
    }
}

impl From<&CheckResult> for StoredCheck {
    fn from(check: &CheckResult) -> Self {
        Self {
            checked_at: check.checked_at.unix_seconds(),
            release: check.release.as_ref().map(|release| StoredRelease {
                tag: release.tag.clone(),
                version: release.version.clone(),
                channel: release.channel.as_str().to_owned(),
                published_at: release.published_at.map(Timestamp::unix_seconds),
                notes: release.notes.clone(),
                package: (&release.package).into(),
                sums: (&release.sums).into(),
                signature: (&release.signature).into(),
            }),
            release_error: check.release_error.clone(),
            host: check.host.as_ref().map(StoredReport::from),
            host_error: check.host_error.clone(),
        }
    }
}

impl TryFrom<StoredCheck> for CheckResult {
    type Error = PortError;

    fn try_from(stored: StoredCheck) -> Result<Self, PortError> {
        let release = match stored.release {
            Some(release) => Some(Release {
                channel: Channel::parse(&release.channel)
                    .ok_or_else(|| PortError::new("stored update check", "channel"))?,
                tag: release.tag,
                version: release.version,
                published_at: release.published_at.map(Timestamp::from_unix_seconds),
                notes: release.notes,
                package: release.package.into(),
                sums: release.sums.into(),
                signature: release.signature.into(),
            }),
            None => None,
        };
        Ok(Self {
            checked_at: Timestamp::from_unix_seconds(stored.checked_at),
            release,
            release_error: stored.release_error,
            host: stored.host.map(HostReport::from),
            host_error: stored.host_error,
        })
    }
}

#[async_trait]
impl UpdateStore for SqliteSystemUpdates {
    async fn settings(&self) -> Result<UpdateSettings, PortError> {
        let row = sqlx::query!(
            r#"SELECT channel, check_daily AS "check_daily: bool" FROM system_update_settings WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(UpdateSettings::default()) };
        let channel = Channel::parse(&row.channel)
            .ok_or_else(|| PortError::new("stored update settings", "channel"))?;
        Ok(UpdateSettings { channel, check_daily: row.check_daily })
    }

    async fn save_settings(&self, settings: &UpdateSettings) -> Result<(), PortError> {
        let channel = settings.channel.as_str();
        sqlx::query!(
            "INSERT INTO system_update_settings (id, channel, check_daily) VALUES (1, ?, ?)
             ON CONFLICT (id) DO UPDATE SET channel = excluded.channel, check_daily = excluded.check_daily",
            channel,
            settings.check_daily
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn last_check(&self) -> Result<Option<CheckResult>, PortError> {
        let Some(text) = self.value("check").await? else { return Ok(None) };
        let stored: StoredCheck =
            serde_json::from_str(&text).map_err(|e| PortError::new("stored update check", e))?;
        Ok(Some(stored.try_into()?))
    }

    async fn save_check(&self, check: &CheckResult) -> Result<(), PortError> {
        let text = serde_json::to_string(&StoredCheck::from(check))
            .map_err(|e| PortError::new("stored update check", e))?;
        self.set_value("check", &text).await
    }

    async fn said(&self, topic: &str) -> Result<Option<String>, PortError> {
        self.value(&format!("said:{topic}")).await
    }

    async fn last_run(&self) -> Result<Option<UpdateRun>, PortError> {
        match self.value("run").await? {
            Some(text) => {
                let stored: StoredRun =
                    serde_json::from_str(&text).map_err(|e| PortError::new("stored update run", e))?;
                Ok(Some(stored.try_into()?))
            }
            None => Ok(None),
        }
    }

    async fn save_last_run(&self, run: &UpdateRun) -> Result<(), PortError> {
        let text =
            serde_json::to_string(&StoredRun::from(run)).map_err(|e| PortError::new("update run", e))?;
        self.set_value("run", &text).await
    }

    async fn save_said(&self, topic: &str, key: &str) -> Result<(), PortError> {
        self.set_value(&format!("said:{topic}"), key).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_updates::testing::release;

    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn settings_the_last_check_and_what_was_said_are_kept() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteSystemUpdates::new(db);
        assert_eq!(store.settings().await.unwrap(), UpdateSettings::default());
        let settings = UpdateSettings { channel: Channel::Development, check_daily: false };
        store.save_settings(&settings).await.unwrap();
        assert_eq!(store.settings().await.unwrap(), settings);

        assert_eq!(store.last_check().await.unwrap(), None);
        let check = CheckResult {
            checked_at: Timestamp::from_unix_seconds(1_793_959_200),
            release: Some(release("v0.2.0", "0.2.0", false)),
            release_error: None,
            host: Some(HostReport {
                packages: vec![PackageUpdate {
                    name: "openssl".into(),
                    from: None,
                    to: "3.5".into(),
                    security: true,
                }],
                running_kernel: "6.12.48+deb13-amd64".into(),
                newest_kernel: Some("6.12.57+deb13-amd64".into()),
                zfs_ready: true,
                interrupted: true,
                boot: Some(BootChoice {
                    kernel: Some("6.12.48+deb13-amd64".into()),
                    reason: BootReason::NewestLacksZfs,
                    detail: None,
                }),
            }),
            host_error: Some("nothing".into()),
        };
        store.save_check(&check).await.unwrap();
        assert_eq!(store.last_check().await.unwrap(), Some(check.clone()));

        assert_eq!(store.last_run().await.unwrap(), None);
        let run = UpdateRun {
            id: "ab12".into(),
            installs: true,
            finishes: true,
            state: RunState::Failed,
            step: "debian".into(),
            started_at: Timestamp::from_unix_seconds(100),
            finished_at: Some(Timestamp::from_unix_seconds(150)),
            error: Some("dpkg was interrupted".into()),
            zendiloos: Some("0.2.0".into()),
            report: check.host.clone(),
        };
        store.save_last_run(&run).await.unwrap();
        assert_eq!(store.last_run().await.unwrap(), Some(run));

        assert_eq!(store.said("run").await.unwrap(), None);
        store.save_said("run", "ab12").await.unwrap();
        store.save_said("run", "cd34").await.unwrap();
        assert_eq!(store.said("run").await.unwrap().as_deref(), Some("cd34"));
    }
}
