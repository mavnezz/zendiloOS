//! Pools of another installation (docs/architecture.md, 4.15): found on
//! the disks by `zpool import`, not imported, and what the configuration
//! knows of them. A pool is imported only when an administrator asks:
//! importing one another machine still uses would corrupt it.

use super::pool::{Pool, VolumeNumber};

/// A pool `zpool import` finds on the disks that is not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportablePool {
    pub name: String,
    /// Its numeric identifier (the pool's GUID), by which it is imported.
    pub id: u64,
    /// As ZFS names it: `ONLINE`, `DEGRADED`, `UNAVAIL`, …
    pub state: String,
    /// Another machine (another host id) used it last and did not export
    /// it: ZFS imports it only by force.
    pub other_host: bool,
    /// ZFS can import it: enough of its devices are there.
    pub can_import: bool,
    /// What ZFS says of it (`status:`), in its words.
    pub status: Option<String>,
    /// What ZFS says can be done (`action:`), in its words.
    pub action: Option<String>,
    /// Its devices as `zpool import` lists them, with their state.
    pub devices: Vec<(String, String)>,
}

/// A pool the configuration names: shares, apps, the personal folders or
/// the virtual machines' storage are on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPool {
    pub name: String,
    pub volume: VolumeNumber,
    /// The shares on it, by name.
    pub shares: Vec<String>,
}

/// A pool the configuration names, not imported, found on the disks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownImportable {
    pub known: KnownPool,
    pub found: ImportablePool,
}

/// The pools the configuration names that are not imported, and the
/// others found on the disks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AbsentPools {
    /// Known and found: imported as the volume the configuration says.
    pub known: Vec<KnownImportable>,
    /// Known, but on no disk here: its disks are missing.
    pub missing: Vec<KnownPool>,
    /// Found, not known: perhaps made by zendiloOS on another
    /// installation, adopted with a volume number of this one.
    pub foreign: Vec<ImportablePool>,
    /// The lowest volume number free to adopt one with.
    pub free_volume: Option<VolumeNumber>,
}

/// What of `known` is not among the pools `imported`, found among those
/// `zpool import` lists or not, and what else it lists. A volume number is
/// free when no pool imported is that volume and the configuration names
/// none on it.
pub fn absent_pools(known: &[KnownPool], imported: &[Pool], found: &[ImportablePool]) -> AbsentPools {
    let mut absent = AbsentPools::default();
    for pool in known {
        if imported.iter().any(|imported| imported.name == pool.name) {
            continue;
        }
        let matching: Vec<&ImportablePool> = found.iter().filter(|found| found.name == pool.name).collect();
        if matching.is_empty() {
            absent.missing.push(pool.clone());
        }
        for found in matching {
            absent.known.push(KnownImportable { known: pool.clone(), found: found.clone() });
        }
    }
    absent.foreign = found
        .iter()
        .filter(|found| !absent.known.iter().any(|known| known.found.id == found.id))
        .cloned()
        .collect();
    absent.free_volume = VolumeNumber::first_free(used_volumes(known, imported));
    absent
}

/// The volume numbers taken: by the pools imported, and those the
/// configuration names.
pub fn used_volumes(known: &[KnownPool], imported: &[Pool]) -> Vec<VolumeNumber> {
    let mut used: Vec<VolumeNumber> =
        imported.iter().filter_map(|pool| pool.volume).chain(known.iter().map(|pool| pool.volume)).collect();
    used.sort_unstable();
    used.dedup();
    used
}

/// What a pool's problems are once it is imported anyway: none when its
/// configuration and the pool agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// The pool imported says another volume number than the
    /// configuration: its mounts are left as they are.
    Volume { found: Option<VolumeNumber>, known: VolumeNumber },
}

/// Whether `imported`, which the configuration knows as `known`, may get
/// its mounts: only as the same volume.
pub fn mismatch(known: &KnownPool, imported: &Pool) -> Option<Mismatch> {
    (imported.volume != Some(known.volume))
        .then_some(Mismatch::Volume { found: imported.volume, known: known.volume })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PoolLayout, VdevState};

    fn volume(number: u16) -> VolumeNumber {
        VolumeNumber::new(number).unwrap()
    }

    fn known(name: &str, number: u16) -> KnownPool {
        KnownPool { name: name.into(), volume: volume(number), shares: vec!["photos".into()] }
    }

    fn found(name: &str, id: u64) -> ImportablePool {
        ImportablePool {
            name: name.into(),
            id,
            state: "ONLINE".into(),
            other_host: false,
            can_import: true,
            status: None,
            action: None,
            devices: Vec::new(),
        }
    }

    fn imported(name: &str, number: Option<u16>) -> Pool {
        Pool {
            name: name.into(),
            guid: "1".into(),
            state: VdevState::Online,
            layout: PoolLayout::Single,
            devices: Vec::new(),
            size_bytes: 0,
            allocated_bytes: 0,
            scan: None,
            data_errors: 0,
            volume: number.and_then(VolumeNumber::new),
            space: None,
        }
    }

    #[test]
    fn pools_the_configuration_names_are_found_missing_or_imported() {
        let names = [known("tank", 1), known("cold", 2), known("fast", 4)];
        let pools = [imported("fast", Some(4)), imported("other", None)];
        let listed = [found("tank", 11), found("stray", 22), found("tank", 33)];
        let absent = absent_pools(&names, &pools, &listed);
        let ids: Vec<u64> = absent.known.iter().map(|k| k.found.id).collect();
        assert_eq!(ids, [11, 33], "both pools named tank are offered");
        assert!(absent.known.iter().all(|k| k.known.volume == volume(1)));
        assert_eq!(absent.missing, [known("cold", 2)]);
        assert_eq!(absent.foreign, [found("stray", 22)]);
        assert_eq!(absent.free_volume, Some(volume(3)), "1, 2 and 4 are taken");
        assert_eq!(used_volumes(&names, &pools), [volume(1), volume(2), volume(4)]);
    }

    #[test]
    fn nothing_is_absent_once_everything_is_imported() {
        let absent = absent_pools(&[known("tank", 1)], &[imported("tank", Some(1))], &[]);
        assert_eq!(absent, AbsentPools { free_volume: Some(volume(2)), ..AbsentPools::default() });
    }

    #[test]
    fn mounts_follow_only_a_pool_that_is_the_same_volume() {
        let tank = known("tank", 1);
        assert_eq!(mismatch(&tank, &imported("tank", Some(1))), None);
        assert_eq!(
            mismatch(&tank, &imported("tank", Some(2))),
            Some(Mismatch::Volume { found: Some(volume(2)), known: volume(1) })
        );
        assert_eq!(
            mismatch(&tank, &imported("tank", None)),
            Some(Mismatch::Volume { found: None, known: volume(1) })
        );
    }
}
