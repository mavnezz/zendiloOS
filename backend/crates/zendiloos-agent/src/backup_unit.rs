//! A backup task's work in its unit (`backup-unit <job>`), which the system
//! agent starts with `systemd-run` (docs/architecture.md, 4.14): a run, a
//! listing, retention or a restore, each one or more calls of rsync; of
//! rclone for a cloud or an encrypted task ([`rclone`]).
//!
//! The unit's sandbox (see [`crate::backup`]) shows it the roots of the
//! places it works on and nothing else of the volumes and drives, read-only
//! but the one it writes, and the network only when a server is involved.
//! Within, the folders are opened beneath their roots by descriptor
//! (`openat2`: no `..`, no link, no other file system), and rsync is given
//! those descriptors (`/proc/self/fd/<n>/`), so a folder swapped for a link
//! between the check and the copy changes nothing. rsync copies links as
//! links and leaves out those that point outside the copied folder or are
//! absolute (`--safe-links`): it never reads or writes through one; it
//! stays on the source's file system (`--one-file-system`).
//!
//! Secrets come on standard input, one line of JSON, never on a command
//! line: a daemon's password goes into a file in the unit's private `/tmp`
//! (0600), rsync reads it there (`--password-file`); the NAS's SSH key is
//! the unit's credential (`LoadCredential=`), which only the unit sees;
//! what rclone needs is set for it alone ([`rclone`]).

mod rclone;

use std::io::{BufRead, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags, RenameFlags, ResolveFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::{
    BackupCloud, BackupExisting, BackupFolder, BackupHostKey, BackupHostName, BackupMode, BackupPattern,
    BackupPruned, BackupRemotePath, BackupSecrets, BackupStamp, BackupWord, SharePath, is_stamp,
};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::volume_archive::decode;

/// Beneath a root, never through a link, never onto another file system.
const BENEATH: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
/// What marks a task's folder as a backup of zendiloOS's.
pub(crate) const MARKER: &str = ".zendiloos-backup";
const MARKER_TEXT: &str = "This folder holds a backup of zendiloOS's. Do not change what is in it.\n";
const INCOMPLETE: &str = ".incomplete";
/// Entries of the transfer list kept; the count goes on.
pub(crate) const TRANSFERS_KEPT: usize = 5000;
/// Lines of a listing passed on: a folder of more is listed as far.
const LISTED: usize = 20_000;
/// rsync's own limit of `--link-dest` folders.
const LINK_DESTS: usize = 20;
/// What a line of the unit carries at most of an answer: lines are cut at
/// 4096 bytes, and JSON in a JSON string may take up to twice as much.
const PART_BYTES: usize = 1500;
/// No data for this long ends a transfer with a server.
const IO_TIMEOUT: &str = "--timeout=900";
/// File systems that keep neither owners, modes, links nor ACLs: FAT,
/// exFAT, NTFS (both drivers).
const PLAIN_FILESYSTEMS: [i64; 4] = [0x4d44, 0x2011_bab0, 0x5346_544e, 0x7366_746e];

/// One end, as the unit reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum End {
    /// A folder beneath a root on the NAS: the root's path (a share's
    /// mount point, a drive's place) and the folder in it.
    Local {
        root: PathBuf,
        path: SharePath,
    },
    Daemon {
        host: BackupHostName,
        port: u16,
        user: BackupWord,
        module: BackupWord,
        path: BackupRemotePath,
    },
    Ssh {
        host: BackupHostName,
        port: u16,
        user: BackupWord,
        path: BackupRemotePath,
        host_key: BackupHostKey,
    },
    /// A folder of a cloud account, from its root.
    Cloud {
        cloud: BackupCloud,
        path: SharePath,
    },
}

impl End {
    /// The folder `more` (names separated by `/`) in this one.
    fn join(&self, more: &str) -> End {
        let join = |path: &str| {
            [path.trim_end_matches('/'), more]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        };
        let lead = |path: &str, joined: String| {
            if path.starts_with('/') && !joined.starts_with('/') { format!("/{joined}") } else { joined }
        };
        match self {
            End::Local { root, path } => End::Local {
                root: root.clone(),
                path: SharePath::parse(&join(path.as_str())).unwrap_or_default(),
            },
            End::Daemon { host, port, user, module, path } => End::Daemon {
                host: host.clone(),
                port: *port,
                user: user.clone(),
                module: module.clone(),
                path: BackupRemotePath::parse(&join(path.as_str())).unwrap_or_else(|_| path.clone()),
            },
            End::Ssh { host, port, user, path, host_key } => End::Ssh {
                host: host.clone(),
                port: *port,
                user: user.clone(),
                path: BackupRemotePath::parse(&lead(path.as_str(), join(path.as_str())))
                    .unwrap_or_else(|_| path.clone()),
                host_key: host_key.clone(),
            },
            End::Cloud { cloud, path } => {
                End::Cloud { cloud: *cloud, path: SharePath::parse(&join(path.as_str())).unwrap_or_default() }
            }
        }
    }
}

