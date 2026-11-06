use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_apps::application::ports::InstalledRepository;
use zendiloos_apps::domain::{InstallState, InstallValues, InstalledApp};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`InstalledRepository`] on the `installed_apps` table. The version
/// installed is kept as its manifest text, read again with the catalog's
/// own types.
#[derive(Debug, Clone)]
pub struct SqliteInstalledApps {
    db: Database,
}

impl SqliteInstalledApps {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("installed apps", error)
}

fn stored(what: impl Into<String>) -> PortError {
    PortError::new("stored app", what.into())
}

struct Row {
    id: String,
    source_id: i64,
    manifest: String,
    compose: String,
    icon: String,
    digest: String,
    pool: String,
    volume: i64,
    shares: String,
    ports: String,
    settings: String,
    state: String,
    installed_at: i64,
    updated_at: i64,
}

fn app(row: Row) -> Result<InstalledApp, PortError> {
    let json = |text: &str, what: &str| -> Result<serde_json::Value, PortError> {
        serde_json::from_str(text).map_err(|_| stored(format!("{what} of {}", row.id)))
    };
    let strings = |value: serde_json::Value| -> BTreeMap<String, String> {
        serde_json::from_value(value).unwrap_or_default()
    };
    let ports: BTreeMap<String, u16> = serde_json::from_value(json(&row.ports, "ports")?)
        .map_err(|_| stored(format!("ports of {}", row.id)))?;
    let values = InstallValues {
        shares: strings(json(&row.shares, "shares")?),
        ports,
        settings: strings(json(&row.settings, "settings")?),
    };
    let catalog_app = crate::apps::kept_app(&row.manifest, row.compose, row.icon, row.digest)
        .map_err(|reason| stored(format!("{}: {reason}", row.id)))?;
    Ok(InstalledApp {
        state: InstallState::parse(&row.state).ok_or_else(|| stored(format!("state {:?}", row.state)))?,
        volume: u16::try_from(row.volume).map_err(|_| stored("volume"))?,
        id: row.id,
        source_id: row.source_id,
        app: catalog_app,
        pool: row.pool,
        values,
        installed_at: Timestamp::from_unix_seconds(row.installed_at),
        updated_at: Timestamp::from_unix_seconds(row.updated_at),
    })
}

/// The values as the table keeps them: three JSON objects.
fn values_text(values: &InstallValues) -> (String, String, String) {
    let text = |value: serde_json::Result<String>| value.unwrap_or_else(|_| "{}".to_owned());
    (
        text(serde_json::to_string(&values.shares)),
        text(serde_json::to_string(&values.ports)),
        text(serde_json::to_string(&values.settings)),
    )
}

#[async_trait]
impl InstalledRepository for SqliteInstalledApps {
    async fn list(&self) -> Result<Vec<InstalledApp>, PortError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", source_id, manifest, compose, icon, digest, pool, volume, shares, ports,
                      settings, state, installed_at, updated_at
               FROM installed_apps ORDER BY id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter().map(app).collect()
    }

    async fn find(&self, id: &str) -> Result<Option<InstalledApp>, PortError> {
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", source_id, manifest, compose, icon, digest, pool, volume, shares, ports,
                      settings, state, installed_at, updated_at
               FROM installed_apps WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(app).transpose()
    }

    async fn create(&self, installed: &InstalledApp) -> Result<bool, PortError> {
        let (shares, ports, settings) = values_text(&installed.values);
        let (state, volume) = (installed.state.as_str(), i64::from(installed.volume));
        let (installed_at, updated_at) =
            (installed.installed_at.unix_seconds(), installed.updated_at.unix_seconds());
        let done = sqlx::query!(
            "INSERT INTO installed_apps (id, source_id, manifest, compose, icon, digest, pool, volume, shares,
                                         ports, settings, state, installed_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING",
            installed.id,
            installed.source_id,
            installed.app.manifest,
            installed.app.compose,
            installed.app.icon,
            installed.app.digest,
            installed.pool,
            volume,
            shares,
            ports,
            settings,
            state,
            installed_at,
            updated_at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn update(&self, installed: &InstalledApp) -> Result<(), PortError> {
        let (shares, ports, settings) = values_text(&installed.values);
        let (state, volume, updated_at) =
            (installed.state.as_str(), i64::from(installed.volume), installed.updated_at.unix_seconds());
        sqlx::query!(
            "UPDATE installed_apps SET source_id = ?, manifest = ?, compose = ?, icon = ?, digest = ?, pool = ?,
                                       volume = ?, shares = ?, ports = ?, settings = ?, state = ?, updated_at = ?
             WHERE id = ?",
            installed.source_id,
            installed.app.manifest,
            installed.app.compose,
            installed.app.icon,
            installed.app.digest,
            installed.pool,
            volume,
            shares,
            ports,
            settings,
            state,
            updated_at,
            installed.id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), PortError> {
        sqlx::query!("DELETE FROM installed_apps WHERE id = ?", id)
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

    /// The first app of the community catalog's index, as installed.
    fn installed() -> InstalledApp {
        let index = crate::apps::read_index(include_str!("../apps/fixtures/community-index.json")).unwrap();
        let app = index.apps.into_iter().next().unwrap();
        InstalledApp {
            id: app.id.clone(),
            source_id: 1,
            app,
            pool: "tank".into(),
            volume: 1,
            values: InstallValues {
                shares: BTreeMap::from([("media".into(), "family".into())]),
                ports: BTreeMap::from([("web".into(), 8096)]),
                settings: BTreeMap::from([("DB_PASSWORD".into(), "secret".into())]),
            },
            state: InstallState::Installing,
            installed_at: Timestamp::from_unix_seconds(100),
            updated_at: Timestamp::from_unix_seconds(100),
        }
    }

    #[tokio::test]
    async fn keeps_the_version_installed_and_its_values() {
        let (_dir, db) = temporary_database().await;
        let apps = SqliteInstalledApps::new(db);
        let mut app = installed();
        assert!(apps.create(&app).await.unwrap());
        assert!(!apps.create(&app).await.unwrap(), "one app per id");
        assert_eq!(apps.find(&app.id).await.unwrap().as_ref(), Some(&app));

        app.state = InstallState::Installed;
        app.values.ports.insert("web".into(), 8097);
        app.updated_at = Timestamp::from_unix_seconds(200);
        apps.update(&app).await.unwrap();
        assert_eq!(apps.list().await.unwrap(), [app.clone()]);

        apps.delete(&app.id).await.unwrap();
        assert!(apps.list().await.unwrap().is_empty());
    }
}
