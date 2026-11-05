//! ZFS pools: creating, destroying, replacing disks, scrubbing.
//!
//! The agent does not trust zendiloosd's view of the disks. Before a disk
//! is erased it checks, from the kernel's own information:
//!
//! - the configuration allows erasing it (`[storage] erasable`);
//! - the by-id link leads to a whole disk, which is not read-only;
//! - neither the disk nor a partition is held by another device (md, LVM,
//!   dm-crypt), mounted, or used as swap;
//! - the disk and its partitions can be opened exclusively (`O_EXCL`),
//!   which fails while ZFS, md or a filesystem uses them.
//!
//! A pool may only be destroyed or changed if its `zendiloos:volume`
//! property shows that zendiloos created it.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use zendiloos_agent_proto::{
    DiskId, DiskName, Layout, PoolName, Response, ShareName, SnapshotFrequency, SnapshotLabel, SnapshotNote,
    VolumeNumber,
};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

/// `O_EXCL` on Linux for x86-64 and arm64, the architectures zendiloos
/// ships for. On a block device without `O_CREAT` it claims the device
/// exclusively, and fails with `EBUSY` while it is in use.
const O_EXCL: i32 = 0o200;
const MIB: u64 = 1024 * 1024;
/// Unused space at the end of every disk, so that a slightly smaller disk
/// of the same nominal size can replace a member.
const RESERVE_BYTES: u64 = 128 * MIB;
/// The partition starts here; GPT needs the space before it.
const START_BYTES: u64 = MIB;
/// The ZFS user property that marks pools zendiloos created.
const VOLUME_PROPERTY: &str = "zendiloos:volume";
/// Snapshot properties: the schedules it was taken for, and a note.
const SCHEDULE_PROPERTY: &str = "zendiloos:schedule";
const NOTE_PROPERTY: &str = "zendiloos:note";
const PARTITION_NAME: &str = "zendiloos";
/// The GPT type OpenZFS uses for its partitions.
const PARTITION_TYPE: &str = "BF01";
/// Properties of every new pool's root dataset (docs/architecture.md, 5).
const DATASET_PROPERTIES: [&str; 11] = [
    "compression=lz4",
    "atime=off",
    "xattr=sa",
    "acltype=posixacl",
    "dnodesize=auto",
    "normalization=formD",
    "utf8only=on",
    // Volumes hold data: no setuid programs or device files take effect.
    "setuid=off",
    "devices=off",
    "mountpoint=legacy",
    "canmount=on",
];

/// Which disks may be erased.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Erasable {
    Policy(ErasePolicy),
    /// Only these `/dev/disk/by-id` names, e.g. on a test machine.
    Only(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErasePolicy {
    /// Any disk that passes the checks.
    Unused,
    /// None: the agent never erases a disk.
    None,
}

impl Default for Erasable {
    fn default() -> Self {
        Erasable::Policy(ErasePolicy::Unused)
    }
}

/// Where the agent looks at the host; other than the real places only in
/// tests.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub dev: PathBuf,
    pub by_id: PathBuf,
    pub sys_block: PathBuf,
    pub mountinfo: PathBuf,
    pub swaps: PathBuf,
    pub units: PathBuf,
    /// Where `/volume<N>` directories are: `/`.
    pub volumes: PathBuf,
    pub samba_config: PathBuf,
    pub nfs_exports: PathBuf,
    /// The console login prompt's text (agetty's issue.d).
    pub console_issue: PathBuf,
}

