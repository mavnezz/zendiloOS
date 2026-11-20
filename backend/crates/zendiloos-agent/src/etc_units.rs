//! Changes to the files that lie directly in `/etc`, each in a transient
//! unit of its own (docs/architecture.md, 4.6).
//!
//! The agent's own sandbox may write only the folders below `/etc` it works
//! in (`ReadWritePaths=` of zendiloos-agent.service), never `/etc` itself.
//! The files there it changes are replaced by renaming a new file onto
//! them, and the shadow tools lock theirs with files made beside them
//! (`/etc/.pwd.lock`, `passwd.lock`, `passwd+`, `passwd-`), so a single
//! file bound writable would not do: a rename onto a mount point fails
//! (EBUSY). These changes run in transient units (`systemd-run`, waited
//! for) that may write `/etc` and do nothing else:
//!
//! - users and groups: the shadow tools (useradd, userdel, groupadd,
//!   groupdel, gpasswd) on `passwd`, `group`, `shadow`, `gshadow`, `subuid`
//!   and `subgid`, with the arguments the agent checked, and `CAP_CHOWN`
//!   alone (`/etc/shadow` keeps the group `shadow`); of `/run` they see the
//!   journal's socket for their log lines and nothing else;
//! - `/etc/hosts`, `/etc/hostid`, `/etc/resolv.conf` and the copy of what
//!   ifupdown had beside it ([`ETC_FILES`], by name): this binary as
//!   `put-etc-file`, which makes one of them a file of what comes on its
//!   standard input, a link or nothing, as [`crate::durable`] does, with no
//!   capability and connecting to nothing.
//!
//! Neither sees zendiloOS's secrets, SSH's host keys or the database
//! (`put-etc-file` not the shadow files either), the network, devices or
//! other processes, and neither may run for more than 50 seconds.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zendiloos_exec::{ExecError, Limits, Program};

use crate::operations::OperationError;
use crate::volume_archive::{decode, encode};

/// The files directly in `/etc` that `put-etc-file` writes, by name: the
/// machine's own names, ZFS's host id, the resolver's settings and what
/// they were before a takeover of the network (`network::takeover`).
pub const ETC_FILES: [&str; 4] = ["hosts", "hostid", "resolv.conf", "resolv.conf.zendiloos-previous"];

/// Far more than any of [`ETC_FILES`] takes.
const MAX_FILE: u64 = 256 * 1024;

/// What the agent waits for a unit: longer than it may run.
const LIMITS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 64 * 1024 };

/// What both kinds of unit have: the agent's own sandbox, but no devices,
/// no other processes (a PID namespace of their own, whose first process
/// takes no signal it has no handler for, so they are stopped with
/// SIGKILL), an empty read-only `/run` and less than a minute.
const SANDBOX: [&str; 32] = [
    "--quiet",
    "--wait",
    "--pipe",
    "--collect",
    "--service-type=exec",
    "--property=NoNewPrivileges=yes",
    "--property=ProtectSystem=strict",
    "--property=ProtectHome=yes",
    "--property=PrivateTmp=yes",
    "--property=PrivateDevices=yes",
    "--property=PrivateNetwork=yes",
    "--property=PrivateIPC=yes",
    "--property=ProtectProc=invisible",
    "--property=PrivatePIDs=yes",
    "--property=KillSignal=SIGKILL",
    "--property=ProtectKernelTunables=yes",
    "--property=ProtectKernelModules=yes",
    "--property=ProtectKernelLogs=yes",
    "--property=ProtectControlGroups=yes",
    "--property=ProtectClock=yes",
    "--property=ProtectHostname=yes",
    "--property=RestrictNamespaces=yes",
    "--property=RestrictRealtime=yes",
    "--property=RestrictSUIDSGID=yes",
    "--property=LockPersonality=yes",
    "--property=MemoryDenyWriteExecute=yes",
    "--property=RestrictAddressFamilies=AF_UNIX",
    "--property=SystemCallArchitectures=native",
    "--property=SystemCallFilter=@system-service",
    "--property=TemporaryFileSystem=/run:ro",
    "--property=RuntimeMaxSec=50",
    "--property=MemoryMax=256M",
];

