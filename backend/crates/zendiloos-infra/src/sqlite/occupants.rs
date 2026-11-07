use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_apps::application::ports::DockerHost;
use zendiloos_apps::domain::docker_volume;
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::VolumeOccupants;
use zendiloos_storage::domain::{Occupant, VolumeNumber};

use super::Database;

/// Storage's [`VolumeOccupants`]: the shares and installed apps made on a
/// volume, the personal folders' place, and Docker's data as the host has
/// it.
pub struct Occupants {
    db: Database,
    docker: Arc<dyn DockerHost>,
}

impl Occupants {
    pub fn new(db: Database, docker: Arc<dyn DockerHost>) -> Self {
        Self { db, docker }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("what lives on a volume", error)
}

#[async_trait]
impl VolumeOccupants for Occupants {
    async fn on_volume(
        &self,
        volume: VolumeNumber,
        context: &RequestContext,
    ) -> Result<Vec<Occupant>, PortError> {
        let number = i64::from(volume.get());
        let pool = self.db.pool();
        let shares = sqlx::query_scalar!("SELECT name FROM shares WHERE volume = ? ORDER BY name", number)
            .fetch_all(pool)
            .await
            .map_err(db_error)?;
        let apps = sqlx::query_scalar!("SELECT id FROM installed_apps WHERE volume = ? ORDER BY id", number)
            .fetch_all(pool)
            .await
            .map_err(db_error)?;
        let homes = sqlx::query_scalar!("SELECT volume FROM homes_location WHERE id = 1")
            .fetch_optional(pool)
            .await
            .map_err(db_error)?;
        let docker = self.docker.state(context).await?.data_root.as_deref().and_then(docker_volume);

        let mut occupants: Vec<Occupant> = shares.into_iter().map(Occupant::Share).collect();
        occupants.extend(apps.into_iter().map(Occupant::App));
        if homes == Some(number) {
            occupants.push(Occupant::Homes);
        }
        if docker == Some(volume.get()) {
            occupants.push(Occupant::Docker);
        }
        Ok(occupants)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_apps::application::ports::DockerError;
    use zendiloos_apps::domain::{AppVolume, DockerHostState};

    use super::*;
    use crate::sqlite::tests::temporary_database;

    struct Docker(Option<&'static str>);

    #[async_trait]
    impl DockerHost for Docker {
        async fn state(&self, _: &RequestContext) -> Result<DockerHostState, PortError> {
            Ok(DockerHostState {
                installed: true,
                running: true,
                data_root: self.0.map(str::to_owned),
                version: None,
            })
        }

        async fn install(&self, _: &RequestContext) -> Result<(), DockerError> {
            unreachable!()
        }

        async fn enable(&self, _: &AppVolume, _: &RequestContext) -> Result<(), DockerError> {
            unreachable!()
        }

        async fn volumes(&self, _: &RequestContext) -> Result<Vec<AppVolume>, PortError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn finds_shares_apps_homes_and_dockers_data_by_volume() {
        let (_dir, db) = temporary_database().await;
        let pool = db.pool();
        for (name, volume) in [("photos", 1), ("media", 1), ("backup", 2)] {
            sqlx::query(
                "INSERT INTO shares (name, pool, volume, recycle_bin, created_at, updated_at)
                 VALUES (?, 'tank', ?, 0, 0, 0)",
            )
            .bind(name)
            .bind(volume)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO installed_apps (id, source_id, manifest, compose, icon, digest, pool, volume, shares,
                 ports, settings, state, installed_at, updated_at)
             VALUES ('immich', 1, '{}', '', '', '', 'tank', 1, '{}', '{}', '{}', 'installed', 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO homes_location (id, pool, volume) VALUES (1, 'tank', 1)")
            .execute(pool)
            .await
            .unwrap();
        let occupants = Occupants::new(db.clone(), Arc::new(Docker(Some("/volume1/@docker"))));
        let context = RequestContext::default();

        let first = occupants.on_volume(VolumeNumber::new(1).unwrap(), &context).await.unwrap();
        assert_eq!(
            first,
            [
                Occupant::Share("media".into()),
                Occupant::Share("photos".into()),
                Occupant::App("immich".into()),
                Occupant::Homes,
                Occupant::Docker,
            ]
        );
        let second = occupants.on_volume(VolumeNumber::new(2).unwrap(), &context).await.unwrap();
        assert_eq!(second, [Occupant::Share("backup".into())]);
        let elsewhere = Occupants::new(db, Arc::new(Docker(Some("/var/lib/docker"))));
        let third = elsewhere.on_volume(VolumeNumber::new(3).unwrap(), &context).await.unwrap();
        assert!(third.is_empty());
    }
}
