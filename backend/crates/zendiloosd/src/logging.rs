use std::io::IsTerminal;

use anyhow::{Context, anyhow};
use tracing_subscriber::EnvFilter;

use crate::config::{LogConfig, LogFormat};

/// Logs go to stderr; under systemd they end up in the journal. stdout stays
/// free for command output such as `zendiloosd openapi`.
pub fn init(config: &LogConfig) -> anyhow::Result<()> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(directive) if !directive.is_empty() => {
            EnvFilter::try_new(&directive).context("invalid RUST_LOG")?
        }
        _ => EnvFilter::try_new(&config.filter).context("invalid log filter in configuration")?,
    };
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr);
    match config.format {
        // The journal adds its own timestamps.
        LogFormat::Text if crate::systemd::logging_to_journal() => {
            builder.without_time().with_ansi(false).try_init()
        }
        LogFormat::Text => builder.with_ansi(std::io::stderr().is_terminal()).try_init(),
        LogFormat::Json => builder.json().try_init(),
    }
    .map_err(|error| anyhow!(error))
}
