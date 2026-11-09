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
    /// Directory for the database, its backups and the setup code (or the
    /// marker of an open setup, which the ISO's installer leaves). Created
    /// with mode 0700 if missing. A relative path is resolved against the
    /// working directory.
    pub state_dir: PathBuf,
    /// Session cookies get `Secure` and the `__Host-` prefix, so browsers
    /// send them over HTTPS only. Turn off only where the interface is
    /// reached over plain HTTP.
    pub secure_cookies: bool,
    pub agent: AgentConfig,
    pub journal: JournalConfig,
    pub network: NetworkConfig,
    pub log: LogConfig,
    pub updates: UpdatesConfig,
}

/// Where zendiloOS's releases come from, and what changed in the one that
/// runs.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// GitHub's API, or something answering like it.
    pub api: String,
    /// The repository whose releases are zendiloOS's, `owner/name`.
    pub repository: String,
    /// What changed in the version that runs, in Markdown, which the
    /// package brings along; without the file the update page shows none.
    pub notes: PathBuf,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            api: "https://api.github.com".into(),
            repository: "mavnezz/zendiloOS".into(),
            notes: PathBuf::from("/usr/share/zendiloos/changes.md"),
        }
    }
}

/// How zendiloosd reads the system journal for the log viewer.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct JournalConfig {
    /// Absolute path of `journalctl`; looked up in the usual places if
    /// absent.
    pub journalctl: Option<PathBuf>,
}

/// How zendiloosd reads the machine's network.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// Absolute path of `ip`; looked up in the usual places if absent.
    pub ip: Option<PathBuf>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub transport: AgentTransportKind,
    /// The system agent's socket, for transport `unix`.
    pub socket: PathBuf,
    /// The files agent's socket, for transport `unix`.
    pub files_socket: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            transport: AgentTransportKind::Unix,
            socket: PathBuf::from("/run/zendiloos/agent.sock"),
            files_socket: PathBuf::from("/run/zendiloos/files.sock"),
        }
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
            journal: JournalConfig::default(),
            network: NetworkConfig::default(),
            log: LogConfig::default(),
            updates: UpdatesConfig::default(),
        }
    }
}

impl Config {
    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join("zendiloos.db")
    }

    /// Where releases are downloaded to; the agent takes them from there.
    pub fn update_dir(&self) -> PathBuf {
        self.state_dir.join("updates")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.state_dir.join("backup")
    }

    pub fn setup_code_path(&self) -> PathBuf {
        self.state_dir.join("setup-code")
    }

    /// While this file exists and no administrator does, the setup is open:
    /// no code, the first one in from the NAS's own network sets it up. The
    /// ISO's installer creates it; the setup removes it.
    pub fn setup_open_path(&self) -> PathBuf {
        self.state_dir.join("setup-open")
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

            [journal]
            journalctl = "/bin/journalctl"

            [network]
            ip = "/bin/ip"

            [log]
            filter = "debug"
            format = "json"

            [updates]
            notes = "/srv/changes.md"
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
                socket: PathBuf::from("/tmp/agent.sock"),
                files_socket: PathBuf::from("/run/zendiloos/files.sock"),
            }
        );
        assert_eq!(config.journal.journalctl.as_deref(), Some(Path::new("/bin/journalctl")));
        assert_eq!(config.network.ip.as_deref(), Some(Path::new("/bin/ip")));
        assert_eq!(config.log, LogConfig { filter: "debug".to_owned(), format: LogFormat::Json });
        assert_eq!(config.updates.notes, Path::new("/srv/changes.md"));
        assert_eq!(config.updates.repository, "mavnezz/zendiloOS", "the others keep their defaults");
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
