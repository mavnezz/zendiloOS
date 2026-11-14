//! Wire protocol between `zendiloosd` (unprivileged) and `zendiloos-agent`
//! (root). This is the privilege boundary, so the protocol is narrow:
//!
//! - one request per connection, one JSON object per line, size-limited;
//! - a closed set of operations ([`Request`]) with typed arguments that are
//!   validated while deserializing, so an invalid value never reaches the
//!   code that runs a program;
//! - a protocol version in every request; the agent refuses others.
//!
//! See docs/architecture.md, section 3.2.

mod acme;
mod apps;
mod backup;
mod client;
mod configuration;
mod descriptor;
mod docker;
mod files;
mod firewall;
mod framing;
mod host;
mod network;
mod packages;
mod removable;
mod services;
mod ssh;
mod time;
mod ups;
mod vms;
mod web;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use acme::{
    AcmeCa, AcmeChallenge, AcmeDomain, AcmeDomains, AcmeEmail, AcmeKey, AcmeOrder, DnsApi, DnsCredentials,
    DnsValue, DnsVariableName, InvalidAcmeDomain, InvalidAcmeDomains, InvalidAcmeEmail, InvalidDnsApi,
    InvalidDnsCredentials, InvalidDnsValue, InvalidDnsVariableName, InvalidWebPort, PendingWebServer,
    WebPort, WebServerConfig,
};
pub use apps::{
    AppFolder, AppName, DevicePath, InvalidAppFolder, InvalidAppName, InvalidDevicePath, MAX_LOG_LINES,
};
pub use backup::{
    BACKUP_EXCLUDES_MAX, BACKUP_HOST_KEY_CHANGED, BACKUP_KEY, BACKUP_PRUNE_MAX, BACKUP_RESTORE_PATHS_MAX,
    BACKUP_STATE, BackupCloud, BackupCloudAccess, BackupCloudWord, BackupCrypt, BackupEnd, BackupExisting,
    BackupFolder, BackupHostKey, BackupHostName, BackupMode, BackupPassword, BackupPattern, BackupPruned,
    BackupRemotePath, BackupRestore, BackupRoot, BackupRun, BackupSecret, BackupSecrets, BackupServer,
    BackupStamp, BackupTaskId, BackupTransport, BackupWord, InvalidBackupCloudWord, InvalidBackupFolder,
    InvalidBackupHostKey, InvalidBackupHostName, InvalidBackupPassword, InvalidBackupPattern,
    InvalidBackupPruned, InvalidBackupRemotePath, InvalidBackupSecret, InvalidBackupStamp,
    InvalidBackupTaskId, InvalidBackupWord, is_stamp,
};
pub use client::{AgentTransport, Report, TransportError, UnixTransport};
pub use configuration::{
    ConfigurationFile, ConfigurationPath, FileContent, InvalidConfigurationFile, InvalidConfigurationPath,
    InvalidFileContent, MANAGED_IDS, RestoredGroup, RestoredUser, network_file_name,
};
pub use descriptor::{MessageReader, read_message_with_file, write_message_with_file};
pub use docker::{
    ArchiveDigest, BackupStem, ComposeText, ContainerAction, ContainerId, DOCKER_AROUND, DOCKER_PATIENCE,
    DOCKER_WORK, ExecCommand, FoundBackup, ImageId, ImageReference, InvalidDockerName, NetworkName,
    ProjectAction, ProjectConfig, ProjectDirectory, ProjectName, RefusedBackupFile, ShareLocation, Subnet,
    TerminalSize, VolumeLabels, VolumeName, VolumeSize, WrittenFile,
};
pub use files::{
    CountStop, DirectoryEntry, EntryFacts, EntryKind, FileLocation, FileRoot, FoundEntry, HOMES_DIRECTORY,
    InvalidSearchText, InvalidSharePath, MAX_COUNTED, MAX_DIRECTORY_ENTRIES, MAX_SEARCH_RESULTS, SearchStop,
    SearchText, SharePath, is_file_name,
};
pub use firewall::{
    DockerBridge, FirewallConfig, FirewallPortRange, FirewallPorts, FirewallRule, FirewallRules,
    FirewallSource, FirewallTraffic, FirewallVerdict, InvalidFirewallPorts, InvalidFirewallSource,
    NotADockerBridge, PendingFirewall, TooManyFirewallRules, WaitingFirewall,
};
pub use framing::{FrameError, read_message, write_message};
pub use host::{HostName, InvalidHostName};
pub use network::{
    BondMode, ConfirmWithin, DeviceAction, FilesDigest, Ifupdown, IfupdownFile, InterfaceName,
    InvalidConfirmWithin, InvalidFilesDigest, InvalidInterfaceName, InvalidMacAddress, InvalidNameServers,
    InvalidNetdev, InvalidNetworkConfig, InvalidNetworkEdit, InvalidPrefix, InvalidTakeoverConfigs,
    InvalidVlanId, InvalidVlans, Ipv4Config, Ipv6Config, MacAddress, NameServers, Netdev, NetdevKind,
    NetworkChange, NetworkConfig, NetworkEdit, NetworkdFile, PendingNetwork, PortOf, Prefix, TakeoverConfigs,
    VlanId, Vlans,
};
pub use packages::{
    BootKernel, BootReason, InvalidReleasePackage, PackageReport, PackageRun, PackageRunState, PackageTask,
    ReleaseChannel, ReleaseInstall, ReleasePackage, UpgradablePackage,
};
pub use removable::{
    InvalidRemovableId, MEMBER_SIGNATURES, REMOVABLE_PLACE, RemovableFilesystem, RemovableId,
};
pub use services::{
    FileServices, InvalidVetoFiles, InvalidWorkgroup, SmbEncryption, SmbProtocol, SmbSigning, VetoFiles,
    Workgroup,
};
pub use ssh::{InvalidOffAfter, InvalidSshPort, OffAfter, RootLogin, SshConfig, SshPort};
pub use time::{
    ClockTime, InvalidClockTime, InvalidTimeServer, InvalidTimeServers, InvalidTimeZone, MAX_TIME_SERVERS,
    TimeServer, TimeServers, TimeZone,
};
pub use ups::{
    FoundUps, InvalidUpsConfig, InvalidUpsHost, InvalidUpsServerPort, InvalidUpsWord, InvalidUsbId,
    SnmpVersion, UPS_LOCAL_USER, UPS_NAME, UPS_PORT, UpsAccount, UpsConfig, UpsHost, UpsServerPort,
    UpsShutdown, UpsSource, UpsWord, UsbDriver, UsbId,
};
pub use vms::{
    HostIommu, HostNeeds, HostPciDevice, HostUsbDevice, InvalidPciAddress, InvalidVmBackupSettings,
    InvalidVmCpus, InvalidVmDiskSize, InvalidVmId, InvalidVmImageId, InvalidVmMemory, InvalidVmSlot,
    InvalidVmSpec, PciAddress, VM_BACKUP_MANIFEST, VM_CONSOLES, VM_DISKS_MAX, VM_DOMAIN_PREFIX,
    VM_DRIVES_MAX, VM_IMAGE_UPLOAD_MAX, VM_IMAGES_DIRECTORY, VM_NICS_MAX, VM_PCI_MAX, VM_USB_MAX,
    VMS_DATASET, VMS_DIRECTORY, VmAction, VmBackupFile, VmBackupPart, VmBackupSettings, VmBoot,
    VmConsistency, VmCpus, VmDevices, VmDisk, VmDiskBus, VmDiskSize, VmDiskSlot, VmDrive, VmDriveSlot,
    VmFirmware, VmId, VmImageFile, VmImageId, VmIsoSource, VmMemory, VmNetwork, VmNic, VmNicModel, VmNicSlot,
    VmOs, VmSpec, VmStorage, VmUsbDevice,
};
pub use web::TlsMinimum;

/// The recycle bin at a share's root: `.recycle/<user>` per user, as
/// Samba's `vfs_recycle` keeps it.
pub const RECYCLE_DIRECTORY: &str = ".recycle";

/// Agent and API come from the same release; any difference is refused.
pub const PROTOCOL_VERSION: u32 = 1;

/// Debian's name of the architecture this build of zendiloOS runs on, which
/// is its package's (`zendiloos_<version>_<arch>.deb`): `amd64` or `arm64`.
#[cfg(target_arch = "x86_64")]
pub const ARCHITECTURE: &str = "amd64";
/// Debian's name of the architecture this build of zendiloOS runs on, which
/// is its package's (`zendiloos_<version>_<arch>.deb`): `amd64` or `arm64`.
#[cfg(target_arch = "aarch64")]
pub const ARCHITECTURE: &str = "arm64";
/// Debian's name of the architecture this build of zendiloOS runs on; none
/// other has a package, so Rust's name stands for it.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const ARCHITECTURE: &str = std::env::consts::ARCH;
/// Largest request the agent reads: room for the largest project's
/// configuration ([`ProjectConfig::MAX_BYTES`]) and the request around it.
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const _: () = assert!(ProjectConfig::MAX_BYTES + 64 * 1024 <= MAX_REQUEST_BYTES);
/// Largest response the client reads.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// What travels from `zendiloosd` to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    #[serde(default)]
    pub request_id: Option<RequestId>,
    /// How long the client waits for the answer, in milliseconds. The
    /// agent waits for the request's turn half of it at most, so what was
    /// asked has time left to run (none: as long as it takes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    pub request: Request,
}

impl Envelope {
    pub fn new(request: Request, request_id: Option<RequestId>) -> Self {
        Self { version: PROTOCOL_VERSION, request_id, wait_ms: None, request }
    }

    /// Says how long the client waits for the answer.
    pub fn waiting(self, wait: std::time::Duration) -> Self {
        Self { wait_ms: Some(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)), ..self }
    }
}