/// A run, as the agent hands it to the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunJob {
    pub source: End,
    pub target: End,
    pub folder: BackupFolder,
    pub mode: BackupMode,
    pub stamp: BackupStamp,
    pub excludes: Vec<BackupPattern>,
    pub hidden: bool,
    pub bwlimit_kib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreJob {
    /// The task's folder (where a crypt begins).
    pub from: End,
    /// The version in it, or the mirror's copy.
    #[serde(default)]
    pub within: SharePath,
    pub paths: Vec<SharePath>,
    /// Always [`End::Local`].
    pub to: End,
    pub existing: BackupExisting,
    /// The folder is in a share: what is restored takes its owner and the
    /// share's group and ACL, as the explorer's copies do.
    pub into_share: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "work", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Work {
    /// The folder `inside` of `at` (through the crypt rooted at `at`).
    List {
        at: End,
        #[serde(default)]
        inside: SharePath,
    },
    Run(Box<RunJob>),
    Prune {
        at: End,
        names: Vec<BackupPruned>,
    },
    Restore(Box<RestoreJob>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnitJob {
    pub work: Work,
    pub rsync: PathBuf,
    #[serde(default)]
    pub rclone: PathBuf,
    pub ssh: PathBuf,
    /// The NAS's SSH key as the unit's credential:
    /// `/run/credentials/<unit>/backup-key`.
    pub key: PathBuf,
}

impl Work {
    /// Whether rclone does it: a cloud is involved, or the task's folder
    /// is encrypted.
    pub(crate) fn rclone(&self, encrypted: bool) -> bool {
        let cloud = |end: &End| matches!(end, End::Cloud { .. });
        encrypted
            || match self {
                Work::List { at, .. } | Work::Prune { at, .. } => cloud(at),
                Work::Run(run) => cloud(&run.target) || cloud(&run.source),
                Work::Restore(restore) => cloud(&restore.from),
            }
    }
}

/// How a unit's work failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailKind {
    /// It would not: the request does not fit what is there.
    Refused,
    Failed,
    /// The SSH server's host key is not the one pinned.
    HostKey,
}

/// A line the unit writes: JSON, one per line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnitSays {
    Progress {
        step: String,
        percent: u8,
    },
    /// A piece of the answer, as JSON text.
    Part(String),
    /// The answer is complete.
    Done,
    Failed {
        message: String,
        kind: FailKind,
    },
}

/// What a run tells, as JSON in parts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub files: u64,
    pub bytes: u64,
    pub partial: Option<String>,
    pub transfers: Vec<String>,
    pub transfers_total: u64,
    pub listing: String,
    pub missing: bool,
    /// A cloud's token rclone refreshed meanwhile.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug)]
struct Fail {
    message: String,
    kind: FailKind,
}

fn refused(message: impl Into<String>) -> Fail {
    Fail { message: message.into(), kind: FailKind::Refused }
}

fn failed(message: impl Into<String>) -> Fail {
    Fail { message: message.into(), kind: FailKind::Failed }
}

fn say(out: &mut dyn Write, line: &UnitSays) {
    let _ = writeln!(out, "{}", serde_json::to_string(line).unwrap_or_default());
    let _ = out.flush();
}

/// Runs the job, reading its secrets from `input` and writing
/// [`UnitSays`] lines to `out`; whether it succeeded.
pub async fn run_backup_unit(hex: &str, input: &mut dyn BufRead, out: &mut (dyn Write + Send)) -> bool {
    let outcome = async {
        let job = decode::<UnitJob>(hex).map_err(refused)?;
        let mut line = String::new();
        let _ = std::io::Read::take(&mut *input, 64 * 1024).read_line(&mut line);
        let secrets: BackupSecrets = if line.trim().is_empty() {
            BackupSecrets::default()
        } else {
            serde_json::from_str(&line).map_err(|e| refused(format!("the secrets cannot be read: {e}")))?
        };
        let scratch = Scratch::make()?;
        let unit = Unit { job: &job, scratch: &scratch, secrets };
        let result = unit.work(out).await;
        drop(scratch);
        result
    }
    .await;
    match outcome {
        Ok(report) => {
            let text = serde_json::to_string(&report).unwrap_or_default();
            let mut rest = text.as_str();
            while !rest.is_empty() {
                let mut at = rest.len().min(PART_BYTES);
                while !rest.is_char_boundary(at) {
                    at -= 1;
                }
                say(out, &UnitSays::Part(rest[..at].to_owned()));
                rest = &rest[at..];
            }
            say(out, &UnitSays::Done);
            true
        }
        Err(fail) => {
            say(out, &UnitSays::Failed { message: fail.message, kind: fail.kind });
            false
        }
    }
}

/// A folder of the unit's own in its private `/tmp`, root's alone, gone
/// with the unit (and removed when done).
struct Scratch(PathBuf);

impl Scratch {
    fn make() -> Result<Self, Fail> {
        let base = std::env::temp_dir();
        let mut bytes = [0u8; 8];
        rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| failed(format!("no random numbers: {e}")))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = base.join(format!("zendiloos-backup-{name}"));
        std::fs::create_dir(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(Self(path))
    }

    /// Writes `content` to `name`, readable by root alone.
    fn file(&self, name: &str, content: &str) -> Result<PathBuf, Fail> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.0.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| failed(format!("{}: {e}", path.display())))?;
        file.write_all(content.as_bytes()).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(path)
    }

    /// As [`Scratch::file`], once: the second time the file is there.
    fn once(&self, name: &str, content: &str) -> Result<PathBuf, Fail> {
        let path = self.0.join(name);
        if path.exists() { Ok(path) } else { self.file(name, content) }
    }

    fn folder(&self, name: &str) -> Result<PathBuf, Fail> {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A folder opened on the NAS: rsync is given it as `/proc/self/fd/<n>/`.
struct Opened {
    fd: OwnedFd,
    /// Its file system keeps no owners, modes or links (FAT, exFAT, NTFS).
    plain: bool,
}

impl Opened {
    fn arg(&self, more: &str) -> String {
        let base = format!("/proc/self/fd/{}/", self.fd.as_raw_fd());
        if more.is_empty() { base } else { format!("{base}{more}/") }
    }
}

/// Opens `path` beneath `root`, making the folders that are missing if
/// `make`; `None` if it is not there (and not made). The descriptor is
/// left to rsync (not closed when it starts).
fn open_beneath(root: &Path, path: &SharePath, make: bool) -> Result<Option<Opened>, Fail> {
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root_fd = rustix::fs::open(root, directory, Mode::empty())
        .map_err(|e| failed(format!("{} cannot be opened: {e}", root.display())))?;
    let mut current = root_fd;
    for name in path.names() {
        let next = match rustix::fs::openat2(&current, name, directory, Mode::empty(), BENEATH) {
            Ok(next) => next,
            Err(Errno::NOENT) if make => {
                match rustix::fs::mkdirat(&current, name, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(failed(format!("{name} cannot be made: {e}"))),
                }
                rustix::fs::openat2(&current, name, directory, Mode::empty(), BENEATH)
                    .map_err(|e| failed(format!("{name} cannot be opened: {e}")))?
            }
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::XDEV | Errno::LOOP | Errno::NOTDIR) => {
                return Err(refused(format!("{} is reached through a link, or is no folder", path.as_str())));
            }
            Err(e) => return Err(failed(format!("{} cannot be opened: {e}", path.as_str()))),
        };
        current = next;
    }
    let plain =
        rustix::fs::fstatfs(&current).map(|stat| PLAIN_FILESYSTEMS.contains(&stat.f_type)).unwrap_or(false);
    // rsync, run next, inherits it.
    rustix::io::fcntl_setfd(&current, rustix::io::FdFlags::empty())
        .map_err(|e| failed(format!("{}: {e}", path.as_str())))?;
    Ok(Some(Opened { fd: current, plain }))
}

