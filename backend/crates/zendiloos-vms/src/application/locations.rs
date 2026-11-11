//! The locations of the VMs' storage: one dataset `<pool>/zendiloos.vms`
//! per pool that holds VMs' disks, the first also the image library. A
//! location is added on a pool zendiloOS made, shows what lies on it, and
//! goes only once empty, with the pool's name typed again.

use zendiloos_kernel::{Actor, RequestContext};

use super::{VmPlugin, VmPool, Vms, VmsError, outcome};
use crate::domain::Location;

/// A zvol below a location's dataset, as ZFS counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZvolUsage {
    /// Below the dataset: `<id>-disk<N>`.
    pub name: String,
    /// What it takes of the pool, snapshots included.
    pub used_bytes: u64,
    /// What the guest wrote and is there now.
    pub referenced_bytes: u64,
    /// Its size, as the guest sees it.
    pub volsize_bytes: u64,
}

/// What a location's dataset holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetUsage {
    pub pool: String,
    /// All of it: the zvols and the library's files.
    pub used_bytes: u64,
    /// The dataset's own files: the image library's.
    pub files_bytes: u64,
    /// Free for it: the pool's.
    pub available_bytes: u64,
    pub zvols: Vec<ZvolUsage>,
}

/// A VM with disks on a location, and what they take there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmOnLocation {
    pub id: String,
    pub name: String,
    /// The slots of its disks there.
    pub disks: Vec<u8>,
    /// What its zvols there take of the pool, and what they hold; none
    /// while ZFS cannot be asked.
    pub used_bytes: Option<u64>,
    pub referenced_bytes: Option<u64>,
    /// Their sizes added up.
    pub size_bytes: u64,
}

/// A location as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationView {
    pub location: Location,
    /// The first: the image library is here.
    pub library: bool,
    /// The pool's health and space; none while it cannot be read.
    pub pool: Option<VmPool>,
    /// What its dataset holds; none while ZFS cannot be asked.
    pub usage: Option<DatasetUsage>,
    /// The library's images here, and what they hold.
    pub images: usize,
    pub vms: Vec<VmOnLocation>,
}

impl Vms {
    /// The locations, the library's first, with their pools, what lies on
    /// them and what each VM's disks take there. None before the storage
    /// is set up.
    pub async fn locations(&self, context: &RequestContext) -> Result<Vec<LocationView>, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        let locations = self.ordered_locations(&facts).await?;
        if locations.is_empty() {
            return Ok(Vec::new());
        }
        let pools = self.ports.host.pools(context).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "vms: cannot read the pools of the locations");
            Vec::new()
        });
        let usage = self.ports.host.storage_usage(context).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "vms: cannot read what the locations hold");
            Vec::new()
        });
        let vms = self.ports.repository.list().await?;
        let images = self.ports.images.list().await?;
        Ok(locations
            .iter()
            .enumerate()
            .map(|(index, location)| {
                let usage = usage.iter().find(|usage| usage.pool == location.pool).cloned();
                let vms = vms
                    .iter()
                    .filter(|vm| vm.settings.on_pool(&location.pool))
                    .map(|vm| {
                        let disks: Vec<u8> = vm
                            .settings
                            .disks
                            .iter()
                            .filter(|disk| disk.location.pool == location.pool)
                            .map(|disk| disk.slot)
                            .collect();
                        let zvols: Vec<&ZvolUsage> = usage
                            .iter()
                            .flat_map(|usage| &usage.zvols)
                            .filter(|zvol| {
                                disks.iter().any(|slot| zvol.name == format!("{}-disk{slot}", vm.id))
                            })
                            .collect();
                        let sum = |pick: fn(&ZvolUsage) -> u64| {
                            usage.as_ref().map(|_| zvols.iter().map(|zvol| pick(zvol)).sum::<u64>())
                        };
                        let size_bytes = vm
                            .settings
                            .disks
                            .iter()
                            .filter(|disk| disk.location.pool == location.pool)
                            .map(|disk| u64::from(disk.size_gib) << 30)
                            .sum();
                        VmOnLocation {
                            id: vm.id.clone(),
                            name: vm.settings.name.clone(),
                            disks,
                            used_bytes: sum(|zvol| zvol.used_bytes),
                            referenced_bytes: sum(|zvol| zvol.referenced_bytes),
                            size_bytes,
                        }
                    })
                    .collect();
                LocationView {
                    location: location.clone(),
                    library: index == 0,
                    pool: pools.iter().find(|pool| pool.location.pool == location.pool).cloned(),
                    usage,
                    images: images.iter().filter(|image| image.volume == location.volume).count(),
                    vms,
                }
            })
            .collect())
    }

    /// One more location, on a pool zendiloOS made: its dataset made and
    /// mounted, libvirt waiting for it. Already one is no failure.
    pub async fn add_location(
        &self,
        pool: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmPlugin, VmsError> {
        self.place_location(pool, "vms.location_add", actor, context).await
    }

    pub(super) async fn place_location(
        &self,
        pool: &str,
        action: &'static str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<VmPlugin, VmsError> {
        let facts = self.ports.host.facts(context).await?;
        if !facts.installed {
            return Err(VmsError::NotInstalled);
        }
        // Those the host has go into the records first, so the one there
        // was stays the first.
        self.ordered_locations(&facts).await?;
        let found = self
            .ports
            .host
            .pools(context)
            .await?
            .into_iter()
            .find(|found| found.location.pool == pool)
            .ok_or(VmsError::PoolNotFound)?;
        let _library = self.library.lock().await;
        let done = self.ports.host.add_location(&found.location, context).await;
        self.audit(action, outcome(&done), pool, actor, context).await;
        done?;
        self.ports.locations.add(&found.location).await?;
        drop(_library);
        self.plugin(context).await
    }

    /// Removes a location; `confirm` must be its pool's name, typed again.
    /// Refused while a VM has a disk there, while the library has images
    /// there, and for the library's location while there are others: the
    /// library stays where it is.
    pub async fn remove_location(
        &self,
        pool: &str,
        confirm: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), VmsError> {
        let facts = self.ports.host.facts(context).await?;
        let locations = self.ordered_locations(&facts).await?;
        let index = locations.iter().position(|location| location.pool == pool).ok_or(VmsError::NotFound)?;
        if confirm.trim() != pool {
            return Err(VmsError::ConfirmMismatch);
        }
        let location = locations[index].clone();
        let _library = self.library.lock().await;
        let mut users: Vec<String> = self
            .ports
            .repository
            .list()
            .await?
            .into_iter()
            .filter(|vm| vm.settings.on_pool(pool))
            .map(|vm| vm.settings.name)
            .collect();
        let images =
            self.ports.images.list().await?.iter().filter(|image| image.volume == location.volume).count();
        if images > 0 || (index == 0 && locations.len() > 1) {
            users.push("the image library".to_owned());
        }
        if !users.is_empty() {
            return Err(VmsError::LocationInUse(users));
        }
        let done = self.ports.host.remove_location(&location, context).await;
        self.audit("vms.location_remove", outcome(&done), pool, actor, context).await;
        done?;
        self.ports.locations.remove(pool).await?;
        Ok(())
    }
}
