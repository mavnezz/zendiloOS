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
    /// The named volumes it mounts, by their keys in the file.
    pub volumes: Vec<String>,
    /// The networks it joins, by their keys in the file.
    pub networks: Vec<String>,
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

/// A named volume of the file (`volumes:` at its top). Docker's local
/// driver mounts what its `driver_opts` say: a folder of the machine with
/// `o: bind`, a disk or another file system with `type` and `device`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeVolume {
    /// What the services call it.
    pub key: String,
    /// `None` for Docker's own, `local`.
    pub driver: Option<String>,
    /// Made outside the project: the file does not say what it mounts.
    pub external: bool,
    /// Of `driver_opts`: the file system's `type`, the options `o` and
    /// the `device`.
    pub kind: Option<String>,
    pub options: Option<String>,
    pub device: Option<String>,
}

impl ComposeVolume {
    /// The folder or file of the machine it binds: its `device`, when `o`
    /// has `bind` or `rbind`, which mount it as a bind mount does.
    pub fn bound(&self) -> Option<&str> {
        let options = self.options.as_deref().unwrap_or_default();
        let binds = options.split(',').any(|option| matches!(option.trim(), "bind" | "rbind"));
        if binds { self.device.as_deref() } else { None }
    }
}

/// A network of the file (`networks:` at its top).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeNetwork {
    /// What the services call it.
    pub key: String,
    /// Its name in Docker: `<project>_<key>` unless the file names it.
    pub name: String,
    /// `None` for Docker's own, `bridge`.
    pub driver: Option<String>,
    /// Made outside the project: the file does not say what it is.
    pub external: bool,
    /// The names of its `driver_opts`, which can make a bridge something
    /// else (another interface, routed addresses).
    pub options: Vec<String>,
}

/// A Compose file as Compose read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeModel {
    pub services: Vec<ComposeService>,
    pub volumes: Vec<ComposeVolume>,
    pub networks: Vec<ComposeNetwork>,
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
    /// A folder of the system, or one zendiloOS keeps for itself on a
    /// volume (`@docker`, `@homes`, `@apps`): no share's.
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
    /// Devices given to the container, or mounted for it as a named
    /// volume's file system (a partition of a disk).
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

/// Whether `path` lies on a volume, `/volume<N>/…`, outside the folders
/// zendiloOS keeps there for itself: Docker's data (`@docker`, where the
/// containers' settings are, root at their next start), the personal
/// folders (`@homes`), the apps installed the earlier way (`@apps`), and
/// whatever else starts with `@` or `.` (ZFS's `.zfs`), as no share's name
/// does. A path through `..` may lead anywhere.
fn on_a_volume(path: &str) -> bool {
    path.strip_prefix("/volume").and_then(|rest| rest.split_once('/')).is_some_and(|(number, rest)| {
        !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
            && !rest.is_empty()
            && !rest.starts_with(['@', '.'])
            && !rest.split('/').any(|name| name == "..")
    })
}

/// File systems a named volume may mount without reaching into the
/// machine: memory, and shares of other machines over the network.
const HARMLESS_FILE_SYSTEMS: &[&str] = &["tmpfs", "nfs", "nfs4", "cifs", "smb3"];

/// What mounting the named `volume` lets `service` reach beyond a volume
/// of Docker's own; a bind is among the service's binds already.
fn volume_risk(service: &str, volume: &ComposeVolume) -> Option<Risk> {
    let unusual = |what: String| {
        let setting = format!("volume {}: {what}", volume.key);
        Some(Risk::Unusual { service: service.to_owned(), setting })
    };
    if volume.external {
        return unusual("external".into());
    }
    if let Some(driver) = volume.driver.as_deref().filter(|driver| *driver != "local") {
        return unusual(format!("driver {driver}"));
    }
    let kind = volume.kind.as_deref();
    if volume.bound().is_some() || kind.is_some_and(|kind| HARMLESS_FILE_SYSTEMS.contains(&kind)) {
        return None;
    }
    // The local driver mounts the device as that file system: a partition
    // of a disk of the machine, the system's too.
    if let Some(device) = volume.device.as_deref().filter(|device| device.starts_with('/')) {
        return Some(Risk::Devices { service: service.to_owned(), devices: vec![device.to_owned()] });
    }
    // Another kind (`overlay`, whose options name folders of the machine).
    kind.and_then(|kind| unusual(format!("type {kind}")))
}

