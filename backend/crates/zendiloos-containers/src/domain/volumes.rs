//! Docker's volumes, as Docker reports them, and who uses them.

use std::collections::BTreeMap;

use zendiloos_kernel::Timestamp;

use super::Container;

/// The label Compose gives what a project made.
pub const PROJECT_LABEL: &str = "com.docker.compose.project";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    /// `local`, or a plugin's.
    pub driver: String,
    pub labels: BTreeMap<String, String>,
    /// Docker mounts something else in its place (options such as
    /// `type: nfs` or `o: bind`): its data is not in Docker's folder.
    pub mounts_elsewhere: bool,
    pub created_at: Option<Timestamp>,
}

impl Volume {
    /// The Compose project that made it.
    pub fn project(&self) -> Option<&str> {
        self.labels.get(PROJECT_LABEL).map(String::as_str)
    }

    /// Whether zendiloOS backs it up and restores it: a local volume whose
    /// data lies in Docker's folder.
    pub fn backed_up_here(&self) -> bool {
        self.driver == "local" && !self.mounts_elsewhere
    }
}

/// A container that uses a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeUser {
    pub name: String,
    pub running: bool,
}

/// The containers, running or not, that mount the volume `name`, by name.
pub fn users(name: &str, containers: &[Container]) -> Vec<VolumeUser> {
    let mut users: Vec<VolumeUser> = containers
        .iter()
        .filter(|c| c.mounts.iter().any(|m| m.kind == "volume" && m.source == name))
        .map(|c| VolumeUser { name: c.name.clone(), running: c.state.is_up() })
        .collect();
    users.sort_by(|a, b| a.name.cmp(&b.name));
    users
}

/// What one volume's data takes up; `complete` is false where counting
/// stopped at its limits, so it takes up at least that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSize {
    pub name: String,
    pub bytes: u64,
    pub complete: bool,
}

/// Whether Docker takes `name` for a volume: a letter or digit, then
/// letters, digits, `_`, `.` and `-`; at most 200 characters here, so that a
/// backup's name stays a file name.
pub fn is_volume_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    (2..=200).contains(&name.len())
        && bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContainerState, Mount};

    fn container(name: &str, running: bool, mounts: &[(&str, &str)]) -> Container {
        Container {
            id: format!("{name}-id"),
            name: name.to_owned(),
            image: "postgres:17".into(),
            image_id: "sha256:x".into(),
            state: if running { ContainerState::Running } else { ContainerState::Exited },
            health: None,
            exit_code: None,
            created_at: Timestamp::from_unix_seconds(0),
            started_at: None,
            project: None,
            service: None,
            ports: vec![],
            networks: vec![],
            mounts: mounts
                .iter()
                .map(|(kind, source)| Mount {
                    kind: (*kind).into(),
                    source: (*source).into(),
                    destination: "/data".into(),
                    read_only: false,
                })
                .collect(),
            restart_policy: "no".into(),
        }
    }

    #[test]
    fn a_volume_is_used_by_the_containers_that_mount_it_running_or_not() {
        let containers = [
            container("tesla-db-1", true, &[("volume", "tesla_db")]),
            container("backup", false, &[("volume", "tesla_db"), ("bind", "/volume1/x")]),
            container("web", true, &[("bind", "tesla_db")]),
        ];
        assert_eq!(
            users("tesla_db", &containers),
            [
                VolumeUser { name: "backup".into(), running: false },
                VolumeUser { name: "tesla-db-1".into(), running: true }
            ]
        );
        assert!(users("other", &containers).is_empty());
    }

    #[test]
    fn local_volumes_with_their_data_in_dockers_folder_are_backed_up() {
        let volume = Volume {
            name: "tesla_db".into(),
            driver: "local".into(),
            labels: BTreeMap::from([(PROJECT_LABEL.to_owned(), "tesla".to_owned())]),
            mounts_elsewhere: false,
            created_at: None,
        };
        assert_eq!(volume.project(), Some("tesla"));
        assert!(volume.backed_up_here());
        assert!(!Volume { mounts_elsewhere: true, ..volume.clone() }.backed_up_here());
        assert!(!Volume { driver: "rclone".into(), ..volume.clone() }.backed_up_here());
        assert_eq!(Volume { labels: BTreeMap::new(), ..volume }.project(), None);
        assert!(is_volume_name("tesla_db") && is_volume_name(&"a".repeat(64)));
        for bad in ["", "x", "-x", ".x", "a/b", "a b", &"x".repeat(201)] {
            assert!(!is_volume_name(bad), "{bad}");
        }
    }
}
