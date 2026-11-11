//! A backup task: what is backed up (a folder of a share, or of an rsync
//! server), where to (a folder of a share or of a removable drive on the
//! NAS, or of an rsync server), how (a mirror or dated versions), when (a
//! schedule, and when its drive is plugged in), and what is left out.

use zendiloos_kernel::Timestamp;

use super::schedule::Schedule;

/// The longest name of a task.
pub const MAX_NAME: usize = 64;
/// The most exclude patterns a task has.
pub const MAX_EXCLUDES: usize = 64;
/// The most versions a task keeps by number, and days by age.
pub const MAX_KEEP_VERSIONS: u16 = 1000;
pub const MAX_KEEP_DAYS: u16 = 3650;
/// The highest bandwidth limit, in KiB/s (about 10 GB/s).
pub const MAX_BWLIMIT_KIB: u32 = 10_000_000;

/// What a new task leaves out unless told otherwise: temporary files,
/// thumbnails other systems leave, and the recycle bins.
pub const DEFAULT_EXCLUDES: [&str; 7] =
    ["*.tmp", "Thumbs.db", ".DS_Store", "@eaDir", ".recycle", "#recycle", "$RECYCLE.BIN"];

/// A folder on the NAS: in a share, or on a removable drive; `path` is the
/// folder in it, `""` its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NasPlace {
    Share {
        share_id: i64,
        path: String,
    },
    /// `drive` is its file system's UUID, which names it.
    Drive {
        drive: String,
        path: String,
    },
}

impl NasPlace {
    pub fn path(&self) -> &str {
        match self {
            Self::Share { path, .. } | Self::Drive { path, .. } => path,
        }
    }

    pub fn drive(&self) -> Option<&str> {
        match self {
            Self::Drive { drive, .. } => Some(drive),
            Self::Share { .. } => None,
        }
    }
}

/// How the NAS reaches an rsync server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// An rsync daemon (`rsync://host:873/module`) with a user and a
    /// password: plain, unencrypted.
    Daemon { module: String },
    /// rsync over SSH with the NAS's own key; the server's host key pinned
    /// when the task was made (its OpenSSH line, `ssh-ed25519 AAAA…`) and
    /// checked at every connection.
    Ssh { host_key: String, fingerprint: String },
}

/// An rsync server, by address, port and user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl Server {
    pub const DAEMON_PORT: u16 = 873;
    pub const SSH_PORT: u16 = 22;

    /// Whether a password stored for `self` may serve `other`: only the
    /// same daemon, port, user and module. Another server needs it typed
    /// again, so a stored password never goes where it was not typed for.
    pub fn keeps_password_for(&self, other: &Server) -> bool {
        matches!((&self.transport, &other.transport), (Transport::Daemon { module: a }, Transport::Daemon { module: b }) if a == b)
            && self.host.eq_ignore_ascii_case(&other.host)
            && self.port == other.port
            && self.user == other.user
    }

    pub fn is_daemon(&self) -> bool {
        matches!(self.transport, Transport::Daemon { .. })
    }
}

/// A folder of an rsync server: in the daemon's module, or on the SSH
/// server (relative to the user's home unless it starts with `/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlace {
    pub server: Server,
    pub path: String,
}

impl RemotePlace {
    /// Whether the server and the folder are given as rsync may be given
    /// them.
    pub fn check(&self) -> Result<(), Invalid> {
        check_place(&Place::Remote(self.clone()), "folder")
    }
}

/// One end of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Nas(NasPlace),
    Remote(RemotePlace),
}

impl Place {
    pub fn remote(&self) -> Option<&RemotePlace> {
        match self {
            Self::Remote(remote) => Some(remote),
            Self::Nas(_) => None,
        }
    }

    pub fn nas(&self) -> Option<&NasPlace> {
        match self {
            Self::Nas(nas) => Some(nas),
            Self::Remote(_) => None,
        }
    }
}

/// What a task does, as its places say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// From a share to another place on the NAS.
    Local,
    /// From a share to an rsync server.
    Push,
    /// From an rsync server into the NAS.
    Pull,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Push => "push",
            Self::Pull => "pull",
        }
    }
}

