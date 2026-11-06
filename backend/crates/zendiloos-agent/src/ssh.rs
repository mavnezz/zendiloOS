//! The SSH server, as a service: on or off, its port, and whether root and
//! passwords may log in. Who may log in otherwise stays as the system has
//! it.
//!
//! The settings go to `/etc/ssh/sshd_config.d/00-zendiloos.conf`, which
//! sshd reads before the other drop-ins and its own file; for each keyword
//! the first value wins. They are checked with `sshd -t` before the server
//! sees them, and the previous file returns if the check fails. The check
//! needs `/run/sshd`, which goes with the service when it stops, so it runs
//! in a transient unit that brings the directory along. Turning SSH
//! off stops the listener; open sessions are processes of their own
//! (`KillMode=process`) and stay. A timer to turn it off again is a
//! transient unit of systemd, so it fires whatever happens to the agent.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zendiloos_agent_proto::{Response, SshConfig};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
const UNIT: &str = "ssh.service";
/// The transient timer that turns SSH off, and its service.
const OFF: &str = "zendiloos-ssh-off";
/// The timer's description, before the time it fires in Unix seconds.
const DESCRIPTION: &str = "zendiloOS turns SSH off at ";
const HEADER: &str = "# Written by zendiloOS; changes made here are replaced. Read before the\n\
                      # other drop-ins: for each setting, the first value wins.\n";

/// Where the settings are written; other than the real place in tests.
#[derive(Debug, Clone)]
pub struct SshPaths {
    pub config: PathBuf,
}

impl Default for SshPaths {
    fn default() -> Self {
        Self { config: PathBuf::from("/etc/ssh/sshd_config.d/00-zendiloos.conf") }
    }
}

#[derive(Debug)]
pub(crate) struct Ssh {
    systemctl: Program,
    systemd_run: Program,
    sshd: Program,
    paths: SshPaths,
}

/// When the timer turns SSH off: a timer that runs on monotonic time has
/// no wall-clock time of its own, so it carries it in its description.
fn off_at(shown: &str) -> Option<i64> {
    let value = |key: &str| shown.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='));
    if value("SubState")? != "waiting" {
        return None;
    }
    value("Description")?.strip_prefix(DESCRIPTION)?.trim().parse().ok()
}

/// The settings file for `config`.
pub fn sshd_config(config: &SshConfig) -> String {
    let mut text = format!("{HEADER}Port {}\n", config.port.get());
    if let Some(root) = config.root_login {
        text.push_str(&format!("PermitRootLogin {}\n", root.sshd_name()));
    }
    if let Some(passwords) = config.password_login {
        text.push_str(&format!("PasswordAuthentication {}\n", if passwords { "yes" } else { "no" }));
    }
    text
}

