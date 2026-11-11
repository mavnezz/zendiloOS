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
    /// The current pass: a scan starts a new one when it resumes, e.g.
    /// after the machine restarted.
    pub pass: Option<ScanPass>,
}

/// What the current pass of a scan has done so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanPass {
    pub started_at: Timestamp,
    pub issued_bytes: u64,
    /// Time the pass spent paused.
    pub paused_seconds: u64,
    pub paused: bool,
}

/// How fast a running scan goes and when it will be done, as `zpool status`
/// estimates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanEstimate {
    pub bytes_per_second: u64,
    pub seconds_left: u64,
}

impl Scan {
    /// Speed of the current pass and the time left, at `now`. None unless
    /// it runs, is not paused, and has been running for a moment.
    pub fn estimate(&self, now: Timestamp) -> Option<ScanEstimate> {
        let pass = self.pass?;
        if self.state != ScanState::Running || pass.paused {
            return None;
        }
        let elapsed = now.duration_since(pass.started_at).as_secs().saturating_sub(pass.paused_seconds);
        if elapsed == 0 || pass.issued_bytes == 0 {
            return None;
        }
        let bytes_per_second = (pass.issued_bytes / elapsed).max(1);
        let seconds_left = self.total_bytes.saturating_sub(self.done_bytes) / bytes_per_second;
        Some(ScanEstimate { bytes_per_second, seconds_left })
    }

    /// Only a running scrub can be stopped; a resilver restores redundancy
    /// and must finish.
    pub fn may_stop(&self) -> bool {
        self.kind == ScanKind::Scrub && self.state == ScanState::Running
    }

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
/// device by a link in `/dev/disk/by-id` (or `by-path`), usually of a
/// partition (`…-part1`), sometimes by its kernel name. A device no disk
/// matches keeps none, and is shown as ZFS names it.
pub fn resolve_pool_disks(pools: &mut [Pool], devices: &[BlockDevice]) {
    for device in pools.iter_mut().flat_map(|pool| pool.devices.iter_mut()) {
        device.disk = disk_of(devices, device.path.as_deref(), &device.name);
    }
}

/// The whole disk behind a pool device: by its path, else by its name.
fn disk_of(devices: &[BlockDevice], path: Option<&str>, name: &str) -> Option<String> {
    let disks: Vec<&BlockDevice> = devices.iter().filter(|device| device.kind == DeviceKind::Disk).collect();
    let by_path = path.and_then(|path| {
        if let Some(link) = path.strip_prefix("/dev/disk/by-id/") {
            disk_with_link(&disks, link)
        } else if let Some(port) = path.strip_prefix("/dev/disk/by-path/") {
            disk_at_port(&disks, port)
        } else {
            path.strip_prefix("/dev/").and_then(|kernel| disk_named(&disks, kernel))
        }
    });
    by_path.or_else(|| {
        disk_with_link(&disks, name)
            .or_else(|| disk_at_port(&disks, name))
            .or_else(|| disk_named(&disks, name))
    })
}

/// `link` without a partition's `-part<N>`.
fn whole(link: &str) -> &str {
    match link.rsplit_once("-part") {
        Some((disk, number)) if number.parse::<u32>().is_ok() => disk,
        _ => link,
    }
}

/// The disk a `/dev/disk/by-id` link names: the link lsblk reports for it
/// or one of its partitions, its WWN (`wwn-0x…`, NVMe's `nvme-eui.…`), or
/// a link ending in its serial number (`ata-<model>_<serial>`), which only
/// counts when no other disk reports that serial number too.
fn disk_with_link(disks: &[&BlockDevice], link: &str) -> Option<String> {
    let reported = |device: &BlockDevice| device.id_link.as_deref() == Some(link);
    if let Some(disk) = disks.iter().find(|disk| disk.descendants().into_iter().any(reported)) {
        return Some(disk.name.clone());
    }
    let link = whole(link);
    let named = |disk: &&&BlockDevice| {
        disk.id_link.as_deref() == Some(link)
            || disk.wwn.as_deref().is_some_and(|wwn| {
                link.strip_prefix("wwn-").or_else(|| link.strip_prefix("nvme-")) == Some(wwn)
            })
    };
    if let Some(disk) = disks.iter().find(named) {
        return Some(disk.name.clone());
    }
    let serial = |disk: &&&BlockDevice| {
        disk.serial.as_deref().is_some_and(|serial| {
            serial.len() >= 4 && link.strip_suffix(serial).is_some_and(|rest| rest.ends_with('_'))
        })
    };
    let mut matching = disks.iter().filter(serial);
    match (matching.next(), matching.next()) {
        (Some(disk), None) => Some(disk.name.clone()),
        _ => None,
    }
}

/// The disk plugged in at a `/dev/disk/by-path` name, or its partition's.
fn disk_at_port(disks: &[&BlockDevice], port: &str) -> Option<String> {
    let port = whole(port);
    disks.iter().find(|disk| disk.id_path.as_deref() == Some(port)).map(|disk| disk.name.clone())
}

/// The disk with the kernel name `kernel`, or holding a partition of it.
fn disk_named(disks: &[&BlockDevice], kernel: &str) -> Option<String> {
    disks
        .iter()
        .find(|disk| disk.descendants().iter().any(|device| device.name == kernel))
        .map(|disk| disk.name.clone())
}

/// What other parts of the NAS keep on a pool's volume besides the pool's
/// own data. A pool holding any of it is not deleted: it goes first, or is
/// moved, through its own place in the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Occupant {
    Share(String),
    App(String),
    /// The personal folders.
    Homes,
    /// Docker's data.
    Docker,
    /// A location of the virtual machines' storage: their disks, maybe the
    /// image library.
    VirtualMachines,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::block_device::testing::{disk, partition};

