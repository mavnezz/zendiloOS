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
    AppName, DiskId, DiskName, HOMES_DIRECTORY, Layout, PoolName, Response, ShareCompression, ShareName,
    ShareRecordSize, SnapshotFrequency, SnapshotLabel, SnapshotNote, UserName, VolumeNumber,
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
    /// What the firmware says about the machine, `/sys/class/dmi/id`.
    pub dmi: PathBuf,
    pub mountinfo: PathBuf,
    pub swaps: PathBuf,
    pub units: PathBuf,
    /// Where `/volume<N>` directories are: `/`.
    pub volumes: PathBuf,
    pub samba_config: PathBuf,
    pub nfs_exports: PathBuf,
    /// The console login prompt's text (agetty's issue.d).
    pub console_issue: PathBuf,
    /// Docker's configuration, `/etc/docker/daemon.json`.
    pub docker_config: PathBuf,
    /// The normalized compose files of the installed apps.
    pub apps: PathBuf,
    /// The normalized compose files of administrators' projects, as they
    /// were last deployed.
    pub projects: PathBuf,
    /// Its link names the time zone apps get.
    pub localtime: PathBuf,
    /// Where the folders of shares apps were given are mounted
    /// (`app_shares`), only root may change it.
    pub app_shares: PathBuf,
    /// The same for the folders and files administrators' projects bind.
    pub project_shares: PathBuf,
    /// Where removable drives are mounted, a folder each
    /// ([`zendiloos_agent_proto::REMOVABLE_PLACE`]).
    pub removable: PathBuf,
    /// This program, which units run to mount those folders and apt.
    pub agent: PathBuf,
    /// Where the runs of apt are recorded.
    pub packages: PathBuf,
    /// Docker's socket, which an app may be given.
    pub docker_socket: PathBuf,
    /// Where zendiloosd downloads a release of zendiloOS to.
    pub update_spool: PathBuf,
    /// The keys a release may be signed with, the first that exists counts:
    /// one an administrator put in `/etc`, then the package's own.
    pub release_keys: Vec<PathBuf>,
}