/// Opens (or makes) the folder `name` in `parent`, not through a link.
fn open_in(parent: &Opened, name: &str, make: bool) -> Result<Option<Opened>, Fail> {
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let opened = match rustix::fs::openat2(&parent.fd, name, directory, Mode::empty(), BENEATH) {
        Ok(fd) => fd,
        Err(Errno::NOENT) if make => {
            match rustix::fs::mkdirat(&parent.fd, name, Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(e) => return Err(failed(format!("{name} cannot be made: {e}"))),
            }
            rustix::fs::openat2(&parent.fd, name, directory, Mode::empty(), BENEATH)
                .map_err(|e| failed(format!("{name} cannot be opened: {e}")))?
        }
        Err(Errno::NOENT) => return Ok(None),
        Err(Errno::XDEV | Errno::LOOP | Errno::NOTDIR) => {
            return Err(refused(format!("{name} is reached through a link, or is no folder")));
        }
        Err(e) => return Err(failed(format!("{name} cannot be opened: {e}"))),
    };
    rustix::io::fcntl_setfd(&opened, rustix::io::FdFlags::empty())
        .map_err(|e| failed(format!("{name}: {e}")))?;
    Ok(Some(Opened { fd: opened, plain: parent.plain }))
}

/// An entry of a folder, as far as the unit needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    name: String,
    folder: bool,
}

/// Whether an entry of a task's folder is the backup's: the marker,
/// `current`, `removed`, versions and versions cut off.
fn ours(entry: &Listed) -> bool {
    entry.name == MARKER
        || (entry.folder
            && (entry.name == "current"
                || entry.name == "removed"
                || is_stamp(&entry.name)
                || entry.name.strip_suffix(INCOMPLETE).is_some_and(is_stamp)))
}

/// The name and kind of an entry `rsync --list-only` printed: the mode's
/// first letter, then size, date and time, then the name (with ` -> …`
/// for a link). rsync writes odd characters as `\#ooo`.
fn listed(line: &str) -> Option<Listed> {
    let mut rest = line;
    let mut fields = Vec::new();
    for _ in 0..4 {
        rest = rest.trim_start();
        let end = rest.find(' ')?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let mode = fields[0];
    let name = rest.strip_prefix(' ')?;
    if mode.len() != 10 || name.is_empty() {
        return None;
    }
    let name = if mode.starts_with('l') { name.split(" -> ").next().unwrap_or(name) } else { name };
    Some(Listed { name: name.to_owned(), folder: mode.starts_with('d') })
}

/// The stats rsync prints at the end (`--info=stats2`, numbers plain).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Stats {
    files: u64,
    bytes: u64,
}