/// Every operation the agent performs. Nothing else can be asked of it.
///
/// Variants without arguments are written `Reboot {}` rather than `Reboot`:
/// serde ignores unknown fields next to the tag of a unit variant, and the
/// agent must refuse `{"op": "reboot", "force": true}` instead of quietly
/// rebooting without the option the caller believes it set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Version handshake; changes nothing.
    Hello {},
    Reboot {},
    PowerOff {},
    /// `lsblk` output for all block devices, columns [`LSBLK_COLUMNS`],
    /// and udev's database of the whole disks, for where each is plugged
    /// in (`ID_PATH`).
    ListBlockDevices {},
    /// What the firmware calls the machine: DMI's `sys_vendor` and
    /// `product_name`. Answers [`Response::Machine`].
    Machine {},
    /// Reads from the whole disk `device` for `millis` milliseconds, which
    /// keeps its activity light on: 1 MiB at a time from places spread
    /// over the disk, opened read-only and with `O_DIRECT`, so the reads
    /// reach the disk rather than the cache. Never writes. Only a disk the
    /// agent itself finds to be whole, as for SMART.
    ReadBurst {
        device: DiskName,
        millis: BurstMillis,
    },
    /// `smartctl` output for one whole disk. Sleeping disks are not woken.
    SmartData {
        device: DiskName,
    },
    /// What an eMMC device (`mmcblk<N>`), which has no SMART, reports of
    /// its wear: the kernel's files of its card in sysfs. Answers
    /// [`Response::Emmc`].
    EmmcHealth {
        device: DiskName,
    },
    /// Has the drive start a self-test (`smartctl -t short|long`); the drive
    /// runs it alongside its work.
    StartSelfTest {
        device: DiskName,
        kind: SelfTestKind,
    },
    /// Stops the drive's running self-test (`smartctl -X`).
    AbortSelfTest {
        device: DiskName,
    },
    /// The kernel's software RAID status, `/proc/mdstat`.
    ArrayStatus {},
    /// `zpool status` and `zfs list` of the pools' root datasets, as JSON
    /// (OpenZFS 2.3); the columns are [`ZFS_DATASET_COLUMNS`].
    ZfsStatus {},
    /// Erases `disks`, gives each one GPT partition of the same size and
    /// creates pool `name` on them, its root dataset mounted at
    /// `/volume<volume>`. Every disk is checked first, independently of
    /// zendiloosd: erasable by configuration, not mounted, not in use.
    CreatePool {
        name: PoolName,
        layout: Layout,
        disks: Vec<DiskId>,
        volume: VolumeNumber,
    },
    /// Unmounts and destroys a pool zendiloos created (its `zendiloos:volume`
    /// must be `volume`), then erases its disks.
    DestroyPool {
        name: PoolName,
        volume: VolumeNumber,
    },
    /// Erases `disk`, gives it a partition of the pool's member size and
    /// replaces the device with GUID `device` with it.
    ReplaceDisk {
        pool: PoolName,
        device: u64,
        disk: DiskId,
    },
    /// The pools `zpool import` finds on the disks that are not imported,
    /// as it lists them: answers [`Response::ImportablePools`]. Imports
    /// nothing.
    ImportablePools {},
    /// Imports the pool with the numeric identifier `id`, which must be
    /// called `name`, without mounting anything (`zpool import -N`), its
    /// devices by `/dev/disk/by-id`. Only one zendiloOS made: its root
    /// dataset's `zendiloos:volume` must be `volume`, or, with `adopt`, any
    /// number, which then becomes `volume`; anything else is exported
    /// again. Refused, as ZFS refuses it, if another machine used the pool
    /// last, unless `force` (`zpool import -f`); a conflict if a pool of
    /// that name is imported or another pool is volume `volume`.
    ImportPool {
        name: PoolName,
        id: u64,
        volume: VolumeNumber,
        #[serde(default)]
        adopt: bool,
        #[serde(default)]
        force: bool,
    },
    /// Makes sure the mount units of an imported pool zendiloOS made exist,
    /// as creating them writes them, are enabled and started: the volume's,
    /// those of `shares` whose datasets are on it, and those of its personal
    /// folders', Docker's and the virtual machines' datasets where it has
    /// them. A unit is written only when its content differs, and systemd
    /// reloaded once. Answers [`Response::VolumeMounted`].
    MountVolume {
        pool: PoolName,
        volume: VolumeNumber,
        #[serde(default)]
        shares: Vec<ShareName>,
    },
    /// Starts a scrub of the pool.
    ScrubPool {
        pool: PoolName,
    },
    /// Stops the pool's scrub (`zpool scrub -s`).
    StopScrub {
        pool: PoolName,
    },
    /// Mounts the removable drive whose file system has UUID `id` at its
    /// place ([`RemovableId::place`]): no setuid programs, device files or
    /// programs run from it, FAT, exFAT and NTFS readable and writable by
    /// the group `zendiloos-users` only; read-only if it cannot be written.
    /// The agent checks the drive itself first: the only file system with
    /// this UUID, one it mounts, on a USB or removable disk that holds no
    /// system, pool, RAID, LVM or encrypted device, nothing on it mounted
    /// or held elsewhere. Answers [`Response::RemovableMounted`].
    MountRemovable {
        id: RemovableId,
    },
    /// Unmounts the removable drive `id` (refused while files on it are
    /// open) and removes its place; with `power_off`, also takes its disk
    /// off the bus so it can be pulled out, as Windows' "Eject" does.
    EjectRemovable {
        id: RemovableId,
        power_off: bool,
    },
    /// Lets go of removable drives pulled out without being ejected:
    /// unmounts what is left of them, lazily, and removes empty places.
    /// Answers [`Response::RemovableCleaned`].
    CleanRemovable {},
    /// Takes snapshot `<pool>/<share>@<label>` of a share on a pool
    /// zendiloos created, noting the schedules it is for and a note as ZFS
    /// properties.
    CreateSnapshot {
        pool: PoolName,
        volume: VolumeNumber,
        share: ShareName,
        label: SnapshotLabel,
        #[serde(default)]
        schedules: Vec<SnapshotFrequency>,
        #[serde(default)]
        note: Option<SnapshotNote>,
    },
    /// Destroys one snapshot of such a share; never the share itself.
    DestroySnapshot {
        pool: PoolName,
        volume: VolumeNumber,
        share: ShareName,
        label: SnapshotLabel,
    },
    /// Whether Docker is installed and running, and where its data is.
    /// Answers [`Response::DockerState`].
    DockerState {},
    /// Starts apt on a task in a unit of its own; refused while a run is
    /// going on. Answers [`Response::PackagesStarted`].
    StartPackages {
        task: PackageTask,
        /// For [`PackageTask::Update`] only: zendiloOS from this release,
        /// if the NAS's channel takes its kind of build.
        #[serde(default)]
        release: Option<ReleaseInstall>,
    },
    /// The latest run of apt, and the last update: [`Response::Packages`].
    /// A run whose unit is gone without finishing counts as failed.
    PackagesState {},
    /// Puts Docker's data on a pool zendiloos created: the dataset
    /// `<pool>/zendiloos.docker`, mounted at `/volume<N>/@docker`, as
    /// `data-root` with the `overlay2` driver; Docker starts only with it
    /// mounted.
    EnableDocker {
        pool: PoolName,
        volume: VolumeNumber,
    },
    /// Moves Docker's data to another pool zendiloos created: Docker stops,
    /// the dataset is copied (`zfs send | zfs receive`), Docker starts from
    /// the copy, and only then is the old dataset destroyed. Containers
    /// that restart by themselves start again; apps' data stays where it is.
    MoveDocker {
        from_pool: PoolName,
        from_volume: VolumeNumber,
        to_pool: PoolName,
        to_volume: VolumeNumber,
    },
    /// Starts an app installed the earlier way (its kept compose file in
    /// `/etc/zendiloos/apps`), after checking that no folder it mounts has
    /// become a link.
    StartApp {
        app: AppName,
    },
    StopApp {
        app: AppName,
    },
    /// Removes the app's containers, images and compose file, and destroys
    /// its dataset with all its data. The account stays until
    /// [`Request::DeleteAppAccount`]: share permissions name it.
    RemoveApp {
        app: AppName,
        pool: PoolName,
        volume: VolumeNumber,
    },
    DeleteAppAccount {
        app: AppName,
    },
    /// The Compose projects and their containers' states. Answers
    /// [`Response::Apps`].
    AppsStatus {},
    /// The last `lines` lines of the app's containers' output. Answers
    /// [`Response::AppLogs`].
    AppLogs {
        app: AppName,
        lines: u32,
    },
    /// What filling in an app of a catalog for an administrator's project
    /// takes from the machine: the UID of `user`, an account zendiloOS
    /// manages, and the GID of `zendiloos-files`, which the containers run
    /// as, so that they read and write the shares' folders as that group
    /// does; the time zone; and which of `devices` the machine has (at most
    /// [`DevicePath::MAX_COUNT`]). Answers [`Response::HostFacts`].
    HostFacts {
        user: UserName,
        devices: Vec<DevicePath>,
    },
    // The Docker engine, whoever started what runs on it. Each answers
    // [`Response::DockerOutput`] with what `docker` printed, or
    // [`Response::Done`].
    /// `docker container inspect` of every container: a JSON array.
    DockerContainers {},
    /// One sample of every container's use, `docker stats --no-stream`:
    /// JSON lines.
    DockerStats {},
    /// `docker image ls --digests`: JSON lines.
    DockerImages {},
    /// `docker network inspect` of every network: a JSON array.
    DockerNetworks {},
    /// `docker system df`: JSON lines.
    DockerDiskUsage {},
    ControlContainer {
        container: ContainerId,
        action: ContainerAction,
    },
    /// The last `lines` lines a container wrote, both streams in order,
    /// with timestamps.
    ContainerLogs {
        container: ContainerId,
        lines: u32,
    },
    /// `docker container inspect` of one container: a JSON array of one.
    InspectContainer {
        container: ContainerId,
    },
    /// The processes of a running container, `docker top` with fixed
    /// columns: a table, its header first.
    ContainerProcesses {
        container: ContainerId,
    },
    /// Starts `command` in a running container on a terminal of its own
    /// (`docker exec --interactive --tty`) and answers
    /// [`Response::Terminal`], which carries the terminal's master side:
    /// what is written to it is typed, what is read from it is what the
    /// program wrote, its size is set on it. Closing it hangs up, which
    /// ends the program; a session lasts a day at most.
    OpenTerminal {
        container: ContainerId,
        command: ExecCommand,
        size: TerminalSize,
    },
    RemoveImage {
        image: ImageId,
    },
    PullImage {
        reference: ImageReference,
    },
    /// Removes the images no container uses; prints what was freed.
    PruneImages {},
    /// A bridge network, with the subnet and gateway given or Docker's.
    CreateNetwork {
        name: NetworkName,
        subnet: Option<Subnet>,
        gateway: Option<std::net::IpAddr>,
    },
    RemoveNetwork {
        name: NetworkName,
    },
    /// `docker volume inspect` of every volume: a JSON array.
    DockerVolumes {},
    /// What each local volume's data takes up, counted in Docker's data
    /// root with a bounded walk (no links followed, no other file system
    /// entered). Answers [`Response::VolumeSizes`].
    DockerVolumeSizes {},
    /// Removes a volume no container uses: Docker refuses one that a
    /// container, also a stopped one, uses.
    RemoveVolume {
        name: VolumeName,
    },
    /// Removes every volume no container uses, named ones too; prints what
    /// was freed.
    PruneVolumes {},
    /// A local volume with `labels`.
    CreateVolume {
        name: VolumeName,
        labels: VolumeLabels,
    },
    /// Backs up a local volume (no mount options: its data lies in Docker's
    /// data root) into the share: `<folder>/<volume>/<name>.tar.zst`, with
    /// `<name>.json` beside it, its manifest. The agent inspects the volume
    /// itself and reads its data as root in a transient unit, which may
    /// write only the share; what it writes takes the folder's inherited
    /// ACL. Answers [`Response::VolumeBackedUp`].
    BackUpVolume {
        volume: VolumeName,
        share: ShareLocation,
        folder: SharePath,
        name: BackupStem,
    },
    /// Replaces the contents of a local volume no running container uses
    /// with the archive `archive` of the share, if only root may write it
    /// and its SHA-256 is `sha256`: checked before anything is extracted,
    /// and again of what was extracted. Extracted into a fresh folder beside
    /// the data by one unit, each entry beneath it (no absolute paths, no
    /// `..`, no link followed, no devices), no more than the backup's
    /// `data_bytes` and `entries` and a little; then, if still no running
    /// container uses the volume, swapped in by another. A failure leaves
    /// the old contents.
    RestoreVolume {
        volume: VolumeName,
        share: ShareLocation,
        archive: SharePath,
        sha256: ArchiveDigest,
        data_bytes: u64,
        /// Unknown for backups recorded before entries were counted.
        entries: Option<u64>,
    },
    /// Stops what a backup or restore of `volume` left running (its units,
    /// should their client have gone) and answers once nothing runs for it
    /// any more. Answers [`Response::VolumeWorkEnded`].
    EndVolumeWork {
        volume: VolumeName,
    },
    /// Looks for backups in `folder` of the share and the folders in it, to
    /// record them again: archive and manifest pairs
    /// (`<volume>-YYYYMMDD-HHMMSS.tar.zst` and `.json`), both writable by
    /// root only, the archive hashed and its SHA-256 the manifest's. Those
    /// `known` are passed over. Read only, in a unit of its own. Answers
    /// [`Response::VolumeBackupsScanned`].
    ScanVolumeBackups {
        share: ShareLocation,
        folder: SharePath,
        known: Vec<SharePath>,
    },
    /// Deletes a file a backup wrote, if `path` still is that file: its
    /// inode, size and time of writing (if recorded), on the share's file
    /// system as it is mounted now; never anything else. Answers
    /// [`Response::BackupRemoved`].
    RemoveVolumeBackup {
        share: ShareLocation,
        path: SharePath,
        inode: u64,
        bytes: u64,
        modified: Option<i64>,
    },
    /// `docker compose config` of `compose` as if it were in `directory`:
    /// the file as Compose understands it, as JSON, or refused with why
    /// not.
    CheckCompose {
        project: ProjectName,
        directory: ProjectDirectory,
        compose: ComposeText,
    },
    /// Acts on a Compose project that is not an app of a catalog's (the App
    /// Center keeps those). `Up` and `Pull` take the files Compose reports,
    /// refused when they lie in a volume, where whoever may write the share
    /// could change them, or bind a folder or file in a share, which runs
    /// unpinned by its path; administrators' projects deploy with
    /// [`Request::DeployProject`]. `Down` of an administrator's project also
    /// removes what deploying it kept.
    ControlProject {
        project: ProjectName,
        action: ProjectAction,
    },
    /// Deploys an administrator's project as they confirmed it: `config`
    /// is what Compose made of their file then. The folders and files its
    /// binds name in volumes are pinned (walked to without links, mounted
    /// where only root can change them), the result is kept by root, and
    /// with `pull` its images are fetched before it comes up.
    DeployProject {
        project: ProjectName,
        config: ProjectConfig,
        pull: bool,
    },
    /// The last `lines` lines of the project's containers, with timestamps.
    ProjectLogs {
        project: ProjectName,
        lines: u32,
    },
    /// The Compose files Compose reports for a project that is not an app
    /// of a catalog's: a JSON object of path and content.
    ProjectFiles {
        project: ProjectName,
    },
    /// Whether systemd-networkd manages the network, a change that waits
    /// to be confirmed, and where it does not, what does. Answers
    /// [`Response::NetworkState`].
    NetworkState {},
    /// Writes the systemd-networkd files `change` names (an interface's
    /// settings; a bridge, bond or VLAN made, changed or removed with what
    /// that changes of its members and parent), removes the files and
    /// links of devices that go, and applies it all; unless
    /// [`Request::ConfirmNetwork`] comes within `confirm_within`, every
    /// file returns as it was and the devices it made go (also after a
    /// restart). One change at a time.
    ChangeNetwork {
        change: NetworkEdit,
        confirm_within: ConfirmWithin,
    },
    /// Takes the network over from ifupdown: `configs` written as
    /// systemd-networkd files, ifupdown left with the loopback only,
    /// systemd-networkd and systemd-resolved started. Refused unless
    /// ifupdown's files are still those of `digest`. Unless
    /// [`Request::ConfirmNetwork`] comes within `confirm_within`, ifupdown
    /// returns exactly as it was (also after a restart). One change at a
    /// time.
    TakeOverNetwork {
        configs: TakeoverConfigs,
        digest: FilesDigest,
        confirm_within: ConfirmWithin,
    },
    /// Keeps the change that waits to be confirmed.
    ConfirmNetwork {},
    /// Whether the SSH server is installed and runs, the settings
    /// zendiloOS wrote, and when it turns itself off. Answers
    /// [`Response::SshState`].
    SshState {},
    /// Writes the SSH server's settings, checks them (`sshd -t`), starts and
    /// enables it or stops and disables it; open sessions stay. With
    /// `off_after`, a timer turns it off again.
    SetSsh {
        config: SshConfig,
    },
    /// Whether Network UPS Tools are installed, and its SNMP driver.
    /// Answers [`Response::UpsFacts`].
    UpsFacts {},
    /// The UPSes `nut-scanner` finds on USB. Answers [`Response::UpsFound`].
    ScanUps {},
    /// Writes NUT's configuration for `config` and starts what it needs:
    /// the driver and the server for a UPS on this machine, the monitor in
    /// every case. Without one, NUT is switched off (`MODE=none`) and its
    /// units stopped.
    SetUps {
        config: Option<UpsConfig>,
    },
    /// The web interface's TLS settings zendiloOS wrote. Answers
    /// [`Response::WebTlsState`].
    WebTlsState {},
    /// Sets the oldest TLS version the web interface accepts and reloads
    /// nginx; connections already open stay.
    SetWebTls {
        minimum: TlsMinimum,
    },
    /// What the machine offers virtual machines: KVM, the CPU's
    /// virtualisation, the IOMMU, whether libvirt and QEMU are installed,
    /// cores, memory, and where the VMs' datasets are (the bridges and
    /// cards VMs may go on zendiloosd reads itself, 4.13). Answers
    /// [`Response::VmHost`].
    VmHost {},
    /// Puts a location of the VMs' storage on a pool zendiloos created:
    /// the dataset `<pool>/zendiloos.vms`, mounted at `/volume<N>/@vms`;
    /// libvirt starts only with every location mounted, and is enabled.
    /// A pool may hold one location; there may be several pools.
    EnableVms {
        pool: PoolName,
        volume: VolumeNumber,
    },
    /// Removes a location of the VMs' storage: refused while its dataset
    /// holds a zvol or its library a file; otherwise unmounted, its unit
    /// removed, libvirt no longer waiting for it, and the dataset
    /// destroyed.
    DisableVms {
        pool: PoolName,
        volume: VolumeNumber,
    },
    /// What the locations' datasets and the VMs' zvols on them hold.
    /// Answers [`Response::VmStorageUsage`].
    VmStorageUsage {},
    /// Copies an ISO from a share into the library,
    /// `/volume<N>/@vms/isos/<image>.iso`, in a unit of its own (blocks
    /// cloned where the pool can), reporting how far it is; QEMU only ever
    /// sees the copy. Answers [`Response::VmImageStored`].
    CopyVmImage {
        image: VmImageId,
        volume: VolumeNumber,
        source: VmIsoSource,
    },
    /// The library's images, as files: [`Response::VmImages`].
    VmImages {
        volume: VolumeNumber,
    },
    /// Removes an image's file from the library; one that is gone already
    /// is no failure. zendiloosd checked that no VM has it.
    DeleteVmImage {
        image: VmImageId,
        volume: VolumeNumber,
    },
    /// Starts receiving an upload of `bytes` into the library as `image`:
    /// a unit of its own reads a pipe, whose writing end comes along with
    /// [`Response::VmImageUpload`], into a file without a name, and names it
    /// only once exactly `bytes` came (at most [`VM_IMAGE_UPLOAD_MAX`]).
    OpenVmImageUpload {
        image: VmImageId,
        volume: VolumeNumber,
        bytes: u64,
    },
    /// Once the pipe is closed: waits for the upload's unit, and answers
    /// [`Response::VmImageStored`], or why the image was not kept.
    FinishVmImageUpload {
        image: VmImageId,
    },
    /// Makes a new VM's disks (sparse zvols) and defines the domain
    /// `zendiloos-<id>` from XML the agent builds from `spec`.
    DefineVm {
        spec: VmSpec,
    },
    /// Changes a VM that is shut off: disks of `spec` it does not have are
    /// made on their locations, those smaller than `spec` grown (never
    /// shrunk), the domain is defined anew from `spec`, and the disks in
    /// `remove` (none of `spec`'s) are destroyed, on whichever location
    /// they are.
    UpdateVm {
        spec: VmSpec,
        remove: Vec<VmDiskSlot>,
    },
    /// Puts the image of `spec`'s drive `drive` into that drive, or ejects
    /// it if the drive is empty in `spec`: the domain defined anew from
    /// `spec` and, while the VM runs, the medium changed at once.
    ChangeVmMedia {
        spec: VmSpec,
        drive: VmDriveSlot,
    },
    /// libvirt starts the VM with the machine, or not.
    SetVmAutostart {
        id: VmId,
        autostart: bool,
    },
    ControlVm {
        id: VmId,
        action: VmAction,
    },
    /// Powers the VM off if it runs, undefines it, and destroys its disks
    /// on every location; the library's images stay.
    DeleteVm {
        id: VmId,
    },
    /// `virsh list --all`: every domain and its state. Answers
    /// [`Response::VmStates`].
    VmStates {},
    /// What the running VMs zendiloOS made use: `virsh domstats` once for
    /// all, with their CPU time, memory and network counters. Answers
    /// [`Response::VmStats`].
    VmStats {},
    /// Connects to a running VM's console, VNC on a Unix socket, and
    /// answers [`Response::VmConsole`] with the connected socket.
    OpenVmConsole {
        id: VmId,
    },
    /// The host's USB and PCI devices a VM could be given, with the PCI
    /// devices' IOMMU groups and why the host keeps those it needs, and
    /// whether the IOMMU is on. Answers [`Response::VmHostDevices`].
    VmHostDevices {},
    /// Gives a VM the USB devices `spec` names, or takes them away: its
    /// domain defined anew and, while it runs, `attach` plugged into it
    /// and `detach` pulled out at once (hot-plug).
    ChangeVmUsb {
        spec: VmSpec,
        attach: Vec<VmUsbDevice>,
        detach: Vec<VmUsbDevice>,
    },
    /// Snapshots every disk of the VM, `<zvol>@<label>` on each location,
    /// in one `zfs snapshot` per pool: while it runs, its file systems
    /// frozen first by the QEMU guest agent if it answers, and the VM
    /// paused meanwhile if its disks are on more than one pool without.
    /// Answers [`Response::VmSnapshotTaken`].
    SnapshotVm {
        id: VmId,
        label: SnapshotLabel,
        schedules: Vec<SnapshotFrequency>,
        note: Option<SnapshotNote>,
    },
    /// The snapshots of the VMs' zvols with their properties: `zfs list
    /// -t snapshot` of the locations' datasets. Answers
    /// [`Response::VmSnapshots`].
    VmSnapshots {},
    /// Destroys the snapshot `label` of every disk of the VM.
    DestroyVmSnapshot {
        id: VmId,
        label: SnapshotLabel,
    },
    /// Returns a shut off VM's disks to the snapshot `label`; each disk
    /// keeps its size. Refused if a disk has snapshots after it, unless
    /// `newer`: they are destroyed then (`zfs rollback -r`). A disk without
    /// that snapshot (added since) stays as it is.
    RollbackVmSnapshot {
        id: VmId,
        label: SnapshotLabel,
        newer: bool,
    },
    /// Backs up a VM into `<folder>/<id>/<stem>` beneath `root`: a snapshot
    /// `label` of its disks taken (as [`Request::SnapshotVm`]), each disk
    /// read from it into a zstd-compressed raw image, its UEFI variables
    /// and its TPM's state, and a manifest with `settings`, all in a
    /// transient unit that may write only there; the snapshot goes again.
    /// Answers [`Response::VmBackedUp`].
    BackUpVm {
        id: VmId,
        root: BackupRoot,
        folder: SharePath,
        stem: BackupStem,
        label: SnapshotLabel,
        settings: VmBackupSettings,
    },
    /// Restores a backup's `files` from `folder` beneath `root` into the VM
    /// `spec` describes, checking each file's SHA-256 while it is read: a
    /// new VM is defined first (its disks made), one there already
    /// (`over`, shut off) gets a snapshot `label` of its disks first, to
    /// which they return should the restore fail, its disks made or grown
    /// as `spec` says, `remove` destroyed once it is defined anew.
    RestoreVm {
        spec: VmSpec,
        root: BackupRoot,
        folder: SharePath,
        files: Vec<VmBackupFile>,
        over: bool,
        remove: Vec<VmDiskSlot>,
        label: SnapshotLabel,
    },
    /// Stops what a backup or restore of the VM left running (its unit,
    /// should its client have gone) and answers once nothing runs for it
    /// any more. Answers [`Response::VmWorkEnded`].
    EndVmWork {
        id: VmId,
    },
    /// Removes a VM's backup: the files named, `<folder>` beneath `root`,
    /// and the folder once empty.
    RemoveVmBackup {
        root: BackupRoot,
        folder: SharePath,
        files: Vec<VmBackupPart>,
    },
    /// Whether rsync, the SSH client and rclone are installed, and the
    /// NAS's public key for SSH servers once made. Answers
    /// [`Response::BackupFacts`].
    BackupFacts {},
    /// Makes the NAS's key pair for SSH servers (ed25519) in
    /// [`BACKUP_STATE`], root's alone, if it has none; in a unit of its own.
    /// Answers [`Response::BackupKey`].
    MakeBackupKey {},
    /// The host keys an SSH server offers (`ssh-keyscan`, in a unit with
    /// the network and nothing else). Answers [`Response::BackupHostKeys`].
    ScanBackupHostKeys {
        host: BackupHostName,
        port: u16,
    },
    /// What the folder `inside` of `at` holds, one level (`rsync
    /// --list-only`, `rclone lsjson`; through the crypt rooted at `at` if
    /// `secrets` have one), in a unit of its own that may only read it.
    /// Answers [`Response::BackupListing`].
    ListBackup {
        at: BackupEnd,
        #[serde(default)]
        inside: SharePath,
        #[serde(default)]
        secrets: BackupSecrets,
    },
    /// Runs a task's backup in a unit of its own that reads only the source
    /// and writes only the target, with the network only for a server or a
    /// cloud; reporting how far. rsync does it, rclone for a cloud or with
    /// a crypt. Answers [`Response::BackupRan`].
    RunBackup {
        run: BackupRun,
        #[serde(default)]
        secrets: BackupSecrets,
    },
    /// Stops a task's run (its unit); none running is no failure.
    StopBackup {
        task: BackupTaskId,
    },
    /// Removes versions (and kept changes) from a task's folder `at`.
    /// Answers [`Response::BackupPruned`].
    PruneBackup {
        task: BackupTaskId,
        at: BackupEnd,
        names: Vec<BackupPruned>,
        #[serde(default)]
        secrets: BackupSecrets,
    },
    /// Restores from a task's backup into a folder of the NAS, in a unit of
    /// its own. Answers [`Response::BackupRestored`].
    RestoreBackup {
        restore: BackupRestore,
        #[serde(default)]
        secrets: BackupSecrets,
    },
    /// Whether acme.sh is installed. Answers [`Response::AcmeState`].
    AcmeState {},
    /// Issues a certificate (again) with acme.sh, which runs in a unit of
    /// its own as the account `zendiloos-acme`, reporting what it writes
    /// line by line; then puts the certificate where nginx reads it.
    /// Answers [`Response::CertificateIssued`].
    IssueCertificate {
        order: AcmeOrder,
    },
    /// Removes what acme.sh and nginx kept of a certificate.
    RemoveCertificate {
        certificate: u32,
    },
    /// Removes what acme.sh kept of an account at the authority: its key
    /// and settings.
    RemoveAcmeAccount {
        account: u32,
    },
    /// A change of the web server that waits to be confirmed. Answers
    /// [`Response::WebServerState`].
    WebServerState {},
    /// Writes nginx's ports and certificate, checks them (`nginx -t`) and
    /// reloads nginx; unless [`Request::ConfirmWebServer`] comes within
    /// `confirm_within`, the old settings return (also after a restart).
    /// Without `confirm_within`, `config` was confirmed before and is put in
    /// force for good (nothing happens when the files say it already); the
    /// old settings return only if nginx refuses it. Refused while a change
    /// waits.
    ApplyWebServer {
        config: WebServerConfig,
        confirm_within: Option<ConfirmWithin>,
    },
    /// Keeps the change that waits to be confirmed.
    ConfirmWebServer {},
    /// nginx reads the certificate it serves again: it was renewed.
    ReloadWebCertificate {},
    /// A change of the firewall that waits to be confirmed. Answers
    /// [`Response::FirewallState`].
    FirewallState {},
    /// Writes the firewall's rules (none: no rules at all) for nftables,
    /// checks them (`nft -c`) and puts them in force. Answers
    /// [`Response::FirewallState`], as it is afterwards.
    ///
    /// With `confirm_within`, a change begins: unless
    /// [`Request::ConfirmFirewall`] comes in time the rules from before
    /// return (also after a restart), one change at a time. Without, `config`
    /// is the rules kept. While the change the caller began waits, `waiting`
    /// has its rules, brought up to date: they stay in force, and `config`
    /// is what returns unless it is confirmed; if it went back meanwhile,
    /// `config` is put in force. A change waiting that the caller does not
    /// name (it restarted since) can never be confirmed: it ends, and
    /// `config` is in force.
    ApplyFirewall {
        config: Option<FirewallConfig>,
        confirm_within: Option<ConfirmWithin>,
        #[serde(default)]
        waiting: Option<WaitingFirewall>,
    },
    /// Keeps the change that waits to be confirmed.
    ConfirmFirewall {},
    /// The files of the configuration backup as they are (see
    /// [`ConfigurationPath`]), those missing left out, and the people's
    /// users and groups with their IDs. Answers
    /// [`Response::Configuration`].
    ReadConfiguration {},
    /// Writes the files of a configuration backup back, each at its place
    /// (only those [`ConfigurationPath::restorable`]), with what follows:
    /// nginx checks and takes the certificates (the ones before return if
    /// it refuses), Samba stops while its passwords are written.
    RestoreConfiguration {
        files: Vec<ConfigurationFile>,
    },
    /// Makes the accounts' Linux users and groups with the IDs they had,
    /// where they are missing; refused as a whole if a name or ID is taken
    /// otherwise here.
    RestoreAccounts {
        users: Vec<RestoredUser>,
        groups: Vec<RestoredGroup>,
    },
    /// The machine's name as `/etc/hostname` holds it. Answers
    /// [`Response::HostNameState`].
    HostNameState {},
    /// Renames the machine: systemd-hostnamed sets the name (static and
    /// the kernel's) and `/etc/hosts` names it where it named the old one;
    /// the machine's own, self-signed certificate is made anew for it
    /// (nginx reloaded if it serves that one; an issued one is never
    /// touched); Avahi, Samba and WS-Discovery restart if they run, as
    /// they read the name when they start. Answers
    /// [`Response::HostNameSet`].
    SetHostName {
        name: HostName,
    },
    /// The clock's settings and state, and the time zones there are.
    /// Answers [`Response::TimeState`].
    TimeState {},
    /// Sets the time zone, the time servers and whether they keep the
    /// clock. Without them, `clock` sets the time; with them it is refused.
    SetTime {
        zone: TimeZone,
        network_time: bool,
        servers: TimeServers,
        #[serde(default)]
        clock: Option<ClockTime>,
    },
    /// Returns a share to one of its snapshots (`zfs rollback -r`): what
    /// changed since is gone, and so are the snapshots taken after it.
    RollbackSnapshot {
        pool: PoolName,
        volume: VolumeNumber,
        share: ShareName,
        label: SnapshotLabel,
    },
    /// All snapshots with their properties (`zfs list -t snapshot`).
    ListSnapshots {},
    /// Quota, compression and record size of share dataset
    /// `<pool>/<share>` on a pool zendiloos created.
    SetShareProperties {
        pool: PoolName,
        volume: VolumeNumber,
        share: ShareName,
        /// `refquota`: the share's own data, snapshots not counted; `None`
        /// for no quota.
        quota_bytes: Option<u64>,
        compression: ShareCompression,
        record_size: ShareRecordSize,
    },
    /// What the share datasets hold and may take (`zfs list -d 1`).
    ShareUsage {},
    /// What the console login prompt shows: where the web interface is and,
    /// while the initial setup is pending, the setup code, or that the setup
    /// is open (no code: a machine installed from the ISO). The agent writes
    /// the text itself; only the code comes from zendiloosd.
    ShowOnConsole {
        #[serde(default)]
        setup_code: Option<ConsoleSetupCode>,
        /// Sent only when set, so an older agent still understands the rest.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        open_setup: bool,
    },
    /// Creates dataset `<pool>/<name>` of a pool zendiloos created and
    /// mounts it at `/volume<volume>/<name>` through a systemd mount unit.
    CreateShare {
        pool: PoolName,
        volume: VolumeNumber,
        name: ShareName,
    },
    /// Unmounts and destroys a share's dataset with all its files.
    DestroyShare {
        pool: PoolName,
        volume: VolumeNumber,
        name: ShareName,
    },
    /// Writes the Samba shares and NFS exports zendiloos manages, checks
    /// them and makes the servers load them; starts and enables the
    /// servers that are on, stops and disables those that are off.
    /// Replaces what was there.
    ApplySharing {
        smb: Vec<SmbShare>,
        nfs: Vec<NfsExport>,
        /// Which servers run, and Samba's settings for all shares.
        services: FileServices,
        /// The share `home`, each user's own folder; absent while personal
        /// folders are off.
        #[serde(default)]
        homes: Option<HomesShare>,
        /// Shares whose open SMB connections end once the servers run with
        /// the new configuration (`smbcontrol smbd close-share`): someone
        /// lost access there, and clients connect again under the new
        /// rules.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        close: Vec<ShareName>,
    },
    /// Makes the dataset of the personal folders, `<pool>/zendiloos.homes`,
    /// mounted at `/volume<N>/@homes`. What exists stays.
    PrepareHomes {
        pool: PoolName,
        volume: VolumeNumber,
    },
    /// How much the user may store in the personal folders' dataset
    /// (`userquota`); `None` for no limit.
    SetHomeQuota {
        pool: PoolName,
        volume: VolumeNumber,
        user: UserName,
        quota_bytes: Option<u64>,
    },
    /// Makes the user's personal folder if missing, owner the user, mode
    /// 0700, in `/volume<N>/@homes` (role `files`: only that agent writes
    /// to the volumes).
    MakeHome {
        volume: VolumeNumber,
        user: UserName,
    },
    /// Gives a share's folder what every share has: owner `root`, group
    /// `zendiloos-files`, mode 2770 and the one ACL that lets that group
    /// read and write, inherited by what is made in it; and the recycle
    /// bins' folder `.recycle` (role `files`). Access is the share's as a
    /// whole, enforced by Samba and zendiloosd, never by entries per person.
    PrepareShare {
        volume: VolumeNumber,
        name: ShareName,
    },
    /// The update to share-level access, once per share: [`Request::PrepareShare`],
    /// then every folder and file in the share gets the entry of
    /// `zendiloos-files` (folders the default entry too) and loses the
    /// named entries of the accounts and groups zendiloOS manages; apps'
    /// and other IDs' entries and the owners stay (role `files`). Reports
    /// how many entries it went through as lines.
    MigrateShare {
        volume: VolumeNumber,
        name: ShareName,
    },
    /// Creates a Linux user in group `zendiloos-users`: UID from 2000, no
    /// login shell, no home directory, locked Linux password. `conflict` if
    /// the name is taken.
    CreateUser {
        name: UserName,
    },
    /// Deletes a managed user and its Samba entry. Its files stay.
    DeleteUser {
        name: UserName,
    },
    /// Sets the Samba password of a managed user, adding the Samba entry if
    /// needed. The password travels on the program's stdin.
    SetSmbPassword {
        name: UserName,
        password: Password,
    },
    /// Enables or disables the Samba entry of a managed user.
    SetUserEnabled {
        name: UserName,
        enabled: bool,
    },
    /// Creates a Linux group with a GID from 2000. `conflict` if the name is
    /// taken.
    CreateGroup {
        name: GroupName,
    },
    DeleteGroup {
        name: GroupName,
    },
    /// Replaces the members of a managed group; all must be managed users.
    SetGroupMembers {
        group: GroupName,
        members: Vec<UserName>,
    },
    // The files agent: each of these runs with the credentials of `user`,
    // a managed user, so the file system decides what is allowed, as it
    // does for SMB and NFS.
    /// Answers [`Response::Directory`].
    ListDirectory {
        user: UserName,
        location: FileLocation,
    },
    /// Looks through the folder `location` and everything below it for
    /// names containing `text`, whatever the case; names starting with `.`
    /// only with `hidden`. Answers [`Response::Found`]: at most
    /// [`MAX_SEARCH_RESULTS`], within a few seconds, ending early once its
    /// client hangs up.
    SearchFiles {
        user: UserName,
        location: FileLocation,
        text: SearchText,
        #[serde(default)]
        hidden: bool,
    },
    /// What `location` is: kind, times and owner, and for a folder its
    /// files, folders and hidden entries counted as the user may see them.
    /// Answers [`Response::Facts`]: at most [`MAX_COUNTED`] entries within
    /// half a minute, ending early once its client hangs up.
    EntryFacts {
        user: UserName,
        location: FileLocation,
    },
    /// Opens a regular file for reading; [`Response::File`] carries it.
    OpenFile {
        user: UserName,
        location: FileLocation,
    },
    /// Creates an empty hidden file in the directory `location` for an
    /// upload; [`Response::Upload`] carries its name and it, opened for
    /// writing. Renamed with [`Request::MoveEntry`] when complete.
    CreateUpload {
        user: UserName,
        location: FileLocation,
    },
    MakeDirectory {
        user: UserName,
        location: FileLocation,
        /// On a share, the folder is `user`'s, not its parent folder's
        /// owner's: a folder an administrator's project binds, which its
        /// containers, running as that administrator, may need to own (a
        /// database refuses a data folder of another owner).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        own: bool,
    },
    /// Renames or moves; between shares by copying, writing the copy out and
    /// removing what was copied, unchanged since. `conflict` if `to`
    /// exists, unless `replace` and both are files; between shares a move
    /// goes on into a folder `to` (a move tried again, into its earlier
    /// copy) and takes a file `to` of the same contents for the source's.
    /// A move that left part of its source answers
    /// [`Response::PartlyMoved`]. Within one root it is a rename, and quick
    /// ([`Request::is_long`]).
    MoveEntry {
        user: UserName,
        from: FileLocation,
        to: FileLocation,
        replace: bool,
    },
    /// Copies a file or directory tree. If `to` exists, a free name next to
    /// it is chosen: `name (2).ext`. Answers [`Response::Copied`].
    CopyEntry {
        user: UserName,
        from: FileLocation,
        to: FileLocation,
    },
    /// Moves a file or directory tree into the user's recycle bin,
    /// `.recycle/<user>` in the share, or deletes it for good if not
    /// `recycle` or if it is in a recycle bin already.
    RemoveEntry {
        user: UserName,
        location: FileLocation,
        recycle: bool,
    },
    /// Removes the hidden file of an upload that was given up
    /// (`.zendiloos-upload-…`): only a file of that name, never a folder,
    /// which whoever may write there could have put in its place.
    DiscardUpload {
        user: UserName,
        location: FileLocation,
    },
    /// Moves an entry of the user's recycle bin back to where it was
    /// deleted from, or beside it if the name is taken. Answers
    /// [`Response::Restored`].
    RestoreEntry {
        user: UserName,
        location: FileLocation,
    },
    /// Plans a tar archive of the folder `location` as the user, then
    /// writes it, as the user, into a pipe whose reading end comes along
    /// with [`Response::Archive`]. Refused if anything inside cannot be
    /// read.
    ArchiveFolder {
        user: UserName,
        location: FileLocation,
    },
}

