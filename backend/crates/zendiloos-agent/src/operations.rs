use std::io;
use std::os::fd::OwnedFd;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use zendiloos_agent_proto::{
    DiskName, ErrorCode, LSBLK_COLUMNS, PROTOCOL_VERSION, ProjectAction, Request, Response, Role,
    SelfTestKind, ZFS_DATASET_COLUMNS,
};
use zendiloos_exec::{ExecError, Limits, Output, Program};
use zendiloos_fileops::FileError;

use crate::accounts::{AccountFiles, Accounts};
use crate::apps::Apps;
use crate::config::Tools;
use crate::docker::Docker;
use crate::files::Files;
use crate::network::{Network, NetworkPaths};
use crate::packages::Packages;
use crate::pools::{Erasable, HostPaths, Pools};
use crate::projects::Projects;
use crate::removable::Removable;
use crate::sharing::Sharing;
use crate::ssh::{Ssh, SshPaths};
use crate::time::{Time, TimePaths};
use crate::web::{Web, WebPaths};

/// Where the agent finds the host's account databases, devices and units;
/// other than the real places only in tests.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    pub accounts: AccountFiles,
    pub host: HostPaths,
    pub network: NetworkPaths,
    pub time: TimePaths,
    pub ssh: SshPaths,
    pub web: WebPaths,
}

const MDSTAT: &str = "/proc/mdstat";
/// Far more than any real number of arrays needs.
const MAX_MDSTAT: u64 = 1024 * 1024;

/// Carries out requests. Each operation runs fixed programs with fixed
/// arguments; values from a request reach a command line only as validated
/// protocol types.
#[derive(Debug)]
pub struct Operations {
    systemctl: Program,
    lsblk: Program,
    smartctl: Program,
    zpool: Program,
    zfs: Program,
    accounts: Accounts,
    apps: Apps,
    packages: Packages,
    pools: Pools,
    sharing: Sharing,
    files: Files,
    network: std::sync::Arc<Network>,
    time: Time,
    ssh: Ssh,
    web: Web,
    docker: Docker,
    projects: Projects,
    removable: Removable,
    console_issue: std::path::PathBuf,
    role: Role,
}

impl Operations {
    pub fn new(role: Role, tools: &Tools, erasable: Erasable) -> Result<Self, ExecError> {
        Self::with_paths(role, tools, erasable, Paths::default())
    }