fn number_after(line: &str, label: &str) -> Option<u64> {
    let rest = line.strip_prefix(label)?.trim_start();
    let digits: String =
        rest.chars().take_while(|c| c.is_ascii_digit() || *c == ',').filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// What rsync says when all is well, besides its changes and stats.
const BENIGN: [&str; 16] = [
    "sending incremental file list",
    "receiving incremental file list",
    "building file list",
    "sent ",
    "total size is",
    "Number of ",
    "Total ",
    "Literal data",
    "Matched data",
    "File list ",
    "created directory",
    "skipping non-regular file",
    "delta-transmission",
    "done",
    "receiving file list",
    "deleting ",
];

/// How rsync's lines are heard.
#[derive(Default)]
struct Heard {
    stats: Stats,
    transfers: Vec<String>,
    transfers_total: u64,
    listing: Vec<String>,
    complaints: Vec<String>,
    percent: Option<u8>,
    changed: bool,
}

/// Whether `line` is one of rsync's itemized changes: `>f+++++++++ a.jpg`,
/// `cd+++++++++ photos/`, `*deleting   old.txt`.
fn itemized(line: &str) -> bool {
    if line.starts_with("*deleting ") {
        return true;
    }
    let bytes = line.as_bytes();
    bytes.len() > 12 && b"<>ch.".contains(&bytes[0]) && b"fdLDS".contains(&bytes[1]) && bytes[11] == b' '
}

/// The percent of a `--info=progress2` line: `  1234567  45%  10.00MB/s …`.
fn progress(line: &str) -> Option<u8> {
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let second = words.next()?;
    if !first.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return None;
    }
    second.strip_suffix('%')?.parse().ok()
}

impl Heard {
    fn hear(&mut self, line: &str, listing: bool) {
        if listing {
            if self.listing.len() < LISTED && listed(line).is_some() {
                self.listing.push(line.to_owned());
            } else if listed(line).is_none() {
                self.complain(line);
            }
            return;
        }
        if itemized(line) {
            self.transfers_total += 1;
            if self.transfers.len() < TRANSFERS_KEPT {
                self.transfers.push(line.to_owned());
            }
        } else if let Some(percent) = progress(line) {
            if self.percent != Some(percent) {
                self.percent = Some(percent);
                self.changed = true;
            }
        } else if let Some(files) = number_after(line, "Number of regular files transferred:") {
            self.stats.files = files;
        } else if let Some(bytes) = number_after(line, "Total transferred file size:") {
            self.stats.bytes = bytes;
        } else if !BENIGN.iter().any(|start| line.starts_with(start)) {
            self.complain(line);
        }
    }

    /// Keeps the last lines that may say what went wrong.
    fn complain(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.complaints.len() == 64 {
            self.complaints.remove(0);
        }
        self.complaints.push(line.to_owned());
    }

    /// What rsync and ssh said went wrong, the lines that matter.
    fn reason(&self) -> String {
        let said: Vec<&str> = self
            .complaints
            .iter()
            .map(String::as_str)
            .filter(|line| {
                line.starts_with("rsync")
                    || line.contains("vanished")
                    || line.starts_with("@ERROR")
                    || line.contains("ssh:")
                    || line.contains("Permission denied")
                    || line.contains("Host key")
                    || line.contains("No such file")
            })
            .take(6)
            .collect();
        if said.is_empty() {
            self.complaints.iter().take(4).cloned().collect::<Vec<_>>().join(" ")
        } else {
            said.join(" ")
        }
    }

    fn host_key_changed(&self) -> bool {
        self.complaints.iter().any(|line| {
            line.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
                || line.contains("Host key verification failed")
        })
    }
}

/// How rsync ended.
enum Ended {
    Done,
    /// Complete but for files it could not read, or that vanished (23, 24).
    Partial(String),
}

struct Unit<'a> {
    job: &'a UnitJob,
    scratch: &'a Scratch,
    secrets: BackupSecrets,
}

