use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, Request, Response};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_notify::channels::Channels;
use zendiloos_system::application::ports::{HostNameControl, HostNameObserver};
use zendiloos_system::domain::{HostName, Renamed};

use super::{WHAT, call, unexpected};
use crate::host::static_name;

/// [`HostNameControl`] through the system agent, which asks hostnamed and
/// has what follows the name follow it.
pub struct AgentHostName {
    transport: Arc<dyn AgentTransport>,
}

impl AgentHostName {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl HostNameControl for AgentHostName {
    async fn name(&self, context: &RequestContext) -> Result<String, PortError> {
        match call(self.transport.as_ref(), Request::HostNameState {}, context).await? {
            Response::HostNameState { hostname } => Ok(static_name(&hostname).unwrap_or_default().to_owned()),
            other => Err(unexpected(other)),
        }
    }

    async fn rename(&self, name: &HostName, context: &RequestContext) -> Result<Renamed, PortError> {
        let name = proto::HostName::parse(name.as_str())
            .map_err(|_| PortError::new(WHAT, "not a valid host name for the agent"))?;
        match call(self.transport.as_ref(), Request::SetHostName { name: name.clone() }, context).await? {
            Response::HostNameSet { certificate_renewed, own_certificate_served } => {
                Ok(Renamed { name: name.as_str().to_owned(), certificate_renewed, own_certificate_served })
            }
            other => Err(unexpected(other)),
        }
    }
}

/// [`HostNameObserver`]: the notifications going out by email and to
/// webhooks name the NAS by its new name.
pub struct ChannelsFollowHostName(pub Channels);

impl HostNameObserver for ChannelsFollowHostName {
    fn renamed(&self, name: &str) {
        self.0.rename(name);
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[tokio::test]
    async fn a_rename_goes_through_the_agent() {
        let host = AgentHostName::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert_eq!(host.name(&context).await.unwrap(), "zendiloos");

        let renamed = host.rename(&HostName::parse("office").unwrap(), &context).await.unwrap();

        assert_eq!(
            renamed,
            Renamed { name: "office".into(), certificate_renewed: true, own_certificate_served: true }
        );
        assert_eq!(host.name(&context).await.unwrap(), "office");
    }
}
