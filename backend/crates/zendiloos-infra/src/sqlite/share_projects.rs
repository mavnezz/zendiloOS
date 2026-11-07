use async_trait::async_trait;
use serde_json::Value;
use zendiloos_kernel::PortError;
use zendiloos_sharing::application::ports::ShareProjects;
use zendiloos_sharing::domain::ShareId;

use super::Database;

/// Sharing's [`ShareProjects`]: administrators' projects that live in a
/// share, or mount something of it in what they confirmed (binds, named
/// volumes that are binds, secrets and configs from files).
pub struct SqliteShareProjects {
    db: Database,
}

impl SqliteShareProjects {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ShareProjects for SqliteShareProjects {
    async fn using(&self, share: ShareId, path: &str) -> Result<Vec<String>, PortError> {
        let rows: Vec<(String, i64, Option<String>)> =
            sqlx::query_as("SELECT name, share_id, config FROM docker_projects ORDER BY name")
                .fetch_all(self.db.pool())
                .await
                .map_err(|error| PortError::new("the projects in a share", error))?;
        Ok(rows
            .into_iter()
            .filter(|(_, share_id, config)| {
                *share_id == share.get() || config.as_deref().is_some_and(|config| mounts(config, path))
            })
            .map(|(name, _, _)| name)
            .collect())
    }
}

/// Whether a normalized Compose configuration mounts `path` or something
/// below it.
fn mounts(config: &str, path: &str) -> bool {
    let Ok(config) = serde_json::from_str::<Value>(config) else { return false };
    let at =
        |source: &str| source == path || source.strip_prefix(path).is_some_and(|rest| rest.starts_with('/'));
    let services = config.get("services").and_then(Value::as_object).into_iter().flat_map(|s| s.values());
    let binds = services
        .flat_map(|service| service.get("volumes").and_then(Value::as_array).into_iter().flatten())
        .filter(|volume| volume.get("type").and_then(Value::as_str) == Some("bind"))
        .filter_map(|volume| volume.get("source").and_then(Value::as_str));
    let devices = config
        .get("volumes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|volumes| volumes.values())
        .filter_map(|volume| volume.pointer("/driver_opts/device").and_then(Value::as_str));
    let files = ["secrets", "configs"]
        .into_iter()
        .filter_map(|kind| config.get(kind).and_then(Value::as_object))
        .flat_map(|entries| entries.values())
        .filter_map(|entry| entry.get("file").and_then(Value::as_str));
    binds.chain(devices).chain(files).any(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn finds_projects_living_in_a_share_or_mounting_from_it() {
        let (_dir, db) = temporary_database().await;
        let config = |source: &str| {
            serde_json::json!({ "services": { "app": { "volumes": [
                { "type": "bind", "source": "/etc/localtime", "target": "/etc/localtime" },
                { "type": "bind", "source": source, "target": "/data" },
            ] } } })
            .to_string()
        };
        let secret =
            serde_json::json!({ "services": {}, "secrets": { "t": { "file": "/volume1/media/t.txt" } } });
        for (name, share, config) in [
            ("lives", 1, None),
            ("binds", 2, Some(config("/volume1/media/Films"))),
            ("whole", 2, Some(config("/volume1/media"))),
            ("secret", 2, Some(secret.to_string())),
            ("other", 2, Some(config("/volume1/mediathek"))),
            ("legacy", 2, None),
        ] {
            sqlx::query(
                "INSERT INTO docker_projects (name, share_id, folder, directory, file, created_at, created_by, config)
                 VALUES (?, ?, 'docker/x', '/volume1/docker/docker/x', 'compose.yaml', 0, 'admin', ?)",
            )
            .bind(name)
            .bind(share)
            .bind(config)
            .execute(db.pool())
            .await
            .unwrap();
        }
        let projects = SqliteShareProjects::new(db);

        let using = projects.using(ShareId::new(1), "/volume1/media").await.unwrap();
        assert_eq!(using, ["binds", "lives", "secret", "whole"], "not mediathek, nor what confirmed nothing");
        assert!(projects.using(ShareId::new(3), "/volume1/photos").await.unwrap().is_empty());
    }
}
