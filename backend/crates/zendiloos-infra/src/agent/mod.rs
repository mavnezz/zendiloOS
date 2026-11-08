//! Gateways to the privileged agent: implement the contexts' ports by
//! sending protocol requests through an [`AgentTransport`], which is either
//! the agent's socket or the simulation.

mod accounts;
mod acme;
mod apps;
mod docker;
mod docker_data;
mod engine;
mod files;
mod firewall;
mod network;
mod sharing;
mod ssh;
mod storage;
mod time;
mod updates;
mod volume_backups;
mod web;

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, PROTOCOL_VERSION, Report, Request, RequestId, Response,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_system::application::ports::PowerControl;
use zendiloos_system::domain::PowerAction;

pub use accounts::AgentSystemAccounts;
pub use acme::AgentAcme;
pub use apps::AgentApps;
pub use docker_data::AgentDockerData;
pub use engine::AgentDockerEngine;
pub use files::AgentFiles;
pub use firewall::AgentFirewall;
pub use network::AgentNetwork;
pub use sharing::AgentSharing;
pub use ssh::AgentSsh;
pub use storage::{AgentStorage, RandomPlanTokens};
pub use time::AgentTime;
pub use updates::AgentUpdates;
pub use volume_backups::AgentVolumeArchiver;
pub use web::AgentWebTls;

const WHAT: &str = "privileged agent";

/// The agent's answer, errors included.
async fn call_raw(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<Response, PortError> {
    // Request IDs come from zendiloosd itself (UUIDs); anything else is dropped.
    let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
    transport.call(request, request_id).await.map_err(|error| PortError::new(WHAT, error))
}

/// As [`call_raw`], telling `report` what the agent reports on the way.
async fn call_raw_reporting(
    transport: &dyn AgentTransport,
    request: Request,
    report: &dyn Report,
    context: &RequestContext,
) -> Result<Response, PortError> {
    let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
    transport.call_reporting(request, request_id, report).await.map_err(|error| PortError::new(WHAT, error))
}

/// The agent's answer; its errors become port errors.
async fn call(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<Response, PortError> {
    match call_raw(transport, request, context).await? {
        Response::Error { code, message } => Err(agent_error(code, &message)),
        response => Ok(response),
    }
}

fn agent_error(code: ErrorCode, message: &str) -> PortError {
    PortError::new(WHAT, format!("{code:?}: {message}"))
}

async fn expect_done(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<(), PortError> {
    match call(transport, request, context).await? {
        Response::Done => Ok(()),
        other => Err(unexpected(other)),
    }
}

fn unexpected(response: Response) -> PortError {
    PortError::new(WHAT, format!("unexpected answer: {response:?}"))
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
        other => Err(unexpected(other)),
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

/// An agent for the adapters' tests: answers what a test scripted, `Done`
/// once nothing is left, and records what it was asked.
#[cfg(test)]
pub(crate) mod scripted {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_agent_proto::{AgentTransport, ErrorCode, Request, RequestId, Response, TransportError};

    #[derive(Default)]
    pub(crate) struct Scripted {
        answers: Mutex<VecDeque<Response>>,
        asked: Mutex<Vec<Request>>,
    }

    impl Scripted {
        pub(crate) fn answering(answers: impl IntoIterator<Item = Response>) -> Self {
            Self { answers: Mutex::new(answers.into_iter().collect()), asked: Mutex::default() }
        }

        /// An error of the agent, with its code, then `Done`.
        pub(crate) fn refusing(code: ErrorCode, message: &str) -> Self {
            Self::answering([Response::Error { code, message: message.to_owned() }])
        }

        pub(crate) fn answer(&self, response: Response) {
            self.answers.lock().unwrap().push_back(response);
        }

        pub(crate) fn asked(&self) -> Vec<Request> {
            self.asked.lock().unwrap().clone()
        }

        /// The one request it was asked.
        pub(crate) fn only(&self) -> Request {
            let asked = self.asked();
            assert_eq!(asked.len(), 1, "{asked:?}");
            asked.into_iter().next().unwrap()
        }
    }

    #[async_trait]
    impl AgentTransport for Scripted {
        async fn call(&self, request: Request, _: Option<RequestId>) -> Result<Response, TransportError> {
            self.asked.lock().unwrap().push(request);
            Ok(self.answers.lock().unwrap().pop_front().unwrap_or(Response::Done))
        }
    }

    /// A transport that cannot reach the agent.
    pub(crate) struct Unreachable;

    #[async_trait]
    impl AgentTransport for Unreachable {
        async fn call(&self, _: Request, _: Option<RequestId>) -> Result<Response, TransportError> {
            Err(TransportError::Timeout(std::time::Duration::from_secs(1)))
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::TransportError;
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
        let other = scripted::Scripted::answering([Response::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            agent_version: "9".into(),
        }]);
        let error = agent_version(&other).await.unwrap_err();
        assert!(std::error::Error::source(&error).unwrap().to_string().contains("speaks protocol"));
        let odd = scripted::Scripted::answering([Response::Done]);
        assert!(agent_version(&odd).await.is_err(), "not a handshake");
        assert!(agent_version(&scripted::Unreachable).await.is_err());
    }

    #[tokio::test]
    async fn an_answer_of_another_kind_is_an_error() {
        let context = RequestContext { request_id: Some("not an id".into()), ..RequestContext::default() };
        let odd = scripted::Scripted::answering([Response::Snapshots { zfs_list: String::new() }]);
        assert!(expect_done(&odd, Request::Reboot {}, &context).await.is_err());
        assert_eq!(odd.only(), Request::Reboot {});
    }
}
