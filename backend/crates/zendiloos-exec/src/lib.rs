//! The only way zendiloos runs external programs.
//!
//! The rules are enforced here, so no caller can get them wrong:
//!
//! - no shell, ever: the program is executed directly, arguments are passed
//!   as a list and never joined or parsed;
//! - absolute paths only, so a manipulated `PATH` cannot substitute another
//!   binary;
//! - an empty environment (so also the C locale and stable tool output) and
//!   `/` as working directory, unless the caller names variables and a
//!   directory ([`run_with`]: Docker Compose reads its values from there);
//! - stdin is closed unless the caller passes input, which is how secrets
//!   reach a program: never on the command line, where `/proc` shows them;
//! - a timeout, after which the program is killed, and a limit on the
//!   output that is read.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// An executable at a checked, absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    path: PathBuf,
}

impl Program {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ExecError> {
        let path = path.into();
        if path.is_absolute() { Ok(Self { path }) } else { Err(ExecError::RelativePath(path)) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn name(&self) -> String {
        self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    /// Most bytes read from stdout; more is an error, not a silent truncation.
    pub max_stdout: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(10), max_stdout: 1024 * 1024 }
    }
}

/// Bytes of stderr kept for error messages.
const MAX_STDERR: usize = 16 * 1024;

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// At most 16 KiB, lossily decoded.
    pub stderr: String,
}

impl Output {
    /// `Ok(self)` if the program exited with status 0.
    pub fn success(self, program: &Program) -> Result<Self, ExecError> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(ExecError::Failed { program: program.name(), status: self.status, stderr: self.stderr })
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("program path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),
    #[error("cannot run {program}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("{program} did not finish within {after:?} and was killed")]
    Timeout { program: String, after: Duration },
    #[error("{program} wrote more than {limit} bytes")]
    OutputTooLarge { program: String, limit: usize },
    #[error("{program} failed with {status}: {stderr}")]
    Failed { program: String, status: ExitStatus, stderr: String },
    #[error("I/O with {program} failed")]
    Io {
        program: String,
        #[source]
        source: io::Error,
    },
}

/// Runs `program` with `args` and waits for it to finish.
pub async fn run<I, A>(program: &Program, args: I, limits: Limits) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    run_inner(program, args, Stdin::Null, &Surroundings::default(), limits).await
}

/// Variables and a working directory for [`run_with`]; nothing else of the
/// caller's environment reaches the program.
#[derive(Debug, Default)]
pub struct Surroundings<'a> {
    pub variables: Vec<(String, String)>,
    pub directory: Option<&'a Path>,
}

/// Like [`run_with_input`] (or [`run`] without `input`), with `surroundings`.
pub async fn run_with<I, A>(
    program: &Program,
    args: I,
    input: Option<&[u8]>,
    surroundings: &Surroundings<'_>,
    limits: Limits,
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    let input = input.map_or(Stdin::Null, Stdin::Bytes);
    run_inner(program, args, input, surroundings, limits).await
}

/// Like [`run`], but writes `input` to the program's stdin and closes it.
pub async fn run_with_input<I, A>(
    program: &Program,
    args: I,
    input: &[u8],
    limits: Limits,
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    run_inner(program, args, Stdin::Bytes(input), &Surroundings::default(), limits).await
}

/// Like [`run`], with an opened file as the program's stdin. The program
/// names it `/proc/self/fd/0`: exactly the file opened, whatever its path
/// leads to by now.
pub async fn run_on<I, A>(
    program: &Program,
    args: I,
    file: std::fs::File,
    limits: Limits,
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    run_inner(program, args, Stdin::File(file), &Surroundings::default(), limits).await
}