/// How a run leaves the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// An exact copy in `current/`, what the source deleted deleted too.
    /// With `keep_changed_days`, what a run deletes or replaces is kept in
    /// `removed/<time of the run>/` for that many days.
    Mirror { keep_changed_days: Option<u16> },
    /// Each run a dated version, files unchanged since the previous one
    /// hard links to it. Kept: the newest `keep_last`, and those younger
    /// than `keep_days`; neither set keeps all. The newest is always kept.
    Versioned { keep_last: Option<u16>, keep_days: Option<u16> },
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mirror { .. } => "mirror",
            Self::Versioned { .. } => "versioned",
        }
    }
}

/// What a run leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filters {
    /// rsync's patterns, each excluded wherever it matches (`*.tmp`,
    /// `Thumbs.db`, `/cache` for one at the top only).
    pub excludes: Vec<String>,
    /// Whether names starting with `.` are backed up.
    pub hidden: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self { excludes: DEFAULT_EXCLUDES.map(str::to_owned).to_vec(), hidden: true }
    }
}

/// A task as an administrator sets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSettings {
    pub name: String,
    pub source: Place,
    pub target: Place,
    pub mode: Mode,
    /// None: by hand only (and when its drive is plugged in, if so set).
    pub schedule: Option<Schedule>,
    /// Back up whenever its target drive is plugged in.
    pub on_plug: bool,
    pub filters: Filters,
    /// KiB/s; none follows the settings' default, 0 is no limit.
    pub bwlimit_kib: Option<u32>,
    /// Tell when a run succeeded too, not only when it failed.
    pub notify_success: bool,
}

/// A task as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub settings: TaskSettings,
    /// The folder in the target that holds this task's backup, named
    /// after the task when it was made; renaming the task keeps it.
    pub folder: String,
    pub created_at: Timestamp,
    /// Runs on its schedule are due from then: when it was made, changed
    /// or last started.
    pub since: Timestamp,
    /// Which plugging in of its target drive it last backed up for.
    pub last_plug: Option<String>,
}

impl Task {
    pub fn kind(&self) -> TaskKind {
        self.settings.kind()
    }

    pub fn name(&self) -> &str {
        &self.settings.name
    }

    /// The target drive, if it backs up to one.
    pub fn target_drive(&self) -> Option<&str> {
        self.settings.target.nas().and_then(NasPlace::drive)
    }

    /// The server it talks to, if any.
    pub fn server(&self) -> Option<&Server> {
        self.settings.source.remote().or(self.settings.target.remote()).map(|remote| &remote.server)
    }
}

/// Why a task's settings are refused, in words for the administrator.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

fn invalid(reason: impl Into<String>) -> Invalid {
    Invalid(reason.into())
}

/// Whether `name` may be one name of a path: not empty, `.` or `..`, no
/// `/`, no control characters, at most 255 bytes.
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.chars().any(char::is_control)
}

/// Whether `path` names a folder beneath a root: names separated by `/`,
/// each one [`is_name`]; `""` for the root itself.
pub fn is_folder_path(path: &str) -> bool {
    path.is_empty() || (path.len() <= 1024 && path.split('/').all(is_name))
}

/// A folder of an rsync server: as [`is_folder_path`], also starting with
/// `/` (an absolute path over SSH), and never starting with `-`.
pub fn is_remote_path(path: &str) -> bool {
    let relative = path.strip_prefix('/').unwrap_or(path);
    !path.starts_with('-') && is_folder_path(relative)
}

/// A host name or an IP address: letters, digits, `.`, `-` and `:` (IPv6),
/// not starting with `-` or `.`, at most 253.
pub fn is_host(host: &str) -> bool {
    (1..=253).contains(&host.len())
        && !host.starts_with(['-', '.'])
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
}