impl Default for HostPaths {
    fn default() -> Self {
        Self {
            dev: PathBuf::from("/dev"),
            by_id: PathBuf::from("/dev/disk/by-id"),
            sys_block: PathBuf::from("/sys/class/block"),
            dmi: PathBuf::from("/sys/class/dmi/id"),
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
            swaps: PathBuf::from("/proc/swaps"),
            units: PathBuf::from("/etc/systemd/system"),
            volumes: PathBuf::from("/"),
            samba_config: PathBuf::from("/etc/samba/zendiloos.conf"),
            nfs_exports: PathBuf::from("/etc/exports.d/zendiloos.exports"),
            console_issue: PathBuf::from("/run/issue.d/zendiloos.issue"),
            docker_config: PathBuf::from("/etc/docker/daemon.json"),
            apps: PathBuf::from("/etc/zendiloos/apps"),
            projects: PathBuf::from("/etc/zendiloos/projects"),
            localtime: PathBuf::from("/etc/localtime"),
            app_shares: PathBuf::from("/run/zendiloos-app-shares"),
            project_shares: PathBuf::from("/run/zendiloos-project-shares"),
            removable: PathBuf::from(zendiloos_agent_proto::REMOVABLE_PLACE),
            agent: PathBuf::from("/usr/libexec/zendiloos/zendiloos-agent"),
            packages: PathBuf::from("/var/lib/zendiloos-packages"),
            docker_socket: PathBuf::from("/run/docker.sock"),
            update_spool: PathBuf::from("/var/lib/zendiloos/updates"),
            release_keys: vec![
                PathBuf::from("/etc/zendiloos/release-signing.pub"),
                PathBuf::from("/usr/share/zendiloos/release-signing.pub"),
            ],
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
    docker: Program,
    dockerd: PathBuf,
    exportfs: Program,
    /// One change to disks and pools at a time.
    serial: tokio::sync::Mutex<()>,
    /// One move of Docker's data at a time; the copy runs without `serial`.
    docker_move: tokio::sync::Mutex<()>,
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
            docker: Program::new(&tools.docker)?,
            dockerd: tools.dockerd.clone(),
            exportfs: Program::new(&tools.exportfs)?,
            serial: tokio::sync::Mutex::new(()),
            docker_move: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) async fn create(
        &self,
        name: &PoolName,
        layout: Layout,
        ids: &[DiskId],
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        for (done, disk) in disks.iter().enumerate() {
            crate::progress::step("preparing_disks", crate::progress::percent(done, disks.len()));
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
        crate::progress::step("creating_pool", None);
        run(&self.zpool, &args).await?;

        crate::progress::step("mounting", None);
        self.mount(name, volume).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn destroy(
        &self,
        name: &PoolName,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(name, Some(volume)).await?;
        let members = self.members(name).await?;
        // Docker would start without its data, on the system disk.
        let docker_root = docker_config(&self.paths.docker_config)?
            .get("data-root")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from);
        let mount_point = self.paths.volumes.join(format!("volume{}", volume.get()));
        if docker_root.is_some_and(|root| root.starts_with(&mount_point)) {
            return Err(refused("Docker's data lies on this pool: move it to another pool first".to_owned()));
        }

        // The shares' mounts first, then the volume's: all are stopped
        // before anything is removed, so one that is busy leaves everything
        // as it was, mounted again.
        let mut units: Vec<String> = fs::read_dir(&self.paths.units)
            .map_err(|e| OperationError::Read(self.paths.units.clone(), e))?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|unit| unit.starts_with(&format!("volume{}-", volume.get())) && unit.ends_with(".mount"))
            .collect();
        units.sort_unstable();
        units.push(unit_name(volume));
        units.retain(|unit| self.paths.units.join(unit).exists());
        crate::progress::step("unmounting", None);
        self.release_nfs().await?;
        let mut stopped: Vec<&str> = Vec::new();
        for unit in &units {
            if let Err(error) = run(&self.systemctl, &["stop", unit]).await {
                self.mount_again(&stopped).await;
                let place = unit
                    .trim_end_matches(".mount")
                    .replace('-', "/")
                    .replace("\\x2d", "-")
                    .replace("\\x40", "@");
                return Err(OperationError::Failed(format!(
                    "/{place} is in use (a program or a client keeps something there open): \
                     nothing was changed; close it and try again ({error})"
                )));
            }
            stopped.push(unit);
        }
        crate::progress::step("destroying_pool", None);
        if let Err(error) = run(&self.zpool, &["destroy", name.as_str()]).await {
            self.mount_again(&stopped).await;
            return Err(error);
        }
        // The pool is gone; so go the units for its mounts.
        crate::progress::step("clearing_disks", None);
        for unit in &units {
            self.remove_unit(unit).await?;
        }

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
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        crate::progress::step("preparing_disks", None);
        let partition = self.prepare(&disk, member_bytes / MIB).await?;
        crate::progress::step("replacing_disk", None);
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
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        // Its mount's stop would stop theirs, and their containers lose
        // what they mount.
        let share = self.paths.volumes.join(format!("volume{}", volume.get())).join(name.as_str());
        let users = crate::app_shares::pinned_from(&self.paths, &share);
        if !users.is_empty() {
            return Err(refused(format!("{} mount folders of it", users.join(", "))));
        }
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
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = self.share_dataset(pool, share).await?;
        run(&self.zfs, &["destroy", &format!("{dataset}@{}", label.as_str())]).await?;
        Ok(Response::Done)
    }

    /// Returns a share to one of its snapshots; later snapshots go too
    /// (`-r`), since ZFS can only roll back to the most recent one.
    pub(crate) async fn rollback_snapshot(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        share: &ShareName,
        label: &SnapshotLabel,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = self.share_dataset(pool, share).await?;
        run(&self.zfs, &["rollback", "-r", &format!("{dataset}@{}", label.as_str())]).await?;
        Ok(Response::Done)
    }

    /// Quota, compression and record size of a share's dataset.
    pub(crate) async fn set_share_properties(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        share: &ShareName,
        quota_bytes: Option<u64>,
        compression: ShareCompression,
        record_size: ShareRecordSize,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = self.share_dataset(pool, share).await?;
        let quota = quota_bytes.map_or("none".to_owned(), |bytes| bytes.to_string());
        let properties = [
            format!("refquota={quota}"),
            format!("compression={}", compression.as_str()),
            format!("recordsize={}", record_size.as_str()),
        ];
        let mut args = vec!["set".to_owned()];
        args.extend(properties);
        args.push(dataset);
        run(&self.zfs, &args).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn share_usage(&self) -> Result<Response, OperationError> {
        let args = ["list", "-H", "-p", "-t", "filesystem", "-d", "1", "-o", "name,used,available"];
        Ok(Response::ShareUsage { zfs_list: output(&self.zfs, &args).await? })
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
        crate::durable::replace(&self.paths.units.join(unit), content.as_bytes())?;
        run(&self.systemctl, &["daemon-reload"]).await?;
        run(&self.systemctl, &["enable", "--now", unit]).await
    }

    /// Starts `units` again, the volume's before its shares', after a
    /// delete that could not go through.
    async fn mount_again(&self, units: &[&str]) {
        for unit in units.iter().rev() {
            if let Err(error) = run(&self.systemctl, &["start", unit]).await {
                tracing::error!(unit, %error, "cannot mount again after a delete that failed");
            }
        }
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

    /// Whether Docker is there and running, and where its data is.
    pub(crate) async fn docker_state(&self) -> Result<Response, OperationError> {
        let installed = self.docker.path().exists() && self.dockerd.exists();
        let running = installed
            && zendiloos_exec::run(&self.systemctl, &["is-active", "--quiet", "docker"], LIMITS)
                .await
                .is_ok_and(|output| output.status.success());
        let data_root = docker_config(&self.paths.docker_config)?
            .get("data-root")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let version = if running {
            let args = ["version", "--format", "{{.Server.Version}}"];
            zendiloos_exec::run(&self.docker, args, LIMITS)
                .await
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .filter(|version| !version.is_empty() && version.len() <= 64)
        } else {
            None
        };
        Ok(Response::DockerState { installed, running, data_root, version })
    }

    /// Puts Docker's data on the pool: dataset, mount unit, a drop-in so
    /// Docker starts only with it mounted, and `data-root` with `overlay2`
    /// in `daemon.json` (other settings there stay). Docker's data from
    /// before stays where it was, unused.
    pub(crate) async fn enable_docker(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        if !self.docker.path().exists() || !self.dockerd.exists() {
            return Err(refused("Docker is not installed: apt install docker.io docker-compose".into()));
        }
        self.require_managed(pool, Some(volume)).await?;
        let root = format!("/volume{}/{DOCKER_DIRECTORY}", volume.get());
        let config = docker_config(&self.paths.docker_config)?;
        if let Some(other) = config.get("data-root").and_then(serde_json::Value::as_str)
            && other != root
            && other.ends_with(&format!("/{DOCKER_DIRECTORY}"))
        {
            return Err(refused(format!("Docker's data is on another pool already ({other})")));
        }
        let dataset = format!("{}/{DOCKER_DATASET}", pool.as_str());
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "1", pool.as_str()]).await?;
        if !existing.lines().any(|line| line == dataset) {
            run(&self.zfs, &["create", "-o", "mountpoint=legacy", &dataset]).await?;
        }
        self.point_docker(pool, volume, config).await?;
        run(&self.systemctl, &["restart", "docker"]).await?;
        Ok(Response::Done)
    }

    /// Docker's data on `pool`: its mount unit, a drop-in so Docker starts
    /// only with it mounted, and `data-root` with `overlay2` in `config`,
    /// written back as `daemon.json`.
    async fn point_docker(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        mut config: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), OperationError> {
        let root = format!("/volume{}/{DOCKER_DIRECTORY}", volume.get());
        let dataset = format!("{}/{DOCKER_DATASET}", pool.as_str());
        let unit = docker_unit_name(volume);
        let content = format!(
            "# Created by zendiloOS: Docker's data (images, containers) on pool {pool}.\n\
             [Unit]\n\
             Description=zendiloOS: Docker's data (ZFS dataset {dataset})\n\
             Requires=zfs-import.target\n\
             After=zfs-import.target\n\
             Before=docker.service\n\
             \n\
             [Mount]\n\
             What={dataset}\n\
             Where={root}\n\
             Type=zfs\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            pool = pool.as_str(),
        );
        self.install_unit(&unit, &content).await?;
        let drop_in = self.paths.units.join("docker.service.d");
        fs::create_dir_all(&drop_in).map_err(|e| OperationError::Write(drop_in.clone(), e))?;
        let drop_in = drop_in.join("zendiloos.conf");
        let requires = format!(
            "# Written by zendiloOS: Docker's data is on a pool; without it, Docker waits.\n[Unit]\nRequiresMountsFor={root}\n"
        );
        write_file(&drop_in, &requires)?;
        config.insert("data-root".to_owned(), root.clone().into());
        config.insert("storage-driver".to_owned(), "overlay2".into());
        let text = serde_json::to_string_pretty(&config).unwrap_or_default();
        if let Some(parent) = self.paths.docker_config.parent() {
            fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
        }
        write_file(&self.paths.docker_config, &format!("{text}\n"))?;
        run(&self.systemctl, &["daemon-reload"]).await?;
        Ok(())
    }

    /// Moves Docker's data from one pool to another: stopped, copied with
    /// `zfs send | zfs receive`, started from the copy; the old dataset is
    /// destroyed last. If the copy fails, Docker starts as it was.
    pub(crate) async fn move_docker(
        &self,
        from: &PoolName,
        from_volume: VolumeNumber,
        to: &PoolName,
        to_volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        // The pools' own lock only while something is checked or changed,
        // never through the hours a copy may take: every other change to a
        // pool would wait. Meanwhile the source cannot be destroyed (the
        // data root is still on it) and a second move waits here.
        let _moving = crate::waiting::lock(&self.docker_move).await?;
        let serial = crate::waiting::lock(&self.serial).await?;
        if !self.docker.path().exists() || !self.dockerd.exists() {
            return Err(refused("Docker is not installed".into()));
        }
        if from == to {
            return Err(refused(format!("Docker's data is on {} already", to.as_str())));
        }
        self.require_managed(from, Some(from_volume)).await?;
        self.require_managed(to, Some(to_volume)).await?;
        let old_root = format!("/volume{}/{DOCKER_DIRECTORY}", from_volume.get());
        let config = docker_config(&self.paths.docker_config)?;
        if config.get("data-root").and_then(serde_json::Value::as_str) != Some(old_root.as_str()) {
            return Err(refused(format!("Docker's data is not on {}", from.as_str())));
        }
        let source = format!("{}/{DOCKER_DATASET}", from.as_str());
        let target = format!("{}/{DOCKER_DATASET}", to.as_str());
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "1", to.as_str()]).await?;
        if existing.lines().any(|line| line == target) {
            return Err(refused(format!("{} holds Docker's data already", to.as_str())));
        }
        // Stopped, so nothing changes while it is copied.
        let _ = run(&self.systemctl, &["stop", "docker.socket"]).await;
        run(&self.systemctl, &["stop", "docker.service"]).await?;
        drop(serial);

