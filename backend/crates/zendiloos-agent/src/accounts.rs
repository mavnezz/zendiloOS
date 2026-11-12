//! Linux users and groups for zendiloos accounts, and their Samba entries;
//! and the account each app runs as.
//!
//! The agent changes only what zendiloos manages: users with a UID from
//! [`ID_MIN`] whose primary group is [`USERS_GROUP`], and groups with a GID
//! from [`ID_MIN`]. System accounts and the owner's own user (UID 1000) stay
//! untouched, whatever zendiloosd asks for. An app's account (`app-<id>`,
//! with [`APP_COMMENT`] and a primary group of the same name) is neither a
//! managed user nor a managed group: only the app requests touch it.

use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{AppName, GroupName, Password, Response, RestoredGroup, RestoredUser, UserName};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::passwd;

/// Primary group of every managed user; created by the package (sysusers).
pub const USERS_GROUP: &str = "zendiloos-users";
/// The system user and group the shares' files are given to, created by
/// the package (sysusers): every share's folder lets this group read and
/// write, NFS clients are squashed to it and apps run in it
/// (docs/architecture.md, 4.4). No account of a person.
pub const FILES_GROUP: &str = "zendiloos-files";
pub const ID_MIN: u32 = 2000;
pub const ID_MAX: u32 = 59999;
const MANAGED_IDS: RangeInclusive<u32> = ID_MIN..=ID_MAX;
const NOLOGIN: &str = "/usr/sbin/nologin";
/// The comment of every app's account: how the agent tells them apart.
pub const APP_COMMENT: &str = "zendiloOS app";

/// UID and GID of `app`'s account, if zendiloos made it: user `app-<id>`
/// with a managed UID and [`APP_COMMENT`], whose primary group has the same
/// name and a managed GID.
pub(crate) fn app_account(passwd: &str, group: &str, app: &AppName) -> Option<(u32, u32)> {
    let name = app.account();
    let fields: Vec<&str> =
        passwd.lines().find(|line| line.split(':').next() == Some(name.as_str()))?.split(':').collect();
    let (uid, gid) = (fields.get(2)?.parse::<u32>().ok()?, fields.get(3)?.parse::<u32>().ok()?);
    let ours = MANAGED_IDS.contains(&uid)
        && MANAGED_IDS.contains(&gid)
        && fields.get(4) == Some(&APP_COMMENT)
        && passwd::group_id(group, &name) == Some(gid);
    ours.then_some((uid, gid))
}

/// Whether `name` is the group of an app's account.
fn is_app_group(passwd: &str, group: &str, name: &str) -> bool {
    name.strip_prefix(AppName::ACCOUNT_PREFIX)
        .and_then(|app| AppName::parse(app).ok())
        .is_some_and(|app| app_account(passwd, group, &app).is_some())
}

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

