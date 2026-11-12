//! Backup tasks (docs/architecture.md, 4.14): rsync between a folder of a
//! share and another place on the NAS (a share, a removable drive) or an
//! rsync server, through a daemon (`rsync://`) or SSH; rclone to Google
//! Drive and OneDrive, and wherever a task is encrypted (rclone's crypt).
//! Every value that reaches rsync's, rclone's or ssh's command line is one
//! of these types, checked while the request is read; a secret never
//! reaches one.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{RemovableId, ShareLocation, SharePath};

/// Where the backup plugin keeps the NAS's key pair for SSH servers, root's
/// alone (0700): `id_ed25519` and `id_ed25519.pub`.
pub const BACKUP_STATE: &str = "/var/lib/zendiloos-backup";
/// The NAS's private key for SSH servers, in [`BACKUP_STATE`].
pub const BACKUP_KEY: &str = "id_ed25519";
/// How the agent's refusal begins when an SSH server's host key is not the
/// one pinned: zendiloosd tells it apart by this.
pub const BACKUP_HOST_KEY_CHANGED: &str = "the server's host key changed";
/// The most exclude patterns, paths of a restore, and names pruned at once.
pub const BACKUP_EXCLUDES_MAX: usize = 64;
pub const BACKUP_RESTORE_PATHS_MAX: usize = 100;
pub const BACKUP_PRUNE_MAX: usize = 1000;