/// A user on a server, or a daemon's module: letters, digits, `.`, `_`
/// and `-`, not starting with `-` or `.`, at most 64.
pub fn is_word(word: &str) -> bool {
    (1..=64).contains(&word.len())
        && !word.starts_with(['-', '.'])
        && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A removable drive's id: its file system's UUID.
pub fn is_drive_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && !id.starts_with('-')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A key line as OpenSSH writes it: its type and the key in base64.
pub fn is_host_key(line: &str) -> bool {
    let mut words = line.split(' ');
    let (Some(kind), Some(key), None) = (words.next(), words.next(), words.next()) else { return false };
    matches!(
        kind,
        "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" | "ssh-rsa"
    ) && (16..=4096).contains(&key.len())
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
}

/// Whether an exclude pattern may be given to rsync: one line, not empty,
/// none of the prefixes rsync reads as a rule of its own (`+ `, `- `, `!`).
pub fn is_pattern(pattern: &str) -> bool {
    (1..=255).contains(&pattern.len())
        && !pattern.chars().any(char::is_control)
        && pattern.trim() == pattern
        && !pattern.starts_with("+ ")
        && !pattern.starts_with("- ")
        && !pattern.starts_with('!')
}

/// The name of the folder a task's backup goes into, from its name: what
/// cannot be in a file name (`/`, `\`, control characters) becomes `_`,
/// dots and spaces at either end go.
pub fn folder_name(name: &str) -> String {
    let replaced: String =
        name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let trimmed = replaced.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() { "backup".to_owned() } else { trimmed.to_owned() }
}

/// Whether one folder lies within the other (or is it).
fn nested(a: &str, b: &str) -> bool {
    let within = |inner: &str, outer: &str| {
        outer.is_empty()
            || inner == outer
            || inner.strip_prefix(outer).is_some_and(|rest| rest.starts_with('/'))
    };
    within(a, b) || within(b, a)
}

impl TaskSettings {
    pub fn kind(&self) -> TaskKind {
        match (&self.source, &self.target) {
            (Place::Remote(_), _) => TaskKind::Pull,
            (_, Place::Remote(_)) => TaskKind::Push,
            _ => TaskKind::Local,
        }
    }

    /// The settings with the name trimmed, or why not: every value, and
    /// that no other task (`taken`, their names) has the name.
    pub fn check(mut self, taken: &[&str]) -> Result<Self, Invalid> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty()
            || self.name.chars().count() > MAX_NAME
            || self.name.chars().any(char::is_control)
        {
            return Err(invalid(format!(
                "a task's name has 1 to {MAX_NAME} characters, none of them control characters"
            )));
        }
        if taken.iter().any(|other| other.to_lowercase() == self.name.to_lowercase()) {
            return Err(invalid("another task has that name"));
        }
        check_place(&self.source, "source")?;
        check_place(&self.target, "target")?;
        match (&self.source, &self.target) {
            (Place::Remote(_), Place::Remote(_)) => {
                return Err(invalid("a task goes from or to the NAS, not from one server to another"));
            }
            (Place::Nas(NasPlace::Drive { .. }), _) => {
                return Err(invalid("the source is a folder of a share"));
            }
            (
                Place::Nas(NasPlace::Share { share_id: a, path: from }),
                Place::Nas(NasPlace::Share { share_id: b, path: to }),
            ) if a == b && nested(from, to) => {
                return Err(invalid(
                    "the target cannot lie within the source, nor the source within the target",
                ));
            }
            _ => {}
        }
        match self.mode {
            Mode::Mirror { keep_changed_days: Some(days) } if !(1..=MAX_KEEP_DAYS).contains(&days) => {
                return Err(invalid(format!("what a run changes is kept 1 to {MAX_KEEP_DAYS} days")));
            }
            Mode::Versioned { keep_last, keep_days } => {
                if keep_last.is_some_and(|n| !(1..=MAX_KEEP_VERSIONS).contains(&n)) {
                    return Err(invalid(format!("a task keeps 1 to {MAX_KEEP_VERSIONS} versions")));
                }
                if keep_days.is_some_and(|n| !(1..=MAX_KEEP_DAYS).contains(&n)) {
                    return Err(invalid(format!("versions are kept 1 to {MAX_KEEP_DAYS} days")));
                }
            }
            Mode::Mirror { .. } => {}
        }
        if let Some(schedule) = &self.schedule {
            schedule.check().map_err(|e| invalid(e.to_string()))?;
        }
        if self.on_plug && self.target.nas().and_then(NasPlace::drive).is_none() {
            return Err(invalid(
                "only a task whose target is a removable drive backs up when it is plugged in",
            ));
        }
        if self.filters.excludes.len() > MAX_EXCLUDES {
            return Err(invalid(format!("a task has at most {MAX_EXCLUDES} exclude patterns")));
        }
        if let Some(bad) = self.filters.excludes.iter().find(|pattern| !is_pattern(pattern)) {
            return Err(invalid(format!("{bad:?} is no pattern rsync can be given")));
        }
        if self.bwlimit_kib.is_some_and(|limit| limit > MAX_BWLIMIT_KIB) {
            return Err(invalid(format!("the bandwidth limit is at most {MAX_BWLIMIT_KIB} KiB/s")));
        }
        Ok(self)
    }
}

