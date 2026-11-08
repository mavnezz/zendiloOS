//! What the firewall's services stand for now, from the other contexts:
//! the web interface's ports, the SSH server, the file services, the ports
//! Docker projects publish, and the NAS's interfaces.

use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_certificates::application::Certificates;
use zendiloos_containers::application::Containers;
use zendiloos_containers::domain::Project;
use zendiloos_firewall::application::ServiceFacts;
use zendiloos_firewall::domain::{Facts, LocalInterface, ProjectPorts, Transport};
use zendiloos_kernel::RequestContext;
use zendiloos_network::application::NetworkUseCases;
use zendiloos_sharing::application::SharingUseCases;
use zendiloos_system::application::SystemUseCases;

/// [`ServiceFacts`] from the other contexts' use cases. What cannot be
/// found out now is as it was found out last.
pub struct ServicesForFirewall {
    certificates: Certificates,
    system: SystemUseCases,
    sharing: SharingUseCases,
    containers: Containers,
    network: NetworkUseCases,
    last: Mutex<Facts>,
}

impl ServicesForFirewall {
    pub fn new(
        certificates: Certificates,
        system: SystemUseCases,
        sharing: SharingUseCases,
        containers: Containers,
        network: NetworkUseCases,
    ) -> Self {
        Self { certificates, system, sharing, containers, network, last: Mutex::new(Facts::default()) }
    }
}

/// The host ports a project's running containers publish to other
/// machines (not those bound to the loopback only).
pub fn published(project: &Project) -> ProjectPorts {
    let mut ports: Vec<(Transport, u16)> = project
        .containers
        .iter()
        .filter(|container| container.state.is_up())
        .flat_map(|container| container.ports.iter())
        .filter(|port| {
            !port
                .host_ip
                .as_deref()
                .is_some_and(|ip| ip.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))
        })
        .filter_map(|port| Transport::parse(&port.protocol).map(|transport| (transport, port.host_port)))
        .filter(|(_, port)| *port != 0)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ProjectPorts { name: project.name.clone(), ports }
}

#[async_trait]
impl ServiceFacts for ServicesForFirewall {
    async fn facts(&self, context: &RequestContext) -> Facts {
        let mut facts = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        match self.certificates.web_server(context).await {
            Ok(web) => {
                let mut ports: Vec<u16> = Vec::new();
                for server in std::iter::once(web.current).chain(web.pending.map(|(pending, _)| pending)) {
                    ports.extend(server.http_port);
                    ports.push(server.https_port);
                }
                ports.sort_unstable();
                ports.dedup();
                facts.web_ports = ports;
                facts.https_port = web.current.https_port;
            }
            Err(error) => tracing::warn!(%error, "firewall: cannot read the web interface's ports"),
        }
        match self.system.get_ssh.execute(context).await {
            Ok(ssh) => (facts.ssh_on, facts.ssh_port) = (ssh.installed && ssh.running, ssh.port),
            Err(error) => tracing::warn!(%error, "firewall: cannot read whether SSH runs"),
        }
        match self.sharing.file_services().await {
            Ok(services) => {
                facts.smb_on = services.smb;
                facts.discovery_on = services.wsdd || services.bonjour;
                match self.sharing.list().await {
                    Ok(shares) => {
                        facts.nfs_on = services.nfs && shares.iter().any(|s| s.shared && !s.nfs.is_empty());
                    }
                    Err(error) => tracing::warn!(%error, "firewall: cannot read the shared folders"),
                }
            }
            Err(error) => tracing::warn!(%error, "firewall: cannot read the file services"),
        }
        match self.containers.projects(context).await {
            Ok(projects) => facts.projects = projects.iter().map(published).collect(),
            // Docker not installed or not answering: the ports as last seen.
            Err(error) => tracing::debug!(%error, "firewall: cannot read the Docker projects"),
        }
        facts.clone_into(&mut self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        facts
    }

    async fn interfaces(&self) -> Vec<LocalInterface> {
        match self.network.get_network.execute().await {
            Ok(network) => network
                .interfaces
                .into_iter()
                .map(|interface| LocalInterface {
                    name: interface.name,
                    addresses: interface.addresses.iter().map(|a| (a.ip, a.prefix)).collect(),
                })
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "firewall: cannot read the network interfaces");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_containers::domain::{Container, ContainerState, Origin, PublishedPort};
    use zendiloos_kernel::Timestamp;

    use super::*;

    fn container(state: ContainerState, ports: Vec<PublishedPort>) -> Container {
        Container {
            id: "c".into(),
            name: "web".into(),
            image: "nginx".into(),
            image_id: "sha256:1".into(),
            state,
            health: None,
            exit_code: None,
            created_at: Timestamp::from_unix_seconds(0),
            started_at: None,
            project: Some("site".into()),
            service: Some("web".into()),
            ports,
            networks: vec![],
            mounts: vec![],
            restart_policy: "no".into(),
        }
    }

    fn port(host_ip: Option<&str>, host_port: u16, protocol: &str) -> PublishedPort {
        PublishedPort {
            host_ip: host_ip.map(str::to_owned),
            host_port,
            container_port: 80,
            protocol: protocol.into(),
        }
    }

    #[test]
    fn a_project_publishes_its_running_containers_ports_to_others() {
        let project = Project {
            name: "site".into(),
            origin: Origin::Foreign { files: vec![] },
            containers: vec![
                container(
                    ContainerState::Running,
                    vec![
                        port(None, 8080, "tcp"),
                        port(Some("0.0.0.0"), 8080, "tcp"),
                        port(Some("::"), 3478, "udp"),
                        port(Some("127.0.0.1"), 9000, "tcp"),
                        port(Some("::1"), 9001, "tcp"),
                        port(None, 9100, "sctp"),
                    ],
                ),
                container(ContainerState::Exited, vec![port(None, 7000, "tcp")]),
            ],
        };
        assert_eq!(published(&project).ports, [(Transport::Tcp, 8080), (Transport::Udp, 3478)]);
        assert_eq!(published(&project).name, "site");
    }
}
