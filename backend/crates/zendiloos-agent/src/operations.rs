use std::io;
use std::os::fd::OwnedFd;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use zendiloos_agent_proto::{
    DiskName, ErrorCode, LSBLK_COLUMNS, PROTOCOL_VERSION, Request, Response, Role, ZFS_DATASET_COLUMNS,
};
use zendiloos_exec::{ExecError, Limits, Output, Program};
use zendiloos_fileops::FileError;

use crate::accounts::{AccountFiles, Accounts};
use crate::config::Tools;
use crate::files::Files;
use crate::pools::{Erasable, HostPaths, Pools};
use crate::sharing::Sharing;

/// Where the agent finds the host's account databases, devices and units;
/// other than the real places only in tests.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    pub accounts: AccountFiles,
    pub host: HostPaths,
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
    pools: Pools,
    sharing: Sharing,
    files: Files,
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
            files: Files::new(paths.accounts.clone(), paths.host.volumes.clone()),
            accounts: Accounts::new(tools, paths.accounts)?,
            sharing: Sharing::new(tools, paths.host.clone())?,
            pools: Pools::new(tools, erasable, paths.host)?,
            role,
        })
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
            Request::Reboot {} => self.systemctl("reboot").await,
            Request::PowerOff {} => self.systemctl("poweroff").await,
            Request::ListBlockDevices {} => self.list_block_devices().await,
            Request::SmartData { device } => self.smart_data(device).await,
            Request::ArrayStatus {} => array_status().await,
            Request::ZfsStatus {} => self.zfs_status().await,
            Request::CreatePool { name, layout, disks, volume } => {
                self.pools.create(name, *layout, disks, *volume).await
            }
            Request::DestroyPool { name, volume } => self.pools.destroy(name, *volume).await,
            Request::ReplaceDisk { pool, device, disk } => self.pools.replace(pool, *device, disk).await,
            Request::ScrubPool { pool } => self.pools.scrub(pool).await,
            Request::CreateShare { pool, volume, name } => self.pools.create_share(pool, *volume, name).await,
            Request::DestroyShare { pool, volume, name } => {
                self.pools.destroy_share(pool, *volume, name).await
            }
            Request::ApplySharing { smb, nfs } => self.sharing.apply(smb, nfs).await,
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
            | Request::OpenFile { .. }
            | Request::CreateUpload { .. }
            | Request::MakeDirectory { .. }
            | Request::MoveEntry { .. }
            | Request::CopyEntry { .. }
            | Request::RemoveEntry { .. } => {
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
}

impl OperationError {
    fn code(&self) -> ErrorCode {
        match self {
            OperationError::Conflict => ErrorCode::Conflict,
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
