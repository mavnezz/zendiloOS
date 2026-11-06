//! What runs on the Docker engine, as Docker reports it.

use zendiloos_kernel::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
}

impl ContainerState {
    /// Docker's word for it; anything unknown counts as dead.
    pub fn parse(text: &str) -> Self {
        match text {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" => Self::Exited,
            _ => Self::Dead,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Removing => "removing",
            Self::Exited => "exited",
            Self::Dead => "dead",
        }
    }

    /// Counts as running: its processes exist.
    pub fn is_up(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Restarting)
    }
}

/// A port the container publishes on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPort {
    /// `None`: every address.
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
}

/// A folder or volume in the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// `bind` or `volume`.
    pub kind: String,
    /// The host's path, or the volume's name.
    pub source: String,
    pub destination: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    /// The full ID.
    pub id: String,
    pub name: String,
    /// As it was given: `postgres:17`.
    pub image: String,
    pub image_id: String,
    pub state: ContainerState,
    /// `healthy`, `unhealthy` or `starting`, for containers with a check.
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    /// The Compose project and service it belongs to.
    pub project: Option<String>,
    pub service: Option<String>,
    pub ports: Vec<PublishedPort>,
    pub networks: Vec<String>,
    pub mounts: Vec<Mount>,
    /// `no`, `always`, `unless-stopped`, `on-failure`.
    pub restart_policy: String,
}

/// One container's use at one moment. Bytes over the network are counted
/// since it started.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerUsage {
    pub id: String,
    pub name: String,
    /// 100 is one processor fully used.
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    /// The container's limit, or the machine's memory.
    pub memory_limit_bytes: u64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

/// Every container's use at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSample {
    pub at: Timestamp,
    pub containers: Vec<ContainerUsage>,
    /// The machine's processors: 100 % of each.
    pub processors: u32,
}

impl UsageSample {
    /// What all containers use of the machine's processors, 0 to 100.
    pub fn cpu_share(&self) -> f64 {
        let total: f64 = self.containers.iter().map(|c| c.cpu_percent).sum();
        (total / f64::from(self.processors.max(1))).clamp(0.0, 100.0)
    }

    pub fn memory_bytes(&self) -> u64 {
        self.containers.iter().map(|c| c.memory_bytes).sum()
    }

    /// The machine's memory, as Docker gives it for containers without a
    /// limit of their own.
    pub fn memory_total_bytes(&self) -> u64 {
        self.containers.iter().map(|c| c.memory_limit_bytes).max().unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `sha256:…`.
    pub id: String,
    /// `repository:tag` each, `postgres:17`; empty for an image that lost
    /// its names.
    pub references: Vec<String>,
    /// The repository's digests: `postgres@sha256:…`.
    pub digests: Vec<String>,
    pub size_bytes: u64,
    pub created_at: Option<Timestamp>,
}

impl Image {
    /// Its first name, or its short ID.
    pub fn label(&self) -> String {
        self.references.first().cloned().unwrap_or_else(|| short_id(&self.id).to_owned())
    }
}

/// The 12 hex digits Docker shows.
pub fn short_id(id: &str) -> &str {
    let hex = id.strip_prefix("sha256:").unwrap_or(id);
    &hex[..hex.len().min(12)]
}

/// Networks Docker itself has; they cannot be removed.
pub const BUILT_IN_NETWORKS: [&str; 3] = ["bridge", "host", "none"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub id: String,
    pub name: String,
    /// `bridge`, `host`, `macvlan`, …
    pub driver: String,
    pub internal: bool,
    pub subnets: Vec<String>,
    pub gateways: Vec<String>,
    /// The names of the containers in it now.
    pub containers: Vec<String>,
    /// The Compose project that made it.
    pub project: Option<String>,
    pub created_at: Option<Timestamp>,
}

impl Network {
    pub fn is_built_in(&self) -> bool {
        BUILT_IN_NETWORKS.contains(&self.name.as_str())
    }
}

/// What Docker's data takes up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub images_bytes: u64,
    pub containers_bytes: u64,
    pub volumes_bytes: u64,
    pub build_cache_bytes: u64,
}

impl DiskUsage {
    pub fn total_bytes(&self) -> u64 {
        self.images_bytes + self.containers_bytes + self.volumes_bytes + self.build_cache_bytes
    }
}

/// A Compose project as `docker compose ls` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeListing {
    pub name: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    /// Only one that does not run.
    Remove,
}

impl ContainerAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Remove => "remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(cpu: f64, memory: u64, limit: u64) -> ContainerUsage {
        ContainerUsage {
            id: "a".into(),
            name: "a".into(),
            cpu_percent: cpu,
            memory_bytes: memory,
            memory_limit_bytes: limit,
            received_bytes: 0,
            sent_bytes: 0,
        }
    }

    #[test]
    fn the_machine_is_shared_by_its_processors() {
        let sample = UsageSample {
            at: Timestamp::from_unix_seconds(0),
            containers: vec![usage(150.0, 100, 8_000), usage(50.0, 300, 1_000)],
            processors: 4,
        };
        assert!((sample.cpu_share() - 50.0).abs() < f64::EPSILON);
        assert_eq!((sample.memory_bytes(), sample.memory_total_bytes()), (400, 8_000));
    }

    #[test]
    fn states_and_ids_read_as_docker_writes_them() {
        assert!(ContainerState::parse("running").is_up());
        assert!(!ContainerState::parse("exited").is_up());
        assert_eq!(ContainerState::parse("whatever"), ContainerState::Dead);
        assert_eq!(short_id("sha256:0772aa7e48ba0ed3cec5277b"), "0772aa7e48ba");
    }
}