/// Like [`run_with`] without input, but with stderr written into the same
/// pipe as stdout, in the order the program wrote them: for output such as
/// a container's, whose two streams belong together. `stderr` of the
/// result is empty.
pub async fn run_combined<I, A>(
    program: &Program,
    args: I,
    surroundings: &Surroundings<'_>,
    limits: Limits,
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    let name = program.name();
    let io_error = |source| ExecError::Io { program: name.clone(), source };
    let (reader, writer) = io::pipe().map_err(io_error)?;
    let mut command = Command::new(program.path());
    command
        .args(args)
        .env_clear()
        .envs(surroundings.variables.iter().map(|(name, value)| (name, value)))
        .current_dir(surroundings.directory.unwrap_or(Path::new("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer.try_clone().map_err(io_error)?))
        .stderr(Stdio::from(writer))
        .process_group(0)
        .kill_on_drop(true);
    let mut child =
        spawn(&mut command).await.map_err(|source| ExecError::Spawn { program: name.clone(), source })?;
    let mut group = Group::of(&child);
    // Only the child may hold the writing end, or reading never ends.
    drop(command);
    let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(std::os::fd::OwnedFd::from(reader))
        .map_err(io_error)?;

    let finished = async {
        let stdout = read_limited(reader, limits.max_stdout).await.map_err(io_error)?;
        if stdout.len() > limits.max_stdout {
            return Err(ExecError::OutputTooLarge { program: name.clone(), limit: limits.max_stdout });
        }
        let status = child.wait().await.map_err(io_error)?;
        group.ended();
        Ok(Output { status, stdout, stderr: String::new() })
    };
    match tokio::time::timeout(limits.timeout, finished).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout { program: name.clone(), after: limits.timeout }),
    }
}

/// Longest line [`run_lines`] passes on at once; a longer one comes in
/// pieces of this size.
pub const MAX_LINE: usize = 4096;

/// Like [`run_combined`], but passes each line the program writes, to
/// either stream, to `line` as it comes: without its line break (`\n`, or
/// `\r` as progress bars use), lossily decoded, empty ones left out. Lines
/// are passed on, not kept, so `limits.max_stdout` does not apply; the
/// last 16 KiB written are kept for an error message, as `stderr` of the
/// result, and `stdout` is empty.
pub async fn run_lines<I, A>(
    program: &Program,
    args: I,
    surroundings: &Surroundings<'_>,
    limits: Limits,
    line: &mut (dyn FnMut(&str) + Send),
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    let name = program.name();
    let io_error = |source| ExecError::Io { program: name.clone(), source };
    let (reader, writer) = io::pipe().map_err(io_error)?;
    let mut command = Command::new(program.path());
    command
        .args(args)
        .env_clear()
        .envs(surroundings.variables.iter().map(|(name, value)| (name, value)))
        .current_dir(surroundings.directory.unwrap_or(Path::new("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer.try_clone().map_err(io_error)?))
        .stderr(Stdio::from(writer))
        .process_group(0)
        .kill_on_drop(true);
    let mut child =
        spawn(&mut command).await.map_err(|source| ExecError::Spawn { program: name.clone(), source })?;
    let mut group = Group::of(&child);
    // Only the child may hold the writing end, or reading never ends.
    drop(command);
    let mut reader = tokio::net::unix::pipe::Receiver::from_owned_fd(std::os::fd::OwnedFd::from(reader))
        .map_err(io_error)?;

    let finished = async {
        let mut lines = Lines::default();
        let mut chunk = [0u8; 8192];
        loop {
            let read = reader.read(&mut chunk).await.map_err(io_error)?;
            if read == 0 {
                break;
            }
            lines.take(&chunk[..read], line);
        }
        lines.flush(line);
        let status = child.wait().await.map_err(io_error)?;
        group.ended();
        Ok(Output { status, stdout: Vec::new(), stderr: lines.tail() })
    };
    match tokio::time::timeout(limits.timeout, finished).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout { program: name.clone(), after: limits.timeout }),
    }
}

/// Splits what a program writes into lines, keeping the last bytes.
#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
}

impl Lines {
    fn take(&mut self, bytes: &[u8], line: &mut (dyn FnMut(&str) + Send)) {
        self.tail.extend(bytes);
        let excess = self.tail.len().saturating_sub(MAX_STDERR);
        self.tail.drain(..excess);
        for &byte in bytes {
            if byte == b'\n' || byte == b'\r' {
                self.flush(line);
            } else {
                self.pending.push(byte);
                if self.pending.len() >= MAX_LINE {
                    self.flush(line);
                }
            }
        }
    }

    fn flush(&mut self, line: &mut (dyn FnMut(&str) + Send)) {
        if !self.pending.is_empty() {
            line(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }

    fn tail(&self) -> String {
        let bytes: Vec<u8> = self.tail.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A program started on a terminal of its own ([`start_on_terminal`]).
#[derive(Debug)]
pub struct OnTerminal {
    /// The terminal's master side: what is written to it is typed, what is
    /// read from it is what the program wrote, its size is set on it.
    /// Closing it hangs up the terminal.
    pub master: std::os::fd::OwnedFd,
    /// The program, to wait for; dropping it kills it.
    pub child: tokio::process::Child,
}

/// Starts `program` with `args` on a new pseudo terminal of `columns` and
/// `rows`, as the leader of a session of its own whose controlling
/// terminal it is (through `setsid --ctty`, as no code here may run
/// between fork and exec): its stdin, stdout and stderr are the terminal,
/// it gets `SIGWINCH` when the size changes and `SIGHUP` when the master
/// is closed. Same rules as the other ways: no shell, the environment and
/// directory of `surroundings` only. No timeout: the caller waits for it.
pub async fn start_on_terminal<I, A>(
    setsid: &Program,
    program: &Program,
    args: I,
    surroundings: &Surroundings<'_>,
    columns: u16,
    rows: u16,
) -> Result<OnTerminal, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;

    let name = program.name();
    let io_error = |source: io::Error| ExecError::Io { program: name.clone(), source };
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .map_err(|e| io_error(e.into()))?;
    rustix::pty::grantpt(&master).map_err(|e| io_error(e.into()))?;
    rustix::pty::unlockpt(&master).map_err(|e| io_error(e.into()))?;
    let path = rustix::pty::ptsname(&master, Vec::new()).map_err(|e| io_error(e.into()))?;
    let slave =
        rustix::fs::open(path.as_c_str(), OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC, Mode::empty())
            .map_err(|e| io_error(e.into()))?;
    let size = rustix::termios::Winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    rustix::termios::tcsetwinsize(&master, size).map_err(|e| io_error(e.into()))?;
    let slave = std::fs::File::from(slave);

    let mut command = Command::new(setsid.path());
    command
        .arg("--ctty")
        .arg(program.path())
        .args(args)
        .env_clear()
        .envs(surroundings.variables.iter().map(|(name, value)| (name, value)))
        .current_dir(surroundings.directory.unwrap_or(Path::new("/")))
        .stdin(Stdio::from(slave.try_clone().map_err(io_error)?))
        .stdout(Stdio::from(slave.try_clone().map_err(io_error)?))
        .stderr(Stdio::from(slave))
        .kill_on_drop(true);
    let child =
        spawn(&mut command).await.map_err(|source| ExecError::Spawn { program: name.clone(), source })?;
    // The child holds the terminal's slave side now; ours is gone with
    // `command`, so the master reads end of file once the program ended.
    drop(command);
    Ok(OnTerminal { master, child })
}

enum Stdin<'a> {
    Null,
    Bytes(&'a [u8]),
    File(std::fs::File),
}

/// Starts `command`. A program file that was just written can be busy for a
/// moment: a child forked meanwhile by another thread holds it open for
/// writing until it calls `exec` (Linux reports `ETXTBSY`). That happens
/// in tests that write fake programs; starting again shortly after works.
async fn spawn(command: &mut Command) -> io::Result<tokio::process::Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10 * attempts)).await;
            }
            result => return result,
        }
    }
}

