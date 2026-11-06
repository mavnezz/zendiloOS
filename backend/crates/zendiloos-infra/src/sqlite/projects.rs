use async_trait::async_trait;
use zendiloos_containers::application::ports::ProjectRepository;
use zendiloos_containers::domain::{OwnProject, ProjectName};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`ProjectRepository`] on `docker_projects`.
#[derive(Debug, Clone)]
pub struct SqliteProjects {
    db: Database,
}

impl SqliteProjects {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("Docker projects", error)
}

#[async_trait]
impl ProjectRepository for SqliteProjects {
    async fn list(&self) -> Result<Vec<OwnProject>, PortError> {
        let rows = sqlx::query!(
            "SELECT name, share_id, folder, directory, file, created_at, created_by FROM docker_projects ORDER BY name"
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(OwnProject {
                    name: ProjectName::parse(&row.name).map_err(|e| PortError::new("stored project", e))?,
                    share_id: row.share_id,
                    folder: row.folder,
                    directory: row.directory,
                    file: row.file,
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                    created_by: row.created_by,
                })
            })
            .collect()
    }

    async fn create(&self, project: &OwnProject) -> Result<bool, PortError> {
        let (name, created_at) = (project.name.as_str(), project.created_at.unix_seconds());
        let result = sqlx::query!(
            "INSERT INTO docker_projects (name, share_id, folder, directory, file, created_at, created_by)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (name) DO NOTHING",
            name,
            project.share_id,
            project.folder,
            project.directory,
            project.file,
            created_at,
            project.created_by
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete(&self, name: &ProjectName) -> Result<bool, PortError> {
        let name = name.as_str();
        let result = sqlx::query!("DELETE FROM docker_projects WHERE name = ?", name)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(result.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn project(name: &str) -> OwnProject {
        OwnProject {
            name: ProjectName::parse(name).unwrap(),
            share_id: 3,
            folder: format!("docker/{name}"),
            directory: format!("/volume1/docker/docker/{name}"),
            file: format!("/volume1/docker/docker/{name}/compose.yaml"),
            created_at: Timestamp::from_unix_seconds(1_800_000_000),
            created_by: "alice".into(),
        }
    }

    #[tokio::test]
    async fn keeps_projects_by_name() {
        let (_dir, db) = temporary_database().await;
        let projects = SqliteProjects::new(db);

        assert!(projects.create(&project("tesla")).await.unwrap());
        assert!(projects.create(&project("arr")).await.unwrap());
        assert!(!projects.create(&project("tesla")).await.unwrap(), "the name is taken");

        assert_eq!(projects.list().await.unwrap(), [project("arr"), project("tesla")]);
        assert!(projects.delete(&ProjectName::parse("tesla").unwrap()).await.unwrap());
        assert!(!projects.delete(&ProjectName::parse("tesla").unwrap()).await.unwrap());
    }
}
