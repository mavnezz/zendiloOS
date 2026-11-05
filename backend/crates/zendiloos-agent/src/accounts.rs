//! Linux users and groups for zendiloos accounts, and their Samba entries.
//!
//! The agent changes only what zendiloos manages: users with a UID from
//! [`ID_MIN`] whose primary group is [`USERS_GROUP`], and groups with a GID
//! from [`ID_MIN`]. System accounts and the owner's own user (UID 1000) stay
//! untouched, whatever zendiloosd asks for.

use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{GroupName, Password, Response, UserName};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::passwd;

/// Primary group of every managed user; created by the package (sysusers).
pub const USERS_GROUP: &str = "zendiloos-users";
pub const ID_MIN: u32 = 2000;
pub const ID_MAX: u32 = 59999;
const MANAGED_IDS: RangeInclusive<u32> = ID_MIN..=ID_MAX;
const NOLOGIN: &str = "/usr/sbin/nologin";

/// Where the account databases are; other than `/etc` only in tests.
#[derive(Debug, Clone)]
pub struct AccountFiles {
    pub passwd: PathBuf,
    pub group: PathBuf,
}

impl Default for AccountFiles {
    fn default() -> Self {
        Self { passwd: PathBuf::from("/etc/passwd"), group: PathBuf::from("/etc/group") }
    }
}

#[derive(Debug)]
pub(crate) struct Accounts {
    files: AccountFiles,
    useradd: Program,
    userdel: Program,
    groupadd: Program,
    groupdel: Program,
    gpasswd: Program,
    smbpasswd: Program,
    /// The shadow tools lock the account files themselves, but give up
    /// quickly when they find them locked; one change at a time avoids that.
    serial: tokio::sync::Mutex<()>,
}

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };

impl Accounts {
    pub(crate) fn new(tools: &Tools, files: AccountFiles) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            files,
            useradd: Program::new(&tools.useradd)?,
            userdel: Program::new(&tools.userdel)?,
            groupadd: Program::new(&tools.groupadd)?,
            groupdel: Program::new(&tools.groupdel)?,
            gpasswd: Program::new(&tools.gpasswd)?,
            smbpasswd: Program::new(&tools.smbpasswd)?,
            serial: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) async fn create_user(&self, name: &UserName) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let (passwd, group) = self.read_files().await?;
        if passwd::lookup(&passwd, name.as_str()).is_some() {
            return Err(OperationError::Conflict);
        }
        if passwd::group_id(&group, USERS_GROUP).is_none() {
            return Err(OperationError::Setup(format!("group {USERS_GROUP} is missing")));
        }
        let uid_min = format!("UID_MIN={ID_MIN}");
        let uid_max = format!("UID_MAX={ID_MAX}");
        let args = [
            "--no-log-init",
            "--no-create-home",
            "--home-dir",
            "/nonexistent",
            "--shell",
            NOLOGIN,
            "--gid",
            USERS_GROUP,
            "--key",
            &uid_min,
            "--key",
            &uid_max,
            "--",
            name.as_str(),
        ];
        run(&self.useradd, &args).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn delete_user(&self, name: &UserName) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed_user(name).await?;
        // The Samba entry may be missing, e.g. after a failed creation.
        if let Err(error) = run(&self.smbpasswd, &["-x", name.as_str()]).await {
            tracing::info!(user = name.as_str(), %error, "no Samba entry removed");
        }
        run(&self.userdel, &["--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn set_smb_password(
        &self,
        name: &UserName,
        password: &Password,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed_user(name).await?;
        // -s reads the new password twice from stdin, without prompts.
        let input = format!("{0}\n{0}\n", password.expose());
        zendiloos_exec::run_with_input(
            &self.smbpasswd,
            ["-s", "-a", name.as_str()],
            input.as_bytes(),
            LIMITS,
        )
        .await?
        .success(&self.smbpasswd)?;
        Ok(Response::Done)
    }

    pub(crate) async fn set_user_enabled(
        &self,
        name: &UserName,
        enabled: bool,
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        self.require_managed_user(name).await?;
        run(&self.smbpasswd, &[if enabled { "-e" } else { "-d" }, name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn create_group(&self, name: &GroupName) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let (_, group) = self.read_files().await?;
        if passwd::group_id(&group, name.as_str()).is_some() {
            return Err(OperationError::Conflict);
        }
        let gid_min = format!("GID_MIN={ID_MIN}");
        let gid_max = format!("GID_MAX={ID_MAX}");
        run(&self.groupadd, &["--key", &gid_min, "--key", &gid_max, "--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn delete_group(&self, name: &GroupName) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let (_, group) = self.read_files().await?;
        require_managed_group(&group, name)?;
        run(&self.groupdel, &["--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn set_group_members(
        &self,
        name: &GroupName,
        members: &[UserName],
    ) -> Result<Response, OperationError> {
        let _serial = self.serial.lock().await;
        let (passwd, group) = self.read_files().await?;
        require_managed_group(&group, name)?;
        for member in members {
            require_managed(&passwd, &group, member)?;
        }
        let list = members.iter().map(UserName::as_str).collect::<Vec<_>>().join(",");
        run(&self.gpasswd, &["--members", &list, "--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    async fn read_files(&self) -> Result<(String, String), OperationError> {
        let read = |path: &PathBuf| {
            let path = path.clone();
            async move { tokio::fs::read_to_string(&path).await.map_err(|e| OperationError::Read(path, e)) }
        };
        Ok((read(&self.files.passwd).await?, read(&self.files.group).await?))
    }

    async fn require_managed_user(&self, name: &UserName) -> Result<(), OperationError> {
        let (passwd, group) = self.read_files().await?;
        require_managed(&passwd, &group, name)
    }
}

fn require_managed(passwd: &str, group: &str, name: &UserName) -> Result<(), OperationError> {
    let users_gid = passwd::group_id(group, USERS_GROUP);
    match passwd::lookup(passwd, name.as_str()) {
        Some((uid, gid)) if MANAGED_IDS.contains(&uid) && Some(gid) == users_gid => Ok(()),
        _ => Err(OperationError::NotManaged(format!("user {}", name.as_str()))),
    }
}

fn require_managed_group(group: &str, name: &GroupName) -> Result<(), OperationError> {
    match passwd::group_id(group, name.as_str()) {
        Some(gid) if MANAGED_IDS.contains(&gid) && name.as_str() != USERS_GROUP => Ok(()),
        _ => Err(OperationError::NotManaged(format!("group {}", name.as_str()))),
    }
}

async fn run(program: &Program, args: &[&str]) -> Result<(), OperationError> {
    zendiloos_exec::run(program, args, LIMITS).await?.success(program)?;
    Ok(())
}