    #[test]
    fn a_running_scrub_estimates_its_end_like_zpool_does() {
        let at = |seconds: i64| Timestamp::from_unix_seconds(1_000 + seconds);
        let mut scan = Scan {
            kind: ScanKind::Scrub,
            state: ScanState::Running,
            done_bytes: 600,
            total_bytes: 1_000,
            started_at: at(0),
            finished_at: None,
            errors: 0,
            pass: Some(ScanPass { started_at: at(0), issued_bytes: 600, paused_seconds: 20, paused: false }),
        };

        // 600 bytes in 60 seconds of work (80 minus 20 paused): 10 per second.
        assert_eq!(scan.estimate(at(80)), Some(ScanEstimate { bytes_per_second: 10, seconds_left: 40 }));
        assert!(scan.may_stop());
        assert_eq!(scan.estimate(at(20)), None, "no time worked yet");
        scan.pass = scan.pass.map(|pass| ScanPass { paused: true, ..pass });
        assert_eq!(scan.estimate(at(80)), None, "paused");
        scan.kind = ScanKind::Resilver;
        assert!(!scan.may_stop(), "a resilver must finish");
    }

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
            pass: None,
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

    #[test]
    fn finds_disks_by_their_wwn_serial_number_port_or_kernel_name() {
        let mut sda = disk("sda", vec![partition("sda1", Some("zfs_member"), &[])]);
        sda.id_link = Some("ata-SAMSUNG_MZ7LN256HCHP-000L7_S1ZENX0H802190".into());
        sda.serial = Some("S1ZENX0H802190".into());
        let mut sdb = disk("sdb", vec![partition("sdb1", Some("zfs_member"), &[])]);
        sdb.id_link = Some("ata-SAMSUNG_MZ7TY256HDHP_S2YXNDAHA16081".into());
        sdb.wwn = Some("0x5002538d4130f2e7".into());
        let mut sdc = disk("sdc", vec![partition("sdc1", Some("zfs_member"), &[])]);
        sdc.id_path = Some("pci-0000:00:12.0-ata-3".into());
        let mut nvme = disk("nvme0n1", vec![partition("nvme0n1p1", Some("zfs_member"), &[])]);
        nvme.wwn = Some("eui.0025388b91b2c3d4".into());
        // Two disks of a USB dock that report the bridge's serial number.
        let mut sdd = disk("sdd", vec![]);
        sdd.serial = Some("DOCK0001".into());
        let mut sde = disk("sde", vec![]);
        sde.serial = Some("DOCK0001".into());
        let devices = [sda, sdb, sdc, nvme, sdd, sde];
        let mut pools = [pool_with(&[
            "/dev/disk/by-id/wwn-0x5002538d4130f2e7-part1",
            "/dev/disk/by-id/ata-Samsung_SSD_850_S1ZENX0H802190-part1",
            "/dev/disk/by-path/pci-0000:00:12.0-ata-3-part1",
            "/dev/disk/by-id/nvme-eui.0025388b91b2c3d4-part1",
            "/dev/sda1",
            "/dev/disk/by-id/usb-JMicron_Dock_DOCK0001-part1",
            "/dev/disk/by-id/wwn-0x5000000000000000-part1",
        ])];
        // A device ZFS knows by name only, its path gone.
        let mut bare = pools[0].devices[0].clone();
        (bare.path, bare.name) = (None, "wwn-0x5002538d4130f2e7-part1".into());
        pools[0].devices.push(bare);

        resolve_pool_disks(&mut pools, &devices);

        let disks: Vec<_> = pools[0].devices.iter().map(|d| d.disk.as_deref()).collect();
        assert_eq!(
            disks,
            [Some("sdb"), Some("sda"), Some("sdc"), Some("nvme0n1"), Some("sda"), None, None, Some("sdb")],
            "a serial number two disks report names neither"
        );
    }
}