async fn run_inner<I, A>(
    program: &Program,
    args: I,
    input: Stdin<'_>,
    surroundings: &Surroundings<'_>,
    limits: Limits,
) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    let name = program.name();
    let io_error = |source| ExecError::Io { program: name.clone(), source };

    let mut command = Command::new(program.path());
    command
        .args(args)
        .env_clear()
        .envs(surroundings.variables.iter().map(|(name, value)| (name, value)))
        .current_dir(surroundings.directory.unwrap_or(Path::new("/")))
        .stdin(match &input {
            Stdin::Null => Stdio::null(),
            Stdin::Bytes(_) => Stdio::piped(),
            Stdin::File(file) => Stdio::from(file.try_clone().map_err(io_error)?),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping the child (on timeout or error) kills the program, and
        // the group of its own what it started.
        .process_group(0)
        .kill_on_drop(true);
    let mut child =
        spawn(&mut command).await.map_err(|source| ExecError::Spawn { program: name.clone(), source })?;
    let mut group = Group::of(&child);

    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let finished = async {
        let feed = async {
            if let (Some(stdin), Stdin::Bytes(input)) = (stdin.as_mut(), &input) {
                stdin.write_all(input).await?;
                stdin.shutdown().await?;
            }
            drop(stdin.take());
            Ok::<_, io::Error>(())
        };
        // Output over the limit ends it at once: the program would wait for
        // its pipe to be read, and the others for the program.
        let too_large = || ExecError::OutputTooLarge { program: name.clone(), limit: limits.max_stdout };
        let capped = async {
            let stdout = read_limited(stdout, limits.max_stdout).await.map_err(io_error)?;
            if stdout.len() > limits.max_stdout { Err(too_large()) } else { Ok(stdout) }
        };
        let (_, stdout, stderr) = tokio::try_join!(async { feed.await.map_err(io_error) }, capped, async {
            read_draining(stderr, MAX_STDERR).await.map_err(io_error)
        },)?;
        let status = child.wait().await.map_err(io_error)?;
        group.ended();
        let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
        // Replacement characters may have made it longer; cut between
        // characters.
        stderr.truncate(stderr.floor_char_boundary(MAX_STDERR));
        Ok(Output { status, stdout, stderr })
    };

    match tokio::time::timeout(limits.timeout, finished).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout { program: name.clone(), after: limits.timeout }),
    }
}

