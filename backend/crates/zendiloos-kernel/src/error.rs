use std::error::Error;

/// A secondary adapter behind a port failed (database, host, agent).
///
/// Contexts treat it as opaque: they cannot recover from it, only pass it
/// on. The source chain is meant for logs, never for API clients.
#[derive(Debug, thiserror::Error)]
#[error("{what} is unavailable")]
pub struct PortError {
    what: &'static str,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl PortError {
    /// `what` names the resource in a few words, e.g. `"system information"`.
    pub fn new(what: &'static str, source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self { what, source: source.into() }
    }

    pub fn what(&self) -> &'static str {
        self.what
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_cause_as_source() {
        let error = PortError::new("system information", std::io::Error::other("no /proc"));
        assert_eq!(error.to_string(), "system information is unavailable");
        assert_eq!(error.source().unwrap().to_string(), "no /proc");
    }
}
