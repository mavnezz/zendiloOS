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
    /// The highest UID and GID ever handed out; see [`next_id`].
    pub high_water: PathBuf,
}

impl Default for AccountFiles {
    fn default() -> Self {
        Self {
            passwd: PathBuf::from("/etc/passwd"),
            group: PathBuf::from("/etc/group"),
            high_water: PathBuf::from("/etc/zendiloos/id-high-water"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdKind {
    User,
    Group,
}

impl IdKind {
    fn key(self) -> &'static str {
        match self {
            IdKind::User => "uid",
            IdKind::Group => "gid",
        }
    }
}

/// The next ID in the managed range: above every ID in use *and* every ID
/// handed out before. IDs are never reused, because files and ACL entries
/// keep the number: a new account must not inherit a deleted one's access.
fn next_id(database: &str, high_water: &str, kind: IdKind) -> Option<u32> {
    let in_use = database
        .lines()
        .filter_map(|line| line.split(':').nth(2)?.parse::<u32>().ok())
        .filter(|id| MANAGED_IDS.contains(id))
        .max();
    let handed_out = high_water
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{}=", kind.key()))?.trim().parse::<u32>().ok());
    let next = in_use.max(handed_out).map_or(ID_MIN, |highest| highest + 1);
    MANAGED_IDS.contains(&next).then_some(next)
}

fn set_high_water(high_water: &str, kind: IdKind, id: u32) -> String {
    let mut lines: Vec<String> = high_water
        .lines()
        .filter(|line| !line.starts_with(&format!("{}=", kind.key())) && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    lines.push(format!("{}={id}", kind.key()));
    lines.sort();
    format!("# The highest IDs zendiloos handed out; they are never reused.\n{}\n", lines.join("\n"))
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
        let marks = self.read_high_water()?;
        let uid = next_id(&passwd, &marks, IdKind::User)
            .ok_or_else(|| OperationError::Setup("no user IDs left".to_owned()))?
            .to_string();
        let args = [
            "--no-log-init",
            "--no-create-home",
            "--home-dir",
            "/nonexistent",
            "--shell",
            NOLOGIN,
            "--gid",
            USERS_GROUP,
            "--uid",
            &uid,
            "--",
            name.as_str(),
        ];
        self.write_high_water(&set_high_water(&marks, IdKind::User, uid.parse().unwrap_or(ID_MIN)))?;
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
        let marks = self.read_high_water()?;
        let gid = next_id(&group, &marks, IdKind::Group)
            .ok_or_else(|| OperationError::Setup("no group IDs left".to_owned()))?;
        self.write_high_water(&set_high_water(&marks, IdKind::Group, gid))?;
        run(&self.groupadd, &["--gid", &gid.to_string(), "--", name.as_str()]).await?;
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

    /// Missing until the first account is created.
    fn read_high_water(&self) -> Result<String, OperationError> {
        match std::fs::read_to_string(&self.files.high_water) {
            Ok(text) => Ok(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(OperationError::Read(self.files.high_water.clone(), error)),
        }
    }

    /// Written before the ID is used, so that even a failed creation does
    /// not lead to its reuse.
    fn write_high_water(&self, content: &str) -> Result<(), OperationError> {
        let path = &self.files.high_water;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
        }
        let temporary = path.with_extension("new");
        std::fs::write(&temporary, content).map_err(|e| OperationError::Write(temporary.clone(), e))?;
        std::fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.clone(), e))
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

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0::/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n\
                          alice:x:2001:990::/nonexistent:/usr/sbin/nologin\nnobody:x:65534:65534::/:/bin/false\n";

    #[test]
    fn ids_continue_above_everything_ever_handed_out() {
        assert_eq!(next_id("", "", IdKind::User), Some(2000));
        assert_eq!(next_id(PASSWD, "", IdKind::User), Some(2002), "ignores system and nobody IDs");
        // bob had 2005 and was deleted: his number stays taken.
        assert_eq!(next_id(PASSWD, "uid=2005\ngid=2100\n", IdKind::User), Some(2006));
        assert_eq!(next_id(PASSWD, "gid=2100\n", IdKind::Group), Some(2101));
        assert_eq!(next_id(PASSWD, &format!("uid={ID_MAX}"), IdKind::User), None, "range exhausted");
    }

    #[test]
    fn the_high_water_file_keeps_both_kinds() {
        let marks = set_high_water("", IdKind::User, 2003);
        let marks = set_high_water(&marks, IdKind::Group, 2001);
        let marks = set_high_water(&marks, IdKind::User, 2004);
        assert_eq!(marks.lines().skip(1).collect::<Vec<_>>(), ["gid=2001", "uid=2004"]);
    }
}
