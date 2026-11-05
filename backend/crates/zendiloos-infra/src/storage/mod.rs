//! Parsers for the output of storage tools, which the agent runs and
//! zendiloosd interprets. Each is tested against output captured from real
//! tools in `fixtures/`.

pub(crate) mod lsblk;
pub(crate) mod mdstat;
pub(crate) mod smartctl;

/// Tool output in an unexpected form, e.g. from an unsupported version.
#[derive(Debug, thiserror::Error)]
#[error("unexpected {tool} output")]
pub struct ParseError {
    tool: &'static str,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl ParseError {
    fn new(tool: &'static str, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self { tool, source: source.into() }
    }
}
