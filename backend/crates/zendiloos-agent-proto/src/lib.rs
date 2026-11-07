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

mod apps;
mod client;
mod descriptor;
mod docker;
mod files;
mod framing;
mod network;
mod packages;
mod services;
mod ssh;
mod time;
mod web;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use apps::{
    AppCompose, AppFolder, AppManifest, AppName, AppSettingValue, AppShareChoice, AppValues, DevicePath,
    InvalidAppCompose, InvalidAppFolder, InvalidAppManifest, InvalidAppName, InvalidAppSettingValue,
    InvalidDevicePath, MAX_LOG_LINES,
};
pub use client::{AgentTransport, Report, TransportError, UnixTransport};
pub use descriptor::{MessageReader, read_message_with_file, write_message_with_file};
pub use docker::{
    ComposeText, ContainerAction, ContainerId, ImageId, ImageReference, InvalidDockerName, NetworkName,
    ProjectAction, ProjectConfig, ProjectDirectory, ProjectName, Subnet,
};
pub use files::{
    DirectoryEntry, EntryKind, FileLocation, FileRoot, HOMES_DIRECTORY, InvalidSharePath,
    MAX_DIRECTORY_ENTRIES, SharePath, is_file_name,
};
pub use framing::{FrameError, read_message, write_message};
pub use network::{
    ConfirmWithin, InterfaceName, InvalidConfirmWithin, InvalidInterfaceName, InvalidNameServers,
    InvalidPrefix, Ipv4Config, Ipv6Config, NameServers, NetworkConfig, PendingNetwork, Prefix,
};
pub use packages::{
    InvalidReleasePackage, PackageReport, PackageRun, PackageRunState, PackageTask, ReleasePackage,
    UpgradablePackage,
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
pub use web::TlsMinimum;

/// The recycle bin at a share's root: `.recycle/<user>` per user, as
/// Samba's `vfs_recycle` keeps it.
pub const RECYCLE_DIRECTORY: &str = ".recycle";

/// Agent and API come from the same release; any difference is refused.
pub const PROTOCOL_VERSION: u32 = 1;
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
    /// `lsblk` output for all block devices, columns [`LSBLK_COLUMNS`].
    ListBlockDevices {},
    /// `smartctl` output for one whole disk. Sleeping disks are not woken.
    SmartData {
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
    /// Starts a scrub of the pool.
    ScrubPool {
        pool: PoolName,
    },
    /// Stops the pool's scrub (`zpool scrub -s`).
    StopScrub {
        pool: PoolName,
    },
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
        /// For [`PackageTask::Update`] only: zendiloOS from this release.
        #[serde(default)]
        release: Option<ReleasePackage>,
    },
    /// The latest run of apt: [`Response::Packages`]. A run whose unit is
    /// gone without finishing counts as failed.
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
    /// Makes what an app needs before its compose file is read: its data
    /// folder, the dataset `<pool>/zendiloos.apps/<app>` mounted at
    /// `/volume<N>/@apps/<app>`, and its Linux user and group `app-<app>`.
    /// What exists already stays.
    PrepareApp {
        app: AppName,
        pool: PoolName,
        volume: VolumeNumber,
    },
    /// Checks the app against the catalog's format and policy with the
    /// values given, and keeps the normalized compose file the agent runs
    /// from then on. Answers [`Response::AppFolders`]: the folders below
    /// the data folder its containers mount, to be made by the files agent.
    ConfigureApp {
        app: AppName,
        volume: VolumeNumber,
        manifest: AppManifest,
        compose: AppCompose,
        values: AppValues,
    },
    /// Fetches the app's images.
    PullApp {
        app: AppName,
    },
    /// Starts (or updates) the app's containers, after checking that no
    /// folder it mounts has become a link.
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
    /// takes from the machine: the UID and primary GID of `user`, an
    /// account zendiloOS manages, which the containers run as; the time
    /// zone; and which of `devices` the machine has (at most
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
    /// could change them; administrators' projects deploy with
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
    /// Whether systemd-networkd manages the network, and a change that
    /// waits to be confirmed. Answers [`Response::NetworkState`].
    NetworkState {},
    /// Writes `config` as the interface's systemd-networkd file and applies
    /// it; unless [`Request::ConfirmNetwork`] comes within `confirm_within`,
    /// the old file returns (also after a restart). One change at a time.
    ApplyNetwork {
        config: NetworkConfig,
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
    /// The web interface's TLS settings zendiloOS wrote. Answers
    /// [`Response::WebTlsState`].
    WebTlsState {},
    /// Sets the oldest TLS version the web interface accepts and reloads
    /// nginx; connections already open stay.
    SetWebTls {
        minimum: TlsMinimum,
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
    /// while the initial setup is pending, the setup code. The agent writes
    /// the text itself; only the code comes from zendiloosd.
    ShowOnConsole {
        #[serde(default)]
        setup_code: Option<ConsoleSetupCode>,
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
    /// Sets owner, mode and POSIX ACLs of a share's directory: `access`
    /// gets read (and write); `removed` loses its entries. With
    /// `recursive`, also on everything in the share (role `files`).
    SetShareAccess {
        volume: VolumeNumber,
        name: ShareName,
        access: Vec<ShareAccess>,
        removed: Vec<Principal>,
        recursive: bool,
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
    },
    /// Renames or moves; between shares by copying and removing. `conflict`
    /// if `to` exists, unless `replace` and both are files.
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
    /// Makes `folders` below the app's data folder, without following a
    /// link, owned by the app's account.
    MakeAppFolders {
        app: AppName,
        volume: VolumeNumber,
        folders: Vec<AppFolder>,
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
            Request::SmartData { .. } => "smart_data",
            Request::StartSelfTest { .. } => "start_self_test",
            Request::AbortSelfTest { .. } => "abort_self_test",
            Request::ArrayStatus {} => "array_status",
            Request::ZfsStatus {} => "zfs_status",
            Request::CreatePool { .. } => "create_pool",
            Request::DestroyPool { .. } => "destroy_pool",
            Request::ReplaceDisk { .. } => "replace_disk",
            Request::ScrubPool { .. } => "scrub_pool",
            Request::StopScrub { .. } => "stop_scrub",
            Request::CreateSnapshot { .. } => "create_snapshot",
            Request::DestroySnapshot { .. } => "destroy_snapshot",
            Request::RollbackSnapshot { .. } => "rollback_snapshot",
            Request::DockerState {} => "docker_state",
            Request::EnableDocker { .. } => "enable_docker",
            Request::MoveDocker { .. } => "move_docker",
            Request::PrepareApp { .. } => "prepare_app",
            Request::ConfigureApp { .. } => "configure_app",
            Request::PullApp { .. } => "pull_app",
            Request::StartApp { .. } => "start_app",
            Request::StopApp { .. } => "stop_app",
            Request::RemoveApp { .. } => "remove_app",
            Request::DeleteAppAccount { .. } => "delete_app_account",
            Request::AppsStatus {} => "apps_status",
            Request::AppLogs { .. } => "app_logs",
            Request::HostFacts { .. } => "host_facts",
            Request::MakeAppFolders { .. } => "make_app_folders",
            Request::DockerContainers {} => "docker_containers",
            Request::DockerStats {} => "docker_stats",
            Request::DockerImages {} => "docker_images",
            Request::DockerNetworks {} => "docker_networks",
            Request::DockerDiskUsage {} => "docker_disk_usage",
            Request::ControlContainer { .. } => "control_container",
            Request::ContainerLogs { .. } => "container_logs",
            Request::RemoveImage { .. } => "remove_image",
            Request::PullImage { .. } => "pull_image",
            Request::PruneImages {} => "prune_images",
            Request::CreateNetwork { .. } => "create_network",
            Request::RemoveNetwork { .. } => "remove_network",
            Request::CheckCompose { .. } => "check_compose",
            Request::ControlProject { .. } => "control_project",
            Request::DeployProject { .. } => "deploy_project",
            Request::ProjectLogs { .. } => "project_logs",
            Request::ProjectFiles { .. } => "project_files",
            Request::NetworkState {} => "network_state",
            Request::ApplyNetwork { .. } => "apply_network",
            Request::ConfirmNetwork {} => "confirm_network",
            Request::SshState {} => "ssh_state",
            Request::StartPackages { .. } => "start_packages",
            Request::PackagesState {} => "packages_state",
            Request::SetSsh { .. } => "set_ssh",
            Request::WebTlsState {} => "web_tls_state",
            Request::SetWebTls { .. } => "set_web_tls",
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
            Request::SetShareAccess { .. } => "set_share_access",
            Request::CreateUser { .. } => "create_user",
            Request::DeleteUser { .. } => "delete_user",
            Request::SetSmbPassword { .. } => "set_smb_password",
            Request::SetUserEnabled { .. } => "set_user_enabled",
            Request::CreateGroup { .. } => "create_group",
            Request::DeleteGroup { .. } => "delete_group",
            Request::SetGroupMembers { .. } => "set_group_members",
            Request::ListDirectory { .. } => "list_directory",
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
    BlockDevices {
        lsblk_json: String,
    },
    Smart {
        /// smartctl's exit status: a bit mask, not a plain success flag.
        exit_status: i32,
        smartctl_json: String,
    },
    ArrayStatus {
        /// Empty if the kernel has no software RAID support loaded.
        mdstat: String,
    },
    Zfs {
        zpool_status: String,
        zfs_list: String,
    },
    Directory {
        entries: Vec<DirectoryEntry>,
        /// More than [`MAX_DIRECTORY_ENTRIES`]; the rest is left out.
        truncated: bool,
        /// The user may create and delete entries in it.
        writable: bool,
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
    },
    /// Folders below an app's data folder its containers mount.
    AppFolders {
        folders: Vec<AppFolder>,
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
    },
    /// What `docker` printed, for zendiloosd to read.
    DockerOutput {
        text: String,
    },
    WebTlsState {
        /// The settings file zendiloOS wrote; empty if none.
        config: String,
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
    Snapshots {
        /// `zfs list -H -p -t snapshot -o name,creation,used,referenced,`
        /// `zendiloos:schedule,zendiloos:note`.
        zfs_list: String,
    },
    ShareUsage {
        /// `zfs list -H -p -t filesystem -d 1 -o name,used,available`.
        zfs_list: String,
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
            Request::CopyEntry { .. }
            | Request::MoveEntry { .. }
            // Planning walks the whole folder.
            | Request::ArchiveFolder { .. }
            // Restarts Docker.
            | Request::EnableDocker { .. }
            | Request::MoveDocker { .. }
            // Fetch images, start or stop containers.
            | Request::PrepareApp { .. }
            | Request::PullApp { .. }
            | Request::StartApp { .. }
            | Request::StopApp { .. }
            | Request::RemoveApp { .. }
            | Request::DeployProject { .. }
            | Request::CreatePool { .. }
            | Request::DestroyPool { .. }
            | Request::ReplaceDisk { .. }
            | Request::DestroyShare { .. }
            // Images fetched, projects brought up or down: up to half an hour.
            | Request::PullImage { .. }
            | Request::PruneImages {}
            | Request::ControlProject { .. } => true,
            Request::SetShareAccess { recursive, .. } => *recursive,
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

    /// An administrator's change of a whole share's permissions: it has
    /// places of its own, so users' copies never hold it up for hours.
    pub fn is_walk(&self) -> bool {
        matches!(self, Request::SetShareAccess { recursive: true, .. })
    }

    pub fn role(&self) -> Role {
        match self {
            Request::SetShareAccess { .. }
            | Request::MakeHome { .. }
            | Request::ListDirectory { .. }
            | Request::OpenFile { .. }
            | Request::CreateUpload { .. }
            | Request::MakeDirectory { .. }
            | Request::MoveEntry { .. }
            | Request::CopyEntry { .. }
            | Request::RemoveEntry { .. }
            | Request::DiscardUpload { .. }
            | Request::RestoreEntry { .. }
            | Request::ArchiveFolder { .. }
            | Request::MakeAppFolders { .. } => Role::Files,
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

/// What a user or group may do on a share. A denied user gets an ACL entry
/// without permissions, which comes before every group entry; a denied
/// group is kept out by Samba (`invalid users`), its members in the file
/// system by entries of their own.
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
    /// Deleted files go to `.recycle/<user>` in the share.
    pub recycle_bin: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NfsClient {
    pub network: Network,
    pub write: bool,
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
        let access = |recursive| Request::SetShareAccess {
            volume,
            name: share.clone(),
            access: vec![],
            removed: vec![],
            recursive,
        };
        assert!(access(true).is_long());
        assert!(!access(false).is_long());
        assert!(access(true).is_walk() && !access(false).is_walk());
        assert_eq!(access(true).acting_user(), None, "an administrator's, in places of its own");
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
        let request =
            json!({ "name": "photos", "volume": 1, "access": [], "recycle_bin": false, "comment": "a\nb" });
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
            "smb": [{ "name": "photos", "volume": 1, "recycle_bin": true,
                      "access": [{ "principal": { "group": "family" }, "permission": "read_write" },
                                 { "principal": { "user": "guest-bob" }, "permission": "deny" }] }],
            "nfs": [{ "name": "photos", "volume": 1, "clients": [{ "network": "192.168.1.0/24", "write": false }] }],
            "services": { "smb": true, "nfs": true, "workgroup": "WORKGROUP", "smb_min_protocol": "smb2_1",
                "smb_encryption": "default", "smb_signing": "default", "hide_unreadable": true, "multichannel": true,
                "oplocks": true, "leases": true, "durable_handles": false, "mac_compat": true, "follow_symlinks": true,
                "veto_files": null, "delete_veto_files": false, "wsdd": true, "bonjour": true } } });
        assert!(serde_json::from_value::<Envelope>(request.clone()).is_ok());
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