/// The process group a program was started in (its own), killed when this
/// is dropped before the program ended: with the program, what it started.
struct Group(Option<rustix::process::Pid>);

impl Group {
    fn of(child: &tokio::process::Child) -> Self {
        Self(child.id().and_then(|id| rustix::process::Pid::from_raw(id.cast_signed())))
    }

    /// The program ended: what it left running is its business.
    fn ended(&mut self) {
        self.0 = None;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // Before the child is dropped and reaped, so the group's ID is
        // still taken and cannot be anybody else's.
        if let Some(group) = self.0 {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }
}

/// Reads up to `limit` bytes and throws the rest away until the end, so
/// the program is never kept waiting on its pipe.
async fn read_draining(reader: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut reader = reader;
    let mut buffer = Vec::new();
    (&mut reader).take(limit as u64).read_to_end(&mut buffer).await?;
    tokio::io::copy(&mut reader, &mut tokio::io::sink()).await?;
    Ok(buffer)
}

/// Reads up to `limit + 1` bytes, so the caller can tell "exactly at the
/// limit" from "over it", and stops reading there.
async fn read_limited(reader: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut buffer).await?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only tests use a shell, to script the behaviour under test.
    fn sh() -> Program {
        Program::new("/bin/sh").unwrap()
    }

