//! The machine's clock. Time zone, network time and the time itself are
//! set by systemd-timedated, which `timedatectl` asks over D-Bus; the agent
//! itself may not touch the clock (`ProtectClock`). The time servers go to
//! a drop-in of systemd-timesyncd, the service that keeps the clock when
//! network time is on.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{ClockTime, Response, TimeServers, TimeZone};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 256 * 1024 };
const TIMESYNC_UNIT: &str = "systemd-timesyncd.service";
const HEADER: &str = "# Written by zendiloOS; changes made here are replaced.\n";

/// Where the time servers are written; other than the real place in tests.
#[derive(Debug, Clone)]
pub struct TimePaths {
    /// `/etc/systemd/timesyncd.conf.d/zendiloos.conf`.
    pub servers: PathBuf,
}

impl Default for TimePaths {
    fn default() -> Self {
        Self { servers: PathBuf::from("/etc/systemd/timesyncd.conf.d/zendiloos.conf") }
    }
}

#[derive(Debug)]
pub(crate) struct Time {
    timedatectl: Program,
    systemctl: Program,
    paths: TimePaths,
}

impl Time {
    pub(crate) fn new(tools: &Tools, paths: TimePaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            timedatectl: Program::new(&tools.timedatectl)?,
            systemctl: Program::new(&tools.systemctl)?,
            paths,
        })
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let timedatectl = self.output(&["show"]).await?;
        // Fails while timesyncd is not running; then there is no server in use.
        let timesync = self.output(&["show-timesync"]).await.unwrap_or_default();
        let zones = self.output(&["list-timezones"]).await?;
        let servers_conf = match fs::read_to_string(&self.paths.servers) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(OperationError::Read(self.paths.servers.clone(), error)),
        };
        Ok(Response::TimeState { timedatectl, timesync, zones, servers_conf })
    }

    /// The zone first: timedated refuses one the system does not have,
    /// before anything else changed.
    pub(crate) async fn set(
        &self,
        zone: &TimeZone,
        network_time: bool,
        servers: &TimeServers,
        clock: Option<ClockTime>,
    ) -> Result<Response, OperationError> {
        if network_time && clock.is_some() {
            return Err(OperationError::Refused(
                "the time servers set the clock; turn them off first".into(),
            ));
        }
        self.run(&["set-timezone", "--", zone.as_str()]).await?;
        self.write_servers(servers)?;
        self.run(&["set-ntp", if network_time { "true" } else { "false" }]).await?;
        if network_time {
            // A running timesyncd reads the servers only when it starts.
            let limits = Limits { timeout: Duration::from_secs(30), ..Limits::default() };
            zendiloos_exec::run(&self.systemctl, ["try-restart", TIMESYNC_UNIT], limits)
                .await?
                .success(&self.systemctl)?;
        } else if let Some(clock) = clock {
            self.run(&["set-time", &format!("@{}", clock.unix_seconds())]).await?;
        }
        Ok(Response::Done)
    }

    /// No servers: no drop-in, so timesyncd asks Debian's pool.
    fn write_servers(&self, servers: &TimeServers) -> Result<(), OperationError> {
        let path = &self.paths.servers;
        if servers.as_slice().is_empty() {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(OperationError::Write(path.clone(), error)),
            };
        }
        let list: Vec<&str> = servers.as_slice().iter().map(|server| server.as_str()).collect();
        let content = format!("{HEADER}[Time]\nNTP={}\n", list.join(" "));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
        }
        crate::durable::replace(path, content.as_bytes())
    }

    async fn run(&self, args: &[&str]) -> Result<(), OperationError> {
        zendiloos_exec::run(&self.timedatectl, args, LIMITS).await?.success(&self.timedatectl)?;
        Ok(())
    }

    async fn output(&self, args: &[&str]) -> Result<String, OperationError> {
        let output =
            zendiloos_exec::run(&self.timedatectl, args, LIMITS).await?.success(&self.timedatectl)?;
        String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)
    }
}