impl Unit<'_> {
    async fn work(&self, out: &mut (dyn Write + Send)) -> Result<Report, Fail> {
        if self.job.work.rclone(self.secrets.crypt.is_some()) {
            return match &self.job.work {
                Work::List { at, inside } => self.rclone_list(at, inside).await,
                Work::Run(run) => self.rclone_run(run, out).await,
                Work::Prune { at, names } => self.rclone_prune(at, names).await,
                Work::Restore(restore) => self.rclone_restore(restore, out).await,
            };
        }
        match &self.job.work {
            Work::List { at, inside } => self.list_work(&at.join(inside.as_str())).await,
            Work::Run(run) => self.run(run, out).await,
            Work::Prune { at, names } => self.prune(at, names).await.map(|()| Report::default()),
            Work::Restore(restore) => self.restore(restore, out).await,
        }
    }

    /// `rsync://user@host:port/module/path/more/` or `host:path/more/`.
    fn remote(&self, end: &End, more: &str) -> Option<String> {
        let join = |path: &str| {
            let base = path.trim_matches('/');
            let joined =
                [base, more].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("/");
            if joined.is_empty() { String::new() } else { format!("{joined}/") }
        };
        let host = |host: &BackupHostName| {
            if host.as_str().contains(':') { format!("[{host}]") } else { host.to_string() }
        };
        match end {
            End::Local { .. } | End::Cloud { .. } => None,
            // Within the module: a path from its root.
            End::Daemon { host: name, port, user, module, path } => {
                Some(format!("rsync://{user}@{}:{port}/{module}/{}", host(name), join(path.as_str())))
            }
            // From the user's home, or from `/`.
            End::Ssh { host: name, path, .. } => {
                let lead = if path.as_str().starts_with('/') { "/" } else { "" };
                Some(format!("{}:{lead}{}", host(name), join(path.as_str())))
            }
        }
    }

    /// The options a server needs: the password's file or the SSH command.
    fn server_args(&self, end: &End) -> Result<Vec<String>, Fail> {
        match end {
            End::Local { .. } => Ok(Vec::new()),
            End::Cloud { .. } => Err(refused("a cloud is reached with rclone alone")),
            End::Daemon { .. } => {
                let mut args = vec![IO_TIMEOUT.to_owned(), "--contimeout=30".to_owned()];
                if let Some(password) = &self.secrets.password {
                    let password = password.expose();
                    if password.contains(['\n', '\r']) {
                        return Err(refused("a password is one line"));
                    }
                    let path = self.scratch.once("password", &format!("{password}\n"))?;
                    args.push(format!("--password-file={}", path.display()));
                }
                Ok(args)
            }
            End::Ssh { .. } => {
                let End::Ssh { port, user, .. } = end else { unreachable!() };
                let known = self.known_hosts(end)?;
                let ssh = [
                    self.job.ssh.display().to_string(),
                    "-F".into(),
                    "/dev/null".into(),
                    "-i".into(),
                    self.job.key.display().to_string(),
                    "-o".into(),
                    "IdentitiesOnly=yes".into(),
                    "-o".into(),
                    format!("UserKnownHostsFile={}", known.display()),
                    "-o".into(),
                    "GlobalKnownHostsFile=/dev/null".into(),
                    "-o".into(),
                    "StrictHostKeyChecking=yes".into(),
                    "-o".into(),
                    "BatchMode=yes".into(),
                    "-o".into(),
                    "PasswordAuthentication=no".into(),
                    "-o".into(),
                    "KbdInteractiveAuthentication=no".into(),
                    "-o".into(),
                    "ConnectTimeout=20".into(),
                    "-o".into(),
                    "ServerAliveInterval=30".into(),
                    "-p".into(),
                    port.to_string(),
                    "-l".into(),
                    user.to_string(),
                ];
                if ssh.iter().any(|word| word.contains(char::is_whitespace)) {
                    return Err(refused("a path of the SSH command has a space"));
                }
                Ok(vec![IO_TIMEOUT.to_owned(), format!("--rsh={}", ssh.join(" "))])
            }
        }
    }

    /// The pinned host key of an SSH server, as OpenSSH (and rclone) read
    /// known hosts.
    fn known_hosts(&self, end: &End) -> Result<PathBuf, Fail> {
        let End::Ssh { host, port, host_key, .. } = end else {
            return Err(refused("no SSH server"));
        };
        let name = if *port == 22 { host.to_string() } else { format!("[{host}]:{port}") };
        self.scratch.once("known_hosts", &format!("{name} {host_key}\n"))
    }

    /// Runs rsync with `args`, passing its lines to `heard`, telling the
    /// progress to `out`.
    async fn rsync(
        &self,
        args: &[String],
        listing: bool,
        heard: &mut Heard,
        out: &mut (dyn Write + Send),
    ) -> Result<Ended, Fail> {
        let program = Program::new(&self.job.rsync).map_err(|e| failed(e.to_string()))?;
        let home = self.scratch.0.display().to_string();
        // Times as UTC, in listings too.
        let surroundings = Surroundings {
            variables: vec![("HOME".into(), home), ("TZ".into(), "UTC".into())],
            directory: None,
        };
        // The unit's own time limit ends a run that takes too long.
        let limits = Limits { timeout: std::time::Duration::from_secs(7 * 24 * 3600), max_stdout: 0 };
        let mut last_told: Option<std::time::Instant> = None;
        let output = zendiloos_exec::run_lines(&program, args, &surroundings, limits, &mut |line| {
            heard.hear(line, listing);
            if heard.changed
                && last_told.is_none_or(|told| told.elapsed() >= std::time::Duration::from_secs(1))
            {
                heard.changed = false;
                last_told = Some(std::time::Instant::now());
                if let Some(percent) = heard.percent {
                    say(out, &UnitSays::Progress { step: "transferring".into(), percent });
                }
            }
        })
        .await
        .map_err(|e| failed(e.to_string()))?;
        match output.status.code() {
            Some(0) => Ok(Ended::Done),
            Some(23 | 24) if !listing => Ok(Ended::Partial(heard.reason())),
            _ if heard.host_key_changed() => Err(Fail { message: heard.reason(), kind: FailKind::HostKey }),
            code => {
                let reason = heard.reason();
                let reason = if reason.is_empty() {
                    output.stderr.trim().lines().last().unwrap_or_default().to_owned()
                } else {
                    reason
                };
                Err(failed(format!(
                    "rsync ended with {}: {reason}",
                    code.map_or("a signal".to_owned(), |c| c.to_string())
                )))
            }
        }
    }

    /// Lists the folder at `end` (a local one already opened as `opened`).
    async fn list_raw(&self, end: &End, opened: Option<&Opened>) -> Result<(Vec<String>, bool), Fail> {
        let target = match (end, opened) {
            (End::Local { .. }, Some(opened)) => opened.arg(""),
            (End::Local { .. }, None) => return Ok((Vec::new(), true)),
            (remote, _) => self.remote(remote, "").unwrap_or_default(),
        };
        let mut args = vec!["--list-only".to_owned(), "--no-human-readable".to_owned()];
        args.extend(self.server_args(end)?);
        args.push(target);
        let mut heard = Heard::default();
        match self.rsync(&args, true, &mut heard, &mut std::io::sink()).await {
            Ok(_) => Ok((heard.listing, false)),
            Err(fail)
                if fail.kind == FailKind::Failed
                    && heard.complaints.iter().any(|l| l.contains("No such file or directory")) =>
            {
                Ok((Vec::new(), true))
            }
            Err(fail) => Err(fail),
        }
    }

    async fn list_work(&self, at: &End) -> Result<Report, Fail> {
        let opened = match at {
            End::Local { root, path } => open_beneath(root, path, false)?,
            _ => None,
        };
        let (lines, missing) = self.list_raw(at, opened.as_ref()).await?;
        Ok(Report { listing: lines.join("\n"), missing, ..Report::default() })
    }

    /// rsync's options for copying from `source` into `target`.
    fn copy_args(&self, source: &End, target: &End, plain: bool) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let local_source = matches!(source, End::Local { .. });
        let local_target = matches!(target, End::Local { .. });
        if plain {
            // FAT, exFAT, NTFS: files and folders with their times; times
            // in two-second steps on FAT.
            args.extend(["--recursive", "--times", "--modify-window=1"].map(str::to_owned));
        } else if local_source && local_target {
            args.extend(["-rlptgoH", "--acls", "--xattrs", "--numeric-ids"].map(str::to_owned));
        } else if local_source {
            // To a server: owners where it lets the user set them; ACLs
            // and extended attributes stay on the NAS.
            args.extend(["-rlptgoH", "--numeric-ids"].map(str::to_owned));
        } else {
            // From a server: its owners mean nothing here.
            args.extend(["-rlptH"].map(str::to_owned));
        }
        args.push("--safe-links".into());
        if local_source {
            args.push("--one-file-system".into());
        }
        args
    }

    fn filter_args(run: &RunJob) -> Vec<String> {
        let mut args = vec!["--exclude=.zendiloos-upload-*".to_owned()];
        if !run.hidden {
            args.push("--exclude=.*".into());
        }
        args.extend(run.excludes.iter().map(|pattern| format!("--exclude={pattern}")));
        if run.bwlimit_kib > 0 {
            args.push(format!("--bwlimit={}", run.bwlimit_kib));
        }
        args
    }

    fn source_arg(&self, source: &End, opened: Option<&Opened>) -> String {
        match opened {
            Some(opened) => opened.arg(""),
            None => self.remote(source, "").unwrap_or_default(),
        }
    }

    async fn run(&self, run: &RunJob, out: &mut (dyn Write + Send)) -> Result<Report, Fail> {
        let folder = run.folder.as_str();
        let source_opened = match &run.source {
            End::Local { root, path } => Some(
                open_beneath(root, path, false)?
                    .ok_or_else(|| refused(format!("the source {} is not there", path.as_str())))?,
            ),
            _ => None,
        };
        // The target's folder, and in it the task's.
        let (target_opened, task_folder) = match &run.target {
            End::Local { root, path } => {
                let target = open_beneath(root, path, false)?
                    .ok_or_else(|| refused(format!("the target {} is not there", path.as_str())))?;
                let task = open_in(&target, folder, true)?
                    .ok_or_else(|| failed("the task's folder cannot be made"))?;
                (Some(target), Some(task))
            }
            _ => (None, None),
        };
        let task_end = self.task_end(&run.target, folder);
        let (lines, _) = self.list_raw(&task_end, task_folder.as_ref()).await?;
        let entries: Vec<Listed> = lines.iter().filter_map(|l| listed(l)).filter(|e| e.name != ".").collect();
        let marked = entries.iter().any(|e| e.name == MARKER && !e.folder);
        if !marked && !entries.is_empty() && entries.iter().any(|e| !ours(e)) {
            return Err(refused(format!(
                "the folder {folder} in the target holds other files: it is no backup of zendiloOS's, and nothing is written into it"
            )));
        }
        if !marked {
            self.mark(&run.target, task_folder.as_ref(), folder).await?;
        }
        let plain = task_folder.as_ref().is_some_and(|f| f.plain);
        if plain && run.mode == BackupMode::Versioned {
            return Err(refused("versions need hard links, which this drive's file system has not"));
        }
        let mut args = vec![
            "--info=progress2,stats2".to_owned(),
            "--no-human-readable".to_owned(),
            "--itemize-changes".to_owned(),
        ];
        args.extend(self.copy_args(&run.source, &run.target, plain));
        args.extend(["--delete".to_owned(), "--delete-excluded".to_owned()]);
        args.extend(Self::filter_args(run));
        args.extend(self.server_args(&run.source)?);
        args.extend(self.server_args(&run.target)?);
        let stamp = run.stamp.as_str();
        let incomplete = format!("{stamp}{INCOMPLETE}");
        let mut previous: Vec<&str> =
            entries.iter().filter(|e| e.folder && is_stamp(&e.name)).map(|e| e.name.as_str()).collect();
        previous.sort_unstable();
        let mut stale: Vec<&str> = entries
            .iter()
            .filter(|e| {
                e.folder && e.name.strip_suffix(INCOMPLETE).is_some_and(is_stamp) && e.name != incomplete
            })
            .map(|e| e.name.as_str())
            .collect();
        stale.sort_unstable();
        let mut kept: Vec<Opened> = Vec::new();
        let destination = match run.mode {
            BackupMode::Mirror { keep_changed } => {
                if keep_changed {
                    args.extend(["--backup".to_owned(), format!("--backup-dir=../removed/{stamp}")]);
                }
                match &task_folder {
                    Some(task) => {
                        let current = open_in(task, "current", true)?
                            .ok_or_else(|| failed("current cannot be made"))?;
                        let arg = current.arg("");
                        // Open while rsync runs, which works through it.
                        kept.push(current);
                        arg
                    }
                    None => self.remote(&task_end, "current").unwrap_or_default(),
                }
            }
            BackupMode::Versioned => {
                if let Some(newest) = previous.last() {
                    args.push(format!("--link-dest=../{newest}"));
                }
                match &task_folder {
                    Some(task) => {
                        // A run cut off before goes on: its folder is taken
                        // over, so what it copied is not copied again.
                        if let Some(cut_off) = stale.last() {
                            rustix::fs::renameat_with(
                                &task.fd,
                                *cut_off,
                                &task.fd,
                                incomplete.as_str(),
                                RenameFlags::NOREPLACE,
                            )
                            .map_err(|e| failed(format!("{cut_off} cannot be taken over: {e}")))?;
                        }
                        let version = open_in(task, &incomplete, true)?
                            .ok_or_else(|| failed(format!("{incomplete} cannot be made")))?;
                        let arg = version.arg("");
                        kept.push(version);
                        arg
                    }
                    None => {
                        // A server cannot rename: what runs cut off left is
                        // linked from, and goes with retention.
                        for cut_off in stale.iter().rev().take(LINK_DESTS - 1) {
                            args.push(format!("--link-dest=../{cut_off}"));
                        }
                        self.remote(&task_end, &incomplete).unwrap_or_default()
                    }
                }
            }
        };
        args.push(self.source_arg(&run.source, source_opened.as_ref()));
        args.push(destination);
        say(out, &UnitSays::Progress { step: "transferring".into(), percent: 0 });
        let mut heard = Heard::default();
        let ended = self.rsync(&args, false, &mut heard, out).await?;
        if run.mode == BackupMode::Versioned {
            say(out, &UnitSays::Progress { step: "finishing".into(), percent: 100 });
            self.complete(run, &task_end, task_folder.as_ref(), source_opened.as_ref(), &incomplete).await?;
        }
        if let Some(task) = &task_folder {
            // A drive pulled out after "done" must not lose the backup.
            rustix::fs::syncfs(&task.fd)
                .map_err(|e| failed(format!("the target cannot be written out: {e}")))?;
        }
        drop((target_opened, kept));
        let (listing, _) = self.list_raw(&task_end, task_folder.as_ref()).await?;
        Ok(Report {
            files: heard.stats.files,
            bytes: heard.stats.bytes,
            partial: match ended {
                Ended::Done => None,
                Ended::Partial(reason) => Some(reason),
            },
            transfers: heard.transfers,
            transfers_total: heard.transfers_total,
            listing: listing.join("\n"),
            missing: false,
            token: None,
        })
    }

    /// The task's folder as an end: in the target folder.
    fn task_end(&self, target: &End, folder: &str) -> End {
        target.join(folder)
    }

    /// Puts the marker into the task's folder: as a file of its own on the
    /// NAS, by rsync on a server (which makes the folder too).
    async fn mark(&self, target: &End, task_folder: Option<&Opened>, folder: &str) -> Result<(), Fail> {
        if let Some(task) = task_folder {
            let file = rustix::fs::openat(
                &task.fd,
                MARKER,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o644),
            );
            return match file {
                Ok(file) => rustix::io::write(&file, MARKER_TEXT.as_bytes())
                    .map(|_| ())
                    .map_err(|e| failed(format!("{MARKER}: {e}"))),
                Err(Errno::EXIST) => Ok(()),
                Err(e) => Err(failed(format!("{MARKER}: {e}"))),
            };
        }
        let stage = self.scratch.folder(&format!("stage/{folder}"))?;
        std::fs::write(stage.join(MARKER), MARKER_TEXT).map_err(|e| failed(format!("{MARKER}: {e}")))?;
        let mut args = vec!["--recursive".to_owned()];
        args.extend(self.server_args(target)?);
        args.push(stage.display().to_string());
        args.push(self.remote(target, "").unwrap_or_default());
        self.rsync(&args, false, &mut Heard::default(), &mut std::io::sink()).await.map(|_| ())
    }

    /// Gives a version its name once complete: renamed on the NAS; on a
    /// server, copied once more as hard links (a server cannot rename)
    /// and the incomplete folder removed.
    async fn complete(
        &self,
        run: &RunJob,
        task_end: &End,
        task_folder: Option<&Opened>,
        source: Option<&Opened>,
        incomplete: &str,
    ) -> Result<(), Fail> {
        let stamp = run.stamp.as_str();
        if let Some(task) = task_folder {
            rustix::fs::renameat_with(&task.fd, incomplete, &task.fd, stamp, RenameFlags::NOREPLACE)
                .map_err(|e| failed(format!("the version cannot be named {stamp}: {e}")))?;
            return rustix::fs::fsync(&task.fd)
                .map_err(|e| failed(format!("the target cannot be written out: {e}")));
        }
        let mut args = vec!["--no-human-readable".to_owned()];
        args.extend(self.copy_args(&run.source, &run.target, false));
        args.extend(["--delete".to_owned(), "--delete-excluded".to_owned()]);
        args.extend(Self::filter_args(run));
        args.extend(self.server_args(&run.source)?);
        args.extend(self.server_args(&run.target)?);
        args.push(format!("--link-dest=../{incomplete}"));
        args.push(self.source_arg(&run.source, source));
        args.push(self.remote(task_end, stamp).unwrap_or_default());
        self.rsync(&args, false, &mut Heard::default(), &mut std::io::sink()).await?;
        let name = BackupPruned::parse(incomplete).map_err(|_| failed("no version cut off"))?;
        self.prune(task_end, &[name]).await
    }

    /// Removes `names` from the task's folder `at` with rsync: an empty
    /// folder synced over it with `--delete`, only those names included,
    /// everything else excluded and so kept.
    async fn prune(&self, at: &End, names: &[BackupPruned]) -> Result<(), Fail> {
        if names.is_empty() {
            return Ok(());
        }
        let opened = match at {
            End::Local { root, path } => match open_beneath(root, path, false)? {
                Some(opened) => Some(opened),
                None => return Ok(()),
            },
            _ => None,
        };
        let empty = self.scratch.folder("empty")?;
        let removed = names.iter().any(|name| name.as_str().starts_with("removed/"));
        if removed {
            std::fs::create_dir_all(empty.join("removed")).map_err(|e| failed(e.to_string()))?;
        }
        let mut args = vec!["--recursive".to_owned(), "--delete".to_owned()];
        args.extend(self.server_args(at)?);
        args.extend(names.iter().map(|name| format!("--include=/{name}/***")));
        if removed {
            args.push("--include=/removed/".into());
        }
        args.push("--exclude=*".into());
        args.push(format!("{}/", empty.display()));
        args.push(match &opened {
            Some(opened) => opened.arg(""),
            None => self.remote(at, "").unwrap_or_default(),
        });
        self.rsync(&args, false, &mut Heard::default(), &mut std::io::sink()).await.map(|_| ())
    }

    async fn restore(&self, restore: &RestoreJob, out: &mut (dyn Write + Send)) -> Result<Report, Fail> {
        let End::Local { root, path } = &restore.to else {
            return Err(refused("a restore goes into a folder of the NAS"));
        };
        let from = restore.from.join(restore.within.as_str());
        let from = &from;
        let from_opened = match from {
            End::Local { root, path } => Some(
                open_beneath(root, path, false)?
                    .ok_or_else(|| refused("the backup to restore from is not there"))?,
            ),
            _ => None,
        };
        let to = open_beneath(root, path, true)?
            .ok_or_else(|| failed("the folder to restore into cannot be made"))?;
        let mut args = vec![
            "--info=progress2,stats2".to_owned(),
            "--no-human-readable".to_owned(),
            "--itemize-changes".to_owned(),
        ];
        if to.plain {
            args.extend(["--recursive", "--times", "--modify-window=1"].map(str::to_owned));
        } else if restore.into_share {
            // As the explorer's copies: the folder's owner, the share's
            // group, and the share's ACL inherited (modes not kept).
            let stat = rustix::fs::fstat(&to.fd).map_err(|e| failed(e.to_string()))?;
            args.extend(["-rltH", "--xattrs", "--owner", "--group", "--numeric-ids"].map(str::to_owned));
            args.push(format!("--chown={}:{}", stat.st_uid, stat.st_gid));
        } else {
            args.extend(["-rltH"].map(str::to_owned));
        }
        args.push("--safe-links".into());
        if restore.existing == BackupExisting::Skip {
            args.push("--ignore-existing".into());
        }
        if !restore.paths.is_empty() {
            let list: String = restore.paths.iter().map(|p| format!("{}\n", p.as_str())).collect();
            let file = self.scratch.file("paths", &list)?;
            args.push(format!("--files-from={}", file.display()));
        }
        args.extend(self.server_args(from)?);
        args.push(self.source_arg(from, from_opened.as_ref()));
        args.push(to.arg(""));
        say(out, &UnitSays::Progress { step: "restoring".into(), percent: 0 });
        let mut heard = Heard::default();
        let ended = self.rsync(&args, false, &mut heard, out).await?;
        rustix::fs::syncfs(&to.fd)
            .map_err(|e| failed(format!("what was restored cannot be written out: {e}")))?;
        Ok(Report {
            files: heard.stats.files,
            bytes: heard.stats.bytes,
            partial: match ended {
                Ended::Done => None,
                Ended::Partial(reason) => Some(reason),
            },
            ..Report::default()
        })
    }
}

#[cfg(test)]
mod tests;
