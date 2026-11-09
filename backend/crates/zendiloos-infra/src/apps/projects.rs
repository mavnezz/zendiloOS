//! The projects made from apps of a catalog, as the Containers context has
//! them: for the App Center to show which of its apps run, and where.

use async_trait::async_trait;
use zendiloos_apps::application::ports::{AppProjects, TemplateProject};
use zendiloos_apps::domain::Containers as Counted;
use zendiloos_containers::application::Containers;
use zendiloos_containers::domain::{ContainerState, Origin, Project};
use zendiloos_kernel::{PortError, RequestContext};

/// [`AppProjects`] through the Containers context's use cases.
#[derive(Clone)]
pub struct ContainersForApps {
    containers: Containers,
}

impl ContainersForApps {
    pub fn new(containers: Containers) -> Self {
        Self { containers }
    }
}

/// A project made from a template, with its containers counted and what
/// they publish; `None` for any other.
fn template_project(project: Project) -> Option<TemplateProject> {
    let Origin::Own(own) = &project.origin else { return None };
    let template = own.template.clone()?;
    let mut counted = Counted::default();
    let mut published = Vec::new();
    for container in &project.containers {
        match container.state {
            ContainerState::Running => counted.running += 1,
            ContainerState::Restarting => counted.restarting += 1,
            _ => counted.other += 1,
        }
        published.extend(container.ports.iter().map(|port| (port.container_port, port.host_port)));
    }
    published.sort_unstable();
    published.dedup();
    Some(TemplateProject {
        name: project.name,
        source: template.source,
        app: template.app,
        containers: counted,
        published,
        created_at: own.created_at,
    })
}

#[async_trait]
impl AppProjects for ContainersForApps {
    async fn projects(&self, context: &RequestContext) -> Result<Vec<TemplateProject>, PortError> {
        let projects = self
            .containers
            .projects(context)
            .await
            .map_err(|error| PortError::new("Docker projects", error))?;
        Ok(projects.into_iter().filter_map(template_project).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zendiloos_containers::application::NewProject;
    use zendiloos_containers::domain::ProjectTemplate;
    use zendiloos_containers::testing::TestContainers;
    use zendiloos_jobs::testing::FixedClock;
    use zendiloos_kernel::{Actor, AuditEntry, AuditLog};

    use super::*;

    struct NoAudit;

    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn only_projects_made_from_templates_count() {
        let test = TestContainers::new(Arc::new(NoAudit), Arc::new(FixedClock));
        let alice = Actor { account_id: 1, name: "alice".into() };
        let context = RequestContext::default();
        let new = NewProject {
            name: "media".into(),
            share_id: 1,
            folder: "docker/media".into(),
            compose: "services:\n  app:\n    image: nginx\n".into(),
            start: false,
            accepted: None,
            template: Some(ProjectTemplate { source: 1, app: "jellyfin".into() }),
        };
        test.containers.create_project(new, &alice, &context).await.unwrap();
        let mine = NewProject { name: "mine".into(), template: None, ..new_project() };
        test.containers.create_project(mine, &alice, &context).await.unwrap();

        let projects = ContainersForApps::new(test.containers.clone()).projects(&context).await.unwrap();

        assert_eq!(projects.len(), 1, "{projects:?}");
        let media = &projects[0];
        assert_eq!((media.name.as_str(), media.source, media.app.as_str()), ("media", 1, "jellyfin"));
        assert_eq!(media.containers, Counted::default(), "not deployed");
    }

    #[test]
    fn containers_are_counted_and_their_ports_named_once() {
        use zendiloos_containers::domain::{Container, OwnProject, ProjectName, PublishedPort};
        use zendiloos_kernel::Timestamp;

        let container = |name: &str, state, ports: &[(u16, u16)]| Container {
            id: name.into(),
            name: name.into(),
            image: "nginx".into(),
            image_id: "sha256:1".into(),
            state,
            health: None,
            exit_code: None,
            created_at: Timestamp::from_unix_seconds(0),
            started_at: None,
            project: Some("media".into()),
            service: None,
            ports: ports
                .iter()
                .map(|(inside, outside)| PublishedPort {
                    host_ip: None,
                    host_port: *outside,
                    container_port: *inside,
                    protocol: "tcp".into(),
                })
                .collect(),
            networks: vec![],
            mounts: vec![],
            restart_policy: "unless-stopped".into(),
        };
        let own = OwnProject {
            name: ProjectName::parse("media").unwrap(),
            share_id: 1,
            folder: "docker/media".into(),
            directory: "/volume1/docker/docker/media".into(),
            file: "/volume1/docker/docker/media/compose.yaml".into(),
            compose: None,
            config: None,
            template: Some(ProjectTemplate { source: 2, app: "jellyfin".into() }),
            created_at: Timestamp::from_unix_seconds(7),
            created_by: "alice".into(),
        };
        let project = Project {
            name: "media".into(),
            origin: Origin::Own(own.clone()),
            containers: vec![
                container("web", ContainerState::Running, &[(8096, 8099), (8096, 8099)]),
                container("db", ContainerState::Restarting, &[]),
                container("job", ContainerState::Exited, &[(80, 81)]),
            ],
        };

        let made = template_project(project).unwrap();

        assert_eq!(made.containers, Counted { running: 1, restarting: 1, other: 1 });
        assert_eq!(made.published, [(80, 81), (8096, 8099)]);
        assert_eq!((made.source, made.created_at), (2, Timestamp::from_unix_seconds(7)));
        let plain = OwnProject { template: None, ..own };
        let project = Project { name: "media".into(), origin: Origin::Own(plain), containers: vec![] };
        assert_eq!(template_project(project), None);
        let foreign =
            Project { name: "x".into(), origin: Origin::Foreign { files: vec![] }, containers: vec![] };
        assert_eq!(template_project(foreign), None);
    }

    fn new_project() -> NewProject {
        NewProject {
            name: String::new(),
            share_id: 1,
            folder: "docker/mine".into(),
            compose: "services:\n  app:\n    image: nginx\n".into(),
            start: false,
            accepted: None,
            template: None,
        }
    }
}
