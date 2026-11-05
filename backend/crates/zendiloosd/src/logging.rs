use crate::config::{LogConfig, LogFormat};

/// Logs go to stderr; under systemd they end up in the journal. stdout stays
/// free for command output such as `zendiloosd openapi`.
pub fn init(config: &LogConfig) -> anyhow::Result<()> {
    let format = match config.format {
        LogFormat::Text => zendiloos_logging::Format::Text,
        LogFormat::Json => zendiloos_logging::Format::Json,
    };
    zendiloos_logging::init(&config.filter, format)
}
