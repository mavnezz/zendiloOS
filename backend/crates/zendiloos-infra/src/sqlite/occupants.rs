use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_apps::application::ports::DockerHost;
use zendiloos_apps::domain::docker_volume;
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::{KnownPools, VolumeOccupants};
use zendiloos_storage::domain::{KnownPool, Occupant, VolumeNumber};

use super::Database;

/// Storage's [`VolumeOccupants`]: the shares and installed apps made on a
/// volume, the personal folders' place, a location of the virtual
/// machines' storage, and Docker's data as the host has it.
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
        let vms = sqlx::query_scalar!("SELECT pool FROM vm_locations WHERE volume = ?", number)
            .fetch_optional(pool)
            .await
            .map_err(db_error)?;
        let docker = self.docker.state(context).await?.data_root.as_deref().and_then(docker_volume);

        let mut occupants: Vec<Occupant> = shares.into_iter().map(Occupant::Share).collect();
        occupants.extend(apps.into_iter().map(Occupant::App));
        if homes == Some(number) {
            occupants.push(Occupant::Homes);
        }
        if vms.is_some() {
            occupants.push(Occupant::VirtualMachines);
        }
        if docker == Some(volume.get()) {
            occupants.push(Occupant::Docker);
        }
        Ok(occupants)
    }
}

/// Storage's [`KnownPools`]: every pool a share, an installed app, the
/// personal folders, a location of the virtual machines' storage or a VM's
/// disk is on, with its volume number (the first, should rows disagree)
/// and the shares on it that are not being deleted.
#[async_trait]
impl KnownPools for Occupants {
    async fn known(&self) -> Result<Vec<KnownPool>, PortError> {
        let db = self.db.pool();
        let places = sqlx::query!(
            r#"SELECT pool AS "pool!: String", volume AS "volume!: i64" FROM shares
               UNION SELECT pool, volume FROM installed_apps
               UNION SELECT pool, volume FROM homes_location
               UNION SELECT pool, volume FROM vm_locations
               UNION SELECT pool, volume FROM vm_disks
               ORDER BY 1, 2"#
        )
        .fetch_all(db)
        .await
        .map_err(db_error)?;
        let shares = sqlx::query!("SELECT name, pool FROM shares WHERE deleting = 0 ORDER BY name")
            .fetch_all(db)
            .await
            .map_err(db_error)?;
        let mut known: Vec<KnownPool> = Vec::new();
        for place in places {
            let number = u16::try_from(place.volume).ok().and_then(VolumeNumber::new);
            let Some(volume) = number else { continue };
            match known.iter().find(|pool| pool.name == place.pool) {
                Some(pool) if pool.volume != volume => {
                    tracing::warn!(
                        pool = place.pool,
                        "storage: the configuration names the pool as two volumes"
                    );
                }
                Some(_) => {}
                None => known.push(KnownPool { name: place.pool, volume, shares: Vec::new() }),
            }
        }
        for share in shares {
            if let Some(pool) = known.iter_mut().find(|pool| pool.name == share.pool) {
                pool.shares.push(share.name);
            }
        }
        Ok(known)
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
        // The VMs' storage on both pools: each is refused on its own.
        for (vms_pool, volume, position) in [("tank", 1, 1), ("cold", 2, 2)] {
            sqlx::query("INSERT INTO vm_locations (pool, volume, position) VALUES (?, ?, ?)")
                .bind(vms_pool)
                .bind(volume)
                .bind(position)
                .execute(pool)
                .await
                .unwrap();
        }
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
                Occupant::VirtualMachines,
                Occupant::Docker,
            ]
        );
        let second = occupants.on_volume(VolumeNumber::new(2).unwrap(), &context).await.unwrap();
        assert_eq!(second, [Occupant::Share("backup".into()), Occupant::VirtualMachines]);
        let elsewhere = Occupants::new(db.clone(), Arc::new(Docker(Some("/var/lib/docker"))));
        let third = elsewhere.on_volume(VolumeNumber::new(3).unwrap(), &context).await.unwrap();
        assert!(third.is_empty());

        // The pools named, with the shares on each that are not being deleted.
        sqlx::query("UPDATE shares SET deleting = 1 WHERE name = 'media'").execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO shares (name, pool, volume, recycle_bin, created_at, updated_at)
             VALUES ('odd', 'cold', 7, 0, 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        let known = elsewhere.known().await.unwrap();
        let named: Vec<(&str, u16, Vec<&str>)> = known
            .iter()
            .map(|pool| {
                (pool.name.as_str(), pool.volume.get(), pool.shares.iter().map(String::as_str).collect())
            })
            .collect();
        assert_eq!(named, [("cold", 2, vec!["odd"]), ("tank", 1, vec!["backup", "photos"])]);
    }
}