    /// Like [`Operations::new`], looking at the host elsewhere. For tests.
    pub fn with_paths(
        role: Role,
        tools: &Tools,
        erasable: Erasable,
        paths: Paths,
    ) -> Result<Self, ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            lsblk: Program::new(&tools.lsblk)?,
            smartctl: Program::new(&tools.smartctl)?,
            zpool: Program::new(&tools.zpool)?,
            zfs: Program::new(&tools.zfs)?,
            files: Files::new(
                paths.accounts.clone(),
                paths.host.volumes.clone(),
                paths.host.removable.clone(),
            ),
            network: std::sync::Arc::new(Network::new(tools, paths.network)?),
            time: Time::new(tools, paths.time)?,
            ssh: Ssh::new(tools, paths.ssh)?,
            web: Web::new(tools, paths.web)?,
            docker: Docker::new(Program::new(&tools.docker)?, Program::new(&tools.setsid)?),
            projects: Projects::new(
                Program::new(&tools.docker)?,
                Program::new(&tools.systemctl)?,
                paths.host.clone(),
            ),
            removable: Removable::new(tools, paths.host.clone(), &paths.accounts)?,
            console_issue: paths.host.console_issue.clone(),
            sharing: Sharing::new(tools, paths.host.clone(), paths.accounts.clone())?,
            apps: Apps::new(
                Program::new(&tools.docker)?,
                Program::new(&tools.systemctl)?,
                paths.host.clone(),
                paths.accounts.clone(),
            ),
            packages: Packages::new(
                Program::new(&tools.systemctl)?,
                Program::new(&tools.systemd_run)?,
                paths.host.clone(),
            ),
            accounts: Accounts::new(tools, paths.accounts)?,
            pools: Pools::new(tools, erasable, paths.host)?,
            role,
        })
    }

    /// Work left from before a restart: network changes nobody confirmed
    /// go back. Call once, before serving.
    pub async fn recover(&self) {
        if self.role == Role::System {
            self.network.restore_unconfirmed().await;
        }
    }

    pub async fn execute(&self, request: &Request) -> Response {
        self.execute_with_file(request).await.0
    }

    /// The answer, and the file it carries, if any.
    pub async fn execute_with_file(&self, request: &Request) -> (Response, Option<OwnedFd>) {
        if request.role() != self.role && !matches!(request, Request::Hello {}) {
            let message = format!("{} is not for the {:?} agent", request.name(), self.role);
            return (Response::Error { code: ErrorCode::Forbidden, message }, None);
        }
        let result = match request {
            Request::Hello {} => Ok(Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            }),
            // systemctl returns once the job is queued; the shutdown follows.
            // Not while apt runs: dpkg cut off leaves packages half done,
            // a kernel maybe without its ZFS module.
            Request::Reboot {} | Request::PowerOff {} if self.packages.running().await => Err(
                OperationError::Refused("an update is being installed; wait until it has finished".into()),
            ),
            Request::Reboot {} => self.systemctl("reboot").await,
            Request::PowerOff {} => self.systemctl("poweroff").await,
            Request::ListBlockDevices {} => self.list_block_devices().await,
            Request::SmartData { device } => self.smart_data(device).await,
            Request::ArrayStatus {} => array_status().await,
            Request::StartSelfTest { device, kind } => {
                let argument = match kind {
                    SelfTestKind::Short => "--test=short",
                    SelfTestKind::Long => "--test=long",
                };
                self.self_test(device, argument).await
            }
            Request::AbortSelfTest { device } => self.self_test(device, "--abort").await,
            Request::ZfsStatus {} => self.zfs_status().await,
            Request::CreatePool { name, layout, disks, volume } => {
                self.pools.create(name, *layout, disks, *volume).await
            }
            Request::DestroyPool { name, volume } => self.pools.destroy(name, *volume).await,
            Request::ReplaceDisk { pool, device, disk } => self.pools.replace(pool, *device, disk).await,
            Request::ScrubPool { pool } => self.pools.scrub(pool).await,
            Request::StopScrub { pool } => self.pools.stop_scrub(pool).await,
            Request::MountRemovable { id } => self.removable.mount(id).await,
            Request::EjectRemovable { id, power_off } => self.removable.eject(id, *power_off).await,
            Request::CleanRemovable {} => self.removable.clean().await,
            Request::CreateSnapshot { pool, volume, share, label, schedules, note } => {
                self.pools.create_snapshot(pool, *volume, share, label, schedules, note.as_ref()).await
            }
            Request::DestroySnapshot { pool, volume, share, label } => {
                self.pools.destroy_snapshot(pool, *volume, share, label).await
            }
            Request::DockerState {} => self.pools.docker_state().await,
            Request::EnableDocker { pool, volume } => self.pools.enable_docker(pool, *volume).await,
            Request::MoveDocker { from_pool, from_volume, to_pool, to_volume } => {
                self.pools.move_docker(from_pool, *from_volume, to_pool, *to_volume).await
            }
            Request::StartApp { app } => self.apps.start(app).await,
            Request::StopApp { app } => self.apps.stop(app).await,
            Request::RemoveApp { app, pool, volume } => match self.apps.remove(app).await {
                Ok(()) => self.pools.destroy_app_dataset(pool, *volume, app).await.map(|()| Response::Done),
                Err(error) => Err(error),
            },
            Request::DeleteAppAccount { app } => self.accounts.delete_app_account(app).await,
            Request::AppsStatus {} => self.apps.status().await,
            Request::StartPackages { task, release } => self.packages.start(*task, release.as_ref()).await,
            Request::PackagesState {} => self.packages.state().await,
            Request::AppLogs { app, lines } => self.apps.logs(app, *lines).await,
            Request::HostFacts { user, devices } => self.apps.facts(user, devices),
            Request::NetworkState {} => self.network.state().await,
            Request::ApplyNetwork { config, confirm_within } => {
                self.network.apply(config, *confirm_within).await
            }
            Request::ConfirmNetwork {} => self.network.confirm().await,
            Request::SshState {} => self.ssh.state().await,
            Request::SetSsh { config } => self.ssh.set(config).await,
            Request::WebTlsState {} => self.web.state(),
            Request::DockerContainers {} => self.docker.containers().await,
            Request::DockerStats {} => self.docker.stats().await,
            Request::DockerImages {} => self.docker.images().await,
            Request::DockerNetworks {} => self.docker.networks().await,
            Request::DockerDiskUsage {} => self.docker.disk_usage().await,
            Request::ControlContainer { container, action } => {
                self.docker.control_container(container, *action).await
            }
            Request::ContainerLogs { container, lines } => {
                self.docker.container_logs(container, *lines).await
            }
            Request::InspectContainer { container } => self.docker.inspect_container(container).await,
            Request::ContainerProcesses { container } => self.docker.container_processes(container).await,
            Request::OpenTerminal { container, command, size } => {
                return match self.docker.open_terminal(container, command, *size).await {
                    Ok((response, master)) => (response, Some(master)),
                    Err(error) => (failure(&error), None),
                };
            }
            Request::RemoveImage { image } => self.docker.remove_image(image).await,
            Request::PullImage { reference } => self.docker.pull_image(reference).await,
            Request::PruneImages {} => self.docker.prune_images().await,
            Request::CreateNetwork { name, subnet, gateway } => {
                self.docker.create_network(name, subnet.as_ref(), *gateway).await
            }
            Request::RemoveNetwork { name } => self.docker.remove_network(name).await,
            Request::CheckCompose { project, directory, compose } => {
                self.docker.check_compose(project, directory, compose).await
            }
            Request::ControlProject { project, action } => {
                if *action == ProjectAction::Down && self.projects.is_kept(project) {
                    self.projects.down(project).await
                } else {
                    self.docker.control_project(project, *action).await
                }
            }
            Request::DeployProject { project, config, pull } => {
                self.projects.deploy(project, config, *pull).await
            }
            Request::ProjectLogs { project, lines } => self.docker.project_logs(project, *lines).await,
            Request::ProjectFiles { project } => self.docker.project_files(project).await,
            Request::SetWebTls { minimum } => self.web.set_tls(*minimum).await,
            Request::TimeState {} => self.time.state().await,
            Request::SetTime { zone, network_time, servers, clock } => {
                self.time.set(zone, *network_time, servers, *clock).await
            }
            Request::RollbackSnapshot { pool, volume, share, label } => {
                self.pools.rollback_snapshot(pool, *volume, share, label).await
            }
            Request::ListSnapshots {} => self.pools.list_snapshots().await,
            Request::SetShareProperties { pool, volume, share, quota_bytes, compression, record_size } => {
                self.pools
                    .set_share_properties(pool, *volume, share, *quota_bytes, *compression, *record_size)
                    .await
            }
            Request::ShareUsage {} => self.pools.share_usage().await,
            Request::ShowOnConsole { setup_code, open_setup } => {
                crate::console::show(&self.console_issue, setup_code.as_ref(), *open_setup)
            }
            Request::CreateShare { pool, volume, name } => self.pools.create_share(pool, *volume, name).await,
            Request::DestroyShare { pool, volume, name } => {
                self.pools.destroy_share(pool, *volume, name).await
            }
            Request::ApplySharing { smb, nfs, services, homes } => {
                self.sharing.apply(smb, nfs, services, homes.as_ref()).await
            }
            Request::PrepareHomes { pool, volume } => self.pools.prepare_homes(pool, *volume).await,
            Request::SetHomeQuota { pool, volume, user, quota_bytes } => {
                self.pools.set_home_quota(pool, *volume, user, *quota_bytes).await
            }
            Request::MakeHome { volume, user } => self.sharing.make_home(*volume, user),
            Request::SetShareAccess { volume, name, access, removed, recursive } => {
                self.sharing.set_access(*volume, name, access, removed, *recursive).await
            }
            Request::CreateUser { name } => self.accounts.create_user(name).await,
            Request::DeleteUser { name } => self.accounts.delete_user(name).await,
            Request::SetSmbPassword { name, password } => {
                self.accounts.set_smb_password(name, password).await
            }
            Request::SetUserEnabled { name, enabled } => self.accounts.set_user_enabled(name, *enabled).await,
            Request::CreateGroup { name } => self.accounts.create_group(name).await,
            Request::DeleteGroup { name } => self.accounts.delete_group(name).await,
            Request::SetGroupMembers { group, members } => {
                self.accounts.set_group_members(group, members).await
            }
            Request::ListDirectory { .. }
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
            | Request::ArchiveFolder { .. } => {
                return self.files.execute(request).await.unwrap_or_else(|error| (failure(&error), None));
            }
        };
        (result.unwrap_or_else(|error| failure(&error)), None)
    }

    async fn systemctl(&self, verb: &'static str) -> Result<Response, OperationError> {
        let limits = Limits { timeout: Duration::from_secs(30), ..Limits::default() };
        zendiloos_exec::run(&self.systemctl, [verb], limits).await?.success(&self.systemctl)?;
        Ok(Response::Done)
    }

    async fn list_block_devices(&self) -> Result<Response, OperationError> {
        let limits = Limits { timeout: Duration::from_secs(10), max_stdout: 4 * 1024 * 1024 };
        let output = zendiloos_exec::run(
            &self.lsblk,
            ["--json", "--bytes", "--tree", "--output", LSBLK_COLUMNS],
            limits,
        )
        .await?
        .success(&self.lsblk)?;
        Ok(Response::BlockDevices { lsblk_json: utf8(output)? })
    }

    /// Only whole disks the kernel has; smartctl returns at once while the
    /// drive tests itself.
    async fn self_test(&self, device: &DiskName, argument: &str) -> Result<Response, OperationError> {
        let path = format!("/dev/{}", device.as_str());
        if !std::path::Path::new(&path).exists() {
            return Err(OperationError::Refused(format!("there is no disk {}", device.as_str())));
        }
        let limits = Limits { timeout: Duration::from_secs(60), ..Limits::default() };
        zendiloos_exec::run(&self.smartctl, [argument, path.as_str()], limits)
            .await?
            .success(&self.smartctl)?;
        Ok(Response::Done)
    }

    /// smartctl's exit status is a bit mask that also reports findings
    /// (e.g. "error log contains errors"), so it is passed on, not judged.
    async fn smart_data(&self, device: &DiskName) -> Result<Response, OperationError> {
        let limits = Limits { timeout: Duration::from_secs(60), max_stdout: 4 * 1024 * 1024 };
        let path = format!("/dev/{}", device.as_str());
        let output = zendiloos_exec::run(
            &self.smartctl,
            ["--json=c", "--all", "--nocheck=standby", path.as_str()],
            limits,
        )
        .await?;
        let exit_status = output.status.code().ok_or(OperationError::Killed("smartctl"))?;
        Ok(Response::Smart { exit_status, smartctl_json: utf8(output)? })
    }
}

