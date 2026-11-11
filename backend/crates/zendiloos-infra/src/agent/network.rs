use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ErrorCode, PackageRunState, PackageTask, Request, Response,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_network::application::{
    Change, ConfigureError, HostNetwork, IfupdownHost, NetworkConfigurator, PendingChange,
};
use zendiloos_network::domain::{IfupdownFile, InterfaceSettings, Ipv4Setting, Ipv6Setting};

use super::{WHAT, agent_error, call, call_raw, unexpected};

/// How often apt's run is looked at while systemd-resolved is installed,
/// and for how long at most.
const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);

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
            other => Err(refused(other)),
        }
    }
}

/// The agent's no, in its words.
fn refused(response: Response) -> ConfigureError {
    match response {
        Response::Error { code: ErrorCode::Forbidden, message } => ConfigureError::Refused(message),
        Response::Error { code: ErrorCode::Conflict, .. } => ConfigureError::Pending,
        Response::Error { code, message } => agent_error(code, &message).into(),
        other => unexpected(other).into(),
    }
}

fn invalid(what: &str) -> ConfigureError {
    ConfigureError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn config(settings: &InterfaceSettings) -> Result<proto::NetworkConfig, ConfigureError> {
    Ok(proto::NetworkConfig {
        interface: proto::InterfaceName::parse(&settings.interface).map_err(|_| invalid("interface"))?,
        ipv4: match settings.ipv4 {
            Ipv4Setting::Dhcp { keep } => proto::Ipv4Config::Dhcp { request: keep },
            Ipv4Setting::Static { address, prefix, gateway } => proto::Ipv4Config::Static {
                address,
                prefix: proto::Prefix::new(prefix).map_err(|_| invalid("prefix"))?,
                gateway,
            },
            Ipv4Setting::Off => proto::Ipv4Config::Off {},
        },
        ipv6: match settings.ipv6 {
            Ipv6Setting::Automatic => proto::Ipv6Config::Automatic,
            Ipv6Setting::Off => proto::Ipv6Config::Off,
        },
        dns: proto::NameServers::new(settings.dns.clone()).map_err(|_| invalid("list of name servers"))?,
    })
}

fn pending(pending: proto::PendingNetwork) -> PendingChange {
    let name = |name: &proto::InterfaceName| name.as_str().to_owned();
    PendingChange {
        change: match &pending.change {
            proto::NetworkChange::Interface { interface } => Change::Interface(name(interface)),
            proto::NetworkChange::Takeover { interfaces } => {
                Change::Takeover(interfaces.iter().map(name).collect())
            }
        },
        revert_at: Timestamp::from_unix_seconds(pending.revert_at),
    }
}

fn ifupdown(host: proto::Ifupdown) -> IfupdownHost {
    IfupdownHost {
        files: host
            .files
            .into_iter()
            .map(|file| IfupdownFile { path: file.path, text: file.text, skipped: file.skipped })
            .collect(),
        digest: host.digest.as_str().to_owned(),
        active: host.active,
        resolved_installed: host.resolved_installed,
    }
}

#[async_trait]
impl NetworkConfigurator for AgentNetwork {
    async fn state(&self, context: &RequestContext) -> Result<HostNetwork, PortError> {
        match call(self.transport.as_ref(), Request::NetworkState {}, context).await? {
            Response::NetworkState { managed, pending: waiting, network_manager, ifupdown: host } => {
                Ok(HostNetwork {
                    managed,
                    network_manager,
                    pending: waiting.map(pending),
                    ifupdown: host.map(ifupdown),
                })
            }
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

    async fn install_resolved(&self, context: &RequestContext) -> Result<(), ConfigureError> {
        let request = Request::StartPackages { task: PackageTask::InstallNetworkd, release: None };
        let id = match call_raw(self.transport.as_ref(), request, context).await? {
            Response::PackagesStarted { id } => id,
            other => return Err(refused(other)),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport.as_ref(), Request::PackagesState {}, context).await? {
                Response::Packages { run, .. } => run,
                other => return Err(unexpected(other).into()),
            };
            match run {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => return Ok(()),
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    let why = run.error.unwrap_or_else(|| "apt failed".into());
                    return Err(
                        PortError::new(WHAT, format!("systemd-resolved was not installed: {why}")).into()
                    );
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(PortError::new(WHAT, "apt did not finish within an hour").into());
                }
                _ => {}
            }
        }
    }

    async fn take_over(
        &self,
        interfaces: &[InterfaceSettings],
        digest: &str,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError> {
        let configs = interfaces.iter().map(config).collect::<Result<Vec<_>, _>>()?;
        let request = Request::TakeOverNetwork {
            configs: proto::TakeoverConfigs::new(configs).map_err(|_| invalid("set of interfaces"))?,
            digest: proto::FilesDigest::parse(digest).map_err(|_| invalid("digest of ifupdown's files"))?,
            confirm_within: proto::ConfirmWithin::seconds(confirm_within_seconds)
                .map_err(|_| invalid("time to confirm"))?,
        };
        self.change(request, context).await
    }

    async fn confirm(&self, context: &RequestContext) -> Result<(), ConfigureError> {
        self.change(Request::ConfirmNetwork {}, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    fn fixed() -> InterfaceSettings {
        InterfaceSettings {
            interface: "enp1s0".into(),
            ipv4: Ipv4Setting::Static {
                address: "192.168.1.10".parse().unwrap(),
                prefix: 24,
                gateway: Some("192.168.1.1".parse().unwrap()),
            },
            ipv6: Ipv6Setting::Automatic,
            dns: vec!["192.168.1.1".parse().unwrap()],
        }
    }

    #[tokio::test]
    async fn changes_wait_for_confirmation_through_the_agent() {
        let network = AgentNetwork::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let settings = fixed();

        network.apply(&settings, 120, &context).await.unwrap();
        let state = network.state(&context).await.unwrap();
        assert!(state.managed && state.ifupdown.is_none());
        assert_eq!(state.pending.map(|p| p.change), Some(Change::Interface("enp1s0".into())));
        assert!(matches!(network.apply(&settings, 120, &context).await, Err(ConfigureError::Pending)));
        network.confirm(&context).await.unwrap();
        assert!(matches!(network.confirm(&context).await, Err(ConfigureError::Refused(_))));
        let off =
            InterfaceSettings { ipv4: Ipv4Setting::Off, ipv6: Ipv6Setting::Off, dns: vec![], ..settings };
        network.apply(&off, 120, &context).await.unwrap();
    }

    /// The simulated host as the Debian installer leaves it: ifupdown, and
    /// no systemd-resolved until installed (apt takes three seconds).
    #[tokio::test]
    async fn ifupdown_is_taken_over_through_the_agent() {
        let interfaces = "auto lo\niface lo inet loopback\nallow-hotplug enp1s0\niface enp1s0 inet dhcp\n";
        let network = AgentNetwork::new(Arc::new(SimulatedAgent::new().with_ifupdown(interfaces)));
        let context = RequestContext::default();
        let state = network.state(&context).await.unwrap();
        assert!(!state.managed);
        let host = state.ifupdown.unwrap();
        assert_eq!(host.files[0].text, interfaces);
        assert!(host.active && !host.resolved_installed);
        let dhcp = InterfaceSettings {
            ipv4: Ipv4Setting::Dhcp { keep: Some("192.168.1.20".parse().unwrap()) },
            dns: vec![],
            ..fixed()
        };
        let refused = network.take_over(std::slice::from_ref(&dhcp), &host.digest, 120, &context).await;
        assert!(matches!(&refused, Err(ConfigureError::Refused(why)) if why.contains("systemd-resolved")));

        network.install_resolved(&context).await.unwrap();
        assert!(network.state(&context).await.unwrap().ifupdown.unwrap().resolved_installed);
        let wrong = network.take_over(std::slice::from_ref(&dhcp), &"0".repeat(64), 120, &context).await;
        assert!(matches!(wrong, Err(ConfigureError::Refused(_))));
        network.take_over(&[dhcp], &host.digest, 120, &context).await.unwrap();
        let state = network.state(&context).await.unwrap();
        assert!(state.managed && state.ifupdown.is_none());
        assert_eq!(state.pending.unwrap().change, Change::Takeover(vec!["enp1s0".into()]));
        network.confirm(&context).await.unwrap();
    }
}
