//! What a Compose file lets its containers do. Administrators may run any
//! Compose file; before one runs, they confirm whatever gives a container
//! power over the machine.

/// A service as Compose reads it, with what matters here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub image: Option<String>,
    pub privileged: bool,
    /// `host`, `none`, `bridge`, `service:…`, `container:…`.
    pub network_mode: Option<String>,
    pub pid: Option<String>,
    pub ipc: Option<String>,
    pub userns_mode: Option<String>,
    /// The host's side of every bind mount.
    pub binds: Vec<String>,
    /// The host's side of every device.
    pub devices: Vec<String>,
    pub cap_add: Vec<String>,
    pub security_opt: Vec<String>,
}

/// A Compose file as Compose read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeModel {
    pub services: Vec<ComposeService>,
}

/// Something a service may do that reaches beyond its container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Risk {
    /// Every device and capability: as good as root on the machine.
    Privileged {
        service: String,
    },
    /// Docker's socket: it can start any container, so as good as root.
    DockerSocket {
        service: String,
        path: String,
    },
    /// A folder of the system, not of a volume.
    SystemPath {
        service: String,
        path: String,
    },
    /// The machine's network, its ports and interfaces.
    HostNetwork {
        service: String,
    },
    /// The machine's processes, shared memory or user IDs.
    HostNamespace {
        service: String,
        namespace: &'static str,
    },
    Devices {
        service: String,
        devices: Vec<String>,
    },
    Capabilities {
        service: String,
        capabilities: Vec<String>,
    },
    /// Without the kernel's confinement (seccomp, AppArmor).
    Unconfined {
        service: String,
    },
}

impl Risk {
    /// A short word for each kind, as the API names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Risk::Privileged { .. } => "privileged",
            Risk::DockerSocket { .. } => "docker_socket",
            Risk::SystemPath { .. } => "system_path",
            Risk::HostNetwork { .. } => "host_network",
            Risk::HostNamespace { .. } => "host_namespace",
            Risk::Devices { .. } => "devices",
            Risk::Capabilities { .. } => "capabilities",
            Risk::Unconfined { .. } => "unconfined",
        }
    }

    pub fn service(&self) -> &str {
        match self {
            Risk::Privileged { service }
            | Risk::DockerSocket { service, .. }
            | Risk::SystemPath { service, .. }
            | Risk::HostNetwork { service }
            | Risk::HostNamespace { service, .. }
            | Risk::Devices { service, .. }
            | Risk::Capabilities { service, .. }
            | Risk::Unconfined { service } => service,
        }
    }

    /// The path, namespace, devices or capabilities it concerns.
    pub fn detail(&self) -> Option<String> {
        match self {
            Risk::DockerSocket { path, .. } | Risk::SystemPath { path, .. } => Some(path.clone()),
            Risk::HostNamespace { namespace, .. } => Some((*namespace).to_owned()),
            Risk::Devices { devices, .. } => Some(devices.join(", ")),
            Risk::Capabilities { capabilities, .. } => Some(capabilities.join(", ")),
            Risk::Privileged { .. } | Risk::HostNetwork { .. } | Risk::Unconfined { .. } => None,
        }
    }
}

/// Whether `path` lies on a volume: `/volume<N>/…`.
fn on_a_volume(path: &str) -> bool {
    path.strip_prefix("/volume").and_then(|rest| rest.split_once('/')).is_some_and(|(number, rest)| {
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty()
    })
}

/// Everything in `model` that reaches beyond its containers, service by
/// service in the file's order.
pub fn risks(model: &ComposeModel) -> Vec<Risk> {
    let mut found = Vec::new();
    for service in &model.services {
        let name = || service.name.clone();
        if service.privileged {
            found.push(Risk::Privileged { service: name() });
        }
        for path in &service.binds {
            if path.ends_with("docker.sock") {
                found.push(Risk::DockerSocket { service: name(), path: path.clone() });
            } else if !on_a_volume(path) {
                found.push(Risk::SystemPath { service: name(), path: path.clone() });
            }
        }
        if service.network_mode.as_deref() == Some("host") {
            found.push(Risk::HostNetwork { service: name() });
        }
        for (mode, namespace) in
            [(&service.pid, "pid"), (&service.ipc, "ipc"), (&service.userns_mode, "userns")]
        {
            if mode.as_deref() == Some("host") {
                found.push(Risk::HostNamespace { service: name(), namespace });
            }
        }
        if !service.devices.is_empty() {
            found.push(Risk::Devices { service: name(), devices: service.devices.clone() });
        }
        if !service.cap_add.is_empty() {
            found.push(Risk::Capabilities { service: name(), capabilities: service.cap_add.clone() });
        }
        if service.security_opt.iter().any(|option| option.ends_with("unconfined")) {
            found.push(Risk::Unconfined { service: name() });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_service_on_a_volume_asks_nothing() {
        let model = ComposeModel {
            services: vec![ComposeService {
                name: "teslamate".into(),
                image: Some("teslamate/teslamate:latest".into()),
                binds: vec!["/volume1/docker/teslamate/import".into()],
                ..ComposeService::default()
            }],
        };
        assert!(risks(&model).is_empty());
    }

    #[test]
    fn power_over_the_machine_is_named() {
        let model = ComposeModel {
            services: vec![
                ComposeService {
                    name: "portainer".into(),
                    binds: vec!["/var/run/docker.sock".into(), "/volume1/docker/portainer".into()],
                    ..ComposeService::default()
                },
                ComposeService {
                    name: "homeassistant".into(),
                    privileged: true,
                    network_mode: Some("host".into()),
                    binds: vec!["/etc/localtime".into(), "/volume".into()],
                    devices: vec!["/dev/ttyUSB0".into()],
                    cap_add: vec!["NET_ADMIN".into()],
                    security_opt: vec!["apparmor:unconfined".into()],
                    pid: Some("host".into()),
                    ..ComposeService::default()
                },
            ],
        };

        let kinds: Vec<_> = risks(&model).iter().map(|r| (r.service().to_owned(), r.kind())).collect();

        assert_eq!(
            kinds,
            [
                ("portainer".to_owned(), "docker_socket"),
                ("homeassistant".to_owned(), "privileged"),
                ("homeassistant".to_owned(), "system_path"),
                ("homeassistant".to_owned(), "system_path"),
                ("homeassistant".to_owned(), "host_network"),
                ("homeassistant".to_owned(), "host_namespace"),
                ("homeassistant".to_owned(), "devices"),
                ("homeassistant".to_owned(), "capabilities"),
                ("homeassistant".to_owned(), "unconfined"),
            ]
        );
    }
}
