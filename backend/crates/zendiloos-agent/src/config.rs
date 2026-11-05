//! Static configuration of the agent, `/etc/zendiloos/agent.toml`.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

pub const DEFAULT_PATH: &str = "/etc/zendiloos/agent.toml";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Socket to create when not started through systemd socket activation.
    pub socket: PathBuf,
    /// The only account allowed to connect: the one zendiloosd runs as.
    pub client_user: String,
    /// `tracing` filter directive; `RUST_LOG` takes precedence.
    pub log_filter: String,
    pub tools: Tools,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket: PathBuf::from("/run/zendiloos/agent.sock"),
            client_user: "zendiloos".to_owned(),
            log_filter: "info".to_owned(),
            tools: Tools::default(),
        }
    }
}

/// Absolute paths of the programs the agent may run. Nothing else is run.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    pub systemctl: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self { systemctl: PathBuf::from("/usr/bin/systemctl") }
    }
}

impl Config {
    /// Loads `explicit` if given (it must exist), otherwise [`DEFAULT_PATH`]
    /// if it exists, otherwise the defaults.
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let path = explicit.unwrap_or(Path::new(DEFAULT_PATH));
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid {}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound && explicit.is_none() => {
                Ok(Self::default())
            }
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_all_fields_and_rejects_unknown_ones() {
        let config: Config = toml::from_str(
            r#"
            socket = "/tmp/agent.sock"
            client_user = "tester"
            log_filter = "debug"

            [tools]
            systemctl = "/bin/systemctl"
            "#,
        )
        .unwrap();
        assert_eq!(config.client_user, "tester");
        assert_eq!(config.tools.systemctl, Path::new("/bin/systemctl"));
        assert!(toml::from_str::<Config>("[tools]\nsh = \"/bin/sh\"").is_err());
    }
}
