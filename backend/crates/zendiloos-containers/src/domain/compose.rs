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
    /// Each bind mount with where it appears in the container.
    pub mounts: Vec<ServiceMount>,
    /// The host's side of every device.
    pub devices: Vec<String>,
    pub cap_add: Vec<String>,
    pub security_opt: Vec<String>,
    /// Every key the service sets, to name those not known as harmless.
    pub keys: Vec<String>,
}

/// A bind mount of a service: the host's folder or file, and where the
/// container sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceMount {
    pub source: String,
    pub target: String,
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
    /// Without the kernel's confinement (seccomp, AppArmor), or confined
    /// otherwise than Docker's default.
    Unconfined {
        service: String,
    },
    /// A setting not known as harmless (`cgroup_parent`, `volumes_from`,
    /// `runtime`, another container's namespace, …): Compose has many, and
    /// new ones come; what is not known is named rather than let through.
    Unusual {
        service: String,
        setting: String,
    },
    /// Takes in files when the project is saved or built (`build`,
    /// `env_file`, `extends`, `include`): whoever may change them, anybody
    /// who may write the share, decides what runs. `include` belongs to no
    /// service (`service` is empty).
    FromShare {
        service: String,
        setting: String,
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
            Risk::Unusual { .. } => "unusual",
            Risk::FromShare { .. } => "from_share",
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
            | Risk::Unconfined { service }
            | Risk::Unusual { service, .. }
            | Risk::FromShare { service, .. } => service,
        }
    }

    /// The path, namespace, devices or capabilities it concerns.
    pub fn detail(&self) -> Option<String> {
        match self {
            Risk::DockerSocket { path, .. } | Risk::SystemPath { path, .. } => Some(path.clone()),
            Risk::HostNamespace { namespace, .. } => Some((*namespace).to_owned()),
            Risk::Devices { devices, .. } => Some(devices.join(", ")),
            Risk::Capabilities { capabilities, .. } => Some(capabilities.join(", ")),
            Risk::Unusual { setting, .. } | Risk::FromShare { setting, .. } => Some(setting.clone()),
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

/// Settings of a service that take in files, from the share the project
/// lives in or wherever they point: `build` at every deployment, the
/// others when the project is saved.
const FROM_FILES: &[&str] = &["build", "env_file", "extends"];

/// The settings of a Compose file as written that take in other files
/// when Compose reads it: `include`, and a service's `extends` and
/// `env_file`. Compose resolves them, so what it made of the file no longer
/// shows them; what they bring in is checked there, but that they do is
/// said too. Block style, as people write Compose files.
pub fn file_reads(text: &str) -> Vec<Risk> {
    let mut found = Vec::new();
    let mut in_services = false;
    let (mut service_indent, mut key_indent): (Option<usize>, Option<usize>) = (None, None);
    let mut service = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let Some((key, _)) = trimmed.split_once(':') else { continue };
        let key = key.trim().trim_matches(['"', '\'']);
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            in_services = key == "services";
            (service_indent, key_indent) = (None, None);
            if key == "include" {
                found.push(Risk::FromShare { service: String::new(), setting: key.to_owned() });
            }
            continue;
        }
        if !in_services {
            continue;
        }
        match service_indent {
            Some(level) if indent < level => {}
            Some(level) if indent > level => {
                let level = *key_indent.get_or_insert(indent);
                if indent == level && matches!(key, "extends" | "env_file") {
                    let risk = Risk::FromShare { service: service.clone(), setting: key.to_owned() };
                    if !found.contains(&risk) {
                        found.push(risk);
                    }
                }
            }
            _ => {
                service_indent = Some(indent);
                key_indent = None;
                service = key.to_owned();
            }
        }
    }
    // Elsewhere too (an anchor merged into services, flow style): named
    // without a service, once.
    for setting in ["env_file", "extends"] {
        let named =
            found.iter().any(|risk| matches!(risk, Risk::FromShare { setting: s, .. } if s == setting));
        if !named && text.lines().any(|line| has_key(line, setting)) {
            found.push(Risk::FromShare { service: String::new(), setting: setting.to_owned() });
        }
    }
    found
}

/// Whether `line` (outside a comment) sets `key`, in block or flow style.
fn has_key(line: &str, key: &str) -> bool {
    let line = line.split(" #").next().unwrap_or_default();
    if line.trim_start().starts_with('#') {
        return false;
    }
    line.match_indices(key).any(|(at, _)| {
        let before = line[..at].trim_end_matches(['"', '\'']);
        let after = line[at + key.len()..].trim_start_matches(['"', '\'']).trim_start();
        let starts = before.is_empty() || before.ends_with([' ', '{', ',', '-', '\t']);
        starts && after.starts_with(':')
    })
}

/// Settings of a service that cannot reach beyond its container, as
/// Compose writes them normalized; those that can are looked at one by one
/// (`privileged`, binds, `devices`, `cap_add`, namespaces, `security_opt`).
const HARMLESS: &[&str] = &[
    "annotations",
    "attach",
    "blkio_config",
    "cap_drop",
    "command",
    "configs",
    "container_name",
    "cpu_count",
    "cpu_percent",
    "cpu_period",
    "cpu_quota",
    "cpu_rt_period",
    "cpu_rt_runtime",
    "cpu_shares",
    "cpus",
    "cpuset",
    "depends_on",
    "deploy",
    "develop",
    "dns",
    "dns_opt",
    "dns_search",
    "domainname",
    "entrypoint",
    "environment",
    "expose",
    "external_links",
    "extra_hosts",
    "group_add",
    "healthcheck",
    "hostname",
    "image",
    "init",
    "labels",
    "links",
    "logging",
    "mac_address",
    "mem_limit",
    "mem_reservation",
    "mem_swappiness",
    "memswap_limit",
    "networks",
    "oom_kill_disable",
    "oom_score_adj",
    "pids_limit",
    "platform",
    "ports",
    "profiles",
    "pull_policy",
    "read_only",
    "restart",
    "scale",
    "secrets",
    "shm_size",
    "stdin_open",
    "stop_grace_period",
    "stop_signal",
    "storage_opt",
    "tmpfs",
    "tty",
    "ulimits",
    "user",
    "volumes",
    "working_dir",
];