macro_rules! checked {
    ($(#[$meta:meta])* $name:ident, $invalid:ident, $what:literal, $check:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
        #[error($what)]
        pub struct $invalid;

        impl $name {
            pub fn parse(value: &str) -> Result<Self, $invalid> {
                let check: fn(&str) -> bool = $check;
                if check(value) { Ok(Self(value.to_owned())) } else { Err($invalid) }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $invalid;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

checked!(
    /// A server's host name or IP address: letters, digits, `.`, `-` and
    /// `:`, not starting with `-` or `.`, at most 253.
    BackupHostName,
    InvalidBackupHostName,
    "not a server's host name or address",
    |host| {
        (1..=253).contains(&host.len())
            && !host.starts_with(['-', '.'])
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
    }
);

checked!(
    /// A user on a server, or an rsync daemon's module: letters, digits,
    /// `.`, `_` and `-`, not starting with `-` or `.`, at most 64.
    BackupWord,
    InvalidBackupWord,
    "not a user or module name rsync can be given",
    |word| {
        (1..=64).contains(&word.len())
            && !word.starts_with(['-', '.'])
            && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }
);

checked!(
    /// A folder on an rsync server: names separated by `/` as in a share
    /// ([`SharePath`]), relative (to the daemon's module, or the SSH user's
    /// home) or starting with `/`; never starting with `-`, at most 1024.
    BackupRemotePath,
    InvalidBackupRemotePath,
    "not a folder on a server",
    |path| {
        let relative = path.strip_prefix('/').unwrap_or(path);
        path.len() <= 1024
            && !path.starts_with('-')
            && !path.chars().any(char::is_control)
            && SharePath::parse(relative).is_ok()
    }
);

checked!(
    /// An SSH server's host key as OpenSSH writes it: its type and the key
    /// in base64.
    BackupHostKey,
    InvalidBackupHostKey,
    "not an SSH host key",
    |line| {
        let mut words = line.split(' ');
        let (Some(kind), Some(key), None) = (words.next(), words.next(), words.next()) else { return false };
        matches!(
            kind,
            "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" | "ssh-rsa"
        ) && (16..=4096).contains(&key.len())
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    }
);

checked!(
    /// An exclude pattern as rsync reads one: one line, at most 255 bytes,
    /// none of the prefixes rsync reads as a rule of its own (`+ `, `- `,
    /// `!`), no spaces at either end.
    BackupPattern,
    InvalidBackupPattern,
    "not an exclude pattern rsync can be given",
    |pattern| {
        (1..=255).contains(&pattern.len())
            && !pattern.chars().any(char::is_control)
            && pattern.trim() == pattern
            && !pattern.starts_with("+ ")
            && !pattern.starts_with("- ")
            && !pattern.starts_with('!')
    }
);

checked!(
    /// A version's name: when its run started, in UTC,
    /// `2026-11-11_030000Z`.
    BackupStamp,
    InvalidBackupStamp,
    "not a version's name",
    is_stamp
);

checked!(
    /// What retention removes from a task's folder: a version, a version cut
    /// off (`<stamp>.incomplete`), or a mirror's kept changes
    /// (`removed/<stamp>`); nothing else.
    BackupPruned,
    InvalidBackupPruned,
    "not a version, a version cut off or kept changes",
    |name| {
        is_stamp(name)
            || name.strip_suffix(".incomplete").is_some_and(is_stamp)
            || name.strip_prefix("removed/").is_some_and(is_stamp)
    }
);

checked!(
    /// The folder a task's backup goes into within its target: one name, as
    /// in a share, not starting with `.` or `-`.
    BackupFolder,
    InvalidBackupFolder,
    "not a task's folder",
    |name| crate::is_file_name(name) && !name.starts_with(['.', '-']) && !name.chars().any(char::is_control)
);

/// Whether `name` is `YYYY-MM-DD_HHMMSSZ`, digits where digits are.
pub fn is_stamp(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 18
        && b.iter().enumerate().all(|(at, c)| match at {
            4 | 7 => *c == b'-',
            10 => *c == b'_',
            17 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

/// An rsync daemon's password: one line, at most 256 bytes. Never shown in
/// logs, never on a command line: it reaches rsync through a file only the
/// run's unit can read.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BackupPassword(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a password is one line of up to 256 bytes")]
pub struct InvalidBackupPassword;

impl BackupPassword {
    pub fn new(value: &str) -> Result<Self, InvalidBackupPassword> {
        let valid = (1..=256).contains(&value.len()) && !value.chars().any(char::is_control);
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidBackupPassword) }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BackupPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BackupPassword(…)")
    }
}

impl TryFrom<String> for BackupPassword {
    type Error = InvalidBackupPassword;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<BackupPassword> for String {
    fn from(value: BackupPassword) -> Self {
        value.0
    }
}

/// A secret rclone is given: a cloud's OAuth token (JSON), its own
/// client's secret, the crypt's password and salt. One line of up to
/// 16 KiB; never shown in logs, never on a command line.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BackupSecret(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a secret is one line of up to 16 KiB")]
pub struct InvalidBackupSecret;

impl BackupSecret {
    pub fn new(value: &str) -> Result<Self, InvalidBackupSecret> {
        let valid = (1..=16 * 1024).contains(&value.len()) && !value.chars().any(char::is_control);
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidBackupSecret) }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BackupSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BackupSecret(…)")
    }
}

impl TryFrom<String> for BackupSecret {
    type Error = InvalidBackupSecret;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<BackupSecret> for String {
    fn from(value: BackupSecret) -> Self {
        value.0
    }
}

checked!(
    /// What names a cloud's client or drive for rclone: an OAuth client's
    /// id, OneDrive's drive id and type. Letters, digits and `._-!~`, at
    /// most 256.
    BackupCloudWord,
    InvalidBackupCloudWord,
    "not a client's or drive's id rclone can be given",
    |word| {
        (1..=256).contains(&word.len())
            && !word.starts_with('-')
            && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-!~".contains(&b))
    }
);

/// A cloud a backup goes to, through rclone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupCloud {
    GoogleDrive,
    #[serde(rename = "onedrive")]
    OneDrive,
}

/// rclone's crypt: names and contents of a task's folder encrypted with
/// its password, and its salt (rclone's `password2`) if given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupCrypt {
    pub password: BackupSecret,
    #[serde(default)]
    pub salt: Option<BackupSecret>,
}

/// How rclone reaches a cloud account: the token zendiloosd keeps, and
/// the administrator's own client if given (else rclone's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupCloudAccess {
    pub token: BackupSecret,
    #[serde(default)]
    pub client_id: Option<BackupCloudWord>,
    #[serde(default)]
    pub client_secret: Option<BackupSecret>,
    /// OneDrive's drive.
    #[serde(default)]
    pub drive_id: Option<BackupCloudWord>,
    #[serde(default)]
    pub drive_type: Option<BackupCloudWord>,
}

/// What a backup's unit needs to reach its places, on its standard input:
/// a daemon's password, the crypt's, a cloud's access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSecrets {
    #[serde(default)]
    pub password: Option<BackupPassword>,
    /// The task's folder is encrypted with rclone's crypt.
    #[serde(default)]
    pub crypt: Option<BackupCrypt>,
    #[serde(default)]
    pub cloud: Option<BackupCloudAccess>,
}

/// A task, by its number in zendiloosd: it names the task's units, so
/// systemd runs one of each at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BackupTaskId(u32);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a task's number")]
pub struct InvalidBackupTaskId;

impl BackupTaskId {
    pub fn new(id: u32) -> Result<Self, InvalidBackupTaskId> {
        if id > 0 { Ok(Self(id)) } else { Err(InvalidBackupTaskId) }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for BackupTaskId {
    type Error = InvalidBackupTaskId;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BackupTaskId> for u32 {
    fn from(id: BackupTaskId) -> Self {
        id.0
    }
}

/// A place on the NAS a backup reads or writes, by its root: a share
/// (`/volume<N>/<share>`, which must be mounted) or a removable drive
/// (`/run/zendiloos-removable/<id>`, which must be mounted there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupRoot {
    Share { location: ShareLocation },
    Drive { drive: RemovableId },
}

/// How the NAS reaches an rsync server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupTransport {
    /// `rsync://user@host:port/module/…`, the password through a file.
    Daemon { module: BackupWord },
    /// rsync over SSH with the NAS's key, the server's host key pinned:
    /// another is refused.
    Ssh { host_key: BackupHostKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupServer {
    pub transport: BackupTransport,
    pub host: BackupHostName,
    pub port: u16,
    pub user: BackupWord,
}

/// One end of a backup: a folder on the NAS beneath a root, on a server,
/// or in a cloud (as a target, and restored from).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupEnd {
    Nas {
        root: BackupRoot,
        path: SharePath,
    },
    Remote {
        server: BackupServer,
        path: BackupRemotePath,
    },
    /// From the account's root (its access in [`BackupSecrets::cloud`]).
    Cloud {
        cloud: BackupCloud,
        path: SharePath,
    },
}

impl BackupEnd {
    /// Off the NAS: a server or a cloud.
    pub fn is_remote(&self) -> bool {
        !matches!(self, Self::Nas { .. })
    }

    pub fn server(&self) -> Option<&BackupServer> {
        match self {
            Self::Remote { server, .. } => Some(server),
            _ => None,
        }
    }
}

/// How a run writes the task's folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupMode {
    /// `current/`, deletions included; with `keep_changed`, what the run
    /// deletes or replaces goes to `removed/<stamp>/`.
    Mirror { keep_changed: bool },
    /// `<stamp>.incomplete/`, files unchanged since the newest complete
    /// version hard links to it, named `<stamp>` once complete.
    Versioned,
}

/// One run of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRun {
    pub task: BackupTaskId,
    /// A folder of a share, or of a server (pulled).
    pub source: BackupEnd,
    /// The folder the task's folder is in.
    pub target: BackupEnd,
    pub folder: BackupFolder,
    pub mode: BackupMode,
    pub stamp: BackupStamp,
    pub excludes: Vec<BackupPattern>,
    /// Names starting with `.` are backed up.
    pub hidden: bool,
    /// KiB/s, 0 for no limit.
    pub bwlimit_kib: u32,
}

/// What a restore does with a file that is there already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupExisting {
    Skip,
    Replace,
}

/// A restore of a task's backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRestore {
    pub task: BackupTaskId,
    /// The task's folder (where an encryption begins).
    pub from: BackupEnd,
    /// The version in it, or the mirror's copy (`current`).
    pub within: SharePath,
    /// What of it, as paths in it; none for all.
    pub paths: Vec<SharePath>,
    /// A folder on the NAS; made if it is missing.
    pub to: BackupEnd,
    pub existing: BackupExisting,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_users_and_paths_never_read_as_options() {
        assert!(BackupHostName::parse("nas2.lan").is_ok());
        assert!(BackupHostName::parse("fe80::1").is_ok());
        for bad in ["-oProxyCommand=sh", ".lan", "a b", "a;b", ""] {
            assert!(BackupHostName::parse(bad).is_err(), "{bad}");
        }
        assert!(BackupWord::parse("backup_user-1").is_ok());
        assert!(BackupWord::parse("-e").is_err());
        assert!(BackupRemotePath::parse("/srv/www").is_ok());
        assert!(BackupRemotePath::parse("nas/photos").is_ok());
        assert!(BackupRemotePath::parse("").is_ok());
        for bad in ["-e", "a/../b", "a//b", "a\nb"] {
            assert!(BackupRemotePath::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn what_retention_removes_is_named_as_versions_are() {
        assert!(BackupStamp::parse("2026-11-11_030000Z").is_ok());
        assert!(BackupStamp::parse("2026-11-11_03000Z").is_err());
        for good in ["2026-11-11_030000Z", "2026-11-11_030000Z.incomplete", "removed/2026-11-11_030000Z"] {
            assert!(BackupPruned::parse(good).is_ok(), "{good}");
        }
        for bad in ["current", ".zendiloos-backup", "removed", "../2026-11-11_030000Z", "removed/x", ""] {
            assert!(BackupPruned::parse(bad).is_err(), "{bad}");
        }
        assert!(BackupFolder::parse("Photos to tank").is_ok());
        for bad in [".hidden", "-x", "a/b", ".."] {
            assert!(BackupFolder::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passwords_stay_out_of_logs() {
        let password = BackupPassword::new("s3cret").unwrap();
        assert_eq!(format!("{password:?}"), "BackupPassword(…)");
        let secrets = BackupSecrets {
            password: Some(password),
            crypt: Some(BackupCrypt { password: BackupSecret::new("crypt pw").unwrap(), salt: None }),
            cloud: Some(BackupCloudAccess {
                token: BackupSecret::new(r#"{"access_token":"ya29.x"}"#).unwrap(),
                client_id: Some(BackupCloudWord::parse("202264815644.apps.googleusercontent.com").unwrap()),
                client_secret: None,
                drive_id: None,
                drive_type: None,
            }),
        };
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("s3cret") && !shown.contains("crypt pw") && !shown.contains("ya29"),
            "{shown}"
        );
        assert!(BackupSecret::new(&"x".repeat(16 * 1024 + 1)).is_err());
        assert!(BackupCloudWord::parse("-x").is_err());
        assert!(BackupCloudWord::parse("b!AbC-12_x").is_ok());
        let cloud: BackupEnd =
            serde_json::from_str(r#"{"kind":"cloud","cloud":"onedrive","path":"nas"}"#).unwrap();
        assert!(cloud.is_remote() && cloud.server().is_none());
        assert!(BackupPassword::new("a\nb").is_err());
        assert!(BackupPassword::new("").is_err());
        let line = r#"{"kind":"daemon","module":"-x"}"#;
        assert!(serde_json::from_str::<BackupTransport>(line).is_err(), "checked while read");
        assert!(BackupTaskId::new(0).is_err());
        assert!(BackupPattern::parse("- *").is_err());
        assert!(BackupHostKey::parse("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoq").is_ok());
        assert!(BackupHostKey::parse("ssh-ed25519 AAAA AAAA").is_err());
    }
}
