//! Gateways to the privileged agent: implement the contexts' ports by
//! sending protocol requests through an [`AgentTransport`], which is either
//! the agent's socket or the simulation.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{AgentTransport, PROTOCOL_VERSION, Request, RequestId, Response};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_system::application::ports::PowerControl;
use zendiloos_system::domain::PowerAction;

const WHAT: &str = "privileged agent";

async fn call(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<Response, PortError> {
    // Request IDs come from zendiloosd itself (UUIDs); anything else is dropped.
    let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
    match transport.call(request, request_id).await.map_err(|error| PortError::new(WHAT, error))? {
        Response::Error { code, message } => Err(PortError::new(WHAT, format!("{code:?}: {message}"))),
        response => Ok(response),
    }
}

async fn expect_done(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<(), PortError> {
    match call(transport, request, context).await? {
        Response::Done => Ok(()),
        other => Err(PortError::new(WHAT, format!("unexpected answer: {other:?}"))),
    }
}

/// The agent's version; an error if it is unreachable or speaks another
/// protocol version.
pub async fn agent_version(transport: &dyn AgentTransport) -> Result<String, PortError> {
    match call(transport, Request::Hello {}, &RequestContext::default()).await? {
        Response::Hello { protocol_version, agent_version } if protocol_version == PROTOCOL_VERSION => {
            Ok(agent_version)
        }
        Response::Hello { protocol_version, .. } => Err(PortError::new(
            WHAT,
            format!("speaks protocol {protocol_version}, zendiloosd speaks {PROTOCOL_VERSION}"),
        )),
        other => Err(PortError::new(WHAT, format!("unexpected answer: {other:?}"))),
    }
}

/// [`PowerControl`] through the agent.
pub struct AgentPowerControl {
    transport: Arc<dyn AgentTransport>,
}

impl AgentPowerControl {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl PowerControl for AgentPowerControl {
    async fn request(&self, action: PowerAction, context: &RequestContext) -> Result<(), PortError> {
        let request = match action {
            PowerAction::Reboot => Request::Reboot {},
            PowerAction::PowerOff => Request::PowerOff {},
        };
        expect_done(self.transport.as_ref(), request, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{ErrorCode, TransportError};
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    struct Refusing;

    #[async_trait]
    impl AgentTransport for Refusing {
        async fn call(&self, _: Request, _: Option<RequestId>) -> Result<Response, TransportError> {
            Ok(Response::Error { code: ErrorCode::Failed, message: "systemctl failed".to_owned() })
        }
    }

    #[tokio::test]
    async fn power_actions_become_agent_requests() {
        let agent = Arc::new(SimulatedAgent::new());
        let power = AgentPowerControl::new(agent.clone());

        power.request(PowerAction::PowerOff, &RequestContext::default()).await.unwrap();

        assert_eq!(agent.received(), [Request::PowerOff {}]);
    }

    #[tokio::test]
    async fn agent_errors_become_port_errors() {
        let power = AgentPowerControl::new(Arc::new(Refusing));
        let error = power.request(PowerAction::Reboot, &RequestContext::default()).await.unwrap_err();
        let cause = std::error::Error::source(&error).unwrap().to_string();
        assert!(cause.contains("systemctl failed"), "{cause}");
    }

    #[tokio::test]
    async fn checks_the_protocol_version() {
        let version = agent_version(&SimulatedAgent::new()).await.unwrap();
        assert!(version.ends_with("-simulated"), "{version}");
    }
}