/// Looked at by `risks` one by one.
const LOOKED_AT: &[&str] =
    &["privileged", "devices", "cap_add", "network_mode", "pid", "ipc", "userns_mode", "security_opt"];

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
        // Another container's namespace: what that container may, this one
        // may too; one of the same project is looked at itself.
        for (key, mode) in
            [("network_mode", &service.network_mode), ("pid", &service.pid), ("ipc", &service.ipc)]
        {
            if let Some(mode) = mode.as_deref().filter(|mode| mode.starts_with("container:")) {
                found.push(Risk::Unusual { service: name(), setting: format!("{key}: {mode}") });
            }
        }
        if !service.devices.is_empty() {
            found.push(Risk::Devices { service: name(), devices: service.devices.clone() });
        }
        if !service.cap_add.is_empty() {
            found.push(Risk::Capabilities { service: name(), capabilities: service.cap_add.clone() });
        }
        // Only `no-new-privileges` adds confinement; anything else (a profile
        // of its own, a label turned off) may take it away.
        let confining =
            |option: &String| matches!(option.as_str(), "no-new-privileges" | "no-new-privileges:true");
        if service.security_opt.iter().any(|option| !confining(option)) {
            found.push(Risk::Unconfined { service: name() });
        }
        for key in &service.keys {
            if FROM_FILES.contains(&key.as_str()) {
                found.push(Risk::FromShare { service: name(), setting: key.clone() });
                continue;
            }
            let known = HARMLESS.contains(&key.as_str())
                || LOOKED_AT.contains(&key.as_str())
                || key.starts_with("x-");
            if !known {
                found.push(Risk::Unusual { service: name(), setting: key.clone() });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_takes_in_files_is_named() {
        let text = "\
# a project
include:
  - other.yaml
services:
  app:
    image: nginx
    env_file: app.env
    environment:
      env_file: not a setting of the service
    extends:
      file: base.yaml
      service: base
  \"db\":
    image: postgres
    env_file:
      - db.env
volumes:
  data:
    env_file: not a service
";
        let found: Vec<(String, Option<String>)> =
            file_reads(text).iter().map(|r| (r.service().to_owned(), r.detail())).collect();
        assert_eq!(
            found,
            [
                (String::new(), Some("include".into())),
                ("app".into(), Some("env_file".into())),
                ("app".into(), Some("extends".into())),
                ("db".into(), Some("env_file".into())),
            ]
        );
        assert!(file_reads("services:\n  app:\n    image: nginx\n").is_empty());
        let anchored =
            "x-base: &base\n  env_file: shared.env\nservices:\n  app:\n    <<: *base\n    image: nginx\n";
        let found: Vec<_> =
            file_reads(anchored).iter().map(|r| (r.service().to_owned(), r.detail())).collect();
        assert_eq!(found, [(String::new(), Some("env_file".into()))], "from an anchor");
        let flow = "services: {app: {image: nginx, extends: {file: base.yaml, service: b}}}\n";
        assert_eq!(file_reads(flow).len(), 1, "flow style");
        assert!(file_reads("services:\n  app:\n    image: nginx # env_file: no\n").is_empty(), "a comment");

        let model = ComposeModel {
            services: vec![ComposeService {
                name: "app".into(),
                keys: vec!["image".into(), "build".into()],
                ..ComposeService::default()
            }],
        };
        assert_eq!(risks(&model), [Risk::FromShare { service: "app".into(), setting: "build".into() }]);
    }

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
    fn settings_not_known_as_harmless_are_named() {
        let keys = |keys: &[&str]| keys.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        let model = ComposeModel {
            services: vec![
                ComposeService {
                    name: "web".into(),
                    keys: keys(&[
                        "image",
                        "ports",
                        "volumes",
                        "networks",
                        "restart",
                        "x-note",
                        "security_opt",
                    ]),
                    security_opt: vec!["no-new-privileges:true".into()],
                    ..ComposeService::default()
                },
                ComposeService {
                    name: "odd".into(),
                    keys: keys(&[
                        "image",
                        "cgroup_parent",
                        "device_cgroup_rules",
                        "volumes_from",
                        "network_mode",
                    ]),
                    network_mode: Some("container:portainer".into()),
                    security_opt: vec!["seccomp=/volume1/docker/odd/profile.json".into()],
                    ..ComposeService::default()
                },
            ],
        };

        let named: Vec<_> =
            risks(&model).iter().map(|r| (r.service().to_owned(), r.kind(), r.detail())).collect();

        assert_eq!(
            named,
            [
                ("odd".to_owned(), "unusual", Some("network_mode: container:portainer".to_owned())),
                ("odd".to_owned(), "unconfined", None),
                ("odd".to_owned(), "unusual", Some("cgroup_parent".to_owned())),
                ("odd".to_owned(), "unusual", Some("device_cgroup_rules".to_owned())),
                ("odd".to_owned(), "unusual", Some("volumes_from".to_owned())),
            ],
            "the plain service asks nothing"
        );
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