/// The `lsblk` columns of [`Request::ListBlockDevices`]; the parser in
/// zendiloosd relies on them.
pub const LSBLK_COLUMNS: &str = "NAME,KNAME,PATH,TYPE,SIZE,MODEL,SERIAL,WWN,TRAN,ROTA,RM,RO,\
FSTYPE,LABEL,UUID,FSSIZE,FSUSED,FSAVAIL,MOUNTPOINTS,PKNAME,ID-LINK";

impl Request {
    /// Name for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Request::Hello {} => "hello",
            Request::Reboot {} => "reboot",
            Request::PowerOff {} => "power_off",
            Request::ListBlockDevices {} => "list_block_devices",
            Request::Machine {} => "machine",
            Request::ReadBurst { .. } => "read_burst",
            Request::SmartData { .. } => "smart_data",
            Request::EmmcHealth { .. } => "emmc_health",
            Request::StartSelfTest { .. } => "start_self_test",
            Request::AbortSelfTest { .. } => "abort_self_test",
            Request::ArrayStatus {} => "array_status",
            Request::ZfsStatus {} => "zfs_status",
            Request::CreatePool { .. } => "create_pool",
            Request::DestroyPool { .. } => "destroy_pool",
            Request::ReplaceDisk { .. } => "replace_disk",
            Request::ImportablePools {} => "importable_pools",
            Request::ImportPool { .. } => "import_pool",
            Request::MountVolume { .. } => "mount_volume",
            Request::ScrubPool { .. } => "scrub_pool",
            Request::StopScrub { .. } => "stop_scrub",
            Request::MountRemovable { .. } => "mount_removable",
            Request::EjectRemovable { .. } => "eject_removable",
            Request::CleanRemovable {} => "clean_removable",
            Request::CreateSnapshot { .. } => "create_snapshot",
            Request::DestroySnapshot { .. } => "destroy_snapshot",
            Request::RollbackSnapshot { .. } => "rollback_snapshot",
            Request::DockerState {} => "docker_state",
            Request::VmHost {} => "vm_host",
            Request::EnableVms { .. } => "enable_vms",
            Request::DisableVms { .. } => "disable_vms",
            Request::VmStorageUsage {} => "vm_storage_usage",
            Request::CopyVmImage { .. } => "copy_vm_image",
            Request::VmImages { .. } => "vm_images",
            Request::DeleteVmImage { .. } => "delete_vm_image",
            Request::OpenVmImageUpload { .. } => "open_vm_image_upload",
            Request::FinishVmImageUpload { .. } => "finish_vm_image_upload",
            Request::DefineVm { .. } => "define_vm",
            Request::UpdateVm { .. } => "update_vm",
            Request::ChangeVmMedia { .. } => "change_vm_media",
            Request::SetVmAutostart { .. } => "set_vm_autostart",
            Request::ControlVm { .. } => "control_vm",
            Request::DeleteVm { .. } => "delete_vm",
            Request::VmStates {} => "vm_states",
            Request::VmStats {} => "vm_stats",
            Request::OpenVmConsole { .. } => "open_vm_console",
            Request::VmHostDevices {} => "vm_host_devices",
            Request::ChangeVmUsb { .. } => "change_vm_usb",
            Request::SnapshotVm { .. } => "snapshot_vm",
            Request::VmSnapshots {} => "vm_snapshots",
            Request::DestroyVmSnapshot { .. } => "destroy_vm_snapshot",
            Request::RollbackVmSnapshot { .. } => "rollback_vm_snapshot",
            Request::BackUpVm { .. } => "back_up_vm",
            Request::RestoreVm { .. } => "restore_vm",
            Request::EndVmWork { .. } => "end_vm_work",
            Request::RemoveVmBackup { .. } => "remove_vm_backup",
            Request::BackupFacts {} => "backup_facts",
            Request::MakeBackupKey {} => "make_backup_key",
            Request::ScanBackupHostKeys { .. } => "scan_backup_host_keys",
            Request::ListBackup { .. } => "list_backup",
            Request::RunBackup { .. } => "run_backup",
            Request::StopBackup { .. } => "stop_backup",
            Request::PruneBackup { .. } => "prune_backup",
            Request::RestoreBackup { .. } => "restore_backup",
            Request::EnableDocker { .. } => "enable_docker",
            Request::MoveDocker { .. } => "move_docker",
            Request::StartApp { .. } => "start_app",
            Request::StopApp { .. } => "stop_app",
            Request::RemoveApp { .. } => "remove_app",
            Request::DeleteAppAccount { .. } => "delete_app_account",
            Request::AppsStatus {} => "apps_status",
            Request::AppLogs { .. } => "app_logs",
            Request::HostFacts { .. } => "host_facts",
            Request::DockerContainers {} => "docker_containers",
            Request::DockerStats {} => "docker_stats",
            Request::DockerImages {} => "docker_images",
            Request::DockerNetworks {} => "docker_networks",
            Request::DockerDiskUsage {} => "docker_disk_usage",
            Request::ControlContainer { .. } => "control_container",
            Request::ContainerLogs { .. } => "container_logs",
            Request::InspectContainer { .. } => "inspect_container",
            Request::ContainerProcesses { .. } => "container_processes",
            Request::OpenTerminal { .. } => "open_terminal",
            Request::RemoveImage { .. } => "remove_image",
            Request::PullImage { .. } => "pull_image",
            Request::PruneImages {} => "prune_images",
            Request::CreateNetwork { .. } => "create_network",
            Request::RemoveNetwork { .. } => "remove_network",
            Request::DockerVolumes {} => "docker_volumes",
            Request::DockerVolumeSizes {} => "docker_volume_sizes",
            Request::RemoveVolume { .. } => "remove_volume",
            Request::PruneVolumes {} => "prune_volumes",
            Request::CreateVolume { .. } => "create_volume",
            Request::BackUpVolume { .. } => "back_up_volume",
            Request::RestoreVolume { .. } => "restore_volume",
            Request::RemoveVolumeBackup { .. } => "remove_volume_backup",
            Request::EndVolumeWork { .. } => "end_volume_work",
            Request::ScanVolumeBackups { .. } => "scan_volume_backups",
            Request::CheckCompose { .. } => "check_compose",
            Request::ControlProject { .. } => "control_project",
            Request::DeployProject { .. } => "deploy_project",
            Request::ProjectLogs { .. } => "project_logs",
            Request::ProjectFiles { .. } => "project_files",
            Request::NetworkState {} => "network_state",
            Request::ChangeNetwork { .. } => "change_network",
            Request::TakeOverNetwork { .. } => "take_over_network",
            Request::ConfirmNetwork {} => "confirm_network",
            Request::SshState {} => "ssh_state",
            Request::StartPackages { .. } => "start_packages",
            Request::PackagesState {} => "packages_state",
            Request::SetSsh { .. } => "set_ssh",
            Request::UpsFacts {} => "ups_facts",
            Request::ScanUps {} => "scan_ups",
            Request::SetUps { .. } => "set_ups",
            Request::WebTlsState {} => "web_tls_state",
            Request::SetWebTls { .. } => "set_web_tls",
            Request::AcmeState {} => "acme_state",
            Request::IssueCertificate { .. } => "issue_certificate",
            Request::RemoveCertificate { .. } => "remove_certificate",
            Request::RemoveAcmeAccount { .. } => "remove_acme_account",
            Request::WebServerState {} => "web_server_state",
            Request::ApplyWebServer { .. } => "apply_web_server",
            Request::ConfirmWebServer {} => "confirm_web_server",
            Request::ReloadWebCertificate {} => "reload_web_certificate",
            Request::FirewallState {} => "firewall_state",
            Request::ApplyFirewall { .. } => "apply_firewall",
            Request::ConfirmFirewall {} => "confirm_firewall",
            Request::ReadConfiguration {} => "read_configuration",
            Request::RestoreConfiguration { .. } => "restore_configuration",
            Request::RestoreAccounts { .. } => "restore_accounts",
            Request::HostNameState {} => "host_name_state",
            Request::SetHostName { .. } => "set_host_name",
            Request::TimeState {} => "time_state",
            Request::SetTime { .. } => "set_time",
            Request::ListSnapshots {} => "list_snapshots",
            Request::ShowOnConsole { .. } => "show_on_console",
            Request::SetShareProperties { .. } => "set_share_properties",
            Request::ShareUsage {} => "share_usage",
            Request::CreateShare { .. } => "create_share",
            Request::DestroyShare { .. } => "destroy_share",
            Request::ApplySharing { .. } => "apply_sharing",
            Request::PrepareHomes { .. } => "prepare_homes",
            Request::SetHomeQuota { .. } => "set_home_quota",
            Request::MakeHome { .. } => "make_home",
            Request::PrepareShare { .. } => "prepare_share",
            Request::MigrateShare { .. } => "migrate_share",
            Request::CreateUser { .. } => "create_user",
            Request::DeleteUser { .. } => "delete_user",
            Request::SetSmbPassword { .. } => "set_smb_password",
            Request::SetUserEnabled { .. } => "set_user_enabled",
            Request::CreateGroup { .. } => "create_group",
            Request::DeleteGroup { .. } => "delete_group",
            Request::SetGroupMembers { .. } => "set_group_members",
            Request::ListDirectory { .. } => "list_directory",
            Request::SearchFiles { .. } => "search_files",
            Request::EntryFacts { .. } => "entry_facts",
            Request::OpenFile { .. } => "open_file",
            Request::CreateUpload { .. } => "create_upload",
            Request::MakeDirectory { .. } => "make_directory",
            Request::MoveEntry { .. } => "move_entry",
            Request::CopyEntry { .. } => "copy_entry",
            Request::RemoveEntry { .. } => "remove_entry",
            Request::DiscardUpload { .. } => "discard_upload",
            Request::RestoreEntry { .. } => "restore_entry",
            Request::ArchiveFolder { .. } => "archive_folder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Hello {
        protocol_version: u32,
        agent_version: String,
    },
    /// The operation was carried out; it has no output.
    Done,
    /// Before the answer of a long operation: the step it is at now (a
    /// word for the interface to name, such as `creating_pool`), and how
    /// far in percent where that can be told. Any number may come; the
    /// answer follows them.
    Progress {
        step: String,
        percent: Option<u8>,
    },
    /// Before the answer of a long operation: a line a program it runs
    /// wrote, such as Docker Compose's "Container x Recreated", for a
    /// person to follow. Any number may come; the answer follows them.
    Output {
        line: String,
    },
    BlockDevices {
        lsblk_json: String,
        /// `udevadm info --export-db` of the block devices that are disks;
        /// empty if udev could not be asked.
        #[serde(default)]
        udev_db: String,
    },
    /// DMI's names of the machine, as the firmware has them (a trailing
    /// newline and all); empty where there are none.
    Machine {
        sys_vendor: String,
        product_name: String,
    },
    Smart {
        /// smartctl's exit status: a bit mask, not a plain success flag.
        exit_status: i32,
        smartctl_json: String,
    },
    /// The files of an eMMC device's card below
    /// `/sys/class/block/<device>/device`, as the kernel writes them (a
    /// trailing newline and all); empty where there is none.
    Emmc {
        /// `type`: `MMC` for eMMC, `SD` for a memory card.
        kind: String,
        /// `life_time`: the two estimates of the life used, `0x01 0x02`.
        life_time: String,
        /// `pre_eol_info`: how far the reserved blocks are used, `0x01`.
        pre_eol_info: String,
        name: String,
        serial: String,
        fwrev: String,
    },
    ArrayStatus {
        /// Empty if the kernel has no software RAID support loaded.
        mdstat: String,
    },
    Zfs {
        zpool_status: String,
        zfs_list: String,
    },
    /// What `zpool import` lists, as it wrote it; empty when it finds
    /// none.
    ImportablePools {
        zpool_import: String,
    },
    /// The volume's mounts are in place; `missing`: shares named whose
    /// datasets the pool does not have; `changed`: a unit was written or
    /// enabled anew.
    VolumeMounted {
        #[serde(default)]
        missing: Vec<ShareName>,
        #[serde(default)]
        changed: bool,
    },
    Directory {
        entries: Vec<DirectoryEntry>,
        /// More than [`MAX_DIRECTORY_ENTRIES`]; the rest is left out.
        truncated: bool,
        /// The user may create and delete entries in it.
        writable: bool,
    },
    /// What [`Request::SearchFiles`] found, in the order it came across it.
    Found {
        entries: Vec<FoundEntry>,
        /// Why it did not look everywhere, if it did not.
        stopped: Option<SearchStop>,
    },
    /// What [`Request::EntryFacts`] found out.
    Facts {
        facts: EntryFacts,
    },
    /// A regular file, opened for reading; its descriptor comes along.
    File {
        size: u64,
    },
    /// A new upload file; its descriptor, opened for writing, comes along.
    Upload {
        name: String,
    },
    Copied {
        path: SharePath,
    },
    /// A move across file systems ([`Request::MoveEntry`]) removed only part
    /// of its source: `kept` entries stayed where they were, such as
    /// `example`, because they changed or came since they were copied, were
    /// never copied (pipes, uploads under way), or could not be removed;
    /// what of them was copied is in the target too. With `too_many`, more
    /// entries than a move checks one by one: the copy is complete, and the
    /// source stayed whole.
    PartlyMoved {
        kept: u64,
        example: Option<SharePath>,
        too_many: bool,
    },
    DockerState {
        /// The `docker` program is there.
        installed: bool,
        running: bool,
        /// Its `data-root`, if `/etc/docker/daemon.json` sets one.
        data_root: Option<String>,
        /// The engine's version, while it runs.
        #[serde(default)]
        version: Option<String>,
    },
    PackagesStarted {
        id: String,
    },
    Packages {
        run: Option<PackageRun>,
        /// The last run of an update ([`PackageTask::updates`]), kept apart:
        /// the runs after it (a check, a plugin installed) do not replace
        /// it. The same as `run` until another came.
        #[serde(default)]
        update: Option<Box<PackageRun>>,
        /// The kernel that runs now (`uname -r`). A run's report names the
        /// one that ran when the run read it, which a restart since may
        /// have changed. None from an agent before it said.
        #[serde(default)]
        running_kernel: Option<String>,
    },
    Apps {
        /// `docker compose ls --all --format json`: every Compose project,
        /// the apps' among them.
        compose_ls: String,
    },
    AppLogs {
        /// With timestamps, oldest first; cut at the response's limit.
        text: String,
    },
    HostFacts {
        uid: u32,
        gid: u32,
        /// From where `/etc/localtime` leads: `Europe/Berlin`; `UTC` if it
        /// leads nowhere in the zone database.
        time_zone: String,
        /// Of the devices asked about, those the machine has.
        devices: Vec<DevicePath>,
    },
    NetworkState {
        /// systemd-networkd manages the network: it can be changed here.
        managed: bool,
        pending: Option<PendingNetwork>,
        /// Not managed: NetworkManager runs.
        #[serde(default)]
        network_manager: bool,
        /// Not managed: ifupdown's files, where `networking.service` is
        /// enabled and `/etc/network/interfaces` is there.
        #[serde(default)]
        ifupdown: Option<Ifupdown>,
        /// zendiloOS's systemd-networkd files as they are (with a change
        /// waiting, its own), by name.
        #[serde(default)]
        files: Vec<NetworkdFile>,
    },
    /// What `docker` printed, for zendiloosd to read.
    DockerOutput {
        text: String,
    },
    /// A terminal in a container was opened; its master side comes along
    /// ([`Request::OpenTerminal`]).
    Terminal {},
    WebTlsState {
        /// The settings file zendiloOS wrote; empty if none.
        config: String,
    },
    VmHost {
        /// Debian's name of the machine's architecture ([`ARCHITECTURE`]).
        architecture: String,
        /// `/dev/kvm` is there: the kernel's KVM module is loaded.
        kvm: bool,
        /// The CPU's virtualisation as `/proc/cpuinfo` names it: `vmx`
        /// (Intel VT-x) or `svm` (AMD-V); none if it has neither.
        cpu_virtualization: Option<String>,
        /// An IOMMU is on (`/sys/kernel/iommu_groups` lists groups).
        iommu: bool,
        /// libvirt's daemon, `virsh` and QEMU are installed.
        installed: bool,
        /// swtpm is installed: VMs can have a TPM.
        #[serde(default)]
        swtpm: bool,
        /// OVMF with Microsoft's keys enrolled is there, as a firmware
        /// QEMU describes: VMs can boot with Secure Boot.
        #[serde(default)]
        secure_boot: bool,
        /// The machine's CPUs (threads) and memory.
        cores: u32,
        memory_bytes: u64,
        /// The locations of the VMs' storage: each dataset whose mount
        /// unit is there, by volume.
        locations: Vec<VmStorage>,
    },
    VmStorageUsage {
        /// `zfs list -H -p` of the locations' datasets and their zvols:
        /// name, type, used, usedbydataset, referenced, available, volsize.
        zfs_list: String,
    },
    VmStates {
        /// What `virsh list --all` printed.
        virsh_list: String,
    },
    VmStats {
        /// What `virsh domstats` printed, only of domains zendiloOS made.
        domstats: String,
    },
    /// The library's images, by their files ([`Request::VmImages`]).
    VmImages {
        files: Vec<VmImageFile>,
    },
    /// An image is in the library, of `bytes`.
    VmImageStored {
        bytes: u64,
    },
    /// An upload's unit waits for the image; the writing end of its pipe
    /// comes along ([`Request::OpenVmImageUpload`]).
    VmImageUpload {},
    /// A VM's console was opened; the connected socket comes along
    /// ([`Request::OpenVmConsole`]).
    VmConsole {},
    VmHostDevices {
        iommu: HostIommu,
        usb: Vec<HostUsbDevice>,
        pci: Vec<HostPciDevice>,
    },
    VmSnapshotTaken {
        consistency: VmConsistency,
    },
    VmSnapshots {
        /// `zfs list -H -p -t snapshot -o name,creation,used,referenced,`
        /// `zendiloos:schedule,zendiloos:note,zendiloos:consistency` of the
        /// locations' datasets.
        zfs_list: String,
    },
    /// What a VM's backup wrote into `folder`, its manifest last.
    VmBackedUp {
        folder: SharePath,
        files: Vec<VmBackupFile>,
        manifest: WrittenFile,
        consistency: VmConsistency,
    },
    /// Whether anything of a VM's backup or restore was still running and
    /// was stopped.
    VmWorkEnded {
        stopped: bool,
    },
    BackupFacts {
        rsync: bool,
        /// The SSH client and `ssh-keygen`.
        ssh: bool,
        #[serde(default)]
        rclone: bool,
        /// `id_ed25519.pub`, once made.
        public_key: Option<String>,
    },
    BackupKey {
        public_key: String,
    },
    BackupHostKeys {
        /// What `ssh-keyscan` printed.
        keyscan: String,
    },
    BackupListing {
        /// What `rsync --list-only` printed (rclone's listing written as
        /// rsync's).
        listing: String,
        /// The folder is not there.
        missing: bool,
        /// A cloud's token rclone refreshed meanwhile: zendiloosd keeps it.
        #[serde(default)]
        token: Option<BackupSecret>,
    },
    BackupRan {
        files: u64,
        bytes: u64,
        /// What rsync could not read or found gone (its exit 23 or 24): the
        /// backup is complete but for it.
        partial: Option<String>,
        /// rsync's itemized changes, the first of them.
        transfers: Vec<String>,
        transfers_total: u64,
        /// The task's folder after the run, as `rsync --list-only` prints it.
        listing: String,
        #[serde(default)]
        token: Option<BackupSecret>,
    },
    BackupRestored {
        files: u64,
        bytes: u64,
        partial: Option<String>,
        #[serde(default)]
        token: Option<BackupSecret>,
    },
    BackupPruned {
        #[serde(default)]
        token: Option<BackupSecret>,
    },
    AcmeState {
        /// acme.sh is installed.
        installed: bool,
    },
    CertificateIssued {
        /// Until when it holds, in Unix seconds.
        not_after: i64,
    },
    WebServerState {
        pending: Option<PendingWebServer>,
    },
    FirewallState {
        pending: Option<PendingFirewall>,
    },
    UpsFacts {
        /// NUT's server and monitor are installed.
        installed: bool,
        /// Its SNMP driver too (`nut-snmp`).
        snmp: bool,
    },
    UpsFound {
        devices: Vec<FoundUps>,
    },
    SshState {
        /// The server is installed.
        installed: bool,
        /// `systemctl is-enabled` and `is-active` of `ssh.service`.
        enabled: String,
        active: String,
        /// The settings file zendiloOS wrote; empty if none.
        config: String,
        /// When the timer turns it off, Unix seconds.
        off_at: Option<i64>,
    },
    Configuration {
        files: Vec<ConfigurationFile>,
        /// The people's users and groups zendiloOS made, as the account
        /// files have them: their IDs are those their files carry.
        users: Vec<RestoredUser>,
        groups: Vec<RestoredGroup>,
    },
    HostNameState {
        /// `/etc/hostname` as it is; empty if there is none.
        hostname: String,
    },
    HostNameSet {
        /// The machine's own certificate names the new name.
        certificate_renewed: bool,
        /// nginx serves the machine's own certificate (not an issued one):
        /// browsers are shown the new one.
        own_certificate_served: bool,
    },
    TimeState {
        /// `timedatectl show`: zone, whether network time is on and
        /// whether the clock is synchronized.
        timedatectl: String,
        /// `timedatectl show-timesync`: the server in use; empty while
        /// timesyncd is not running.
        timesync: String,
        /// `timedatectl list-timezones`, one per line.
        zones: String,
        /// The time servers zendiloos set, in timesyncd's drop-in; empty if
        /// none.
        servers_conf: String,
    },
    /// Where a restored entry went.
    Restored {
        path: SharePath,
    },
    /// A tar archive being written; the pipe's reading end comes along. It
    /// is exactly `size` bytes long, unless writing fails midway.
    Archive {
        size: u64,
    },
    VolumeSizes {
        sizes: Vec<VolumeSize>,
    },
    /// A volume was backed up: its archive and manifest, the archive's
    /// SHA-256, what its data took up, its entries and the manifest as
    /// written.
    VolumeBackedUp {
        archive: WrittenFile,
        manifest: WrittenFile,
        sha256: ArchiveDigest,
        data_bytes: u64,
        entries: u64,
        manifest_json: String,
    },
    /// Whether the backup's file was removed: `false` if it was gone or is
    /// another file now, which stays.
    BackupRemoved {
        removed: bool,
    },
    /// Nothing runs any more for the volume; `stopped`: something still ran
    /// and was stopped.
    VolumeWorkEnded {
        stopped: bool,
    },
    /// A volume was restored; `left_over`: its old contents could not all be
    /// removed and lie in this folder beside its data, until the next
    /// restore of it or the agent's next start removes them.
    VolumeRestored {
        left_over: Option<String>,
    },
    /// What a scan for backups found, and the files it refused with why.
    VolumeBackupsScanned {
        found: Vec<FoundBackup>,
        refused: Vec<RefusedBackupFile>,
    },
    Snapshots {
        /// `zfs list -H -p -t snapshot -o name,creation,used,referenced,`
        /// `zendiloos:schedule,zendiloos:note`.
        zfs_list: String,
    },
    ShareUsage {
        /// `zfs list -H -p -t filesystem -d 1 -o name,used,available`.
        zfs_list: String,
    },
    /// A removable drive is mounted; `read_only` if it could not be
    /// written (write-protected, or NTFS that Windows left hibernated).
    RemovableMounted {
        read_only: bool,
    },
    /// The removable drives let go of: places of drives that were gone.
    RemovableCleaned {
        ids: Vec<RemovableId>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request could not be read or failed validation.
    InvalidRequest,
    VersionMismatch,
    /// The caller is not the account the agent serves, or the request
    /// concerns a user or group zendiloos does not manage.
    Forbidden,
    /// The name is already taken.
    Conflict,
    /// The file or directory does not exist.
    NotFound,
    /// The file system refused the user the request names.
    Denied,
    /// The operation ran but failed.
    Failed,
}