/// The shadow tools' unit besides: `/etc`, the capability to give
/// `/etc/shadow` its group, and journald's socket, through which they log
/// what they changed (`/dev/log` leads there).
const SHADOW_TOOLS: [&str; 4] = [
    "--property=ReadWritePaths=/etc",
    "--property=CapabilityBoundingSet=CAP_CHOWN",
    "--property=InaccessiblePaths=-/etc/zendiloos -/etc/ssh -/var/lib/zendiloos",
    "--property=BindReadOnlyPaths=-/run/systemd/journal/dev-log",
];

/// `put-etc-file`'s unit besides its folder: no capability, none of the
/// system's secrets, and no `connect()` (it talks over its pipes).
const PUT_FILE: [&str; 3] = [
    "--property=CapabilityBoundingSet=",
    "--property=InaccessiblePaths=-/etc/zendiloos -/etc/shadow -/etc/gshadow -/etc/ssh -/var/lib/zendiloos",
    "--property=SystemCallFilter=~connect:EPERM",
];

/// What a file directly in `/etc` is to become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EtcFile<'a> {
    /// Gone; gone already is fine.
    Missing,
    /// A file of `content` with the permission bits `mode`.
    File { content: &'a [u8], mode: u32 },
    /// A symbolic link to `target`.
    Link { target: &'a str },
}

/// `put-etc-file`'s job; a file's content comes on its standard input.
#[derive(Debug, Serialize, Deserialize)]
struct PutJob {
    path: PathBuf,
    put: Put,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Put {
    Missing,
    File { mode: u32 },
    Link { target: String },
}

/// Starts the units that change what lies directly in `/etc`.
#[derive(Debug, Clone)]
pub(crate) struct EtcUnits {
    systemd_run: Program,
    /// This program, which a unit runs as `put-etc-file`.
    agent: PathBuf,
}

impl EtcUnits {
    pub(crate) fn new(systemd_run: Program, agent: PathBuf) -> Self {
        Self { systemd_run, agent }
    }

    /// Runs `tool`, one of the shadow tools, with `args` in its unit, and
    /// fails as it fails.
    pub(crate) async fn shadow_tool(&self, tool: &Program, args: &[&str]) -> Result<(), OperationError> {
        let name =
            tool.path().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let path = tool.path().to_string_lossy().into_owned();
        let description = format!("--description=zendiloOS: {name} on the accounts");
        let mut command: Vec<&str> = SANDBOX.to_vec();
        command.push(&description);
        command.extend(SHADOW_TOOLS);
        command.push(&path);
        command.extend(args);
        let output = zendiloos_exec::run(&self.systemd_run, &command, LIMITS).await?;
        if output.status.success() {
            return Ok(());
        }
        Err(ExecError::Failed { program: name, status: output.status, stderr: output.stderr }.into())
    }

    /// Makes `path`, one of [`ETC_FILES`], what `file` says, in a unit that
    /// may write its folder.
    pub(crate) async fn put(&self, path: &Path, file: EtcFile<'_>) -> Result<(), OperationError> {
        let folder = place(path).map_err(OperationError::Refused)?;
        let (put, input) = match file {
            EtcFile::Missing => (Put::Missing, &[][..]),
            EtcFile::File { content, mode } => (Put::File { mode }, content),
            EtcFile::Link { target } => (Put::Link { target: target.to_owned() }, &[][..]),
        };
        let job = encode(&PutJob { path: path.to_owned(), put });
        let description = format!("--description=zendiloOS: writing {}", path.display());
        let writable = format!("--property=ReadWritePaths={}", folder.display());
        let agent = self.agent.to_string_lossy().into_owned();
        let mut command: Vec<&str> = SANDBOX.to_vec();
        command.push(&description);
        command.extend(PUT_FILE);
        command.extend([writable.as_str(), agent.as_str(), "put-etc-file", job.as_str()]);
        let output = zendiloos_exec::run_with_input(&self.systemd_run, &command, input, LIMITS).await?;
        if output.status.success() {
            return Ok(());
        }
        let why = output.stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("it failed");
        Err(OperationError::Failed(format!("{} was not written: {why}", path.display())))
    }
}

/// The folder of `path` if it is one of [`ETC_FILES`], named plainly: an
/// absolute path with no `..` in it (a `.` within it names nothing else).
fn place(path: &Path) -> Result<&Path, String> {
    let plain = path.is_absolute()
        && path.components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    let ours = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| ETC_FILES.contains(&name));
    match path.parent() {
        Some(folder) if plain && ours => Ok(folder),
        _ => Err(format!("{} is no file of /etc that zendiloOS writes", path.display())),
    }
}

/// An error with what caused it, in one line.
fn told(error: &OperationError) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

/// `put-etc-file <job>`, in its unit: does what `job` says, a file's
/// content read from `input`.
pub fn run_put_etc_file(job: &str, input: &mut impl Read) -> Result<(), String> {
    let job: PutJob = decode(job)?;
    place(&job.path)?;
    match job.put {
        Put::Missing => match fs::remove_file(&job.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(format!("cannot remove {}: {error}", job.path.display()))
            }
            _ => Ok(()),
        },
        Put::File { mode } => {
            let mut content = Vec::new();
            input
                .take(MAX_FILE + 1)
                .read_to_end(&mut content)
                .map_err(|error| format!("cannot read what to write: {error}"))?;
            if content.len() as u64 > MAX_FILE {
                return Err(format!("more than {MAX_FILE} bytes for {}", job.path.display()));
            }
            crate::durable::replace_mode(&job.path, &content, mode & 0o777).map_err(|error| told(&error))
        }
        Put::Link { target } => crate::durable::link(&job.path, &target).map_err(|error| told(&error)),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn job(path: &Path, put: Put) -> String {
        encode(&PutJob { path: path.to_owned(), put })
    }

    #[test]
    fn only_the_files_of_etc_zendiloos_writes_are_written() {
        assert_eq!(place(Path::new("/etc/hosts")), Ok(Path::new("/etc")));
        assert_eq!(place(Path::new("/etc/resolv.conf.zendiloos-previous")), Ok(Path::new("/etc")));
        for other in [
            "/etc/shadow",
            "/etc/passwd",
            "/etc/zendiloos/hosts/x",
            "hosts",
            "/etc/../root/hosts",
            "/etc/ssh/../hosts",
        ] {
            let path = Path::new(other);
            assert!(place(path).is_err(), "{other}");
            assert!(run_put_etc_file(&job(path, Put::Missing), &mut io::empty()).is_err(), "{other}");
        }
    }

    #[test]
    fn a_file_a_link_or_nothing_takes_the_place_whole() {
        let folder = tempfile::tempdir().unwrap();
        let hosts = folder.path().join("hosts");
        fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();

        let new = b"127.0.0.1 localhost\n127.0.1.1 office\n";
        run_put_etc_file(&job(&hosts, Put::File { mode: 0o4644 }), &mut &new[..]).unwrap();
        assert_eq!(fs::read(&hosts).unwrap(), new);
        assert_eq!(fs::metadata(&hosts).unwrap().permissions().mode() & 0o7777, 0o644, "no set-ID bits");

        let resolv = folder.path().join("resolv.conf");
        let stub = "../run/systemd/resolve/stub-resolv.conf";
        run_put_etc_file(&job(&resolv, Put::Link { target: stub.into() }), &mut io::empty()).unwrap();
        assert_eq!(fs::read_link(&resolv).unwrap(), Path::new(stub));

        run_put_etc_file(&job(&resolv, Put::Missing), &mut io::empty()).unwrap();
        run_put_etc_file(&job(&resolv, Put::Missing), &mut io::empty()).unwrap();
        assert!(resolv.symlink_metadata().is_err());
        let names: Vec<_> = fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["hosts"], "nothing left beside them");
    }

    #[test]
    fn too_much_is_refused_before_anything_is_written() {
        let folder = tempfile::tempdir().unwrap();
        let host_id = folder.path().join("hostid");
        fs::write(&host_id, [1, 2, 3, 4]).unwrap();
        let large = vec![b'x'; MAX_FILE as usize + 1];
        let refused = run_put_etc_file(&job(&host_id, Put::File { mode: 0o644 }), &mut &large[..]);
        assert!(refused.unwrap_err().contains("more than"));
        assert_eq!(fs::read(&host_id).unwrap(), [1, 2, 3, 4]);
        assert!(run_put_etc_file("not hex", &mut io::empty()).is_err());
    }
}