impl Default for HostPaths {
    fn default() -> Self {
        Self {
            dev: PathBuf::from("/dev"),
            by_id: PathBuf::from("/dev/disk/by-id"),
            sys_block: PathBuf::from("/sys/class/block"),
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
            swaps: PathBuf::from("/proc/swaps"),
            units: PathBuf::from("/etc/systemd/system"),
            volumes: PathBuf::from("/"),
            samba_config: PathBuf::from("/etc/samba/zendiloos.conf"),
            nfs_exports: PathBuf::from("/etc/exports.d/zendiloos.exports"),
            console_issue: PathBuf::from("/run/issue.d/zendiloos.issue"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Pools {
    erasable: Erasable,
    paths: HostPaths,
    zpool: Program,
    zfs: Program,
    sgdisk: Program,
    wipefs: Program,
    udevadm: Program,
    systemctl: Program,
    exportfs: Program,
    /// One change to disks and pools at a time.
    serial: tokio::sync::Mutex<()>,
}

/// A disk that passed the checks.
#[derive(Debug)]
struct Checked {
    id: DiskId,
    name: String,
    size_bytes: u64,
    partitions: Vec<String>,
}

fn refused(reason: String) -> OperationError {
    OperationError::Refused(reason)
}

fn read(path: &Path) -> Result<String, OperationError> {
    fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
}

const LIMITS: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 1024 * 1024 };

impl Pools {
    pub(crate) fn new(
        tools: &Tools,
        erasable: Erasable,
        paths: HostPaths,
    ) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            erasable,
            paths,
            zpool: Program::new(&tools.zpool)?,
            zfs: Program::new(&tools.zfs)?,
            sgdisk: Program::new(&tools.sgdisk)?,
            wipefs: Program::new(&tools.wipefs)?,
            udevadm: Program::new(&tools.udevadm)?,
            systemctl: Program::new(&tools.systemctl)?,
            exportfs: Program::new(&tools.exportfs)?,
            serial: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) async fn create(
        &self,
        name: &PoolName,
        layout: Layout,
        ids: &[DiskId],
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        if !layout.disks().contains(&ids.len()) {
            return Err(refused(format!("{layout:?} takes {:?} disks, not {}", layout.disks(), ids.len())));
        }
        let mut disks = Vec::new();
        for id in ids {
            let disk = self.check(id)?;
            if disks.iter().any(|d: &Checked| d.name == disk.name) {
                return Err(refused(format!("disk {} is listed twice", disk.name)));
            }
            disks.push(disk);
        }
        if self.pool_names().await?.iter().any(|existing| existing == name.as_str()) {
            return Err(OperationError::Conflict);
        }
        let unit = self.unit_path(volume);
        if unit.exists() || self.volumes_in_use().await?.contains(&volume.get()) {
            return Err(OperationError::Conflict);
        }

        let smallest = disks.iter().map(|d| d.size_bytes).min().unwrap_or(0);
        let size_mib = smallest.saturating_sub(START_BYTES + RESERVE_BYTES) / MIB;
        let mut partitions = Vec::new();
        for disk in &disks {
            partitions.push(self.prepare(disk, size_mib).await?);
        }

        let mut args = vec!["create".to_owned(), "-o".to_owned(), "ashift=12".to_owned()];
        for property in DATASET_PROPERTIES {
            args.extend(["-O".to_owned(), property.to_owned()]);
        }
        args.extend(["-O".to_owned(), format!("{VOLUME_PROPERTY}={}", volume.get())]);
        args.push(name.as_str().to_owned());
        args.extend(layout.keyword().map(str::to_owned));
        args.extend(partitions);
        run(&self.zpool, &args).await?;

        self.mount(name, volume).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn destroy(
        &self,
        name: &PoolName,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(name, Some(volume)).await?;
        let members = self.members(name).await?;

        // The shares' mounts first, then the volume's.
        let share_units: Vec<String> = fs::read_dir(&self.paths.units)
            .map_err(|e| OperationError::Read(self.paths.units.clone(), e))?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|unit| unit.starts_with(&format!("volume{}-", volume.get())) && unit.ends_with(".mount"))
            .collect();
        self.release_nfs().await?;
        for unit in share_units.iter().chain(std::iter::once(&unit_name(volume))) {
            self.remove_unit(unit).await?;
        }
        run(&self.zpool, &["destroy", name.as_str()]).await?;

        for member in members {
            let _ = run(&self.zpool, &["labelclear", "-f", &member]).await;
            let _ = run(&self.wipefs, &["--all", "--quiet", &member]).await;
            // The disk it was on, if it is one zendiloos may erase.
            let disk = member.strip_prefix(&format!("{}/", self.paths.by_id.display())).and_then(|link| {
                let (disk, _) = link.rsplit_once("-part")?;
                DiskId::parse(disk).ok()
            });
            if let Some(id) = disk {
                match self.check(&id) {
                    Ok(disk) => self.zap(&disk.name).await?,
                    Err(error) => tracing::info!(disk = id.as_str(), %error, "partition table left in place"),
                }
            }
        }
        Ok(Response::Done)
    }

    pub(crate) async fn replace(
        &self,
        pool: &PoolName,
        device: u64,
        id: &DiskId,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(pool, None).await?;
        let disk = self.check(id)?;
        // New partitions match the members' size, whatever the new disk's.
        let mut member_bytes = 0;
        for member in self.members(pool).await? {
            if let Ok(target) = fs::canonicalize(&member)
                && let Some(kname) = target.file_name().and_then(|n| n.to_str())
                && let Ok(size) = self.sectors(kname)
            {
                member_bytes = member_bytes.max(size * 512);
            }
        }
        if member_bytes == 0 {
            return Err(refused("no member of the pool is present to take its size from".to_owned()));
        }
        if disk.size_bytes < START_BYTES + member_bytes + MIB {
            return Err(refused(format!("disk {} is smaller than the pool's members", disk.name)));
        }
        let partition = self.prepare(&disk, member_bytes / MIB).await?;
        run(&self.zpool, &["replace", pool.as_str(), &device.to_string(), &partition]).await?;
        Ok(Response::Done)
    }

    /// A share's dataset, mounted below the pool's volume.
    pub(crate) async fn create_share(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        name: &ShareName,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = format!("{}/{}", pool.as_str(), name.as_str());
        let unit = share_unit_name(volume, name);
        if self.paths.units.join(&unit).exists() {
            return Err(OperationError::Conflict);
        }
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "1", pool.as_str()]).await?;
        if existing.lines().any(|line| line == dataset) {
            return Err(OperationError::Conflict);
        }
        // setuid and devices explicitly, for pools made before they were defaults.
        let options = ["create", "-o", "mountpoint=legacy", "-o", "setuid=off", "-o", "devices=off"];
        run(&self.zfs, &[&options[..], &[dataset.as_str()]].concat()).await?;
        let content = format!(
            "# Created by zendiloOS for the share {share}; removed with the share.\n\
             [Unit]\n\
             Description=zendiloOS share {share} (ZFS dataset {dataset})\n\
             Requires=zfs-import.target\n\
             After=zfs-import.target\n\
             \n\
             [Mount]\n\
             What={dataset}\n\
             Where=/volume{number}/{share}\n\
             Type=zfs\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            share = name.as_str(),
            number = volume.get(),
        );
        self.install_unit(&unit, &content).await?;
        Ok(Response::Done)
    }