/// Kernel name of a whole disk: `sda`, `vdb`, `nvme0n1`, `mmcblk0`, …
///
/// Partitions and anything else (paths, `..`, option-like strings) are
/// refused while deserializing. This is the only form in which a device
/// reaches a command line in the agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DiskName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the kernel name of a whole disk")]
pub struct InvalidDiskName;

impl DiskName {
    pub fn parse(name: &str) -> Result<Self, InvalidDiskName> {
        let letters = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_lowercase());
        let digits = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
        let valid = if let Some(rest) = name.strip_prefix("nvme") {
            // nvme<controller>n<namespace>, but not its partitions (…p1).
            rest.split_once('n').is_some_and(|(controller, ns)| digits(controller) && digits(ns))
        } else if let Some(rest) = name.strip_prefix("mmcblk") {
            digits(rest)
        } else {
            ["sd", "vd", "xvd", "hd"].iter().any(|prefix| name.strip_prefix(prefix).is_some_and(letters))
        };
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidDiskName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DiskName {
    type Error = InvalidDiskName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DiskName> for String {
    fn from(name: DiskName) -> Self {
        name.0
    }
}

/// How long one [`Request::ReadBurst`] reads: 1 to 2000 milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct BurstMillis(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a burst reads for 1 to {} milliseconds", BurstMillis::MAX)]
pub struct InvalidBurstMillis;

impl BurstMillis {
    pub const MAX: u16 = 2000;

    pub fn new(millis: u16) -> Result<Self, InvalidBurstMillis> {
        if (1..=Self::MAX).contains(&millis) { Ok(Self(millis)) } else { Err(InvalidBurstMillis) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for BurstMillis {
    type Error = InvalidBurstMillis;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BurstMillis> for u16 {
    fn from(millis: BurstMillis) -> Self {
        millis.0
    }
}

/// How a new pool spreads data over its disks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Single,
    Mirror,
    Raidz1,
    Raidz2,
    Raidz3,
}

impl Layout {
    /// The `zpool create` vdev keyword, if any.
    pub fn keyword(self) -> Option<&'static str> {
        match self {
            Layout::Single => None,
            Layout::Mirror => Some("mirror"),
            Layout::Raidz1 => Some("raidz1"),
            Layout::Raidz2 => Some("raidz2"),
            Layout::Raidz3 => Some("raidz3"),
        }
    }

    /// Disks the layout accepts.
    pub fn disks(self) -> std::ops::RangeInclusive<usize> {
        match self {
            Layout::Single => 1..=1,
            Layout::Mirror => 2..=MAX_POOL_DISKS,
            Layout::Raidz1 => 3..=MAX_POOL_DISKS,
            Layout::Raidz2 => 4..=MAX_POOL_DISKS,
            Layout::Raidz3 => 5..=MAX_POOL_DISKS,
        }
    }
}

/// Most disks in one pool.
pub const MAX_POOL_DISKS: usize = 12;

/// Name of a `/dev/disk/by-id` link, e.g. `ata-WDC_WD40EFRX_WD-1234`: the
/// form in which disks to erase travel, so that a disk is recognized even
/// if its kernel name changed. Letters, digits, `_ . : + -`, not starting
/// with `-` or `.`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DiskId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the name of a /dev/disk/by-id link")]
pub struct InvalidDiskId;

impl DiskId {
    pub fn parse(value: &str) -> Result<Self, InvalidDiskId> {
        let valid = (1..=128).contains(&value.len())
            && !value.starts_with(['-', '.'])
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:+-".contains(&b));
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidDiskId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DiskId {
    type Error = InvalidDiskId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DiskId> for String {
    fn from(id: DiskId) -> Self {
        id.0
    }
}

/// Which agent carries out a request: the system agent (disks, ZFS,
/// accounts, server configuration) or the files agent (the contents of
/// volumes). Each refuses the other's requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    Files,
}

impl Request {
    /// Whether the request may take long: copying, moving or deleting
    /// trees, permissions of a whole share, preparing or destroying pools,
    /// fetching and running apps and projects, moving Docker's data. The
    /// agent runs these in places of their own, so they never keep quick
    /// ones waiting; clients wait for them longer.
    pub fn is_long(&self) -> bool {
        match self {
            // Within one root a move is a rename (an upload's last step,
            // F2), which never waits behind hours of copying.
            Request::MoveEntry { from, to, .. } => !from.same_share(to),
            Request::CopyEntry { .. }
            // Planning walks the whole folder.
            | Request::ArchiveFolder { .. }
            // Restarts Docker.
            | Request::EnableDocker { .. }
            // An ISO of gigabytes copied; a VM's disks destroyed.
            | Request::CopyVmImage { .. }
            | Request::DeleteVm { .. }
            | Request::UpdateVm { .. }
            // A VM's disks read or written whole: up to hours.
            | Request::BackUpVm { .. }
            | Request::RestoreVm { .. }
            | Request::EndVmWork { .. }
            | Request::RemoveVmBackup { .. }
            | Request::MoveDocker { .. }
            // Fetch images, start or stop containers.
            | Request::StartApp { .. }
            | Request::StopApp { .. }
            | Request::RemoveApp { .. }
            | Request::DeployProject { .. }
            | Request::CreatePool { .. }
            | Request::DestroyPool { .. }
            | Request::ReplaceDisk { .. }
            // Opening a pool reads all its disks' labels; a large one
            // replays its log.
            | Request::ImportPool { .. }
            | Request::DestroyShare { .. }
            // A file system replays its journal; unmounting writes what is
            // cached, minutes for a slow stick.
            | Request::MountRemovable { .. }
            | Request::EjectRemovable { .. }
            // Images fetched, projects brought up or down: up to half an hour.
            | Request::PullImage { .. }
            | Request::PruneImages {}
            // Volumes walked, backed up and restored: up to hours.
            | Request::DockerVolumeSizes {}
            | Request::PruneVolumes {}
            | Request::BackUpVolume { .. }
            | Request::RestoreVolume { .. }
            | Request::EndVolumeWork { .. }
            | Request::ScanVolumeBackups { .. }
            // A backup, a restore or retention: up to a day.
            | Request::RunBackup { .. }
            | Request::RestoreBackup { .. }
            | Request::PruneBackup { .. }
            | Request::ControlProject { .. }
            // The ACME server checks the names: up to minutes with DNS.
            | Request::IssueCertificate { .. }
            // Every entry of a share, once.
            | Request::MigrateShare { .. } => true,
            // Into the user's recycle bin it is a rename; for good (what is
            // in the bin already, or without one) it walks the tree.
            Request::RemoveEntry { location, recycle, .. } => {
                !*recycle || location.path.names().next() == Some(RECYCLE_DIRECTORY)
            }
            _ => false,
        }
    }

    /// The user a long files request acts as: each has one such place at a
    /// time, so nobody takes them all.
    pub fn acting_user(&self) -> Option<&UserName> {
        match self {
            Request::CopyEntry { user, .. }
            | Request::MoveEntry { user, .. }
            | Request::RemoveEntry { user, .. }
            | Request::ArchiveFolder { user, .. } => self.is_long().then_some(user),
            _ => None,
        }
    }

    /// A walk through a whole share's permissions: it has places of its
    /// own, so users' copies never hold it up for hours.
    pub fn is_walk(&self) -> bool {
        matches!(self, Request::MigrateShare { .. })
    }

    /// Reading from a disk to make it blink: places of their own, so disks
    /// blinking for minutes never keep the quick requests waiting.
    pub fn is_burst(&self) -> bool {
        matches!(self, Request::ReadBurst { .. })
    }

    /// A backup or restore of a volume or a VM, or what ends one: up to
    /// hours in units of their own, in places of their own, so that the
    /// long places stay free for deploys, pulls, drives and pools.
    pub fn is_volume_work(&self) -> bool {
        matches!(
            self,
            Request::BackUpVolume { .. }
                | Request::RestoreVolume { .. }
                | Request::RemoveVolumeBackup { .. }
                | Request::EndVolumeWork { .. }
                | Request::ScanVolumeBackups { .. }
                | Request::BackUpVm { .. }
                | Request::RestoreVm { .. }
                | Request::EndVmWork { .. }
                | Request::RemoveVmBackup { .. }
        )
    }

    /// Whether the request only reads the state of the NAS. zendiloosd asks
    /// for that again and again: every minute for the disks and pools, every
    /// 30 seconds for what the firewall needs to know, every 20 seconds for
    /// what containers use, every few seconds for a page that is open. The
    /// agent logs those that went well at debug, so that the journal, on
    /// the system disk, keeps what changed the NAS rather than some 30 000
    /// lines a day of looking. A file opened or packed for a person's
    /// download is not among them.
    pub fn only_reads(&self) -> bool {
        matches!(
            self,
            Request::Hello {}
                | Request::ListBlockDevices {}
                | Request::Machine {}
                | Request::SmartData { .. }
                | Request::EmmcHealth { .. }
                | Request::ArrayStatus {}
                | Request::ZfsStatus {}
                | Request::ImportablePools {}
                | Request::DockerState {}
                | Request::VmHost {}
                | Request::VmImages { .. }
                | Request::VmStates { .. }
                | Request::VmStats { .. }
                | Request::VmHostDevices { .. }
                | Request::VmSnapshots { .. }
                | Request::BackupFacts { .. }
                | Request::ListBackup { .. }
                | Request::AppsStatus { .. }
                | Request::AppLogs { .. }
                | Request::HostFacts { .. }
                | Request::DockerContainers { .. }
                | Request::DockerStats { .. }
                | Request::DockerImages { .. }
                | Request::DockerNetworks { .. }
                | Request::DockerDiskUsage { .. }
                | Request::ContainerLogs { .. }
                | Request::InspectContainer { .. }
                | Request::ContainerProcesses { .. }
                | Request::DockerVolumes { .. }
                | Request::DockerVolumeSizes { .. }
                | Request::ScanVolumeBackups { .. }
                | Request::CheckCompose { .. }
                | Request::ProjectLogs { .. }
                | Request::ProjectFiles { .. }
                | Request::NetworkState { .. }
                | Request::SshState { .. }
                | Request::UpsFacts { .. }
                | Request::ScanUps { .. }
                | Request::PackagesState { .. }
                | Request::WebTlsState { .. }
                | Request::AcmeState { .. }
                | Request::WebServerState { .. }
                | Request::FirewallState { .. }
                | Request::TimeState { .. }
                | Request::HostNameState { .. }
                | Request::ListSnapshots { .. }
                | Request::ShareUsage { .. }
                | Request::ListDirectory { .. }
                | Request::SearchFiles { .. }
                | Request::EntryFacts { .. }
        )
    }

    /// A backup task's run, restore or retention: up to a day in units of
    /// their own, in places of their own, so tasks never keep the long
    /// places (deploys, pulls, drives, pools) or the volumes' backups
    /// waiting.
    pub fn is_backup_work(&self) -> bool {
        matches!(
            self,
            Request::RunBackup { .. } | Request::RestoreBackup { .. } | Request::PruneBackup { .. }
        )
    }

    /// Counting what the volumes' data takes up: bounded, but seconds of
    /// walking each time; one at a time, in a place of its own.
    pub fn is_count(&self) -> bool {
        matches!(self, Request::DockerVolumeSizes {})
    }

    pub fn role(&self) -> Role {
        match self {
            Request::PrepareShare { .. }
            | Request::MigrateShare { .. }
            | Request::MakeHome { .. }
            | Request::ListDirectory { .. }
            | Request::SearchFiles { .. }
            | Request::EntryFacts { .. }
            | Request::OpenFile { .. }
            | Request::CreateUpload { .. }
            | Request::MakeDirectory { .. }
            | Request::MoveEntry { .. }
            | Request::CopyEntry { .. }
            | Request::RemoveEntry { .. }
            | Request::DiscardUpload { .. }
            | Request::RestoreEntry { .. }
            | Request::ArchiveFolder { .. } => Role::Files,
            // The handshake works with both.
            _ => Role::System,
        }
    }
}

/// Name of a share, also its dataset and directory name: a lowercase
/// letter, then up to 31 lowercase letters, digits, `_` or `-`; not a name
/// Samba reserves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ShareName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid share name")]
pub struct InvalidShareName;

impl ShareName {
    const RESERVED: [&str; 5] = ["global", "homes", "printers", "print", "ipc"];

    pub fn parse(name: &str) -> Result<Self, InvalidShareName> {
        if is_linux_name(name) && !Self::RESERVED.contains(&name) {
            Ok(Self(name.to_owned()))
        } else {
            Err(InvalidShareName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ShareName {
    type Error = InvalidShareName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ShareName> for String {
    fn from(name: ShareName) -> Self {
        name.0
    }
}

/// A user or group a share grants access to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Principal {
    User(UserName),
    Group(GroupName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareAccess {
    pub principal: Principal,
    pub permission: SharePermission,
}

impl ShareAccess {
    pub fn writes(&self) -> bool {
        self.permission == SharePermission::ReadWrite
    }

    pub fn denied(&self) -> bool {
        self.permission == SharePermission::Deny
    }
}

/// What a user or group may do on a share, as Samba enforces it: `deny`
/// becomes `invalid users`, which wins over `valid users`, also for the
/// members of a group listed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharePermission {
    Deny,
    Read,
    ReadWrite,
}

/// The share `home`: `/volume<volume>/@homes/<user>` for each user who
/// connects, if among `users`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomesShare {
    pub volume: VolumeNumber,
    pub users: Vec<UserName>,
}

/// A Samba share at `/volume<volume>/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmbShare {
    pub name: ShareName,
    pub volume: VolumeNumber,
    pub access: Vec<ShareAccess>,
    /// Shown by clients beside the share's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<ShareComment>,
    /// Macs back up to it with Time Machine; `Some(None)` without a limit
    /// of its own, `Some(Some(bytes))` up to that much.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_machine: Option<Option<u64>>,
}

/// A setup code as the console shows it: `ABCD-EFGH-JKLM`, from the setup
/// code alphabet (no `I`, `O`, `0` or `1`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConsoleSetupCode(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid setup code")]
pub struct InvalidConsoleSetupCode;

impl ConsoleSetupCode {
    const ALPHABET: &'static [u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

    pub fn parse(code: &str) -> Result<Self, InvalidConsoleSetupCode> {
        let bytes = code.as_bytes();
        let valid = bytes.len() == 14
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| if i == 4 || i == 9 { *b == b'-' } else { Self::ALPHABET.contains(b) });
        if valid { Ok(Self(code.to_owned())) } else { Err(InvalidConsoleSetupCode) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// It is a secret: never in logs.
impl std::fmt::Debug for ConsoleSetupCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConsoleSetupCode(…)")
    }
}

impl TryFrom<String> for ConsoleSetupCode {
    type Error = InvalidConsoleSetupCode;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConsoleSetupCode> for String {
    fn from(code: ConsoleSetupCode) -> Self {
        code.0
    }
}

/// A drive's self-test: a few minutes, or every sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfTestKind {
    Short,
    Long,
}

/// Compression of a share's dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareCompression {
    Off,
    Lz4,
    Zstd,
}

impl ShareCompression {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }
}

/// Record size of a share's dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRecordSize {
    K16,
    K128,
    M1,
}

impl ShareRecordSize {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::K16 => "16K",
            Self::K128 => "128K",
            Self::M1 => "1M",
        }
    }
}

/// [`SnapshotLabel`]s as a `strftime` format, for Samba's `shadow:format`.
pub const SNAPSHOT_LABEL_FORMAT: &str = "zdl-%Y-%m-%d_%H.%M.%S";

/// The name of a snapshot after the `@`: its UTC time as
/// `zdl-YYYY-MM-DD_HH.MM.SS`, the form Samba's shadow_copy2 reads. Nothing
/// else, so it can be neither an option nor a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SnapshotLabel(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid snapshot label")]
pub struct InvalidSnapshotLabel;

impl SnapshotLabel {
    pub fn parse(label: &str) -> Result<Self, InvalidSnapshotLabel> {
        let shape = "zdl-0000-00-00_00.00.00";
        let valid = label.len() == shape.len()
            && label
                .bytes()
                .zip(shape.bytes())
                .all(|(c, s)| if s == b'0' { c.is_ascii_digit() } else { c == s });
        if valid { Ok(Self(label.to_owned())) } else { Err(InvalidSnapshotLabel) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SnapshotLabel {
    type Error = InvalidSnapshotLabel;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SnapshotLabel> for String {
    fn from(label: SnapshotLabel) -> Self {
        label.0
    }
}

/// A schedule a snapshot is taken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFrequency {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl SnapshotFrequency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// A note on a snapshot, kept as a ZFS property: at most 200 characters,
/// no control characters, not empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SnapshotNote(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid snapshot note")]
pub struct InvalidSnapshotNote;

impl SnapshotNote {
    pub const MAX_CHARS: usize = 200;

    pub fn parse(text: &str) -> Result<Self, InvalidSnapshotNote> {
        let valid = !text.is_empty()
            && text.chars().count() <= Self::MAX_CHARS
            && !text.chars().any(char::is_control);
        if valid { Ok(Self(text.to_owned())) } else { Err(InvalidSnapshotNote) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SnapshotNote {
    type Error = InvalidSnapshotNote;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SnapshotNote> for String {
    fn from(note: SnapshotNote) -> Self {
        note.0
    }
}

/// A share's description as Samba's `comment`. It ends up in `smb.conf`, so
/// nothing in it may end the line or continue it (control characters, a
/// backslash) or be expanded by Samba (`%`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ShareComment(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid share comment")]
pub struct InvalidShareComment;

impl ShareComment {
    pub const MAX_CHARS: usize = 200;

    fn allowed(c: char) -> bool {
        !c.is_control() && c != '\\' && c != '%'
    }

    pub fn parse(text: &str) -> Result<Self, InvalidShareComment> {
        let valid = !text.is_empty()
            && text.trim() == text
            && text.chars().count() <= Self::MAX_CHARS
            && text.chars().all(Self::allowed);
        if valid { Ok(Self(text.to_owned())) } else { Err(InvalidShareComment) }
    }

    /// `text` without what [`parse`](Self::parse) refuses, shortened to fit;
    /// `None` if nothing is left.
    pub fn lossy(text: &str) -> Option<Self> {
        let kept: String = text.chars().filter(|&c| Self::allowed(c)).take(Self::MAX_CHARS).collect();
        Self::parse(kept.trim()).ok()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ShareComment {
    type Error = InvalidShareComment;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ShareComment> for String {
    fn from(comment: ShareComment) -> Self {
        comment.0
    }
}

/// An NFS export of `/volume<volume>/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NfsExport {
    pub name: ShareName,
    pub volume: VolumeNumber,
    pub clients: Vec<NfsClient>,
}

/// A host or network an export names. Whoever it is on that client, every
/// request is squashed to the group `zendiloos-files` (`all_squash`, the
/// agent fills in the IDs): the client has the share's access as a whole,
/// read or read and write, as `sec=sys` gives it in effect anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NfsClient {
    pub network: NfsHost,
    pub write: bool,
    /// `async`: writes answered before they are on disk.
    #[serde(default, rename = "async")]
    pub asynchronous: bool,
    /// `insecure`: also from ports above 1024.
    #[serde(default)]
    pub insecure: bool,
}

/// Who an NFS rule is for: an IP address or network, or a host name with
/// the wildcards `*` and `?` as `exportfs` reads them. Only lowercase
/// letters, digits, `-`, `.` and the wildcards: nothing the exports file
/// would read as a separator or an option.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum NfsHost {
    Network(Network),
    Name(String),
}

impl NfsHost {
    pub fn parse(value: &str) -> Result<Self, InvalidNetwork> {
        if let Ok(network) = Network::parse(value) {
            return Ok(Self::Network(network));
        }
        let label = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-*?".contains(&b))
        };
        let named = value.bytes().any(|b| b.is_ascii_lowercase() || b == b'*' || b == b'?');
        if value.len() <= 253 && named && value.split('.').all(label) {
            Ok(Self::Name(value.to_owned()))
        } else {
            Err(InvalidNetwork)
        }
    }
}

impl fmt::Display for NfsHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(network) => network.fmt(f),
            Self::Name(name) => f.write_str(name),
        }
    }
}

impl TryFrom<String> for NfsHost {
    type Error = InvalidNetwork;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<NfsHost> for String {
    fn from(host: NfsHost) -> Self {
        host.to_string()
    }
}

/// An IP address or network in CIDR notation, e.g. `192.168.1.0/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Network {
    address: std::net::IpAddr,
    prefix: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an IP address or network")]
pub struct InvalidNetwork;

impl Network {
    pub fn parse(value: &str) -> Result<Self, InvalidNetwork> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (value, None),
        };
        let address: std::net::IpAddr = address.parse().map_err(|_| InvalidNetwork)?;
        let maximum = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().map_err(|_| InvalidNetwork)?,
            None => maximum,
        };
        if prefix > maximum {
            return Err(InvalidNetwork);
        }
        Ok(Self { address, prefix })
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let maximum = if self.address.is_ipv4() { 32 } else { 128 };
        if self.prefix == maximum {
            write!(f, "{}", self.address)
        } else {
            write!(f, "{}/{}", self.address, self.prefix)
        }
    }
}

impl TryFrom<String> for Network {
    type Error = InvalidNetwork;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Network> for String {
    fn from(network: Network) -> Self {
        network.to_string()
    }
}

/// Name of a pool zendiloos creates: a lowercase letter, then up to 31
/// lowercase letters, digits, `_` or `-`; none of the names ZFS reserves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PoolName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid pool name")]
pub struct InvalidPoolName;

impl PoolName {
    const RESERVED_PREFIXES: [&str; 8] =
        ["mirror", "raidz", "draid", "spare", "log", "cache", "special", "dedup"];

    pub fn parse(name: &str) -> Result<Self, InvalidPoolName> {
        let disk_like = name.len() > 1 && name.starts_with('c') && name.as_bytes()[1].is_ascii_digit();
        let valid = is_linux_name(name)
            && !disk_like
            && !Self::RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix));
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidPoolName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PoolName {
    type Error = InvalidPoolName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PoolName> for String {
    fn from(name: PoolName) -> Self {
        name.0
    }
}

/// The `N` of `/volume<N>`, 1 to 999.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct VolumeNumber(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("volume numbers are 1 to 999")]
pub struct InvalidVolumeNumber;

impl VolumeNumber {
    pub fn new(number: u16) -> Result<Self, InvalidVolumeNumber> {
        if (1..=999).contains(&number) { Ok(Self(number)) } else { Err(InvalidVolumeNumber) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for VolumeNumber {
    type Error = InvalidVolumeNumber;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VolumeNumber> for u16 {
    fn from(number: VolumeNumber) -> Self {
        number.0
    }
}

/// The `zfs list` columns of [`Request::ZfsStatus`].
pub const ZFS_DATASET_COLUMNS: &str = "name,used,available,mounted,zendiloos:volume";

/// Name of a Linux user or group zendiloos manages: a lowercase letter, then
/// up to 31 lowercase letters, digits, `_` or `-`. Valid unchanged for the
/// shadow tools and Samba, and never mistaken for a command-line option.
fn is_linux_name(name: &str) -> bool {
    name.len() <= 32
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

macro_rules! linux_name {
    ($(#[$doc:meta])* $name:ident, $error:ident, $message:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
        #[error($message)]
        pub struct $error;

        impl $name {
            pub fn parse(name: &str) -> Result<Self, $error> {
                if is_linux_name(name) { Ok(Self(name.to_owned())) } else { Err($error) }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(name: $name) -> Self {
                name.0
            }
        }
    };
}

linux_name!(
    /// A Linux user name; see `is_linux_name`.
    UserName,
    InvalidUserName,
    "not a valid user name"
);
linux_name!(
    /// A Linux group name; see `is_linux_name`.
    GroupName,
    InvalidGroupName,
    "not a valid group name"
);

/// A password on its way to Samba. Never printed. At most 1024 bytes and
/// without NUL or line breaks, because it is passed as a line on stdin.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Password(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a password that can be passed on")]
pub struct InvalidPassword;

impl Password {
    pub const MAX_BYTES: usize = 1024;

    pub fn new(password: impl Into<String>) -> Result<Self, InvalidPassword> {
        let password = password.into();
        let valid = !password.is_empty()
            && password.len() <= Self::MAX_BYTES
            && !password.contains(['\0', '\n', '\r']);
        if valid { Ok(Self(password)) } else { Err(InvalidPassword) }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

impl TryFrom<String> for Password {
    type Error = InvalidPassword;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Password> for String {
    fn from(password: Password) -> Self {
        password.0
    }
}

/// Correlates a request across the API's audit log and the agent's journal.
/// Up to 64 ASCII letters, digits, `-` and `_`, so it is safe to log as root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RequestId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid request ID")]
pub struct InvalidRequestId;

impl RequestId {
    pub fn parse(value: &str) -> Result<Self, InvalidRequestId> {
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidRequestId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = InvalidRequestId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RequestId> for String {
    fn from(id: RequestId) -> Self {
        id.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tree_walks_and_pools_take_long_lookups_do_not() {
        let user = || UserName::parse("anna").unwrap();
        let volume = VolumeNumber::new(1).unwrap();
        let share = ShareName::parse("photos").unwrap();
        let location = FileLocation {
            volume,
            root: FileRoot::Share(share.clone()),
            path: SharePath::parse("album").unwrap(),
            snapshot: None,
        };
        assert!(!Request::ListDirectory { user: user(), location: location.clone() }.is_long());
        // A search has a few seconds and ends when its client hangs up.
        let search: Request = serde_json::from_value(json!({
            "op": "search_files", "user": "anna", "text": "beach",
            "location": { "volume": 1, "root": { "share": "photos" }, "path": "album" },
        }))
        .unwrap();
        assert!(matches!(&search, Request::SearchFiles { hidden: false, .. }));
        assert!(!search.is_long() && search.acting_user().is_none());
        assert_eq!((search.name(), search.role()), ("search_files", Role::Files));
        assert!(!Request::ScrubPool { pool: PoolName::parse("tank").unwrap() }.is_long());
        let bin = FileLocation { path: SharePath::parse(".recycle/anna/a.txt").unwrap(), ..location.clone() };
        assert!(!Request::RemoveEntry { user: user(), location: location.clone(), recycle: true }.is_long());
        assert!(Request::RemoveEntry { user: user(), location: location.clone(), recycle: false }.is_long());
        assert!(
            Request::RemoveEntry { user: user(), location: bin.clone(), recycle: true }.is_long(),
            "for good"
        );
        let removing = Request::RemoveEntry { user: user(), location: bin.clone(), recycle: true };
        assert_eq!(removing.acting_user(), Some(&user()), "one long place each");
        let recycling = Request::RemoveEntry {
            user: user(),
            location: FileLocation { path: SharePath::parse("a.txt").unwrap(), ..bin },
            recycle: true,
        };
        assert_eq!(recycling.acting_user(), None, "a quick one takes no long place");
        assert!(Request::DestroyPool { name: PoolName::parse("tank").unwrap(), volume }.is_long());
        let migrate = Request::MigrateShare { volume, name: share.clone() };
        let prepare = Request::PrepareShare { volume, name: share.clone() };
        assert!(migrate.is_long() && !prepare.is_long());
        assert!(migrate.is_walk() && !prepare.is_walk());
        assert_eq!(migrate.acting_user(), None, "zendiloosd's, in places of its own");
        assert_eq!((migrate.role(), prepare.role()), (Role::Files, Role::Files));
        assert_eq!((migrate.name(), prepare.name()), ("migrate_share", "prepare_share"));
        let id = || RemovableId::parse("1A2B-3C4D").unwrap();
        assert!(Request::MountRemovable { id: id() }.is_long());
        assert!(Request::EjectRemovable { id: id(), power_off: true }.is_long());
        assert!(!Request::CleanRemovable {}.is_long());
    }

    #[test]
    fn a_rename_within_one_place_is_quick_a_move_between_places_long() {
        let user = || UserName::parse("anna").unwrap();
        let at = |root: FileRoot, path: &str| FileLocation {
            volume: VolumeNumber::new(1).unwrap(),
            root,
            path: SharePath::parse(path).unwrap(),
            snapshot: None,
        };
        let photos = |path: &str| at(FileRoot::Share(ShareName::parse("photos").unwrap()), path);
        // An upload's last step: the hidden file to its name.
        let upload = Request::MoveEntry {
            user: user(),
            from: photos("album/.zendiloos-upload-0123456789abcdef"),
            to: photos("album/beach.jpg"),
            replace: true,
        };
        // F2, and a move into another folder of the same place.
        let rename = Request::MoveEntry {
            user: user(),
            from: photos("album/beach.jpg"),
            to: photos("album/sea.jpg"),
            replace: false,
        };
        let within = Request::MoveEntry {
            user: user(),
            from: photos("a.jpg"),
            to: photos("album/a.jpg"),
            replace: false,
        };
        for request in [upload, rename, within] {
            assert!(!request.is_long() && request.acting_user().is_none(), "{request:?}");
        }
        let drive = || FileRoot::Removable(RemovableId::parse("1A2B-3C4D").unwrap());
        for to in [
            at(FileRoot::Share(ShareName::parse("music").unwrap()), "a.jpg"),
            at(FileRoot::Home, "a.jpg"),
            at(drive(), "a.jpg"),
            FileLocation { volume: VolumeNumber::new(2).unwrap(), ..photos("a.jpg") },
        ] {
            let across = Request::MoveEntry { user: user(), from: photos("a.jpg"), to, replace: false };
            assert!(across.is_long(), "{across:?}");
            assert_eq!(across.acting_user(), Some(&user()), "one long place each");
        }
        let on_the_drive =
            Request::MoveEntry { user: user(), from: at(drive(), "a"), to: at(drive(), "b"), replace: false };
        assert!(!on_the_drive.is_long());
    }

    #[test]
    fn a_partial_move_says_what_stayed() {
        let partly = Response::PartlyMoved {
            kept: 2,
            example: Some(SharePath::parse("album/new.txt").unwrap()),
            too_many: false,
        };
        let wire =
            json!({ "status": "partly_moved", "kept": 2, "example": "album/new.txt", "too_many": false });
        assert_eq!(serde_json::to_value(&partly).unwrap(), wire);
        assert_eq!(serde_json::from_value::<Response>(wire).unwrap(), partly);
    }

    #[test]
    fn volume_backups_go_to_the_system_agent_and_take_long() {
        let backup: Request = serde_json::from_value(json!({
            "op": "back_up_volume", "volume": "teslamate_db",
            "share": { "volume": 1, "share": "backups" }, "folder": "docker", "name": "teslamate_db-20261108-030000",
        }))
        .unwrap();
        assert_eq!((backup.name(), backup.role(), backup.is_long()), ("back_up_volume", Role::System, true));
        let restore: Request = serde_json::from_value(json!({
            "op": "restore_volume", "volume": "teslamate_db",
            "share": { "volume": 1, "share": "backups" },
            "archive": "docker/teslamate_db/teslamate_db-20261108-030000.tar.zst", "sha256": "ab".repeat(32),
            "data_bytes": 4096, "entries": null,
        }))
        .unwrap();
        assert_eq!((restore.name(), restore.is_long()), ("restore_volume", true));
        for bad in [
            json!({ "op": "restore_volume", "volume": "-x", "share": { "volume": 1, "share": "b" }, "archive": "a", "sha256": "ab".repeat(32), "data_bytes": 1, "entries": 1 }),
            json!({ "op": "restore_volume", "volume": "x", "share": { "volume": 1, "share": "b" }, "archive": "../a", "sha256": "ab".repeat(32), "data_bytes": 1, "entries": 1 }),
            json!({ "op": "restore_volume", "volume": "x", "share": { "volume": 1, "share": "b" }, "archive": "a", "sha256": "short", "data_bytes": 1, "entries": 1 }),
            json!({ "op": "back_up_volume", "volume": "x", "share": { "volume": 1, "share": "b" }, "folder": "", "name": "a/b" }),
            json!({ "op": "create_volume", "name": "x", "labels": { "a=b": "c" } }),
        ] {
            assert!(serde_json::from_value::<Request>(bad.clone()).is_err(), "{bad}");
        }
        let remove = Request::RemoveVolumeBackup {
            share: ShareLocation {
                volume: VolumeNumber::new(1).unwrap(),
                share: ShareName::parse("backups").unwrap(),
            },
            path: SharePath::parse("docker/x/x-20261108-030000.tar.zst").unwrap(),
            inode: 7,
            bytes: 1234,
            modified: Some(1_762_570_800_000_000_000),
        };
        assert_eq!((remove.name(), remove.is_long()), ("remove_volume_backup", false));
        assert!(Request::DockerVolumeSizes {}.is_long() && !Request::DockerVolumes {}.is_long());
        // Places of their own, never the long ones of deploys and pulls.
        let end = Request::EndVolumeWork { volume: VolumeName::parse("x1").unwrap() };
        assert_eq!((end.name(), end.is_long()), ("end_volume_work", true));
        for work in [&backup, &restore, &remove, &end] {
            assert!(work.is_volume_work() && !work.is_count(), "{work:?}");
        }
        assert!(Request::DockerVolumeSizes {}.is_count() && !Request::DockerVolumeSizes {}.is_volume_work());
        let scan: Request = serde_json::from_value(json!({
            "op": "scan_volume_backups", "share": { "volume": 1, "share": "backups" },
            "folder": "docker", "known": ["docker/db/db-20261108-030000.tar.zst"],
        }))
        .unwrap();
        assert_eq!((scan.name(), scan.is_long(), scan.is_volume_work()), ("scan_volume_backups", true, true));
        let outside = json!({ "op": "scan_volume_backups", "share": { "volume": 1, "share": "b" },
                               "folder": "../up", "known": [] });
        assert!(serde_json::from_value::<Request>(outside).is_err());
        assert!(!Request::PruneVolumes {}.is_volume_work() && !Request::DockerVolumes {}.is_count());
        let names = [
            Request::DockerVolumes {}.name(),
            Request::PruneVolumes {}.name(),
            Request::RemoveVolume { name: VolumeName::parse("x1").unwrap() }.name(),
            Request::CreateVolume { name: VolumeName::parse("x1").unwrap(), labels: VolumeLabels::default() }
                .name(),
        ];
        assert_eq!(names, ["docker_volumes", "prune_volumes", "remove_volume", "create_volume"]);
    }

    #[test]
    fn removable_drives_travel_by_their_uuid_to_the_system_agent() {
        let eject = serde_json::from_value::<Request>(
            json!({ "op": "eject_removable", "id": "1A2B-3C4D", "power_off": true }),
        )
        .unwrap();
        assert_eq!(eject.name(), "eject_removable");
        assert_eq!(eject.role(), Role::System);
        for id in ["../etc", "-o", "a b"] {
            let mount = json!({ "op": "mount_removable", "id": id });
            assert!(serde_json::from_value::<Request>(mount).is_err(), "{id}");
        }
        assert_eq!(Request::CleanRemovable {}.name(), "clean_removable");
        assert_eq!(
            Request::MountRemovable { id: RemovableId::parse("A").unwrap() }.name(),
            "mount_removable"
        );
    }
    #[test]
    fn has_a_stable_wire_format() {
        let envelope = Envelope::new(Request::PowerOff {}, Some(RequestId::parse("4f0c-91").unwrap()));
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({ "version": 1, "request_id": "4f0c-91", "request": { "op": "power_off" } })
        );
        assert_eq!(
            serde_json::to_value(Response::Error { code: ErrorCode::Forbidden, message: "no".into() })
                .unwrap(),
            json!({ "status": "error", "code": "forbidden", "message": "no" })
        );
    }

    #[test]
    fn rejects_anything_outside_the_protocol() {
        let parse = |value: serde_json::Value| serde_json::from_value::<Envelope>(value);
        assert!(parse(json!({ "version": 1, "request": { "op": "hello" } })).is_ok());
        assert!(parse(json!({ "version": 1, "request": { "op": "run", "command": "sh" } })).is_err());
        assert!(parse(json!({ "version": 1, "request": { "op": "reboot", "force": true } })).is_err());
        assert!(parse(json!({ "version": 1, "request": { "op": "hello" }, "extra": 1 })).is_err());
        assert!(parse(json!({ "version": 1, "request_id": "a\nb", "request": { "op": "hello" } })).is_err());
    }

    #[test]
    fn a_read_burst_names_a_whole_disk_and_lasts_two_seconds_at_most() {
        let burst: Request =
            serde_json::from_value(json!({ "op": "read_burst", "device": "sdc", "millis": 700 })).unwrap();
        assert_eq!(
            burst,
            Request::ReadBurst {
                device: DiskName::parse("sdc").unwrap(),
                millis: BurstMillis::new(700).unwrap()
            }
        );
        assert_eq!((burst.name(), burst.role()), ("read_burst", Role::System));
        assert!(burst.is_burst() && !burst.is_long() && !burst.is_walk());
        assert!(!Request::Machine {}.is_burst());
        assert_eq!(
            serde_json::to_value(&burst).unwrap(),
            json!({ "op": "read_burst", "device": "sdc", "millis": 700 })
        );
        for bad in [
            json!({ "op": "read_burst", "device": "sdc", "millis": 0 }),
            json!({ "op": "read_burst", "device": "sdc", "millis": 2001 }),
            json!({ "op": "read_burst", "device": "sdc", "millis": -1 }),
            json!({ "op": "read_burst", "device": "sdc1", "millis": 700 }),
            json!({ "op": "read_burst", "device": "/dev/sdc", "millis": 700 }),
            json!({ "op": "read_burst", "device": "sdc" }),
            json!({ "op": "read_burst", "device": "sdc", "millis": 700, "write": true }),
        ] {
            assert!(serde_json::from_value::<Request>(bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(BurstMillis::new(BurstMillis::MAX).map(BurstMillis::get), Ok(2000));
        assert_eq!(u16::from(BurstMillis::new(1).unwrap()), 1);
    }

    #[test]
    fn what_only_reads_is_told_apart_from_what_changes_the_nas() {
        let request = |value: serde_json::Value| serde_json::from_value::<Request>(value).unwrap();
        for looking in [
            json!({ "op": "list_block_devices" }),
            json!({ "op": "zfs_status" }),
            json!({ "op": "importable_pools" }),
            json!({ "op": "smart_data", "device": "sda" }),
            json!({ "op": "emmc_health", "device": "mmcblk0" }),
            json!({ "op": "docker_stats" }),
            json!({ "op": "ssh_state" }),
            json!({ "op": "web_server_state" }),
            json!({ "op": "firewall_state" }),
            json!({ "op": "backup_facts" }),
        ] {
            assert!(request(looking.clone()).only_reads(), "{looking}");
        }
        for changing in [
            json!({ "op": "reboot" }),
            json!({ "op": "scrub_pool", "pool": "tank" }),
            json!({ "op": "import_pool", "name": "tank", "id": 42, "volume": 1 }),
            json!({ "op": "mount_volume", "pool": "tank", "volume": 1 }),
            json!({ "op": "start_self_test", "device": "sda", "kind": "short" }),
            json!({ "op": "read_burst", "device": "sda", "millis": 700 }),
            json!({ "op": "make_backup_key" }),
        ] {
            assert!(!request(changing.clone()).only_reads(), "{changing}");
        }
    }

    #[test]
    fn an_emmc_device_s_wear_is_asked_for_by_its_name_and_comes_raw() {
        let asked: Request =
            serde_json::from_value(json!({ "op": "emmc_health", "device": "mmcblk0" })).unwrap();
        assert_eq!((asked.name(), asked.role()), ("emmc_health", Role::System));
        for device in ["mmcblk0p1", "mmcblk0boot0", "../mmcblk0"] {
            let refused = serde_json::from_value::<Request>(json!({ "op": "emmc_health", "device": device }));
            assert!(refused.is_err(), "{device}");
        }
        let answer: Response = serde_json::from_value(json!({
            "status": "emmc", "kind": "MMC\n", "life_time": "0x01 0x02\n", "pre_eol_info": "0x01\n",
            "name": "AJTD4R\n", "serial": "0x1a2b3c4d\n", "fwrev": "0x0800000000000000\n",
        }))
        .unwrap();
        assert!(matches!(answer, Response::Emmc { ref life_time, .. } if life_time == "0x01 0x02\n"));
    }

    #[test]
    fn the_machine_s_names_and_udev_s_database_travel_as_written() {
        let machine: Request = serde_json::from_value(json!({ "op": "machine" })).unwrap();
        assert_eq!((machine.name(), machine.role()), ("machine", Role::System));
        assert!(serde_json::from_value::<Request>(json!({ "op": "machine", "path": "/etc" })).is_err());
        let answer: Response = serde_json::from_value(
            json!({ "status": "machine", "sys_vendor": "UGREEN\n", "product_name": "DXP4800 Plus\n" }),
        )
        .unwrap();
        assert_eq!(
            answer,
            Response::Machine { sys_vendor: "UGREEN\n".into(), product_name: "DXP4800 Plus\n".into() }
        );
        // Without udev's part, as a test answers.
        let devices: Response =
            serde_json::from_value(json!({ "status": "block_devices", "lsblk_json": "{}" })).unwrap();
        assert_eq!(devices, Response::BlockDevices { lsblk_json: "{}".into(), udev_db: String::new() });
    }

    #[test]
    fn disk_names_are_whole_disks_only() {
        for good in ["sda", "sdab", "vdb", "xvda", "nvme0n1", "nvme12n3", "mmcblk0"] {
            assert!(DiskName::parse(good).is_ok(), "{good}");
        }
        for bad in
            ["sda1", "nvme0n1p1", "nvme0", "/dev/sda", "../sda", "-sda", "sd", "md0", "loop0", "SDA", ""]
        {
            assert_eq!(DiskName::parse(bad), Err(InvalidDiskName), "{bad:?}");
        }
        let smuggled = json!({ "version": 1, "request": { "op": "smart_data", "device": "sda;reboot" } });
        assert!(serde_json::from_value::<Envelope>(smuggled).is_err());
    }

    #[test]
    fn user_and_group_names_cannot_become_options_or_paths() {
        for good in ["alice", "a", "bob-smith", "carol_2", &"x".repeat(32)] {
            assert!(UserName::parse(good).is_ok(), "{good}");
            assert!(GroupName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-alice", "--help", "Alice", "1alice", "al ice", "a/b", "a,b", "a:b", &"x".repeat(33)]
        {
            assert_eq!(UserName::parse(bad), Err(InvalidUserName), "{bad:?}");
            assert_eq!(GroupName::parse(bad), Err(InvalidGroupName), "{bad:?}");
        }
        let smuggled = json!({ "version": 1, "request": { "op": "set_group_members", "group": "family",
            "members": ["alice", "bob,root"] } });
        assert!(serde_json::from_value::<Envelope>(smuggled).is_err());
    }

    #[test]
    fn passwords_are_single_lines_and_never_printed() {
        let password = Password::new("correct horse battery").unwrap();
        assert_eq!(format!("{password:?}"), "Password(***)");
        let request = Request::SetSmbPassword { name: UserName::parse("alice").unwrap(), password };
        assert!(!format!("{request:?}").contains("horse"));
        for bad in ["", "two\nlines", "carriage\rreturn", "nul\0", &"x".repeat(1025)] {
            assert_eq!(Password::new(bad), Err(InvalidPassword), "{bad:?}");
        }
    }

    #[test]
    fn storage_arguments_cannot_become_options_or_paths() {
        for good in ["ata-QEMU_HARDDISK_ZDL-SATA-1", "nvme-eui.0025385b71b0a001", "wwn-0x5000c500a1b2c3d4"] {
            assert!(DiskId::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-f", ".hidden", "../sda", "ata-X/../../sda", "a b", &"x".repeat(129)] {
            assert_eq!(DiskId::parse(bad), Err(InvalidDiskId), "{bad:?}");
        }
        for bad in ["mirror", "raidz", "spare2", "c0t0d0", "-f", "Tank", "a/b"] {
            assert_eq!(PoolName::parse(bad), Err(InvalidPoolName), "{bad:?}");
        }
        assert!(PoolName::parse("tank").is_ok());
        let too_big =
            json!({ "version": 1, "request": { "op": "destroy_pool", "name": "tank", "volume": 1000 } });
        assert!(serde_json::from_value::<Envelope>(too_big).is_err());
        let create = json!({ "version": 1, "request": { "op": "create_pool", "name": "tank", "layout": "raidz2",
            "disks": ["ata-A", "ata-B"], "volume": 1 } });
        assert!(serde_json::from_value::<Envelope>(create).is_ok());
    }

    #[test]
    fn console_setup_codes_are_codes_and_nothing_else() {
        assert!(ConsoleSetupCode::parse("ABCD-EFGH-JK23").is_ok());
        for bad in
            ["", "abcd-efgh-jklm", "ABCD-EFGH-JKL0", "ABCDEFGHJKLM", "ABCD-EFGH-JKLM\n\\4", "ABCD EFGH JKLM"]
        {
            assert_eq!(ConsoleSetupCode::parse(bad), Err(InvalidConsoleSetupCode), "{bad:?}");
        }
        assert_eq!(
            format!("{:?}", ConsoleSetupCode::parse("ABCD-EFGH-JKLM").unwrap()),
            "ConsoleSetupCode(…)"
        );
    }

    #[test]
    fn snapshot_labels_are_times_and_notes_one_line() {
        assert!(SnapshotLabel::parse("zdl-2026-11-05_19.40.01").is_ok());
        for bad in [
            "",
            "-r",
            "zdl-2026-11-05_19.40.0",
            "tank@x",
            "zdl-2026-11-05_19.40.01/../x",
            "zdl-2026-1a-05_19.40.01",
        ] {
            assert_eq!(SnapshotLabel::parse(bad), Err(InvalidSnapshotLabel), "{bad:?}");
        }
        assert!(SnapshotNote::parse("before the move = big").is_ok());
        for bad in ["", "two\nlines", &"x".repeat(201)] {
            assert_eq!(SnapshotNote::parse(bad), Err(InvalidSnapshotNote), "{bad:?}");
        }
        let request = json!({ "version": 1, "request": { "op": "create_snapshot", "pool": "tank", "volume": 1,
            "share": "photos", "label": "zdl-2026-11-05_19.40.01", "schedules": ["daily"] } });
        assert!(serde_json::from_value::<Envelope>(request).is_ok());
    }

    #[test]
    fn share_comments_cannot_reach_beyond_their_line() {
        assert!(ShareComment::parse("Family photos, 2024 – 2026").is_ok());
        let injection = "Photos\n   root preexec = /bin/sh";
        for bad in ["", " padded", "a\\", "50% off", "tab\there", injection] {
            assert_eq!(ShareComment::parse(bad), Err(InvalidShareComment), "{bad:?}");
        }
        assert_eq!(ShareComment::lossy(injection).unwrap().as_str(), "Photos   root preexec = /bin/sh");
        assert_eq!(ShareComment::lossy("100%\\").unwrap().as_str(), "100");
        assert_eq!(ShareComment::lossy("%%"), None);
        assert_eq!(ShareComment::lossy(&"x".repeat(300)).unwrap().as_str().len(), ShareComment::MAX_CHARS);
        let request = json!({ "name": "photos", "volume": 1, "access": [], "comment": "a\nb" });
        assert!(serde_json::from_value::<SmbShare>(request).is_err());
    }

    #[test]
    fn sharing_arguments_are_plain_names_and_networks() {
        for bad in ["homes", "printers", "global", "IPC", "-x", "a b", "a/b", "a]b"] {
            assert_eq!(ShareName::parse(bad), Err(InvalidShareName), "{bad:?}");
        }
        assert!(ShareName::parse("photos").is_ok());
        for (input, shown) in
            [("192.168.1.0/24", "192.168.1.0/24"), ("10.0.0.5", "10.0.0.5"), ("fd00::/64", "fd00::/64")]
        {
            assert_eq!(Network::parse(input).unwrap().to_string(), shown);
        }
        for bad in ["", "host.example", "10.0.0.0/33", "10.0.0.0/", "*", "10.0.0.0/24(rw)", "10.0.0.1 *"] {
            assert_eq!(Network::parse(bad), Err(InvalidNetwork), "{bad:?}");
        }
        let request = json!({ "version": 1, "request": { "op": "apply_sharing",
            "smb": [{ "name": "photos", "volume": 1,
                      "access": [{ "principal": { "group": "family" }, "permission": "read_write" },
                                 { "principal": { "user": "guest-bob" }, "permission": "deny" }] }],
            "nfs": [{ "name": "photos", "volume": 1, "clients": [{ "network": "192.168.1.0/24", "write": false }] }],
            "services": { "smb": true, "nfs": true, "workgroup": "WORKGROUP", "smb_min_protocol": "smb2_1",
                "smb_encryption": "default", "smb_signing": "default", "hide_unreadable": true, "multichannel": true,
                "oplocks": true, "leases": true, "durable_handles": false, "mac_compat": true, "follow_symlinks": true,
                "veto_files": null, "delete_veto_files": false, "wsdd": true, "bonjour": true } } });
        assert!(serde_json::from_value::<Envelope>(request.clone()).is_ok());
        let mut closing = request.clone();
        closing["request"]["close"] = json!(["photos"]);
        let Ok(Envelope { request: Request::ApplySharing { close, .. }, .. }) =
            serde_json::from_value::<Envelope>(closing)
        else {
            panic!("the shares to close are read");
        };
        assert_eq!(close, [ShareName::parse("photos").unwrap()]);
        let injected = json!({ "version": 1, "request": { "op": "apply_sharing", "smb": [], "nfs": [{ "name": "photos",
            "volume": 1, "clients": [{ "network": "*(rw,no_root_squash)", "write": true }] }],
            "services": request["request"]["services"] } });
        assert!(serde_json::from_value::<Envelope>(injected).is_err());
        let mut workgroup = request.clone();
        workgroup["request"]["services"]["workgroup"] = json!("X\n[global]");
        assert!(serde_json::from_value::<Envelope>(workgroup).is_err());
        let mut without = request;
        without["request"].as_object_mut().unwrap().remove("services");
        assert!(
            serde_json::from_value::<Envelope>(without).is_err(),
            "the services are part of every change"
        );
    }

    #[test]
    fn nfs_hosts_are_networks_or_plain_names() {
        assert_eq!(NfsHost::parse("10.0.0.0/8"), Ok(NfsHost::Network(Network::parse("10.0.0.0/8").unwrap())));
        assert_eq!(NfsHost::parse("*.example.com").unwrap().to_string(), "*.example.com");
        for bad in ["", "nas lan", "nas(rw)", "a,b", "NAS.lan", "-a.lan", "a..lan", "@netgroup", "/x"] {
            assert_eq!(NfsHost::parse(bad), Err(InvalidNetwork), "{bad:?}");
        }
        let plain: NfsClient = serde_json::from_str(r#"{"network":"10.0.0.5","write":true}"#).unwrap();
        assert_eq!((plain.asynchronous, plain.insecure), (false, false));
        let json = serde_json::to_string(&NfsClient {
            network: NfsHost::parse("nas.lan").unwrap(),
            write: false,
            asynchronous: true,
            insecure: true,
        })
        .unwrap();
        assert_eq!(json, r#"{"network":"nas.lan","write":false,"async":true,"insecure":true}"#);
        let squash = r#"{"network":"10.0.0.5","write":true,"squash":"none"}"#;
        assert!(serde_json::from_str::<NfsClient>(squash).is_err(), "whose identity is not asked any more");
    }

    #[test]
    fn request_ids_are_short_and_plain() {
        assert!(RequestId::parse("d3b07384-d113-4ec6-a5b3-1f0d0a1b2c3d").is_ok());
        for bad in ["", "with space", "new\nline", "ünï", &"x".repeat(65)] {
            assert_eq!(RequestId::parse(bad), Err(InvalidRequestId), "{bad:?}");
        }
    }

    #[test]
    fn each_layout_takes_its_disks_and_its_keyword() {
        let all = [Layout::Single, Layout::Mirror, Layout::Raidz1, Layout::Raidz2, Layout::Raidz3];
        let keywords: Vec<Option<&str>> = all.iter().map(|layout| layout.keyword()).collect();
        assert_eq!(keywords, [None, Some("mirror"), Some("raidz1"), Some("raidz2"), Some("raidz3")]);
        let least: Vec<usize> = all.iter().map(|layout| *layout.disks().start()).collect();
        assert_eq!(least, [1, 2, 3, 4, 5], "one more than the disks it may lose");
        assert!(all.iter().all(|layout| *layout.disks().end() <= MAX_POOL_DISKS));
    }
}