/// Read rather than run: the file is the interface `mdadm` itself uses.
async fn array_status() -> Result<Response, OperationError> {
    let mut mdstat = String::new();
    match tokio::fs::File::open(MDSTAT).await {
        Ok(file) => {
            file.take(MAX_MDSTAT).read_to_string(&mut mdstat).await.map_err(OperationError::Mdstat)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(OperationError::Mdstat(error)),
    }
    Ok(Response::ArrayStatus { mdstat })
}

impl Operations {
    async fn zfs_status(&self) -> Result<Response, OperationError> {
        let limits = Limits { timeout: Duration::from_secs(30), max_stdout: 8 * 1024 * 1024 };
        let status = zendiloos_exec::run(&self.zpool, ["status", "-j", "--json-int"], limits)
            .await?
            .success(&self.zpool)?;
        let list = zendiloos_exec::run(
            &self.zfs,
            ["list", "-j", "--json-int", "-d", "0", "-o", ZFS_DATASET_COLUMNS],
            limits,
        )
        .await?
        .success(&self.zfs)?;
        Ok(Response::Zfs { zpool_status: utf8(status)?, zfs_list: utf8(list)? })
    }
}

fn failure(error: &OperationError) -> Response {
    Response::Error { code: error.code(), message: error.to_string() }
}

fn utf8(output: Output) -> Result<String, OperationError> {
    String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OperationError {
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error("{0} was killed by a signal")]
    Killed(&'static str),
    #[error("the output is not UTF-8")]
    NotUtf8,
    #[error("cannot read {MDSTAT}")]
    Mdstat(#[source] io::Error),
    #[error("cannot read {}", .0.display())]
    Read(std::path::PathBuf, #[source] io::Error),
    #[error("the name is already taken")]
    Conflict,
    /// What is to be changed is in use, e.g. files open on a drive.
    #[error("in use: {0}")]
    Busy(String),
    #[error("{0} is not managed by zendiloos")]
    NotManaged(String),
    #[error("the system is not set up for this: {0}")]
    Setup(String),
    /// A safety check refused the operation.
    #[error("refused: {0}")]
    Refused(String),
    #[error("cannot write {}", .0.display())]
    Write(std::path::PathBuf, #[source] io::Error),
    #[error("cannot take on the user's credentials")]
    Credentials(#[source] io::Error),
    #[error(transparent)]
    File(zendiloos_fileops::FileError),
    /// A program did not do what was asked, in its words.
    #[error("{0}")]
    Failed(String),
    /// The request did not get its turn (see `waiting`): not carried out.
    #[error("not carried out: {0}")]
    NotNow(String),
}

impl OperationError {
    fn code(&self) -> ErrorCode {
        match self {
            OperationError::Conflict | OperationError::Busy(_) => ErrorCode::Conflict,
            OperationError::NotManaged(_) | OperationError::Refused(_) => ErrorCode::Forbidden,
            OperationError::File(error) => match error {
                FileError::NotFound => ErrorCode::NotFound,
                FileError::Denied => ErrorCode::Denied,
                FileError::Exists => ErrorCode::Conflict,
                FileError::Invalid(_) => ErrorCode::InvalidRequest,
                FileError::Io(_) => ErrorCode::Failed,
            },
            _ => ErrorCode::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_has_the_code_zendiloosd_acts_on() {
        let io = || io::Error::other("x");
        for (error, code) in [
            (OperationError::Conflict, ErrorCode::Conflict),
            (OperationError::Busy("files open".into()), ErrorCode::Conflict),
            (OperationError::NotManaged("user x".into()), ErrorCode::Forbidden),
            (OperationError::Refused("no".into()), ErrorCode::Forbidden),
            (OperationError::File(FileError::NotFound), ErrorCode::NotFound),
            (OperationError::File(FileError::Denied), ErrorCode::Denied),
            (OperationError::File(FileError::Exists), ErrorCode::Conflict),
            (OperationError::File(FileError::Invalid("odd")), ErrorCode::InvalidRequest),
            (OperationError::File(FileError::Io(io())), ErrorCode::Failed),
            (OperationError::Credentials(io()), ErrorCode::Failed),
            (OperationError::NotNow("later".into()), ErrorCode::Failed),
        ] {
            assert_eq!(error.code(), code, "{error}");
        }
    }

    #[test]
    fn programs_are_named_by_absolute_paths_only() {
        let none = Erasable::Policy(crate::pools::ErasePolicy::None);
        assert!(Operations::new(Role::System, &Tools::default(), none.clone()).is_ok());
        let relative = Tools { lsblk: "lsblk".into(), ..Tools::default() };
        assert!(Operations::new(Role::System, &relative, none).is_err(), "found through no PATH");
    }
}
