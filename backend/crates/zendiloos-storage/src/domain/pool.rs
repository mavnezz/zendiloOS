//! ZFS pools: zendiloos's way to store data (docs/architecture.md, 5).

use zendiloos_kernel::Timestamp;

use super::block_device::{BlockDevice, DeviceKind};

/// A ZFS pool as the host reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pool {
    pub name: String,
    /// Decimal, because the 64-bit value does not survive JavaScript numbers.
    pub guid: String,
    pub state: VdevState,
    pub layout: PoolLayout,
    /// Leaf devices of the data vdevs, in the pool's order.
    pub devices: Vec<PoolDevice>,
    pub size_bytes: u64,
    pub allocated_bytes: u64,
    /// The last or running scrub or resilver.
    pub scan: Option<Scan>,
    /// Files with permanent errors.
    pub data_errors: u64,
    /// The volume this pool is, if zendiloos created it.
    pub volume: Option<VolumeNumber>,
    /// The root dataset's space, as files see it (after redundancy).
    pub space: Option<PoolSpace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSpace {
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub mounted: bool,
}

/// The `N` of `/volume<N>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeNumber(u16);

impl VolumeNumber {
    pub const MAX: u16 = 999;

    pub fn new(number: u16) -> Option<Self> {
        (1..=Self::MAX).contains(&number).then_some(Self(number))
    }

    pub fn get(self) -> u16 {
        self.0
    }

    pub fn mount_point(self) -> String {
        format!("/volume{}", self.0)
    }

    /// The lowest number not in `used`.
    pub fn first_free(used: impl IntoIterator<Item = VolumeNumber>) -> Option<Self> {
        let used: Vec<u16> = used.into_iter().map(Self::get).collect();
        (1..=Self::MAX).find(|n| !used.contains(n)).map(Self)
    }
}

/// How the data is spread over the disks. zendiloos creates pools with one
/// data vdev; others are shown as `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolLayout {
    Single,
    Mirror,
    Raidz {
        parity: u8,
    },
    /// Several vdevs or a kind zendiloos does not create, e.g. `draid`.
    Other(String),
}

impl PoolLayout {
    /// Disks that may fail without losing data, for `disks` disks.
    pub fn redundancy(&self, disks: usize) -> usize {
        match self {
            PoolLayout::Single | PoolLayout::Other(_) => 0,
            PoolLayout::Mirror => disks.saturating_sub(1),
            PoolLayout::Raidz { parity } => usize::from(*parity),
        }
    }

    /// The fewest disks the layout needs.
    pub fn minimum_disks(&self) -> usize {
        match self {
            PoolLayout::Single | PoolLayout::Other(_) => 1,
            PoolLayout::Mirror => 2,
            PoolLayout::Raidz { parity } => usize::from(*parity) + 2,
        }
    }

    /// Usable bytes before ZFS overhead, for disks of `smallest` bytes.
    pub fn capacity(&self, disks: usize, smallest: u64) -> u64 {
        let data_disks = match self {
            PoolLayout::Single | PoolLayout::Other(_) => disks,
            PoolLayout::Mirror => 1,
            PoolLayout::Raidz { parity } => disks.saturating_sub(usize::from(*parity)),
        };
        smallest.saturating_mul(data_disks as u64)
    }
}

