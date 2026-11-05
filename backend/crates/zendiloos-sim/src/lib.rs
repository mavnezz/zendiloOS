//! A simulated agent behind the same [`AgentTransport`] as the real one.
//!
//! zendiloosd uses it on a development machine: everything above the
//! transport, including the parsing of tool output, runs as in production,
//! but nothing is executed and nothing needs root.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_agent_proto::{AgentTransport, PROTOCOL_VERSION, Request, RequestId, Response, TransportError};

#[derive(Debug, Default)]
pub struct SimulatedAgent {
    received: Mutex<Vec<Request>>,
}

impl SimulatedAgent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every request received so far, oldest first.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl AgentTransport for SimulatedAgent {
    async fn call(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<Response, TransportError> {
        let request_id = request_id.as_ref().map(RequestId::as_str).unwrap_or("-");
        let response = match &request {
            Request::Hello {} => Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: concat!(env!("CARGO_PKG_VERSION"), "-simulated").to_owned(),
            },
            Request::Reboot {} | Request::PowerOff {} => {
                tracing::warn!(op = request.name(), request_id, "simulated agent: not executing");
                Response::Done
            }
        };
        self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_like_the_agent_without_doing_anything() {
        let agent = SimulatedAgent::new();
        let hello = agent.call(Request::Hello {}, None).await.unwrap();
        assert!(matches!(hello, Response::Hello { protocol_version: PROTOCOL_VERSION, .. }));
        assert_eq!(agent.call(Request::Reboot {}, None).await.unwrap(), Response::Done);
        assert_eq!(agent.received(), [Request::Hello {}, Request::Reboot {}]);
    }
}