    /// Reads from a terminal's master until `wanted` came, or it ended.
    fn read_until(master: &std::os::fd::OwnedFd, wanted: &str) -> String {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 256];
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match rustix::io::read(master, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buffer[..n]),
            }
            if String::from_utf8_lossy(&seen).contains(wanted) {
                break;
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    #[tokio::test]
    async fn a_program_on_a_terminal_is_its_session_and_reads_what_is_typed() {
        let setsid = Program::new("/usr/bin/setsid").unwrap();
        let script = "stty size; [ -t 0 ] && echo on-a-terminal; read line; echo got:$line; \
                      [ \"$(ps -o sid= -p $$ | tr -d ' ')\" = \"$$\" ] && echo leader";
        let started = start_on_terminal(&setsid, &sh(), ["-c", script], &Surroundings::default(), 100, 30)
            .await
            .unwrap();
        let master = started.master;
        let mut child = started.child;
        let seen = read_until(&master, "on-a-terminal");
        assert!(seen.contains("30 100"), "{seen:?}");
        rustix::io::write(&master, b"hello\n").unwrap();
        let seen = read_until(&master, "leader");
        assert!(seen.contains("got:hello"), "{seen:?}");
        assert!(seen.contains("leader"), "the program leads a session of its own: {seen:?}");
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn closing_the_master_hangs_up_the_program() {
        let setsid = Program::new("/usr/bin/setsid").unwrap();
        let started = start_on_terminal(
            &setsid,
            &sh(),
            ["-c", "echo ready; sleep 60"],
            &Surroundings::default(),
            80,
            24,
        )
        .await
        .unwrap();
        let mut child = started.child;
        let seen = read_until(&started.master, "ready");
        assert!(seen.contains("ready"), "{seen:?}");
        drop(started.master);
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap();
        assert!(!status.success(), "ended by the hangup: {status}");
    }

    #[tokio::test]
    async fn output_over_the_limit_ends_it_at_once() {
        let started = std::time::Instant::now();
        let limits = Limits { timeout: Duration::from_secs(20), max_stdout: 1024 };
        let script = "while :; do echo xxxxxxxxxxxxxxxx; done";
        let error = run(&sh(), ["-c", script], limits).await.unwrap_err();
        assert!(matches!(error, ExecError::OutputTooLarge { .. }), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10), "not after the timeout");
    }

    #[tokio::test]
    async fn much_said_on_stderr_does_not_keep_a_program_waiting() {
        let script = "i=0; while [ $i -lt 4000 ]; do \
            echo 'a line of complaint that goes on and on and on and on and on' >&2; i=$((i+1)); done; \
            printf '\\377\\377' >&2; echo done";
        let limits = Limits { timeout: Duration::from_secs(20), ..Limits::default() };
        let output = run(&sh(), ["-c", script], limits).await.unwrap();
        assert_eq!(output.stdout, b"done\n");
        assert!(
            output.stderr.len() <= MAX_STDERR && output.stderr.starts_with("a line"),
            "{}",
            output.stderr.len()
        );
    }

    #[tokio::test]
    async fn a_program_killed_takes_what_it_started_along() {
        let dir = std::env::temp_dir().join(format!("zendiloos-exec-group-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", file.display());
        let limits = Limits { timeout: Duration::from_millis(500), ..Limits::default() };
        let error = run(&sh(), ["-c", &script], limits).await.unwrap_err();
        assert!(matches!(error, ExecError::Timeout { .. }), "{error}");
        let pid: u32 = std::fs::read_to_string(&file).unwrap().trim().parse().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let alive = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit(')').next().is_some_and(|rest| !rest.trim_start().starts_with('Z'))
            })
        };
        for _ in 0..50 {
            if !alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("what the program started still runs");
    }

    #[tokio::test]
    async fn combined_output_keeps_the_order_the_program_wrote() {
        let script = "echo one; echo two >&2; echo three";
        let output =
            run_combined(&sh(), ["-c", script], &Surroundings::default(), Limits::default()).await.unwrap();
        assert_eq!(output.stdout, b"one\ntwo\nthree\n");
        assert!(output.status.success() && output.stderr.is_empty());
    }

    #[tokio::test]
    async fn an_opened_file_is_the_programs_stdin() {
        let path = std::env::temp_dir().join(format!("zendiloos-exec-{}", std::process::id()));
        std::fs::write(&path, "opened").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        // Whatever the path holds now, the program reads what was opened.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replaced").unwrap();

        let output = run_on(&sh(), ["-c", "cat /proc/self/fd/0"], file, Limits::default()).await.unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(output.stdout, b"opened");
    }

    #[tokio::test]
    async fn passes_arguments_verbatim_without_a_shell() {
        let output =
            run(&sh(), ["-c", "printf '%s|' \"$@\"", "sh", "a b", "$HOME", "; rm -rf /"], Limits::default())
                .await
                .unwrap();
        assert_eq!(output.stdout, b"a b|$HOME|; rm -rf /|");
    }

    #[tokio::test]
    async fn runs_with_an_empty_environment_in_the_root_directory() {
        let output = run(&sh(), ["-c", "echo \"${HOME-unset}:${LANG-unset}:$(pwd)\""], Limits::default())
            .await
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "unset:unset:/\n");
    }

    #[tokio::test]
    async fn passes_only_the_variables_and_directory_named() {
        let surroundings = Surroundings {
            variables: vec![("APP_DATA".into(), "/volume1/@apps/demo".into())],
            directory: Some(Path::new("/tmp")),
        };
        let script = "echo \"$APP_DATA:${HOME-unset}:$(pwd):$(cat)\"";
        let output =
            run_with(&sh(), ["-c", script], Some(b"in"), &surroundings, Limits::default()).await.unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "/volume1/@apps/demo:unset:/tmp:in\n");
    }

    #[tokio::test]
    async fn feeds_input_through_stdin() {
        let output = run_with_input(
            &sh(),
            ["-c", "read secret; echo \"got $secret\""],
            b"hunter2\n",
            Limits::default(),
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, b"got hunter2\n");
    }

    #[tokio::test]
    async fn reports_exit_status_and_stderr() {
        let output = run(&sh(), ["-c", "echo broken >&2; exit 3"], Limits::default()).await.unwrap();
        assert_eq!(output.status.code(), Some(3));
        let error = output.success(&sh()).unwrap_err();
        assert!(matches!(&error, ExecError::Failed { stderr, .. } if stderr == "broken\n"), "{error:?}");
    }

    #[tokio::test]
    async fn kills_programs_that_take_too_long() {
        let limits = Limits { timeout: Duration::from_millis(200), ..Limits::default() };
        let error = run(&sh(), ["-c", "while :; do :; done"], limits).await.unwrap_err();
        assert!(matches!(error, ExecError::Timeout { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn refuses_output_beyond_the_limit() {
        let limits = Limits { max_stdout: 10, ..Limits::default() };
        assert_eq!(run(&sh(), ["-c", "printf 0123456789"], limits).await.unwrap().stdout.len(), 10);
        let error = run(&sh(), ["-c", "printf 0123456789x"], limits).await.unwrap_err();
        assert!(matches!(error, ExecError::OutputTooLarge { limit: 10, .. }), "{error:?}");
    }

    #[tokio::test]
    async fn lines_are_passed_on_as_they_come_from_either_stream() {
        let script = r#"echo pulling; echo "50%" >&2; printf 'one\rtwo\n\n'; printf tail; exit 3"#;
        let mut seen = Vec::new();
        let output =
            run_lines(&sh(), ["-c", script], &Surroundings::default(), Limits::default(), &mut |line| {
                seen.push(line.to_owned());
            })
            .await
            .unwrap();
        assert_eq!(seen, ["pulling", "50%", "one", "two", "tail"]);
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.ends_with("two\n\ntail"), "{:?}", output.stderr);

        let long = format!("head -c {} /dev/zero | tr '\\0' x", MAX_LINE + 10);
        let mut pieces = Vec::new();
        run_lines(&sh(), ["-c", long.as_str()], &Surroundings::default(), Limits::default(), &mut |line| {
            pieces.push(line.len());
        })
        .await
        .unwrap();
        assert_eq!(pieces, [MAX_LINE, 10]);
        let many = format!("head -c {} /dev/zero", MAX_STDERR * 2);
        let output =
            run_lines(&sh(), ["-c", many.as_str()], &Surroundings::default(), Limits::default(), &mut |_| {})
                .await
                .unwrap();
        assert_eq!(output.stderr.len(), MAX_STDERR, "only the last bytes are kept");
    }

    #[tokio::test]
    async fn a_streaming_program_that_takes_too_long_is_killed() {
        let limits = Limits { timeout: Duration::from_millis(200), ..Limits::default() };
        let error =
            run_lines(&sh(), ["-c", "echo a; sleep 5"], &Surroundings::default(), limits, &mut |_| {})
                .await
                .unwrap_err();
        assert!(matches!(error, ExecError::Timeout { .. }), "{error:?}");
    }

    #[test]
    fn refuses_relative_paths() {
        assert!(matches!(Program::new("systemctl"), Err(ExecError::RelativePath(_))));
        assert!(matches!(Program::new("./systemctl"), Err(ExecError::RelativePath(_))));
    }

    #[tokio::test]
    async fn reports_missing_programs() {
        let error =
            run(&Program::new("/nonexistent/tool").unwrap(), [""; 0], Limits::default()).await.unwrap_err();
        assert!(matches!(error, ExecError::Spawn { .. }), "{error:?}");
    }
}