/// The highest ID of `kind` handed out, as the high-water file says.
fn handed_out(high_water: &str, kind: IdKind) -> Option<u32> {
    high_water.lines().find_map(|line| line.strip_prefix(&format!("{}=", kind.key()))?.trim().parse().ok())
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
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
        let (passwd, group) = self.read_files().await?;
        // Gone already (a deletion whose answer was lost): deleted, so a
        // second try can end what the first began.
        if passwd::lookup(&passwd, name.as_str()).is_none() {
            return Ok(Response::Done);
        }
        require_managed(&passwd, &group, name)?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_managed_user(name).await?;
        run(&self.smbpasswd, &[if enabled { "-e" } else { "-d" }, name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn create_group(&self, name: &GroupName) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
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
        let _serial = crate::waiting::lock(&self.serial).await?;
        let (passwd, group) = self.read_files().await?;
        if passwd::group_id(&group, name.as_str()).is_none() {
            return Ok(Response::Done);
        }
        require_managed_group(&passwd, &group, name)?;
        run(&self.groupdel, &["--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn set_group_members(
        &self,
        name: &GroupName,
        members: &[UserName],
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let (passwd, group) = self.read_files().await?;
        require_managed_group(&passwd, &group, name)?;
        for member in members {
            require_managed(&passwd, &group, member)?;
        }
        let list = members.iter().map(UserName::as_str).collect::<Vec<_>>().join(",");
        run(&self.gpasswd, &["--members", &list, "--", name.as_str()]).await?;
        Ok(Response::Done)
    }

    /// Deletes `app`'s account and its group; nothing if it is gone.
    pub(crate) async fn delete_app_account(&self, app: &AppName) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let (passwd, group) = self.read_files().await?;
        let name = app.account();
        if app_account(&passwd, &group, app).is_none() {
            return match passwd::lookup(&passwd, &name) {
                Some(_) => Err(OperationError::NotManaged(format!("user {name}"))),
                None => Ok(Response::Done),
            };
        }
        run(&self.userdel, &["--", &name]).await?;
        // userdel removes the group itself where USERGROUPS_ENAB is set.
        let (_, group) = self.read_files().await?;
        if passwd::group_id(&group, &name).is_some() {
            run(&self.groupdel, &["--", &name]).await?;
        }
        Ok(Response::Done)
    }

    /// The accounts of a configuration backup on this machine: each user
    /// and group made with the ID it had, where it is missing, and each
    /// group given its members. Checked first as a whole: a name that is
    /// another ID's here, or an ID another name has, refuses it before
    /// anything changes. The IDs handed out rise to the highest restored,
    /// so none is handed out again.
    pub(crate) async fn restore(
        &self,
        users: &[RestoredUser],
        groups: &[RestoredGroup],
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let (passwd, group) = self.read_files().await?;
        let users_gid = passwd::group_id(&group, USERS_GROUP)
            .ok_or_else(|| OperationError::Setup(format!("group {USERS_GROUP} is missing")))?;
        let restored_names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
        let mut make_users = Vec::new();
        for user in users {
            let taken = passwd::accounts(&passwd)
                .find(|(name, uid, _)| *uid == user.uid && *name != user.name.as_str());
            if let Some((other, ..)) = taken {
                return Err(OperationError::Refused(format!(
                    "UID {} of {} is {other}'s here",
                    user.uid,
                    user.name.as_str()
                )));
            }
            match passwd::lookup(&passwd, user.name.as_str()) {
                Some((uid, gid)) if uid == user.uid && gid == users_gid => {}
                Some(_) => {
                    return Err(OperationError::Refused(format!(
                        "user {} has another UID or group here",
                        user.name.as_str()
                    )));
                }
                None => make_users.push(user),
            }
        }
        let mut make_groups = Vec::new();
        for restored in groups {
            if restored.members.iter().any(|member| {
                !restored_names.contains(&member.as_str())
                    && passwd::lookup(&passwd, member.as_str()).is_none()
            }) {
                return Err(OperationError::Refused(format!(
                    "a member of group {} has no user",
                    restored.name.as_str()
                )));
            }
            let taken = group.lines().find_map(|line| {
                let mut fields = line.split(':');
                let (name, _, gid) = (fields.next()?, fields.next()?, fields.next()?.parse::<u32>().ok()?);
                (gid == restored.gid && name != restored.name.as_str()).then(|| name.to_owned())
            });
            if let Some(other) = taken {
                return Err(OperationError::Refused(format!(
                    "GID {} of {} is {other}'s here",
                    restored.gid,
                    restored.name.as_str()
                )));
            }
            match passwd::group_id(&group, restored.name.as_str()) {
                Some(gid) if gid == restored.gid => {}
                Some(_) => {
                    return Err(OperationError::Refused(format!(
                        "group {} has another GID here",
                        restored.name.as_str()
                    )));
                }
                None => make_groups.push(restored),
            }
        }
        let mut marks = self.read_high_water()?;
        let highest = [
            (IdKind::User, users.iter().map(|user| user.uid).max()),
            (IdKind::Group, groups.iter().map(|group| group.gid).max()),
        ];
        for (kind, restored) in highest {
            if let Some(restored) =
                restored.filter(|id| handed_out(&marks, kind).is_none_or(|mark| *id > mark))
            {
                marks = set_high_water(&marks, kind, restored);
            }
        }
        self.write_high_water(&marks)?;
        for user in make_users {
            let uid = user.uid.to_string();
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
                user.name.as_str(),
            ];
            run(&self.useradd, &args).await?;
        }
        for restored in &make_groups {
            run(&self.groupadd, &["--gid", &restored.gid.to_string(), "--", restored.name.as_str()]).await?;
        }
        for restored in groups {
            let list = restored.members.iter().map(UserName::as_str).collect::<Vec<_>>().join(",");
            run(&self.gpasswd, &["--members", &list, "--", restored.name.as_str()]).await?;
        }
        Ok(Response::Done)
    }

    /// The people's users and groups zendiloOS made, with their IDs and
    /// members, for a configuration backup.
    pub(crate) async fn managed(&self) -> Result<(Vec<RestoredUser>, Vec<RestoredGroup>), OperationError> {
        let (passwd, group) = self.read_files().await?;
        let users: Vec<RestoredUser> = passwd::accounts(&passwd)
            .filter_map(|(name, uid, _)| {
                let name = UserName::parse(name).ok()?;
                require_managed(&passwd, &group, &name).ok()?;
                Some(RestoredUser { name, uid })
            })
            .collect();
        let groups = group
            .lines()
            .filter_map(|line| {
                let mut fields = line.split(':');
                let (name, _, gid) = (fields.next()?, fields.next()?, fields.next()?.parse::<u32>().ok()?);
                let name = GroupName::parse(name).ok()?;
                require_managed_group(&passwd, &group, &name).ok()?;
                let members = passwd::members(&group, name.as_str())
                    .into_iter()
                    .filter(|member| users.iter().any(|user| user.name.as_str() == *member))
                    .filter_map(|member| UserName::parse(member).ok())
                    .collect();
                Some(RestoredGroup { name, gid, members })
            })
            .collect();
        Ok((users, groups))
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
        // Lost in a power cut, a deleted account's IDs could be handed out again.
        crate::durable::replace(path, content.as_bytes())
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

pub(crate) fn require_managed(passwd: &str, group: &str, name: &UserName) -> Result<(), OperationError> {
    let users_gid = passwd::group_id(group, USERS_GROUP);
    match passwd::lookup(passwd, name.as_str()) {
        Some((uid, gid)) if MANAGED_IDS.contains(&uid) && Some(gid) == users_gid => Ok(()),
        _ => Err(OperationError::NotManaged(format!("user {}", name.as_str()))),
    }
}

fn require_managed_group(passwd: &str, group: &str, name: &GroupName) -> Result<(), OperationError> {
    match passwd::group_id(group, name.as_str()) {
        Some(gid)
            if MANAGED_IDS.contains(&gid)
                && name.as_str() != USERS_GROUP
                && !is_app_group(passwd, group, name.as_str()) =>
        {
            Ok(())
        }
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
    fn app_accounts_are_told_apart_from_people_and_groups() {
        let passwd = format!(
            "{PASSWD}app-jellyfin:x:2010:2011:{APP_COMMENT}:/nonexistent:/usr/sbin/nologin\n\
             app-paperless:x:2012:990::/nonexistent:/usr/sbin/nologin\n"
        );
        let group = "zendiloos-users:x:990:\napp-jellyfin:x:2011:\nfamily:x:2002:alice\n";
        let jellyfin = AppName::parse("jellyfin").unwrap();
        assert_eq!(app_account(&passwd, group, &jellyfin), Some((2010, 2011)));
        // A person called app-paperless is no app's account.
        assert_eq!(app_account(&passwd, group, &AppName::parse("paperless").unwrap()), None);

        let app_group = GroupName::parse("app-jellyfin").unwrap();
        assert!(
            require_managed_group(&passwd, group, &app_group).is_err(),
            "only the app requests change it"
        );
        assert!(require_managed_group(&passwd, group, &GroupName::parse("family").unwrap()).is_ok());
        let user = UserName::parse("app-jellyfin").unwrap();
        assert!(require_managed(&passwd, group, &user).is_err(), "not a person's account");
    }

    #[test]
    fn the_high_water_file_keeps_both_kinds() {
        let marks = set_high_water("", IdKind::User, 2003);
        let marks = set_high_water(&marks, IdKind::Group, 2001);
        let marks = set_high_water(&marks, IdKind::User, 2004);
        assert_eq!(marks.lines().skip(1).collect::<Vec<_>>(), ["gid=2001", "uid=2004"]);
    }
}
