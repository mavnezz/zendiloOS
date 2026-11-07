//! One container in detail, as Docker inspects it, its processes, and the
//! words a terminal in it runs.

use super::compose::ComposeService;
use super::engine::{Container, Mount};

/// Where zendiloOS pins the folders in volumes a project made here binds
/// (`docs/architecture.md`, 4.9): Docker sees the pin, the administrator
/// named the folder.
pub const PROJECT_PINS: &str = "/run/zendiloos-project-shares/";

/// A network the container is in, with its addresses there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedNetwork {
    pub name: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub gateway: Option<String>,
    pub mac: Option<String>,
    /// The names it is reached by in that network.
    pub aliases: Vec<String>,
}

/// A device of the host passed into the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMapping {
    pub host: String,
    pub container: String,
    /// `rwm`: read, write, make nodes.
    pub permissions: String,
}

/// What reaches beyond the container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Privileges {
    /// Every device and capability: as good as root on the machine.
    pub privileged: bool,
    pub capabilities_added: Vec<String>,
    pub capabilities_dropped: Vec<String>,
    pub devices: Vec<DeviceMapping>,
    /// `no-new-privileges:true`, `apparmor=unconfined`, …
    pub security_options: Vec<String>,
    pub read_only_root: bool,
    /// `host` for the machine's processes or IPC; empty for its own.
    pub pid_mode: String,
    pub ipc_mode: String,
}

/// What the container may use; `None`: no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResourceLimits {
    /// Processors, `1.5` for one and a half.
    pub cpus: Option<f64>,
    pub memory_bytes: Option<u64>,
    /// What the container gets at least when memory runs short.
    pub memory_reservation_bytes: Option<u64>,
    pub processes: Option<u64>,
}

/// The check Docker runs to tell whether the container is healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Healthcheck {
    /// `CMD-SHELL pg_isready`, as Docker keeps it.
    pub test: Vec<String>,
    pub interval_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub retries: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerDetails {
    pub container: Container,
    /// What it runs: its program and the arguments.
    pub command: Vec<String>,
    pub working_dir: Option<String>,
    /// As set in the image or the Compose file, `999:999`; empty is root.
    pub user: Option<String>,
    pub hostname: Option<String>,
    /// Name and value, in Docker's order; values may be passwords.
    pub environment: Vec<(String, String)>,
    /// `bridge`, `host`, `none`, a network's name or `container:…`.
    pub network_mode: String,
    pub networks: Vec<AttachedNetwork>,
    pub privileges: Privileges,
    pub limits: ResourceLimits,
    /// For the `on-failure` restart policy; 0 is without end.
    pub restart_retries: u64,
    pub healthcheck: Option<Healthcheck>,
    /// What the last health check printed.
    pub health_output: Option<String>,
}

/// A process in a container, as `docker top` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerProcess {
    /// On the machine, not in the container.
    pub pid: u32,
    pub parent: u32,
    pub user: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub resident_bytes: u64,
    /// How long it runs, as `ps` writes it: `02:14:07`, `3-01:02:03`.
    pub elapsed: String,
    pub command: String,
}

/// Where each mount's files are on the machine, as the administrator named
/// them: a project's bind into a volume is a pin Docker sees
/// ([`PROJECT_PINS`]); `service` is the service as its project was
/// confirmed, which names the folder for each target.
pub fn host_paths(mounts: &[Mount], service: Option<&ComposeService>) -> Vec<String> {
    mounts
        .iter()
        .map(|mount| {
            let pinned = mount.kind == "bind" && mount.source.starts_with(PROJECT_PINS);
            let named = service.filter(|_| pinned).and_then(|service| {
                service.mounts.iter().find(|m| m.target == mount.destination).map(|m| m.source.clone())
            });
            named.unwrap_or_else(|| mount.source.clone())
        })
        .collect()
}

/// The words of a terminal's command as typed, split at whitespace: 1 to
/// 16 words of at most 256 bytes, without control characters, the first no
/// `docker exec` option; `None` otherwise.
pub fn command_words(line: &str) -> Option<Vec<String>> {
    let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    let valid = (1..=16).contains(&words.len())
        && words.first().is_some_and(|first| !first.starts_with('-'))
        && words.iter().all(|word| word.len() <= 256 && !word.chars().any(char::is_control));
    valid.then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ServiceMount;

    fn bind(source: &str, destination: &str) -> Mount {
        Mount {
            kind: "bind".into(),
            source: source.into(),
            destination: destination.into(),
            read_only: false,
        }
    }

    #[test]
    fn pinned_binds_show_the_folder_the_administrator_named() {
        let service = ComposeService {
            name: "db".into(),
            mounts: vec![ServiceMount {
                source: "/volume1/docker/tesla/data".into(),
                target: "/var/lib/postgresql/data".into(),
            }],
            ..ComposeService::default()
        };
        let mounts = [
            bind("/run/zendiloos-project-shares/tesla/3f9a", "/var/lib/postgresql/data"),
            bind("/run/zendiloos-project-shares/tesla/77aa", "/elsewhere"),
            bind("/etc/localtime", "/etc/localtime"),
            Mount {
                kind: "volume".into(),
                source: "cache".into(),
                destination: "/cache".into(),
                read_only: true,
            },
        ];
        assert_eq!(
            host_paths(&mounts, Some(&service)),
            [
                "/volume1/docker/tesla/data",
                "/run/zendiloos-project-shares/tesla/77aa",
                "/etc/localtime",
                "cache"
            ]
        );
        assert_eq!(host_paths(&mounts[..1], None), ["/run/zendiloos-project-shares/tesla/3f9a"]);
    }

    #[test]
    fn a_terminals_command_is_words_and_no_option() {
        assert_eq!(command_words(" /bin/bash  -l "), Some(vec!["/bin/bash".to_owned(), "-l".to_owned()]));
        for bad in ["", "  ", "-u root sh", "sh\u{7}", &vec!["a"; 17].join(" "), &"x".repeat(257)] {
            assert_eq!(command_words(bad), None, "{bad:?}");
        }
    }
}