    /// Unmounts a share and destroys its dataset with all files.
    pub(crate) async fn destroy_share(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        name: &ShareName,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(pool, Some(volume)).await?;
        self.release_nfs().await?;
        self.remove_unit(&share_unit_name(volume, name)).await?;
        let dataset = format!("{}/{}", pool.as_str(), name.as_str());
        run(&self.zfs, &["destroy", "-r", &dataset]).await?;
        Ok(Response::Done)
    }

    /// `<pool>/<share>`, if that dataset exists directly below the pool.
    async fn share_dataset(&self, pool: &PoolName, share: &ShareName) -> Result<String, OperationError> {
        let dataset = format!("{}/{}", pool.as_str(), share.as_str());
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "1", pool.as_str()]).await?;
        if existing.lines().any(|line| line == dataset) {
            Ok(dataset)
        } else {
            Err(refused(format!("there is no share {dataset}")))
        }
    }

    /// Takes a snapshot of a share on a pool zendiloos created.
    pub(crate) async fn create_snapshot(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        share: &ShareName,
        label: &SnapshotLabel,
        schedules: &[SnapshotFrequency],
        note: Option<&SnapshotNote>,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = self.share_dataset(pool, share).await?;
        let mut args = vec!["snapshot".to_owned()];
        if !schedules.is_empty() {
            let names: Vec<&str> = schedules.iter().map(|frequency| frequency.as_str()).collect();
            args.extend(["-o".to_owned(), format!("{SCHEDULE_PROPERTY}={}", names.join(","))]);
        }
        if let Some(note) = note {
            args.extend(["-o".to_owned(), format!("{NOTE_PROPERTY}={}", note.as_str())]);
        }
        args.push(format!("{dataset}@{}", label.as_str()));
        run(&self.zfs, &args).await?;
        Ok(Response::Done)
    }

    /// Destroys one snapshot of such a share. The label's shape rules out
    /// ZFS's ranges (`%`) and lists (`,`), so it is exactly one snapshot.
    pub(crate) async fn destroy_snapshot(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        share: &ShareName,
        label: &SnapshotLabel,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = self.share_dataset(pool, share).await?;
        run(&self.zfs, &["destroy", &format!("{dataset}@{}", label.as_str())]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn list_snapshots(&self) -> Result<Response, OperationError> {
        let columns = format!("name,creation,used,referenced,{SCHEDULE_PROPERTY},{NOTE_PROPERTY}");
        let zfs_list = output(&self.zfs, &["list", "-H", "-p", "-t", "snapshot", "-o", &columns]).await?;
        Ok(Response::Snapshots { zfs_list })
    }

    /// The kernel's NFS server keeps file systems it exported busy until
    /// its export cache is flushed. Through a unit of the package, because
    /// the cache is per network namespace and this agent has its own.
    async fn release_nfs(&self) -> Result<(), OperationError> {
        if self.exportfs.path().exists() {
            run(&self.systemctl, &["start", "zendiloos-nfs-flush.service"]).await?;
        }
        Ok(())
    }

    async fn install_unit(&self, unit: &str, content: &str) -> Result<(), OperationError> {
        let path = self.paths.units.join(unit);
        let temporary = path.with_extension("mount.new");
        fs::write(&temporary, content).map_err(|e| OperationError::Write(temporary.clone(), e))?;
        fs::rename(&temporary, &path).map_err(|e| OperationError::Write(path.clone(), e))?;
        run(&self.systemctl, &["daemon-reload"]).await?;
        run(&self.systemctl, &["enable", "--now", unit]).await
    }

    async fn remove_unit(&self, unit: &str) -> Result<(), OperationError> {
        let path = self.paths.units.join(unit);
        if path.exists() {
            run(&self.systemctl, &["disable", "--now", unit]).await?;
            fs::remove_file(&path).map_err(|e| OperationError::Write(path.clone(), e))?;
            run(&self.systemctl, &["daemon-reload"]).await?;
        }
        Ok(())
    }

    /// Only pools zendiloos created, like every change to a pool.
    pub(crate) async fn scrub(&self, pool: &PoolName) -> Result<Response, OperationError> {
        self.require_managed(pool, None).await?;
        run(&self.zpool, &["scrub", pool.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn stop_scrub(&self, pool: &PoolName) -> Result<Response, OperationError> {
        self.require_managed(pool, None).await?;
        run(&self.zpool, &["scrub", "-s", pool.as_str()]).await?;
        Ok(Response::Done)
    }

    /// The checks of the module documentation.
    fn check(&self, id: &DiskId) -> Result<Checked, OperationError> {
        match &self.erasable {
            Erasable::Policy(ErasePolicy::None) => {
                return Err(refused("erasing disks is disabled".to_owned()));
            }
            Erasable::Only(allowed) if !allowed.iter().any(|a| a == id.as_str()) => {
                return Err(refused(format!("disk {} is not in the list of erasable disks", id.as_str())));
            }
            _ => {}
        }
        let link = self.paths.by_id.join(id.as_str());
        let target =
            fs::canonicalize(&link).map_err(|_| refused(format!("disk {} is not present", id.as_str())))?;
        let name = target
            .strip_prefix(&self.paths.dev)
            .ok()
            .and_then(|p| p.to_str())
            .and_then(|name| DiskName::parse(name).ok())
            .ok_or_else(|| refused(format!("{} is not a whole disk", id.as_str())))?
            .as_str()
            .to_owned();
        let sys = self.paths.sys_block.join(&name);
        if read(&sys.join("ro"))?.trim() == "1" {
            return Err(refused(format!("disk {name} is read-only")));
        }
        let partitions: Vec<String> = fs::read_dir(&sys)
            .map_err(|e| OperationError::Read(sys.clone(), e))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join("partition").exists())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();

        let mut devices = vec![name.clone()];
        devices.extend(partitions.iter().cloned());
        let numbers: HashSet<String> = devices
            .iter()
            .map(|device| {
                read(&self.paths.sys_block.join(device).join("dev")).map(|dev| dev.trim().to_owned())
            })
            .collect::<Result<_, _>>()?;
        for device in &devices {
            let holders = self.paths.sys_block.join(device).join("holders");
            if let Some(holder) = fs::read_dir(&holders).ok().and_then(|mut entries| entries.next()) {
                let holder = holder.map(|h| h.file_name().to_string_lossy().into_owned()).unwrap_or_default();
                return Err(refused(format!("{device} is in use by {holder}")));
            }
        }
        // Field 3 of mountinfo is major:minor of the mounted device.
        for line in read(&self.paths.mountinfo)?.lines() {
            if let Some(number) = line.split_whitespace().nth(2)
                && numbers.contains(number)
            {
                let point = line.split_whitespace().nth(4).unwrap_or("?");
                return Err(refused(format!("disk {name} is mounted at {point}")));
            }
        }
        for line in read(&self.paths.swaps)?.lines().skip(1) {
            let device = line.split_whitespace().next().unwrap_or("");
            let device = fs::canonicalize(device).ok();
            let used = device.as_deref().and_then(Path::file_name).and_then(|n| n.to_str());
            if used.is_some_and(|used| devices.iter().any(|d| d == used)) {
                return Err(refused(format!("disk {name} is used as swap")));
            }
        }
        for device in &devices {
            let path = self.paths.dev.join(device);
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_EXCL)
                .open(&path)
                .map_err(|e| refused(format!("{device} cannot be opened exclusively, it is in use: {e}")))?;
        }
        Ok(Checked { id: id.clone(), size_bytes: self.sectors(&name)? * 512, name, partitions })
    }

    fn sectors(&self, kname: &str) -> Result<u64, OperationError> {
        let path = self.paths.sys_block.join(kname).join("size");
        read(&path)?
            .trim()
            .parse()
            .map_err(|_| OperationError::Read(path, std::io::Error::other("not a number")))
    }

    /// Erases `disk` and gives it one ZFS partition of `size_mib`; returns
    /// the partition's by-id path.
    async fn prepare(&self, disk: &Checked, size_mib: u64) -> Result<String, OperationError> {
        for partition in &disk.partitions {
            let path = self.dev(partition);
            let _ = run(&self.zpool, &["labelclear", "-f", &path]).await;
            run(&self.wipefs, &["--all", "--quiet", &path]).await?;
        }
        self.zap(&disk.name).await?;
        let device = self.dev(&disk.name);
        run(
            &self.sgdisk,
            &[
                &format!("--new=1:{}M:+{size_mib}M", START_BYTES / MIB),
                &format!("--typecode=1:{PARTITION_TYPE}"),
                &format!("--change-name=1:{PARTITION_NAME}"),
                &device,
            ],
        )
        .await?;
        run(&self.udevadm, &["settle", "--timeout=30"]).await?;
        let partition = self.paths.by_id.join(format!("{}-part1", disk.id.as_str()));
        for _ in 0..100 {
            if partition.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let partition = partition.to_string_lossy().into_owned();
        // Old data at the same place could look like a pool or filesystem.
        let _ = run(&self.zpool, &["labelclear", "-f", &partition]).await;
        run(&self.wipefs, &["--all", "--quiet", &partition]).await?;
        Ok(partition)
    }

    async fn zap(&self, disk: &str) -> Result<(), OperationError> {
        let device = self.dev(disk);
        run(&self.wipefs, &["--all", "--quiet", &device]).await?;
        run(&self.sgdisk, &["--zap-all", &device]).await
    }

    fn dev(&self, kname: &str) -> String {
        self.paths.dev.join(kname).to_string_lossy().into_owned()
    }

    async fn mount(&self, name: &PoolName, volume: VolumeNumber) -> Result<(), OperationError> {
        let content = format!(
            "# Created by zendiloOS for the ZFS pool {pool}; removed with the pool.\n\
             [Unit]\n\
             Description=zendiloOS volume {number} (ZFS pool {pool})\n\
             Requires=zfs-import.target\n\
             After=zfs-import.target\n\
             \n\
             [Mount]\n\
             What={pool}\n\
             Where=/volume{number}\n\
             Type=zfs\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            pool = name.as_str(),
            number = volume.get(),
        );
        self.install_unit(&unit_name(volume), &content).await
    }

    fn unit_path(&self, volume: VolumeNumber) -> PathBuf {
        self.paths.units.join(unit_name(volume))
    }

    async fn pool_names(&self) -> Result<Vec<String>, OperationError> {
        let output = output(&self.zpool, &["list", "-H", "-o", "name"]).await?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    /// Volume numbers of all pools zendiloos created.
    async fn volumes_in_use(&self) -> Result<Vec<u16>, OperationError> {
        let output =
            output(&self.zfs, &["get", "-H", "-o", "value", "-s", "local", "-d", "0", VOLUME_PROPERTY])
                .await?;
        Ok(output.lines().filter_map(|line| line.trim().parse().ok()).collect())
    }

    async fn require_managed(
        &self,
        pool: &PoolName,
        volume: Option<VolumeNumber>,
    ) -> Result<(), OperationError> {
        let value = output(&self.zfs, &["get", "-H", "-o", "value", VOLUME_PROPERTY, pool.as_str()]).await?;
        let found: Option<u16> = value.trim().parse().ok();
        match (found, volume) {
            (Some(found), Some(expected)) if found == expected.get() => Ok(()),
            (Some(_), None) => Ok(()),
            _ => Err(OperationError::NotManaged(format!("pool {}", pool.as_str()))),
        }
    }

    /// By-id paths of the pool's leaf devices.
    async fn members(&self, pool: &PoolName) -> Result<Vec<String>, OperationError> {
        let output = output(&self.zpool, &["list", "-H", "-P", "-v", pool.as_str()]).await?;
        Ok(output
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|token| token.starts_with('/'))
            .map(str::to_owned)
            .collect())
    }
}

fn unit_name(volume: VolumeNumber) -> String {
    format!("volume{}.mount", volume.get())
}

/// The mount unit of `/volume<N>/<share>`; systemd escapes `-` in paths.
pub(crate) fn share_unit_name(volume: VolumeNumber, share: &ShareName) -> String {
    format!("volume{}-{}.mount", volume.get(), share.as_str().replace('-', "\\x2d"))
}

pub(crate) async fn run<S: AsRef<str>>(program: &Program, args: &[S]) -> Result<(), OperationError> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    zendiloos_exec::run(program, &args, LIMITS).await?.success(program)?;
    Ok(())
}

async fn output(program: &Program, args: &[&str]) -> Result<String, OperationError> {
    let output = zendiloos_exec::run(program, args, LIMITS).await?.success(program)?;
    String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)
}
