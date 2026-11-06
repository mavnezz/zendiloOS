use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, Request, Response};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_system::application::ports::WebTlsControl;
use zendiloos_system::domain::TlsMinimum;

use super::{call, expect_done, unexpected};

/// [`WebTlsControl`] through the system agent.
pub struct AgentWebTls {
    transport: Arc<dyn AgentTransport>,
}

impl AgentWebTls {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

/// What the settings file says; without one, nginx accepts TLS 1.2.
fn minimum(config: &str) -> TlsMinimum {
    let protocols = config.lines().find_map(|line| line.trim().strip_prefix("ssl_protocols "));
    match protocols {
        Some(protocols) if !protocols.contains("TLSv1.2") && protocols.contains("TLSv1.3") => {
            TlsMinimum::Tls13
        }
        _ => TlsMinimum::Tls12,
    }
}

#[async_trait]
impl WebTlsControl for AgentWebTls {
    async fn minimum(&self, context: &RequestContext) -> Result<TlsMinimum, PortError> {
        match call(self.transport.as_ref(), Request::WebTlsState {}, context).await? {
            Response::WebTlsState { config } => Ok(minimum(&config)),
            other => Err(unexpected(other)),
        }
    }

    async fn apply(&self, minimum: TlsMinimum, context: &RequestContext) -> Result<(), PortError> {
        let minimum = match minimum {
            TlsMinimum::Tls12 => proto::TlsMinimum::Tls12,
            TlsMinimum::Tls13 => proto::TlsMinimum::Tls13,
        };
        expect_done(self.transport.as_ref(), Request::SetWebTls { minimum }, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[test]
    fn reads_the_oldest_version_from_the_settings_file() {
        assert_eq!(minimum("# zendiloOS\nssl_protocols TLSv1.3;\n"), TlsMinimum::Tls13);
        assert_eq!(minimum("ssl_protocols TLSv1.2 TLSv1.3;\n"), TlsMinimum::Tls12);
        assert_eq!(minimum(""), TlsMinimum::Tls12, "nginx's default here");
    }

    #[tokio::test]
    async fn the_setting_goes_through_the_agent() {
        let tls = AgentWebTls::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();

        tls.apply(TlsMinimum::Tls13, &context).await.unwrap();

        assert_eq!(tls.minimum(&context).await.unwrap(), TlsMinimum::Tls13);
    }
}