fn check_place(place: &Place, which: &str) -> Result<(), Invalid> {
    match place {
        Place::Nas(NasPlace::Share { path, .. }) if !is_folder_path(path) => {
            Err(invalid(format!("the {which} is no folder of a share")))
        }
        Place::Nas(NasPlace::Drive { drive, path }) if !is_drive_id(drive) || !is_folder_path(path) => {
            Err(invalid(format!("the {which} is no folder of a removable drive")))
        }
        Place::Nas(_) => Ok(()),
        Place::Remote(remote) => {
            let server = &remote.server;
            if !is_host(&server.host) {
                return Err(invalid(format!("{:?} is no host name or address", server.host)));
            }
            if server.port == 0 {
                return Err(invalid("the port is from 1 to 65535"));
            }
            if !is_word(&server.user) {
                return Err(invalid("the user name has letters, digits, '.', '_' and '-'"));
            }
            match &server.transport {
                Transport::Daemon { module } if !is_word(module) => {
                    return Err(invalid("the module's name has letters, digits, '.', '_' and '-'"));
                }
                Transport::Ssh { host_key, .. } if !is_host_key(host_key) => {
                    return Err(invalid("the server's host key is not pinned"));
                }
                _ => {}
            }
            if !is_remote_path(&remote.path) {
                return Err(invalid(format!("the {which}'s folder on the server is no path")));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::schedule::{Frequency, Schedule};

    fn share(id: i64, path: &str) -> Place {
        Place::Nas(NasPlace::Share { share_id: id, path: path.into() })
    }

    fn daemon() -> Server {
        Server {
            transport: Transport::Daemon { module: "backup".into() },
            host: "nas2.lan".into(),
            port: 873,
            user: "zendi".into(),
        }
    }

    fn settings() -> TaskSettings {
        TaskSettings {
            name: " Photos to tank ".into(),
            source: share(1, "photos"),
            target: share(2, "backups"),
            mode: Mode::Versioned { keep_last: Some(30), keep_days: None },
            schedule: Some(Schedule { frequency: Frequency::Daily, hour: 3, minute: 0 }),
            on_plug: false,
            filters: Filters::default(),
            bwlimit_kib: None,
            notify_success: false,
        }
    }

    #[test]
    fn good_settings_pass_with_the_name_trimmed() {
        let checked = settings().check(&["Other"]).unwrap();
        assert_eq!(checked.name, "Photos to tank");
        assert_eq!(checked.kind(), TaskKind::Local);
    }

    #[test]
    fn names_are_unique_whatever_the_case() {
        assert!(settings().check(&["photos TO tank"]).is_err());
        let mut long = settings();
        long.name = "x".repeat(65);
        assert!(long.check(&[]).is_err());
        let mut blank = settings();
        blank.name = "  ".into();
        assert!(blank.check(&[]).is_err());
    }

    #[test]
    fn a_target_within_its_source_is_refused() {
        for (from, to) in [("", "backups"), ("photos", "photos/old"), ("photos/old", "photos"), ("a", "a")] {
            let mut nested = settings();
            nested.source = share(1, from);
            nested.target = share(1, to);
            assert!(nested.check(&[]).is_err(), "{from} → {to}");
        }
        let mut beside = settings();
        beside.source = share(1, "photos");
        beside.target = share(1, "photos-backup");
        assert!(beside.check(&[]).is_ok(), "a name that only begins alike");
    }

    #[test]
    fn places_and_servers_are_checked() {
        let mut drive_source = settings();
        drive_source.source = Place::Nas(NasPlace::Drive { drive: "1A2B-3C4D".into(), path: String::new() });
        assert!(drive_source.check(&[]).is_err(), "the source is a share");

        let mut climbing = settings();
        climbing.target = share(2, "a/../b");
        assert!(climbing.check(&[]).is_err());

        let mut push = settings();
        push.target = Place::Remote(RemotePlace { server: daemon(), path: "nas/photos".into() });
        assert_eq!(push.clone().check(&[]).unwrap().kind(), TaskKind::Push);
        for host in ["-oProxyCommand=x", "", "a b", "host;rm"] {
            let mut bad = push.clone();
            if let Place::Remote(remote) = &mut bad.target {
                remote.server.host = host.into();
            }
            assert!(bad.check(&[]).is_err(), "{host}");
        }
        let mut option_path = push.clone();
        if let Place::Remote(remote) = &mut option_path.target {
            remote.path = "-e sh".into();
        }
        assert!(option_path.check(&[]).is_err());

        let mut unpinned = push.clone();
        if let Place::Remote(remote) = &mut unpinned.target {
            remote.server.transport =
                Transport::Ssh { host_key: "nothing".into(), fingerprint: String::new() };
        }
        assert!(unpinned.check(&[]).is_err(), "SSH needs the host key pinned");

        let mut pull = settings();
        pull.source = Place::Remote(RemotePlace { server: daemon(), path: "/srv/www".into() });
        assert_eq!(pull.clone().check(&[]).unwrap().kind(), TaskKind::Pull);
        let mut both = pull;
        both.target = Place::Remote(RemotePlace { server: daemon(), path: String::new() });
        assert!(both.check(&[]).is_err());
    }

    #[test]
    fn plugging_in_needs_a_drive_and_patterns_are_rsyncs() {
        let mut plug = settings();
        plug.on_plug = true;
        assert!(plug.clone().check(&[]).is_err());
        plug.target = Place::Nas(NasPlace::Drive { drive: "1A2B-3C4D".into(), path: "nas".into() });
        assert!(plug.check(&[]).is_ok());

        for pattern in ["- x", "+ x", "!", "a\nb", "", " x"] {
            let mut filters = settings();
            filters.filters.excludes = vec![pattern.into()];
            assert!(filters.check(&[]).is_err(), "{pattern:?}");
        }
        let mut many = settings();
        many.filters.excludes = (0..65).map(|n| format!("*.{n}")).collect();
        assert!(many.check(&[]).is_err());
    }

    #[test]
    fn retention_and_limits_have_bounds() {
        for mode in [
            Mode::Versioned { keep_last: Some(0), keep_days: None },
            Mode::Versioned { keep_last: None, keep_days: Some(3651) },
            Mode::Mirror { keep_changed_days: Some(0) },
        ] {
            let mut bad = settings();
            bad.mode = mode;
            assert!(bad.check(&[]).is_err(), "{mode:?}");
        }
        let mut fast = settings();
        fast.bwlimit_kib = Some(MAX_BWLIMIT_KIB + 1);
        assert!(fast.check(&[]).is_err());
    }

    #[test]
    fn folder_names_come_from_task_names() {
        assert_eq!(folder_name("Photos to tank"), "Photos to tank");
        assert_eq!(folder_name("a/b\\c"), "a_b_c");
        assert_eq!(folder_name("..hidden. "), "hidden");
        assert_eq!(folder_name("..."), "backup");
    }

    #[test]
    fn passwords_stay_with_their_daemon() {
        let mut other = daemon();
        assert!(daemon().keeps_password_for(&other));
        other.host = "NAS2.lan".into();
        assert!(daemon().keeps_password_for(&other), "host names are not case-sensitive");
        other.port = 874;
        assert!(!daemon().keeps_password_for(&other));
        let mut module = daemon();
        module.transport = Transport::Daemon { module: "other".into() };
        assert!(!daemon().keeps_password_for(&module));
    }

    #[test]
    fn host_keys_are_openssh_lines() {
        assert!(is_host_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
        ));
        assert!(!is_host_key("ssh-ed25519"));
        assert!(!is_host_key("ssh-dss AAAAB3NzaC1kc3MAAACBAP"));
        assert!(!is_host_key("ssh-ed25519 AAAA\nx"));
    }
}
