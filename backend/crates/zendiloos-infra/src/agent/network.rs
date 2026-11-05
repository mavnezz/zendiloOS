use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, ErrorCode, Request, Response};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_network::application::{ConfigureError, NetworkConfigurator, NetworkSetup, PendingChange};
use zendiloos_network::domain::{InterfaceSettings, Ipv4Setting, Ipv6Setting};

use super::{WHAT, agent_error, call, call_raw, unexpected};

/// [`NetworkConfigurator`] through the system agent.
pub struct AgentNetwork {
    transport: Arc<dyn AgentTransport>,
}

impl AgentNetwork {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    async fn change(&self, request: Request, context: &RequestContext) -> Result<(), ConfigureError> {
        match call_raw(self.transport.as_ref(), request, context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden, message } => Err(ConfigureError::Refused(message)),
            Response::Error { code: ErrorCode::Conflict, .. } => Err(ConfigureError::Pending),
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }
}

fn invalid(what: &str) -> ConfigureError {
    ConfigureError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn config(settings: &InterfaceSettings) -> Result<proto::NetworkConfig, ConfigureError> {
    Ok(proto::NetworkConfig {
        interface: proto::InterfaceName::parse(&settings.interface).map_err(|_| invalid("interface"))?,
        ipv4: match settings.ipv4 {
            Ipv4Setting::Dhcp => proto::Ipv4Config::Dhcp,
            Ipv4Setting::Static { address, prefix, gateway } => proto::Ipv4Config::Static {
                address,
                prefix: proto::Prefix::new(prefix).map_err(|_| invalid("prefix"))?,
                gateway,
            },
        },
        ipv6: match settings.ipv6 {
            Ipv6Setting::Automatic => proto::Ipv6Config::Automatic,
            Ipv6Setting::Off => proto::Ipv6Config::Off,
        },
        dns: proto::NameServers::new(settings.dns.clone()).map_err(|_| invalid("list of name servers"))?,
    })
}

#[async_trait]
impl NetworkConfigurator for AgentNetwork {
    async fn setup(&self, context: &RequestContext) -> Result<NetworkSetup, PortError> {
        match call(self.transport.as_ref(), Request::NetworkState {}, context).await? {
            Response::NetworkState { managed, pending } => Ok(NetworkSetup {
                managed,
                pending: pending.map(|p| PendingChange {
                    interface: p.interface.as_str().to_owned(),
                    revert_at: Timestamp::from_unix_seconds(p.revert_at),
                }),
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn apply(
        &self,
        settings: &InterfaceSettings,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError> {
        let confirm_within =
            proto::ConfirmWithin::seconds(confirm_within_seconds).map_err(|_| invalid("time to confirm"))?;
        self.change(Request::ApplyNetwork { config: config(settings)?, confirm_within }, context).await
    }

    async fn confirm(&self, context: &RequestContext) -> Result<(), ConfigureError> {
        self.change(Request::ConfirmNetwork {}, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[tokio::test]
    async fn changes_wait_for_confirmation_through_the_agent() {
        let network = AgentNetwork::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let settings = InterfaceSettings {
            interface: "enp1s0".into(),
            ipv4: Ipv4Setting::Static {
                address: "192.168.1.10".parse().unwrap(),
                prefix: 24,
                gateway: Some("192.168.1.1".parse().unwrap()),
            },
            ipv6: Ipv6Setting::Automatic,
            dns: vec!["192.168.1.1".parse().unwrap()],
        };

        network.apply(&settings, 120, &context).await.unwrap();
        let setup = network.setup(&context).await.unwrap();
        assert!(setup.managed);
        assert_eq!(setup.pending.map(|p| p.interface).as_deref(), Some("enp1s0"));
        assert!(matches!(network.apply(&settings, 120, &context).await, Err(ConfigureError::Pending)));
        network.confirm(&context).await.unwrap();
        assert!(matches!(network.confirm(&context).await, Err(ConfigureError::Refused(_))));
    }
}
