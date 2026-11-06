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
        // Dropping the child (on timeout or error) kills the program.
        .kill_on_drop(true);
    let mut child =
        spawn(&mut command).await.map_err(|source| ExecError::Spawn { program: name.clone(), source })?;

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
        let (fed, stdout, stderr) =
            tokio::join!(feed, read_limited(stdout, limits.max_stdout), read_limited(stderr, MAX_STDERR));
        fed.map_err(io_error)?;
        let stdout = stdout.map_err(io_error)?;
        let stderr = stderr.map_err(io_error)?;
        if stdout.len() > limits.max_stdout {
            return Err(ExecError::OutputTooLarge { program: name.clone(), limit: limits.max_stdout });
        }
        let status = child.wait().await.map_err(io_error)?;
        let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
        stderr.truncate(MAX_STDERR);
        Ok(Output { status, stdout, stderr })
    };

    match tokio::time::timeout(limits.timeout, finished).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout { program: name.clone(), after: limits.timeout }),
    }
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