        let snapshot = format!("{source}@zendiloos-move");
        let _ = run(&self.zfs, &["destroy", &snapshot]).await;
        let copied = async {
            run(&self.zfs, &["snapshot", &snapshot]).await?;
            self.send_receive(&snapshot, &target).await
        }
        .await;
        // In the middle of the move: waits as long as it takes.
        let _serial = self.serial.lock().await;
        if let Err(error) = copied {
            let _ = run(&self.zfs, &["destroy", "-r", &target]).await;
            let _ = run(&self.zfs, &["destroy", &snapshot]).await;
            let _ = run(&self.systemctl, &["start", "docker.service"]).await;
            return Err(error);
        }
        // From here on, Docker's data is the copy. Should switching to it
        // fail, Docker goes back to its data as it was, which is untouched,
        // and starts again: never left stopped.
        let switched = async {
            self.point_docker(to, to_volume, config.clone()).await?;
            self.remove_unit(&docker_unit_name(from_volume)).await?;
            run(&self.systemctl, &["start", "docker.service"]).await
        }
        .await;
        if let Err(error) = switched {
            let _ = self.remove_unit(&docker_unit_name(to_volume)).await;
            if let Err(again) = self.point_docker(from, from_volume, config).await {
                tracing::error!(%again, "cannot point Docker back at its data");
            }
            let _ = run(&self.systemctl, &["start", "docker.service"]).await;
            let _ = run(&self.zfs, &["destroy", "-r", &target]).await;
            let _ = run(&self.zfs, &["destroy", &snapshot]).await;
            return Err(error);
        }
        let _ = run(&self.zfs, &["destroy", &format!("{target}@zendiloos-move")]).await;
        run(&self.zfs, &["destroy", "-r", &source]).await?;
        Ok(Response::Done)
    }

    /// `zfs send -R snapshot | zfs receive -u target`, unmounted, for up to
    /// six hours.
    async fn send_receive(&self, snapshot: &str, target: &str) -> Result<(), OperationError> {
        use std::process::Stdio;
        let spawn_error = |e: std::io::Error| OperationError::Failed(format!("cannot start zfs: {e}"));
        let mut send = tokio::process::Command::new(self.zfs.path())
            .args(["send", "-R", snapshot])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(spawn_error)?;
        let stream: Stdio = send
            .stdout
            .take()
            .ok_or_else(|| OperationError::Failed("zfs send gives no stream".into()))?
            .try_into()
            .map_err(spawn_error)?;
        let receive = tokio::process::Command::new(self.zfs.path())
            .args(["receive", "-u", "-o", "mountpoint=legacy", target])
            .env_clear()
            .stdin(stream)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(spawn_error)?;
        let both = async { tokio::join!(send.wait_with_output(), receive.wait_with_output()) };
        let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(6 * 3600), both)
            .await
            .map_err(|_| OperationError::Failed("copying Docker's data took longer than six hours".into()))?;
        for (what, finished) in [("zfs send", sent), ("zfs receive", received)] {
            let finished = finished.map_err(spawn_error)?;
            if !finished.status.success() {
                let complaint = String::from_utf8_lossy(&finished.stderr).trim().to_owned();
                return Err(OperationError::Failed(format!("{what}: {complaint}")));
            }
        }
        Ok(())
    }

    /// The personal folders' dataset, mounted at `/volume<N>/@homes`.
    /// What exists stays.
    pub(crate) async fn prepare_homes(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = format!("{}/{HOMES_DATASET}", pool.as_str());
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "1", pool.as_str()]).await?;
        if !existing.lines().any(|line| line == dataset) {
            let options = ["create", "-o", "mountpoint=legacy", "-o", "setuid=off", "-o", "devices=off"];
            run(&self.zfs, &[&options[..], &[dataset.as_str()]].concat()).await?;
        }
        let content = format!(
            "# Created by zendiloOS: the personal folders on pool {pool}.\n\
             [Unit]\n\
             Description=zendiloOS personal folders (ZFS dataset {dataset})\n\
             Requires=zfs-import.target\n\
             After=zfs-import.target\n\
             \n\
             [Mount]\n\
             What={dataset}\n\
             Where=/volume{number}/{HOMES_DIRECTORY}\n\
             Type=zfs\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            pool = pool.as_str(),
            number = volume.get(),
        );
        // Its owner and mode come with the first folder (`MakeHome`): only
        // the files agent may write to the volumes.
        self.install_unit(&homes_unit_name(volume), &content).await?;
        Ok(Response::Done)
    }

    /// How much `user` may store in the personal folders' dataset.
    pub(crate) async fn set_home_quota(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        user: &UserName,
        quota_bytes: Option<u64>,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        let dataset = format!("{}/{HOMES_DATASET}", pool.as_str());
        let quota = quota_bytes.map_or_else(|| "none".to_owned(), |bytes| bytes.to_string());
        run(&self.zfs, &["set", &format!("userquota@{}={quota}", user.as_str()), &dataset]).await?;
        Ok(Response::Done)
    }

    /// Unmounts an app's dataset and destroys it with all its data.
    pub(crate) async fn destroy_app_dataset(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        app: &AppName,
    ) -> Result<(), OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed(pool, Some(volume)).await?;
        self.remove_unit(&app_unit_name(volume, app)).await?;
        let dataset = format!("{}/{APPS_DATASET}/{}", pool.as_str(), app.as_str());
        let existing = output(&self.zfs, &["list", "-H", "-o", "name", "-d", "2", pool.as_str()]).await?;
        if existing.lines().any(|line| line == dataset) {
            run(&self.zfs, &["destroy", "-r", &dataset]).await?;
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
/// Docker's dataset below a pool: the dot keeps it from ever being a share.
pub(crate) const DOCKER_DATASET: &str = "zendiloos.docker";
/// Where it is mounted on the volume.
pub(crate) const DOCKER_DIRECTORY: &str = "@docker";

/// The apps' datasets below a pool: `<pool>/zendiloos.apps/<app>`.
pub(crate) const APPS_DATASET: &str = "zendiloos.apps";

/// `volume<N>-\x40apps-<app>.mount`; systemd escapes `-` in paths.
pub(crate) fn app_unit_name(volume: VolumeNumber, app: &AppName) -> String {
    format!("volume{}-\\x40apps-{}.mount", volume.get(), app.as_str().replace('-', "\\x2d"))
}

/// The personal folders' dataset below a pool: `<pool>/zendiloos.homes`.
pub(crate) const HOMES_DATASET: &str = "zendiloos.homes";

/// `volume<N>-\x40homes.mount`.
pub(crate) fn homes_unit_name(volume: VolumeNumber) -> String {
    format!("volume{}-\\x40homes.mount", volume.get())
}

/// `volume<N>-\x40docker.mount`.
pub(crate) fn docker_unit_name(volume: VolumeNumber) -> String {
    format!("volume{}-\\x40docker.mount", volume.get())
}

/// `/etc/docker/daemon.json` as an object; empty if it is not there.
fn docker_config(
    path: &std::path::Path,
) -> Result<serde_json::Map<String, serde_json::Value>, OperationError> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(serde_json::Value::Object(map)) => Ok(map),
            _ => Err(refused(format!("{} is not a JSON object; fix or remove it first", path.display()))),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(OperationError::Read(path.to_owned(), error)),
    }
}

fn write_file(path: &std::path::Path, content: &str) -> Result<(), OperationError> {
    crate::durable::replace(path, content.as_bytes())
}

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