impl Ssh {
    pub(crate) fn new(tools: &Tools, paths: SshPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            sshd: Program::new(&tools.sshd)?,
            paths,
        })
    }

    /// The first line `systemctl` prints, whatever its exit status: for
    /// `is-enabled` and `is-active`, a non-zero status is an answer.
    async fn ask(&self, args: &[&str]) -> Result<String, OperationError> {
        let output = zendiloos_exec::run(&self.systemctl, args, LIMITS).await?;
        let text = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(text.lines().next().unwrap_or_default().trim().to_owned())
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let installed = self.sshd.path().exists();
        let enabled = self.ask(&["is-enabled", UNIT]).await?;
        let active = self.ask(&["is-active", UNIT]).await?;
        let config = match fs::read_to_string(&self.paths.config) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(OperationError::Read(self.paths.config.clone(), error)),
        };
        let timer = format!("{OFF}.timer");
        let output = zendiloos_exec::run(
            &self.systemctl,
            &["show", &timer, "--property=SubState,Description"],
            LIMITS,
        )
        .await?;
        let shown = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        let off_at = off_at(&shown);
        Ok(Response::SshState { installed, enabled, active, config, off_at })
    }

    pub(crate) async fn set(&self, config: &SshConfig) -> Result<Response, OperationError> {
        if !self.sshd.path().exists() {
            return Err(OperationError::Setup(
                "the SSH server is not installed: apt install openssh-server".into(),
            ));
        }
        let path = &self.paths.config;
        let before = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(OperationError::Read(path.clone(), error)),
        };
        self.write(Some(&sshd_config(config)))?;
        if let Err(error) = self.check().await {
            self.write(before.as_deref())?;
            return Err(error);
        }
        // Whatever was planned before is replaced.
        let timer = format!("{OFF}.timer");
        let service = format!("{OFF}.service");
        let _ = self.run(&self.systemctl, &["stop", &timer]).await;
        let _ = self.run(&self.systemctl, &["reset-failed", &service]).await;
        if !config.enabled {
            self.run(&self.systemctl, &["disable", "--now", UNIT]).await?;
            return Ok(Response::Done);
        }
        self.run(&self.systemctl, &["enable", "--now", UNIT]).await?;
        // A new port needs the listener anew; sshd re-executes on reload.
        self.run(&self.systemctl, &["reload-or-restart", UNIT]).await?;
        if let Some(off) = config.off_after {
            let systemctl = self.systemctl.path().to_string_lossy().into_owned();
            let unit = format!("--unit={OFF}");
            let after = format!("--on-active={}min", off.get());
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
            let description = format!("--description={DESCRIPTION}{}", now + u64::from(off.get()) * 60);
            self.run(
                &self.systemd_run,
                &[
                    &unit,
                    &description,
                    &after,
                    "--timer-property=AccuracySec=1s",
                    "--collect",
                    &systemctl,
                    "disable",
                    "--now",
                    UNIT,
                ],
            )
            .await?;
        }
        Ok(Response::Done)
    }

    /// `sshd -t`, with `/run/sshd` there whether the service runs or not.
    async fn check(&self) -> Result<(), OperationError> {
        let sshd = self.sshd.path().to_string_lossy().into_owned();
        self.run(
            &self.systemd_run,
            &[
                "--wait",
                "--pipe",
                "--collect",
                "--quiet",
                "--property=RuntimeDirectory=sshd",
                "--property=RuntimeDirectoryPreserve=yes",
                &sshd,
                "-t",
            ],
        )
        .await
    }

    /// Writes the settings file, or removes it for `None`.
    fn write(&self, content: Option<&str>) -> Result<(), OperationError> {
        let path = &self.paths.config;
        let Some(content) = content else {
            return match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    Err(OperationError::Write(path.clone(), error))
                }
                _ => Ok(()),
            };
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
        }
        let temporary = path.with_extension("new");
        fs::write(&temporary, content).map_err(|e| OperationError::Write(temporary.clone(), e))?;
        fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.clone(), e))
    }

    async fn run(&self, program: &Program, args: &[&str]) -> Result<(), OperationError> {
        zendiloos_exec::run(program, args, LIMITS).await?.success(program)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{OffAfter, RootLogin, SshPort};

    use super::*;

    #[test]
    fn the_time_to_turn_off_is_read_while_the_timer_waits() {
        let waiting = "SubState=waiting\nDescription=zendiloOS turns SSH off at 1800000600\n";
        assert_eq!(off_at(waiting), Some(1_800_000_600));
        assert_eq!(off_at("Description=zendiloOS turns SSH off at 1800000600\nSubState=elapsed\n"), None);
        assert_eq!(off_at("SubState=dead\nDescription=zendiloos-ssh-off.timer\n"), None);
    }

    #[test]
    fn writes_the_port_and_only_the_rules_that_were_chosen() {
        let config = SshConfig {
            enabled: true,
            port: SshPort::new(2222).unwrap(),
            root_login: None,
            password_login: Some(false),
            off_after: Some(OffAfter::minutes(10).unwrap()),
        };
        let text = sshd_config(&config);
        assert!(text.ends_with("Port 2222\nPasswordAuthentication no\n"), "{text}");
        assert!(!text.contains("PermitRootLogin"), "as the system has it: {text}");
        let root = SshConfig { root_login: Some(RootLogin::KeyOnly), ..config };
        assert!(sshd_config(&root).contains("PermitRootLogin prohibit-password\n"));
    }
}