/// The state of a pool or one of its devices, as ZFS names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdevState {
    Online,
    /// Working, with less redundancy than intended.
    Degraded,
    /// Not usable; for a pool, its data is unavailable.
    Faulted,
    Offline,
    /// Gone from the system, e.g. unplugged.
    Removed,
    /// Cannot be opened.
    Unavailable,
    /// The pool stopped all I/O after failures.
    Suspended,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolDevice {
    /// As ZFS shows it, e.g. `ata-WDC_WD40EFRX_WD-1234-part1`.
    pub name: String,
    /// Identifies the device even after it disappeared; used to replace it.
    pub guid: String,
    /// The `/dev/disk/by-id` link ZFS opens, if known.
    pub path: Option<String>,
    /// Kernel name of the disk it is on, if the disk is present.
    pub disk: Option<String>,
    pub state: VdevState,
    pub read_errors: u64,
    pub write_errors: u64,
    pub checksum_errors: u64,
    /// Being replaced (`old`) or replacing another (`new`).
    pub replacing: Option<Replacing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replacing {
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub kind: ScanKind,
    pub state: ScanState,
    /// Bytes processed and to process; their ratio is the progress.
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// Errors the scan found (repaired or not).
    pub errors: u64,
}

impl Scan {
    /// 0 to 100.
    pub fn percent(&self) -> f32 {
        if self.state == ScanState::Finished {
            return 100.0;
        }
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.done_bytes as f64 / self.total_bytes as f64 * 100.0).clamp(0.0, 100.0) as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
    /// Reads and verifies everything, repairing from redundancy.
    Scrub,
    /// Rebuilds redundancy onto a new or returning device.
    Resilver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Running,
    Finished,
    Canceled,
}

impl Pool {
    pub fn is_rebuilding(&self) -> bool {
        self.scan.as_ref().is_some_and(|s| s.kind == ScanKind::Resilver && s.state == ScanState::Running)
    }

    /// Whether `disk` (a kernel name) holds a device of this pool.
    pub fn uses_disk(&self, disk: &str) -> bool {
        self.devices.iter().any(|device| device.disk.as_deref() == Some(disk))
    }
}

/// Fills in [`PoolDevice::disk`] from the block devices: ZFS names a
/// device by its `/dev/disk/by-id` link, usually of a partition
/// (`…-part1`).
pub fn resolve_pool_disks(pools: &mut [Pool], devices: &[BlockDevice]) {
    for device in pools.iter_mut().flat_map(|pool| pool.devices.iter_mut()) {
        let link = device.path.as_deref().and_then(|path| path.strip_prefix("/dev/disk/by-id/"));
        device.disk = link.and_then(|link| disk_with_link(devices, link));
    }
}

fn disk_with_link(devices: &[BlockDevice], link: &str) -> Option<String> {
    let disks = devices.iter().filter(|device| device.kind == DeviceKind::Disk);
    for disk in disks.clone() {
        if disk.descendants().iter().any(|device| device.id_link.as_deref() == Some(link)) {
            return Some(disk.name.clone());
        }
    }
    // A partition link lsblk did not report: `<disk link>-part<N>`.
    let (disk_link, number) = link.rsplit_once("-part")?;
    number.parse::<u32>().ok()?;
    disks.into_iter().find(|disk| disk.id_link.as_deref() == Some(disk_link)).map(|disk| disk.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::block_device::testing::{disk, partition};

    #[test]
    fn volume_numbers_fill_gaps() {
        let n = |v| VolumeNumber::new(v).unwrap();
        assert_eq!(VolumeNumber::first_free([]), Some(n(1)));
        assert_eq!(VolumeNumber::first_free([n(1), n(3)]), Some(n(2)));
        assert_eq!(n(12).mount_point(), "/volume12");
        assert_eq!(VolumeNumber::new(0), None);
        assert_eq!(VolumeNumber::new(1000), None);
    }

    #[test]
    fn layouts_know_their_capacity_and_redundancy() {
        let tb = 4_000_000_000_000;
        assert_eq!(PoolLayout::Mirror.capacity(2, tb), tb);
        assert_eq!(PoolLayout::Mirror.redundancy(3), 2);
        assert_eq!(PoolLayout::Raidz { parity: 1 }.capacity(4, tb), 3 * tb);
        assert_eq!(PoolLayout::Raidz { parity: 2 }.minimum_disks(), 4);
        assert_eq!(PoolLayout::Single.capacity(1, tb), tb);
        assert_eq!(PoolLayout::Single.redundancy(1), 0);
    }

    #[test]
    fn scan_progress() {
        let scan = |state, done| Scan {
            kind: ScanKind::Scrub,
            state,
            done_bytes: done,
            total_bytes: 200,
            started_at: Timestamp::from_unix_seconds(0),
            finished_at: None,
            errors: 0,
        };
        assert_eq!(scan(ScanState::Running, 50).percent(), 25.0);
        assert_eq!(scan(ScanState::Finished, 190).percent(), 100.0);
        assert_eq!(scan(ScanState::Running, 300).percent(), 100.0);
    }

    fn pool_with(paths: &[&str]) -> Pool {
        Pool {
            name: "tank".into(),
            guid: "1".into(),
            state: VdevState::Online,
            layout: PoolLayout::Mirror,
            devices: paths
                .iter()
                .enumerate()
                .map(|(i, path)| PoolDevice {
                    name: path.rsplit('/').next().unwrap().into(),
                    guid: i.to_string(),
                    path: Some((*path).into()),
                    disk: None,
                    state: VdevState::Online,
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
            volume: None,
            space: None,
        }
    }

    #[test]
    fn finds_the_disks_behind_pool_devices() {
        let mut sda = disk("sda", vec![partition("sda1", Some("zfs_member"), &[])]);
        sda.id_link = Some("ata-DISK_A".into());
        sda.children[0].id_link = Some("ata-DISK_A-part1".into());
        let mut sdb = disk("sdb", vec![partition("sdb1", Some("zfs_member"), &[])]);
        sdb.id_link = Some("ata-DISK_B".into());
        let devices = [sda, sdb];
        let mut pools = [pool_with(&[
            "/dev/disk/by-id/ata-DISK_A-part1",
            "/dev/disk/by-id/ata-DISK_B-part1",
            "/dev/disk/by-id/ata-GONE-part1",
        ])];

        resolve_pool_disks(&mut pools, &devices);

        let disks: Vec<_> = pools[0].devices.iter().map(|d| d.disk.as_deref()).collect();
        assert_eq!(disks, [Some("sda"), Some("sdb"), None]);
        assert!(pools[0].uses_disk("sdb"));
    }
}
