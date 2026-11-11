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
use zendiloos_network::domain::{
    BondMode, DeviceAction, DeviceKind, Edit, IfupdownFile, InterfaceSettings, Ipv4Setting, Ipv6Setting,
    LinkFile, NetworkdFile, Subject,
};

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

    async fn send(&self, request: Request, context: &RequestContext) -> Result<(), ConfigureError> {
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

fn name(text: &str) -> Result<proto::InterfaceName, ConfigureError> {
    proto::InterfaceName::parse(text).map_err(|_| invalid("interface name"))
}

fn config(settings: &InterfaceSettings) -> Result<proto::NetworkConfig, ConfigureError> {
    Ok(proto::NetworkConfig::addresses(
        name(&settings.interface)?,
        match settings.ipv4 {
            Ipv4Setting::Dhcp { keep } => proto::Ipv4Config::Dhcp { request: keep },
            Ipv4Setting::Static { address, prefix, gateway } => proto::Ipv4Config::Static {
                address,
                prefix: proto::Prefix::new(prefix).map_err(|_| invalid("prefix"))?,
                gateway,
            },
            Ipv4Setting::Off => proto::Ipv4Config::Off {},
        },
        match settings.ipv6 {
            Ipv6Setting::Automatic => proto::Ipv6Config::Automatic,
            Ipv6Setting::Off => proto::Ipv6Config::Off,
        },
        proto::NameServers::new(settings.dns.clone()).map_err(|_| invalid("list of name servers"))?,
    ))
}

/// An interface's file: its addresses, or the bridge or bond it is a port
/// of (`kinds` names the bonds), and the VLANs on it.
fn link(link: &LinkFile, bonds: &[String]) -> Result<proto::NetworkConfig, ConfigureError> {
    let port_of = match &link.port_of {
        Some(master) if bonds.contains(master) => Some(proto::PortOf::Bond { name: name(master)? }),
        Some(master) => Some(proto::PortOf::Bridge { name: name(master)? }),
        None => None,
    };
    let vlans = link.vlans.iter().map(|vlan| name(vlan)).collect::<Result<Vec<_>, _>>()?;
    let config = proto::NetworkConfig {
        port_of,
        vlans: proto::Vlans::new(vlans).map_err(|_| invalid("set of VLANs"))?,
        keep_up: link.keep_up,
        ..config(&link.settings)?
    };
    // As the agent reads it: a port without addresses of its own.
    let fields = serde_json::to_value(&config).map_err(|_| invalid("interface's file"))?;
    serde_json::from_value(fields).map_err(|_| invalid("interface's file"))
}

fn action(action: DeviceAction) -> proto::DeviceAction {
    match action {
        DeviceAction::Made => proto::DeviceAction::Made,
        DeviceAction::Changed => proto::DeviceAction::Changed,
        DeviceAction::Removed => proto::DeviceAction::Removed,
    }
}

/// `edit` in the agent's terms; what the bonds are, the agent knows from
/// its own files and those written here.
fn edit(edit: &Edit, bonds: &[String]) -> Result<proto::NetworkEdit, ConfigureError> {
    let change = match &edit.subject {
        Subject::Interface(interface) => proto::NetworkChange::Interface { interface: name(interface)? },
        Subject::Device(device, done) => {
            proto::NetworkChange::Device { device: name(device)?, action: action(*done) }
        }
    };
    let netdevs = edit
        .devices
        .iter()
        .map(|device| {
            let kind = match &device.kind {
                DeviceKind::Bridge { stp } => proto::NetdevKind::Bridge { stp: *stp },
                DeviceKind::Bond { mode } => proto::NetdevKind::Bond {
                    mode: match mode {
                        BondMode::ActiveBackup => proto::BondMode::ActiveBackup,
                        BondMode::Lacp => proto::BondMode::Lacp,
                        BondMode::BalanceAlb => proto::BondMode::BalanceAlb,
                    },
                },
                DeviceKind::Vlan { id, .. } => {
                    proto::NetdevKind::Vlan { id: proto::VlanId::new(*id).map_err(|_| invalid("VLAN id"))? }
                }
            };
            let mac = device
                .mac
                .as_deref()
                .map(proto::MacAddress::parse)
                .transpose()
                .map_err(|_| invalid("MAC address"))?;
            let members = device.members.iter().map(|member| name(member)).collect::<Result<Vec<_>, _>>()?;
            let netdev = proto::Netdev { name: name(&device.name)?, kind, mac, members };
            let fields = serde_json::to_value(&netdev).map_err(|_| invalid("device"))?;
            serde_json::from_value::<proto::Netdev>(fields).map_err(|_| invalid("device"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let removed = edit.removed.iter().map(|device| name(device)).collect::<Result<Vec<_>, _>>()?;
    let links = edit.links.iter().map(|file| link(file, bonds)).collect::<Result<Vec<_>, _>>()?;
    let fields = proto::NetworkEdit { change, netdevs, removed, links };
    // Checked as the agent checks it: each device and interface once.
    let value = serde_json::to_value(&fields).map_err(|_| invalid("change"))?;
    serde_json::from_value(value).map_err(|_| invalid("change"))
}

fn pending(pending: proto::PendingNetwork) -> PendingChange {
    let name = |name: &proto::InterfaceName| name.as_str().to_owned();
    PendingChange {
        change: match &pending.change {
            proto::NetworkChange::Interface { interface } => Change::Interface(name(interface)),
            proto::NetworkChange::Takeover { interfaces } => {
                Change::Takeover(interfaces.iter().map(name).collect())
            }
            proto::NetworkChange::Device { device, action } => Change::Device(
                name(device),
                match action {
                    proto::DeviceAction::Made => DeviceAction::Made,
                    proto::DeviceAction::Changed => DeviceAction::Changed,
                    proto::DeviceAction::Removed => DeviceAction::Removed,
                },
            ),
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
            Response::NetworkState { managed, pending: waiting, network_manager, ifupdown: host, files } => {
                Ok(HostNetwork {
                    managed,
                    network_manager,
                    pending: waiting.map(pending),
                    ifupdown: host.map(ifupdown),
                    files: files
                        .into_iter()
                        .map(|file| NetworkdFile { name: file.name, text: file.text })
                        .collect(),
                })
            }
            other => Err(unexpected(other)),
        }
    }

    async fn change(
        &self,
        change: &Edit,
        confirm_within_seconds: u16,
        context: &RequestContext,
    ) -> Result<(), ConfigureError> {
        let confirm_within =
            proto::ConfirmWithin::seconds(confirm_within_seconds).map_err(|_| invalid("time to confirm"))?;
        // Which devices are bonds: those written here, and those zendiloOS
        // made before, as its files say.
        let files = self.state(context).await?.files;
        let configured = zendiloos_network::domain::Configuration::read(&files);
        let bonds: Vec<String> = configured
            .devices
            .iter()
            .chain(change.devices.iter())
            .filter(|device| matches!(device.kind, DeviceKind::Bond { .. }))
            .map(|device| device.name.clone())
            .collect();
        let change = edit(change, &bonds)?;
        self.send(Request::ChangeNetwork { change, confirm_within }, context).await
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
        self.send(request, context).await
    }

    async fn confirm(&self, context: &RequestContext) -> Result<(), ConfigureError> {
        self.send(Request::ConfirmNetwork {}, context).await
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

        let apply = |settings: InterfaceSettings| Edit {
            subject: Subject::Interface("enp1s0".into()),
            devices: vec![],
            removed: vec![],
            links: vec![LinkFile { settings, port_of: None, vlans: vec![], keep_up: false }],
            effects: vec![],
        };
        network.change(&apply(settings.clone()), 120, &context).await.unwrap();
        let state = network.state(&context).await.unwrap();
        assert!(state.managed && state.ifupdown.is_none());
        assert_eq!(state.pending.map(|p| p.change), Some(Change::Interface("enp1s0".into())));
        let again = network.change(&apply(settings.clone()), 120, &context).await;
        assert!(matches!(again, Err(ConfigureError::Pending)));
        network.confirm(&context).await.unwrap();
        assert!(matches!(network.confirm(&context).await, Err(ConfigureError::Refused(_))));
        let off =
            InterfaceSettings { ipv4: Ipv4Setting::Off, ipv6: Ipv6Setting::Off, dns: vec![], ..settings };
        network.change(&apply(off), 120, &context).await.unwrap();
    }

    /// A bridge over the card and a bond in it, in the agent's terms: the
    /// bond's members are ports of a bond, the bond one of the bridge.
    #[test]
    fn devices_go_to_the_agent_as_its_types() {
        use zendiloos_network::domain::Device;
        let port = |interface: &str, of: &str| LinkFile {
            settings: InterfaceSettings {
                interface: interface.into(),
                ipv4: Ipv4Setting::Off,
                ipv6: Ipv6Setting::Off,
                dns: vec![],
            },
            port_of: Some(of.into()),
            vlans: vec![],
            keep_up: false,
        };
        let made = Edit {
            subject: Subject::Device("br0".into(), DeviceAction::Made),
            devices: vec![Device {
                name: "br0".into(),
                kind: DeviceKind::Bridge { stp: false },
                members: vec!["bond0".into()],
                mac: Some("52:54:00:12:34:56".into()),
            }],
            removed: vec![],
            links: vec![port("bond0", "br0"), port("enp2s0", "bond0")],
            effects: vec![],
        };
        let sent = edit(&made, &["bond0".into()]).unwrap();
        assert_eq!(
            sent.change,
            proto::NetworkChange::Device {
                device: proto::InterfaceName::parse("br0").unwrap(),
                action: proto::DeviceAction::Made
            }
        );
        assert_eq!(sent.netdevs[0].mac.as_ref().map(proto::MacAddress::as_str), Some("52:54:00:12:34:56"));
        assert!(matches!(sent.links[0].port_of, Some(proto::PortOf::Bridge { .. })));
        assert!(matches!(sent.links[1].port_of, Some(proto::PortOf::Bond { .. })));

        // What the agent would refuse is refused here.
        let mut twice = made.clone();
        twice.links.push(port("bond0", "br0"));
        assert!(edit(&twice, &[]).is_err());
        let mut odd = made;
        odd.devices[0].mac = Some("not a mac".into());
        assert!(edit(&odd, &[]).is_err());
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
