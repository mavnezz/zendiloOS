//! Static configuration of zendiloosd: how it runs, not what it manages.
//! Everything an administrator changes in the web interface lives in the
//! database instead.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// Read when no `--config` is given; built-in defaults apply if it is missing.
pub const DEFAULT_PATH: &str = "/etc/zendiloos/zendiloosd.toml";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Address of the HTTP listener. Keep it on loopback: the reverse proxy
    /// is the only thing that should reach it.
    pub listen: SocketAddr,
    /// Directory for the database, its backups and the setup code. Created
    /// with mode 0700 if missing. A relative path is resolved against the
    /// working directory.
    pub state_dir: PathBuf,
    /// Session cookies get `Secure` and the `__Host-` prefix, so browsers
    /// send them over HTTPS only. Turn off only where the interface is
    /// reached over plain HTTP.
    pub secure_cookies: bool,
    pub agent: AgentConfig,
    pub log: LogConfig,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub transport: AgentTransportKind,
    /// The agent's socket, for transport `unix`.
    pub socket: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { transport: AgentTransportKind::Unix, socket: PathBuf::from("/run/zendiloos/agent.sock") }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AgentTransportKind {
    /// The privileged zendiloos-agent behind its Unix socket.
    #[default]
    Unix,
    /// An in-process simulation that executes nothing: for development.
    Simulated,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)),
            state_dir: PathBuf::from("/var/lib/zendiloos"),
            secure_cookies: true,
            agent: AgentConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl Config {
    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join("zendiloos.db")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.state_dir.join("backup")
    }

    pub fn setup_code_path(&self) -> PathBuf {
        self.state_dir.join("setup-code")
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// `tracing` filter directive, e.g. `info` or `info,zendiloos_api=debug`.
    /// The `RUST_LOG` environment variable takes precedence.
    pub filter: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { filter: "info".to_owned(), format: LogFormat::default() }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

/// Where the configuration came from, for the startup log.
#[derive(Debug, PartialEq)]
pub enum Source {
    File(PathBuf),
    Defaults,
}

impl Config {
    /// Loads `explicit` if given (it must exist), otherwise [`DEFAULT_PATH`]
    /// if it exists, otherwise the defaults.
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<(Self, Source)> {
        let path = explicit.unwrap_or(Path::new(DEFAULT_PATH));
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config = Self::parse(&text).with_context(|| format!("invalid {}", path.display()))?;
                Ok((config, Source::File(path.to_owned())))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && explicit.is_none() => {
                Ok((Self::default(), Source::Defaults))
            }
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_means_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn reads_all_fields() {
        let config = Config::parse(
            r#"
            listen = "127.0.0.1:9000"
            state_dir = "/srv/state"
            secure_cookies = false

            [agent]
            transport = "simulated"
            socket = "/tmp/agent.sock"

            [log]
            filter = "debug"
            format = "json"
            "#,
        )
        .unwrap();
        assert_eq!(config.listen, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.database_path(), Path::new("/srv/state/zendiloos.db"));
        assert_eq!(config.backup_dir(), Path::new("/srv/state/backup"));
        assert!(!config.secure_cookies);
        assert_eq!(
            config.agent,
            AgentConfig {
                transport: AgentTransportKind::Simulated,
                socket: PathBuf::from("/tmp/agent.sock")
            }
        );
        assert_eq!(config.log, LogConfig { filter: "debug".to_owned(), format: LogFormat::Json });
    }

    #[test]
    fn rejects_unknown_fields() {
        // A typo must not silently fall back to a default.
        assert!(Config::parse("listne = \"127.0.0.1:9000\"").is_err());
        assert!(Config::parse("[log]\nlevel = \"debug\"").is_err());
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let error = Config::load(Some(Path::new("/nonexistent/zendiloosd.toml"))).unwrap_err();
        assert!(error.to_string().contains("/nonexistent/zendiloosd.toml"));
    }
}
