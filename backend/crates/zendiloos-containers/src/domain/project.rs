//! Compose projects: those of administrators, kept by zendiloOS with their
//! file in a share; those of the App Center; and those made some other way.

use zendiloos_kernel::Timestamp;

use super::engine::{ComposeListing, Container};

/// A project's name as Compose allows it: lowercase letters, digits, `-`
/// and `_`, starting with a letter or digit, at most 63.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a project's name has lowercase letters, digits, - and _, starts with a letter or digit, at most 63")]
pub struct InvalidProjectName;

impl ProjectName {
    pub fn parse(text: &str) -> Result<Self, InvalidProjectName> {
        let mut bytes = text.bytes();
        let valid = (1..=63).contains(&text.len())
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if valid { Ok(Self(text.to_owned())) } else { Err(InvalidProjectName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The file name a project's Compose file gets.
pub const COMPOSE_FILE: &str = "compose.yaml";
/// Where the App Center keeps its apps' Compose files.
pub const APP_FILES: &str = "/etc/zendiloos/apps/";

/// A project an administrator made here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnProject {
    pub name: ProjectName,
    /// The share its folder is in.
    pub share_id: i64,
    /// The folder in the share: `docker/teslamate`.
    pub folder: String,
    /// The folder on the machine: `/volume1/docker/docker/teslamate`.
    pub directory: String,
    /// Its Compose file on the machine.
    pub file: String,
    pub created_at: Timestamp,
    pub created_by: String,
}

/// Where a project comes from, and so what may be done with it here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Its Compose file is edited and deployed here.
    Own(OwnProject),
    /// An app of a catalog: the App Center changes it.
    App { app: String },
    /// Made another way: started, stopped, updated and removed here; its
    /// files are shown.
    Foreign { files: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub origin: Origin,
    /// Its containers, by name.
    pub containers: Vec<Container>,
}

impl Project {
    pub fn running(&self) -> usize {
        self.containers.iter().filter(|c| c.state.is_up()).count()
    }
}

/// Every project: Compose's, the containers', and the administrators' own
/// even when none of their containers exist yet; by name.
pub fn assemble(own: Vec<OwnProject>, listings: &[ComposeListing], containers: &[Container]) -> Vec<Project> {
    let mut names: Vec<String> = own.iter().map(|p| p.name.to_string()).collect();
    names.extend(listings.iter().map(|l| l.name.clone()));
    names.extend(containers.iter().filter_map(|c| c.project.clone()));
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let origin = if let Some(project) = own.iter().find(|p| p.name.as_str() == name) {
                Origin::Own(project.clone())
            } else {
                let files =
                    listings.iter().find(|l| l.name == name).map(|l| l.files.clone()).unwrap_or_default();
                match files.iter().find_map(|file| file.strip_prefix(APP_FILES)) {
                    Some(file) => Origin::App { app: file.trim_end_matches(".json").to_owned() },
                    None => Origin::Foreign { files },
                }
            };
            let mut members: Vec<Container> =
                containers.iter().filter(|c| c.project.as_deref() == Some(name.as_str())).cloned().collect();
            members.sort_by(|a, b| a.name.cmp(&b.name));
            Project { name, origin, containers: members }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::engine::ContainerState;

    fn container(name: &str, project: &str, state: ContainerState) -> Container {
        Container {
            id: name.into(),
            name: name.into(),
            image: "nginx".into(),
            image_id: "sha256:1".into(),
            state,
            health: None,
            exit_code: None,
            created_at: Timestamp::from_unix_seconds(0),
            started_at: None,
            project: Some(project.into()),
            service: None,
            ports: vec![],
            networks: vec![],
            mounts: vec![],
            restart_policy: "no".into(),
        }
    }

    fn own(name: &str) -> OwnProject {
        OwnProject {
            name: ProjectName::parse(name).unwrap(),
            share_id: 1,
            folder: format!("docker/{name}"),
            directory: format!("/volume1/docker/docker/{name}"),
            file: format!("/volume1/docker/docker/{name}/compose.yaml"),
            created_at: Timestamp::from_unix_seconds(0),
            created_by: "alice".into(),
        }
    }

    #[test]
    fn projects_come_from_every_source_once() {
        let listings = [
            ComposeListing { name: "kuma".into(), files: vec!["/etc/zendiloos/apps/kuma.json".into()] },
            ComposeListing { name: "stack".into(), files: vec!["/opt/stack/compose.yml".into()] },
            ComposeListing {
                name: "tesla".into(),
                files: vec!["/volume1/docker/docker/tesla/compose.yaml".into()],
            },
        ];
        let containers = [
            container("tesla-db", "tesla", ContainerState::Running),
            container("tesla-app", "tesla", ContainerState::Exited),
            container("kuma-1", "kuma", ContainerState::Running),
        ];

        let projects = assemble(vec![own("tesla"), own("fresh")], &listings, &containers);

        let names: Vec<_> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["fresh", "kuma", "stack", "tesla"]);
        assert!(matches!(&projects[0].origin, Origin::Own(p) if p.name.as_str() == "fresh"));
        assert_eq!(projects[1].origin, Origin::App { app: "kuma".into() });
        assert_eq!(projects[2].origin, Origin::Foreign { files: vec!["/opt/stack/compose.yml".into()] });
        let tesla = &projects[3];
        assert!(matches!(tesla.origin, Origin::Own(_)));
        assert_eq!((tesla.running(), tesla.containers.len()), (1, 2));
        assert_eq!(tesla.containers[0].name, "tesla-app", "by name");
    }

    #[test]
    fn names_are_what_compose_allows() {
        assert!(ProjectName::parse("teslamate").is_ok());
        assert!(ProjectName::parse("home_assistant-2").is_ok());
        for bad in ["", "Tesla", "-x", "a b", "a/b", &"a".repeat(64)] {
            assert!(ProjectName::parse(bad).is_err(), "{bad}");
        }
    }
}