/// What joining `network` lets `service` do beyond a bridge of its
/// project's own.
fn network_risk(service: &str, network: &ComposeNetwork) -> Option<Risk> {
    // Compose makes the network's name the container's network mode: one
    // called `host` is the machine's network, as `network_mode: host` is.
    if network.name == "host" || network.driver.as_deref() == Some("host") {
        return Some(Risk::HostNetwork { service: service.to_owned() });
    }
    let unusual = |what: String| {
        Some(Risk::Unusual { service: service.to_owned(), setting: format!("network {what}") })
    };
    if network.external {
        // Docker's own bridge, or no network at all.
        return match network.name.as_str() {
            "bridge" | "none" => None,
            name => unusual(format!("{name}: external")),
        };
    }
    if let Some(driver) = network.driver.as_deref().filter(|driver| *driver != "bridge") {
        return unusual(format!("{}: driver {driver}", network.key));
    }
    if !network.options.is_empty() {
        return unusual(format!("{}: driver_opts {}", network.key, network.options.join(", ")));
    }
    None
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
        let named = service.volumes.iter().filter_map(|key| model.volumes.iter().find(|v| v.key == *key));
        let joined = service.networks.iter().filter_map(|key| model.networks.iter().find(|n| n.key == *key));
        let beyond = named
            .filter_map(|volume| volume_risk(&service.name, volume))
            .chain(joined.filter_map(|network| network_risk(&service.name, network)));
        for risk in beyond {
            if !found.contains(&risk) {
                found.push(risk);
            }
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
            ..ComposeModel::default()
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
            ..ComposeModel::default()
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
            ..ComposeModel::default()
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
            ..ComposeModel::default()
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

    #[test]
    fn the_folders_zendiloos_keeps_on_a_volume_are_no_shares() {
        let binds = [
            "/volume1/@docker",
            "/volume1/@docker/containers",
            "/volume2/@homes",
            "/volume1/@apps/jellyfin",
            "/volume1/.zfs/snapshot",
            "/volume1/docker/../@docker",
            "/volume1/docker/app/data",
            "/volume1/media",
        ];
        let model = ComposeModel {
            services: vec![ComposeService {
                name: "app".into(),
                binds: binds.iter().map(|b| (*b).to_owned()).collect(),
                ..ComposeService::default()
            }],
            ..ComposeModel::default()
        };

        let named: Vec<_> = risks(&model).iter().filter_map(Risk::detail).collect();

        assert_eq!(named, &binds[..6], "the shares' folders ask nothing");
    }

    #[test]
    fn what_named_volumes_mount_is_named() {
        let volume =
            |key: &str, kind: Option<&str>, options: Option<&str>, device: Option<&str>| ComposeVolume {
                key: key.into(),
                kind: kind.map(Into::into),
                options: options.map(Into::into),
                device: device.map(Into::into),
                ..ComposeVolume::default()
            };
        let volumes = vec![
            volume("plain", None, None, None),
            volume("disk", Some("ext4"), None, Some("/dev/sda2")),
            volume(
                "layers",
                Some("overlay"),
                Some("lowerdir=/,upperdir=/tmp/u,workdir=/tmp/w"),
                Some("overlay"),
            ),
            volume("cache", Some("tmpfs"), Some("size=100m"), Some("tmpfs")),
            volume("music", Some("nfs"), Some("addr=10.0.0.2,ro"), Some(":/export/music")),
            volume("root", Some("none"), Some("rbind"), Some("/")),
            ComposeVolume { key: "plugin".into(), driver: Some("rclone".into()), ..ComposeVolume::default() },
            ComposeVolume { key: "elsewhere".into(), external: true, ..ComposeVolume::default() },
        ];
        let keys: Vec<String> = volumes.iter().map(|v| v.key.clone()).collect();
        let model = ComposeModel {
            services: vec![ComposeService { name: "app".into(), volumes: keys, ..ComposeService::default() }],
            volumes,
            ..ComposeModel::default()
        };

        let named: Vec<_> = risks(&model).iter().map(|r| (r.kind(), r.detail())).collect();

        assert_eq!(
            named,
            [
                ("devices", Some("/dev/sda2".to_owned())),
                ("unusual", Some("volume layers: type overlay".to_owned())),
                ("unusual", Some("volume plugin: driver rclone".to_owned())),
                ("unusual", Some("volume elsewhere: external".to_owned())),
            ],
            "memory, a share of another machine and a bind (among the binds) ask nothing here"
        );
        let bound = |options: &str| volume("v", Some("none"), Some(options), Some("/srv")).bound().is_some();
        assert!(bound("bind") && bound("rbind") && bound("ro, rbind") && !bound("rbindx") && !bound("ro"));
    }

    #[test]
    fn networks_other_than_a_bridge_of_the_project_are_named() {
        let network = |key: &str, name: &str| ComposeNetwork {
            key: key.into(),
            name: name.into(),
            ..ComposeNetwork::default()
        };
        let networks = vec![
            network("default", "app_default"),
            ComposeNetwork { external: true, ..network("machine", "host") },
            ComposeNetwork { driver: Some("host".into()), ..network("own", "app_own") },
            ComposeNetwork { external: true, ..network("proxy", "proxy") },
            ComposeNetwork { external: true, ..network("docker", "bridge") },
            ComposeNetwork { driver: Some("macvlan".into()), ..network("lan", "app_lan") },
            ComposeNetwork {
                options: vec!["com.docker.network.bridge.name".into()],
                ..network("br", "app_br")
            },
        ];
        let service = |name: &str, keys: &[&str]| ComposeService {
            name: name.into(),
            networks: keys.iter().map(|k| (*k).to_owned()).collect(),
            ..ComposeService::default()
        };
        let model = ComposeModel {
            services: vec![
                service("plain", &["default", "docker"]),
                service("hosted", &["machine", "own"]),
                service("other", &["proxy", "lan", "br"]),
            ],
            networks,
            ..ComposeModel::default()
        };

        let named: Vec<_> =
            risks(&model).iter().map(|r| (r.service().to_owned(), r.kind(), r.detail())).collect();

        assert_eq!(
            named,
            [
                ("hosted".to_owned(), "host_network", None),
                ("other".to_owned(), "unusual", Some("network proxy: external".to_owned())),
                ("other".to_owned(), "unusual", Some("network lan: driver macvlan".to_owned())),
                (
                    "other".to_owned(),
                    "unusual",
                    Some("network br: driver_opts com.docker.network.bridge.name".to_owned())
                ),
            ],
            "the host's network once; the project's bridge and Docker's ask nothing"
        );
    }
}
