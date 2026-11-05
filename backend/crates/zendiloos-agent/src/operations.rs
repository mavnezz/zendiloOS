use std::io;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use zendiloos_agent_proto::{DiskName, ErrorCode, LSBLK_COLUMNS, PROTOCOL_VERSION, Request, Response};
use zendiloos_exec::{ExecError, Limits, Output, Program};

use crate::accounts::{AccountFiles, Accounts};
use crate::config::Tools;

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
    accounts: Accounts,
}

impl Operations {
    pub fn new(tools: &Tools) -> Result<Self, ExecError> {
        Self::with_account_files(tools, AccountFiles::default())
    }

    /// Like [`Operations::new`], with the account databases elsewhere than
    /// in `/etc`. For tests.
    pub fn with_account_files(tools: &Tools, files: AccountFiles) -> Result<Self, ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            lsblk: Program::new(&tools.lsblk)?,
            smartctl: Program::new(&tools.smartctl)?,
            accounts: Accounts::new(tools, files)?,
        })
    }

    pub async fn execute(&self, request: &Request) -> Response {
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
        };
        result.unwrap_or_else(|error| Response::Error { code: error.code(), message: error.to_string() })
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
}

impl OperationError {
    fn code(&self) -> ErrorCode {
        match self {
            OperationError::Conflict => ErrorCode::Conflict,
            OperationError::NotManaged(_) => ErrorCode::Forbidden,
            _ => ErrorCode::Failed,
        }
    }
}
