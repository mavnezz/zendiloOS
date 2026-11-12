//! Plans for changes that erase disks. A plan says exactly which disks will
//! be erased and what is on them; it is confirmed by the user and checked
//! again against the disks right before it is carried out.

use super::disk::{Disk, DiskUsage};
use super::pool::{Pool, PoolLayout, VolumeNumber};

/// Most disks in one pool. A single vdev of more disks would be slow to
/// resilver; larger pools need several vdevs, which zendiloos does not
/// create yet.
pub const MAX_POOL_DISKS: usize = 12;

/// Name of a ZFS pool zendiloos creates: a lowercase letter, then up to 31
/// lowercase letters, digits, `_` or `-`; not one of the words ZFS
/// reserves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPoolName {
    #[error("the name must be 1 to 32 characters long")]
    Length,
    #[error(
        "the name must start with a lowercase letter and contain only lowercase letters, digits, '_' and '-'"
    )]
    Characters,
    #[error("the name {0:?} is reserved by ZFS")]
    Reserved(String),
}

impl PoolName {
    const RESERVED_PREFIXES: [&str; 8] =
        ["mirror", "raidz", "draid", "spare", "log", "cache", "special", "dedup"];

    pub fn parse(name: &str) -> Result<Self, InvalidPoolName> {
        if name.is_empty() || name.len() > 32 {
            return Err(InvalidPoolName::Length);
        }
        let valid = name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !valid {
            return Err(InvalidPoolName::Characters);
        }
        // `c0`, `c1t0d0`, …: device names on other systems.
        let disk_like = name.len() > 1 && name.starts_with('c') && name.as_bytes()[1].is_ascii_digit();
        if disk_like || Self::RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            return Err(InvalidPoolName::Reserved(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PoolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A disk as the plan saw it. Disks are recognized again by their by-id
/// link, serial number and size, never by their kernel name, which can
/// change with a reboot or a rescan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDisk {
    pub name: String,
    pub id_link: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub size_bytes: u64,
    /// What is on it now and will be lost, e.g. `ntfs "Backup"`.
    pub erases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanWarning {
    /// Disks of different sizes: each is used only up to the smallest.
    MixedSizes { unused_bytes: u64 },
    /// One disk failure loses all data.
    NoRedundancy,
    /// At least one disk has data on it.
    ErasesData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolPlan {
    pub name: PoolName,
    pub layout: PoolLayout,
    pub disks: Vec<PlannedDisk>,
    pub volume: VolumeNumber,
    /// Usable bytes before ZFS overhead.
    pub capacity_bytes: u64,
    /// Disks that may fail without losing data.
    pub redundancy: usize,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacePlan {
    pub pool: PoolName,
    /// GUID of the device to replace.
    pub device: String,
    pub device_name: String,
    pub disk: PlannedDisk,
    pub warnings: Vec<PlanWarning>,
}

/// Why a disk cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unusable {
    System,
    ArrayMember,
    PoolMember,
    /// No `/dev/disk/by-id` link to recognize it by.
    NoStableId,
    /// Mounted as a removable drive; eject it first.
    Removable,
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error(transparent)]
    InvalidName(#[from] InvalidPoolName),
    #[error("a pool with this name exists")]
    NameTaken,
    #[error("this layout needs {minimum} to {maximum} disks")]
    DiskCount { minimum: usize, maximum: usize },
    #[error("disk {0} does not exist")]
    DiskNotFound(String),
    #[error("disk {disk} cannot be used: {reason:?}")]
    DiskUnusable { disk: String, reason: Unusable },
    #[error("disk {0} is listed twice")]
    DuplicateDisk(String),
    #[error("all volume numbers are taken")]
    NoVolumeNumber,
    #[error("the layout is not one zendiloos creates")]
    UnsupportedLayout,
    #[error("there is no such pool")]
    PoolNotFound,
    #[error("the pool was not created by zendiloos")]
    PoolNotManaged,
    #[error("the pool has no such device")]
    DeviceNotFound,
    #[error("the disk is smaller than the device it would replace")]
    DiskTooSmall,
}

/// Reserve at the end of every disk, so that a slightly smaller disk of the
/// same nominal size can replace a member.
pub const DISK_RESERVE_BYTES: u64 = 128 * 1024 * 1024;

fn usable(disk: &Disk) -> Result<&str, Unusable> {
    match disk.usage {
        DiskUsage::System => Err(Unusable::System),
        DiskUsage::ArrayMember { .. } => Err(Unusable::ArrayMember),
        DiskUsage::PoolMember { .. } => Err(Unusable::PoolMember),
        DiskUsage::Removable => Err(Unusable::Removable),
        _ if disk.read_only => Err(Unusable::ReadOnly),
        _ => disk.id_link.as_deref().ok_or(Unusable::NoStableId),
    }
}

/// What would be lost on `disk`.
fn contents(disk: &Disk) -> Vec<String> {
    disk.partitions
        .iter()
        .filter_map(|partition| {
            let kind = partition.filesystem.as_deref()?;
            Some(match &partition.label {
                Some(label) => format!("{kind} \"{label}\""),
                None => kind.to_owned(),
            })
        })
        .collect()
}

fn planned(disk: &Disk) -> Result<PlannedDisk, PlanError> {
    let id_link =
        usable(disk).map_err(|reason| PlanError::DiskUnusable { disk: disk.name.clone(), reason })?;
    Ok(PlannedDisk {
        name: disk.name.clone(),
        id_link: id_link.to_owned(),
        model: disk.model.clone(),
        serial: disk.serial.clone(),
        size_bytes: disk.size_bytes,
        erases: contents(disk),
    })
}

/// Whether the disk could go into a new pool or replace a device.
pub fn is_usable(disk: &Disk) -> bool {
    usable(disk).is_ok()
}

/// Plans a new pool on `chosen` (kernel names) from the current `disks`
/// and `pools`; its volume number is none of theirs nor of `reserved`, the
/// volumes the configuration names whose pools are not imported.
pub fn plan_pool(
    name: &str,
    layout: PoolLayout,
    chosen: &[String],
    disks: &[Disk],
    pools: &[Pool],
    reserved: &[VolumeNumber],
) -> Result<PoolPlan, PlanError> {
    let name = PoolName::parse(name)?;
    if matches!(layout, PoolLayout::Other(_))
        || matches!(layout, PoolLayout::Raidz { parity } if !(1..=3).contains(&parity))
    {
        return Err(PlanError::UnsupportedLayout);
    }
    if pools.iter().any(|pool| pool.name == name.as_str()) {
        return Err(PlanError::NameTaken);
    }
    let minimum = layout.minimum_disks();
    let maximum = if layout == PoolLayout::Single { 1 } else { MAX_POOL_DISKS };
    if !(minimum..=maximum).contains(&chosen.len()) {
        return Err(PlanError::DiskCount { minimum, maximum });
    }
    let mut planned_disks = Vec::new();
    for (index, chosen_name) in chosen.iter().enumerate() {
        if chosen[..index].contains(chosen_name) {
            return Err(PlanError::DuplicateDisk(chosen_name.clone()));
        }
        let disk = disks
            .iter()
            .find(|d| &d.name == chosen_name)
            .ok_or_else(|| PlanError::DiskNotFound(chosen_name.clone()))?;
        planned_disks.push(planned(disk)?);
    }
    let volume =
        VolumeNumber::first_free(pools.iter().filter_map(|pool| pool.volume).chain(reserved.iter().copied()))
            .ok_or(PlanError::NoVolumeNumber)?;

    let smallest = planned_disks.iter().map(|d| d.size_bytes).min().unwrap_or(0);
    let largest = planned_disks.iter().map(|d| d.size_bytes).max().unwrap_or(0);
    let member = smallest.saturating_sub(DISK_RESERVE_BYTES);
    let mut warnings = Vec::new();
    if largest - smallest > DISK_RESERVE_BYTES {
        let unused = planned_disks.iter().map(|d| d.size_bytes - smallest).sum();
        warnings.push(PlanWarning::MixedSizes { unused_bytes: unused });
    }
    let redundancy = layout.redundancy(planned_disks.len());
    if redundancy == 0 {
        warnings.push(PlanWarning::NoRedundancy);
    }
    if planned_disks.iter().any(|d| !d.erases.is_empty()) {
        warnings.push(PlanWarning::ErasesData);
    }
    Ok(PoolPlan {
        name,
        capacity_bytes: layout.capacity(planned_disks.len(), member),
        redundancy,
        layout,
        disks: planned_disks,
        volume,
        warnings,
    })
}

/// Plans replacing device `device` (a GUID) of `pool` with `disk`.
pub fn plan_replace(
    pool: &str,
    device: &str,
    disk: &str,
    disks: &[Disk],
    pools: &[Pool],
) -> Result<ReplacePlan, PlanError> {
    let pool = pools.iter().find(|p| p.name == pool).ok_or(PlanError::PoolNotFound)?;
    if pool.volume.is_none() {
        return Err(PlanError::PoolNotManaged);
    }
    let target = pool.devices.iter().find(|d| d.guid == device).ok_or(PlanError::DeviceNotFound)?;
    let new =
        disks.iter().find(|d| d.name == disk).ok_or_else(|| PlanError::DiskNotFound(disk.to_owned()))?;
    let new = planned(new)?;
    // The other members' disks tell the size the pool needs; their
    // partitions all have the same size.
    let member_disks: Vec<u64> = pool
        .devices
        .iter()
        .filter_map(|d| d.disk.as_ref())
        .filter_map(|name| disks.iter().find(|d| &d.name == name))
        .map(|d| d.size_bytes)
        .collect();
    if let Some(&smallest) = member_disks.iter().min()
        && new.size_bytes + DISK_RESERVE_BYTES < smallest
    {
        return Err(PlanError::DiskTooSmall);
    }
    let mut warnings = Vec::new();
    if !new.erases.is_empty() {
        warnings.push(PlanWarning::ErasesData);
    }
    Ok(ReplacePlan {
        pool: PoolName::parse(&pool.name).map_err(|_| PlanError::PoolNotManaged)?,
        device: target.guid.clone(),
        device_name: target.name.clone(),
        disk: new,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::disk::{Media, Partition, Transport};
    use crate::domain::pool::{PoolDevice, VdevState};

    const TB: u64 = 4_000_787_030_016;

    fn disk(name: &str, usage: DiskUsage) -> Disk {
        Disk {
            name: name.into(),
            model: Some("HDD".into()),
            serial: Some(format!("S-{name}")),
            wwn: None,
            id_link: Some(format!("ata-HDD_S-{name}")),
            id_path: None,
            size_bytes: TB,
            transport: Transport::Sata,
            media: Media::Rotational,
            removable: false,
            read_only: false,
            usage,
            partitions: Vec::new(),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&n| n.to_owned()).collect()
    }

    #[test]
    fn pool_names() {
        for good in ["tank", "media-1", "a"] {
            assert!(PoolName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "Tank", "1tank", "my pool", "-x", &"x".repeat(33)] {
            assert!(PoolName::parse(bad).is_err(), "{bad:?}");
        }
        for reserved in ["mirror", "raidz2", "spare1", "log", "c0t1"] {
            assert!(matches!(PoolName::parse(reserved), Err(InvalidPoolName::Reserved(_))), "{reserved}");
        }
    }

    #[test]
    fn plans_a_mirror_on_unused_disks() {
        let disks = [disk("sda", DiskUsage::Unused), disk("sdb", DiskUsage::Unused)];
        let plan = plan_pool("tank", PoolLayout::Mirror, &names(&["sda", "sdb"]), &disks, &[], &[]).unwrap();
        assert_eq!(plan.volume, VolumeNumber::new(1).unwrap());
        assert_eq!(plan.capacity_bytes, TB - DISK_RESERVE_BYTES);
        assert_eq!(plan.redundancy, 1);
        assert_eq!(plan.warnings, []);
        assert_eq!(plan.disks[1].id_link, "ata-HDD_S-sdb");
    }

    #[test]
    fn warns_about_lost_data_mixed_sizes_and_no_redundancy() {
        let mut old = disk("sdc", DiskUsage::Foreign);
        old.partitions.push(Partition {
            name: "sdc1".into(),
            size_bytes: 1,
            filesystem: Some("ntfs".into()),
            label: Some("Backup".into()),
            mount_points: vec![],
        });
        old.size_bytes = 2 * TB;
        let plan = plan_pool(
            "tank",
            PoolLayout::Mirror,
            &names(&["sda", "sdc"]),
            &[disk("sda", DiskUsage::Unused), old],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(plan.disks[1].erases, ["ntfs \"Backup\""]);
        assert_eq!(plan.warnings, [PlanWarning::MixedSizes { unused_bytes: TB }, PlanWarning::ErasesData]);

        let single = plan_pool(
            "solo",
            PoolLayout::Single,
            &names(&["sda"]),
            &[disk("sda", DiskUsage::Unused)],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(single.warnings, [PlanWarning::NoRedundancy]);
    }

    #[test]
    fn refuses_disks_in_use_and_bad_requests() {
        let disks = [
            disk("nvme0n1", DiskUsage::System),
            disk("sda", DiskUsage::PoolMember { pool: "tank".into() }),
            Disk { id_link: None, ..disk("vdb", DiskUsage::Unused) },
            disk("sdb", DiskUsage::Unused),
            disk("sdc", DiskUsage::Unused),
            disk("sdg", DiskUsage::Removable),
        ];
        let plan = |layout, chosen: &[&str]| plan_pool("new", layout, &names(chosen), &disks, &[], &[]);
        let unusable = |disk: &str, reason| Err(PlanError::DiskUnusable { disk: disk.into(), reason });
        assert_eq!(plan(PoolLayout::Mirror, &["nvme0n1", "sdb"]), unusable("nvme0n1", Unusable::System));
        assert_eq!(plan(PoolLayout::Mirror, &["sda", "sdb"]), unusable("sda", Unusable::PoolMember));
        assert_eq!(plan(PoolLayout::Mirror, &["vdb", "sdb"]), unusable("vdb", Unusable::NoStableId));
        assert_eq!(plan(PoolLayout::Mirror, &["sdg", "sdb"]), unusable("sdg", Unusable::Removable));
        assert_eq!(plan(PoolLayout::Mirror, &["sdb", "sdb"]), Err(PlanError::DuplicateDisk("sdb".into())));
        assert_eq!(
            plan(PoolLayout::Mirror, &["sdb"]),
            Err(PlanError::DiskCount { minimum: 2, maximum: MAX_POOL_DISKS })
        );
        assert_eq!(
            plan(PoolLayout::Raidz { parity: 1 }, &["sdb", "sdc"]),
            Err(PlanError::DiskCount { minimum: 3, maximum: MAX_POOL_DISKS })
        );
        assert_eq!(
            plan(PoolLayout::Single, &["sdb", "sdc"]),
            Err(PlanError::DiskCount { minimum: 1, maximum: 1 })
        );
        assert_eq!(plan(PoolLayout::Mirror, &["sdb", "sdx"]), Err(PlanError::DiskNotFound("sdx".into())));
        assert_eq!(plan(PoolLayout::Raidz { parity: 4 }, &["sdb"]), Err(PlanError::UnsupportedLayout));
    }

    fn pool(name: &str, volume: Option<u16>, devices: &[(&str, &str)]) -> Pool {
        Pool {
            name: name.into(),
            guid: "1".into(),
            state: VdevState::Degraded,
            layout: PoolLayout::Mirror,
            devices: devices
                .iter()
                .map(|(guid, disk)| PoolDevice {
                    name: format!("ata-HDD_S-{disk}-part1"),
                    guid: (*guid).into(),
                    path: None,
                    disk: Some((*disk).into()).filter(|d: &String| !d.is_empty()),
                    state: if disk.is_empty() { VdevState::Removed } else { VdevState::Online },
                    read_errors: 0,
                    write_errors: 0,
                    checksum_errors: 0,
                    replacing: None,
                })
                .collect(),
            size_bytes: 0,
            allocated_bytes: 0,
            scan: None,
            data_errors: 0,
            volume: volume.and_then(VolumeNumber::new),
            space: None,
        }
    }

    #[test]
    fn takes_the_lowest_free_volume_and_a_free_name() {
        let disks = [disk("sdb", DiskUsage::Unused)];
        let pools = [pool("tank", Some(1), &[]), pool("old", None, &[]), pool("backup", Some(3), &[])];
        let plan = plan_pool("media", PoolLayout::Single, &names(&["sdb"]), &disks, &pools, &[]).unwrap();
        assert_eq!(plan.volume.get(), 2);
        // Volume 2's pool is named by shares, but not imported: 4 is free.
        let reserved = [VolumeNumber::new(2).unwrap()];
        let plan =
            plan_pool("media", PoolLayout::Single, &names(&["sdb"]), &disks, &pools, &reserved).unwrap();
        assert_eq!(plan.volume.get(), 4);
        assert_eq!(
            plan_pool("tank", PoolLayout::Single, &names(&["sdb"]), &disks, &pools, &[]),
            Err(PlanError::NameTaken)
        );
    }

    #[test]
    fn plans_a_replacement() {
        let pools =
            [pool("tank", Some(1), &[("11", ""), ("12", "sda")]), pool("foreign", None, &[("21", "sdx")])];
        let mut small = disk("sdd", DiskUsage::Unused);
        small.size_bytes = TB - 2 * DISK_RESERVE_BYTES;
        let disks = [
            disk("sda", DiskUsage::PoolMember { pool: "tank".into() }),
            disk("sdc", DiskUsage::Unused),
            small,
        ];

        let plan = plan_replace("tank", "11", "sdc", &disks, &pools).unwrap();
        assert_eq!((plan.device.as_str(), plan.device_name.as_str()), ("11", "ata-HDD_S--part1"));
        assert_eq!(plan.disk.id_link, "ata-HDD_S-sdc");

        assert_eq!(plan_replace("tank", "11", "sdd", &disks, &pools), Err(PlanError::DiskTooSmall));
        assert_eq!(plan_replace("tank", "99", "sdc", &disks, &pools), Err(PlanError::DeviceNotFound));
        assert_eq!(plan_replace("foreign", "21", "sdc", &disks, &pools), Err(PlanError::PoolNotManaged));
        assert!(matches!(
            plan_replace("tank", "11", "sda", &disks, &pools),
            Err(PlanError::DiskUnusable { .. })
        ));
    }
}
