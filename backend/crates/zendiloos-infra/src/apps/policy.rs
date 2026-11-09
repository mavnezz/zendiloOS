//! The catalog's policy on an app's template, as Compose reads it
//! (`zendiloos-catalog`, pinned): what the index's checks cannot see in
//! the file as written (named volumes, binds out of the app's folder,
//! `privileged`, the Docker socket without its permission, …) shows only
//! once Compose has made one meaning of it. So the template is filled in
//! with the catalog's stand-in values, as the catalog's own validator does,
//! read by Compose where the project goes, and checked with the catalog's
//! `check_normalized` and `check_variables`: what breaks the policy is not
//! offered, whatever the catalog says of the app's permissions.
//!
//! The administrator's own values then go where the stand-ins were, each
//! quoted so that it reads back as that value (`YamlFiller`), so the file
//! they get differs from the checked one in those values only; what it
//! lets containers do is confirmed as any project's.

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_apps::application::ports::{ComposeFiller, PolicyError, TemplatePlace, TemplatePolicy};
use zendiloos_apps::domain::CatalogApp;
use zendiloos_catalog as catalog;
use zendiloos_containers::application::{Containers, ContainersError};
use zendiloos_kernel::{Actor, PortError, RequestContext};

use super::YamlFiller;

/// [`TemplatePolicy`] with Compose through the Containers context.
#[derive(Clone)]
pub struct CatalogPolicy {
    containers: Containers,
}

impl CatalogPolicy {
    pub fn new(containers: Containers) -> Self {
        Self { containers }
    }
}

fn policy_error(error: ContainersError) -> PolicyError {
    match error {
        ContainersError::InvalidName(_)
        | ContainersError::Invalid(_)
        | ContainersError::Denied
        | ContainersError::NotFound => PolicyError::Place(error.to_string()),
        ContainersError::Refused(reason) => PolicyError::Unreadable(reason),
        other => PolicyError::Port(PortError::new("Docker Compose", other)),
    }
}

#[async_trait]
impl TemplatePolicy for CatalogPolicy {
    async fn problems(
        &self,
        app: &CatalogApp,
        place: &TemplatePlace,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<String>, PolicyError> {
        let unreadable = |what: &str, error: String| PolicyError::Unreadable(format!("{what}: {error}"));
        let manifest: catalog::Manifest =
            serde_json::from_str(&app.manifest).map_err(|e| unreadable("the manifest", e.to_string()))?;
        // Compose names the networks after the project, which the policy
        // checks: the stand-ins' project is the one it reads.
        let mut stand_in = catalog::Placement::stand_in(&manifest);
        stand_in.project = place.project.trim().to_owned();
        let template = YamlFiller
            .fill(&app.compose, &stand_in.variables(), &[])
            .map_err(|e| unreadable("the template", e))?;
        let printed = self
            .containers
            .normalize_compose(&stand_in.project, (place.share_id, &place.folder), &template, actor, context)
            .await
            .map_err(policy_error)?;
        let normalized: Value =
            serde_json::from_str(&printed).map_err(|e| unreadable("what Compose printed", e.to_string()))?;
        let mut problems = catalog::compose::check_normalized(&normalized, &manifest, &stand_in);
        problems.extend(catalog::compose::check_variables(&normalized, &manifest));
        Ok(problems.iter().map(ToString::to_string).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zendiloos_containers::testing::TestContainers;
    use zendiloos_jobs::testing::FixedClock;
    use zendiloos_kernel::{AuditEntry, AuditLog};

    use super::*;
    use crate::apps::read_index;

    struct NoAudit;

    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    /// Uptime Kuma as an index offers it, with `compose` as its file.
    fn kuma(compose: &str) -> CatalogApp {
        let entry = serde_json::json!({
            "id": "uptime-kuma", "name": "Uptime Kuma",
            "tagline": "Monitor websites and services and get notified when they go down",
            "description": "Checks websites.\n", "categories": ["monitoring"],
            "developer": "Louis Lam and contributors", "website": "https://uptime.kuma.pet",
            "source": "https://github.com/louislam/uptime-kuma", "license": "MIT", "maintainers": ["mavnezz"],
            "architectures": ["amd64", "arm64"],
            "web": {"port": "web", "path": "/", "scheme": "http"},
            "ports": [{"id": "web", "label": "Web interface", "default": 3001}],
            "compose": compose, "icon": "icons/uptime-kuma.png",
            "digest": "sha256:2cbf42f705a743b98a7c563720a8b5f8349bf6539b3f1985aa55ed73f5251fbf"
        });
        let index = serde_json::json!({ "format": 1, "apps": [entry] }).to_string();
        let mut read = read_index(&index).unwrap();
        assert_eq!(read.skipped, [], "the file as written passes the index's checks");
        read.apps.remove(0)
    }

    const PLAIN: &str = "services:\n  uptime-kuma:\n    image: louislam/uptime-kuma:2\n    restart: unless-stopped\n    \
                         environment:\n      TZ: ${TZ}\n    ports:\n      - ${PORT_WEB}:3001\n    volumes:\n      \
                         - ${APP_DATA}/data:/app/data\n";

    /// The app's data folder as Compose prints a bind of it, stand-ins in.
    const DATA: &str = r#"{"type":"bind","source":"/zendiloos-stand-in/app-data/data","target":"/app/data",
        "bind":{"create_host_path":true}}"#;

    /// What Compose prints for [`PLAIN`] filled in with the stand-ins, in
    /// the project `kuma`, with the service's `mounts` and `more` of its
    /// settings, and `top` at the top.
    fn printed(mounts: &[&str], more: &str, top: &str) -> String {
        format!(
            r#"{{"name":"kuma","services":{{"uptime-kuma":{{"image":"louislam/uptime-kuma:2","restart":"unless-stopped",
            "environment":{{"TZ":"zendiloos-stand-in-tz"}},"networks":{{"default":null}},
            "ports":[{{"mode":"ingress","target":3001,"published":"61001","protocol":"tcp"}}],
            "volumes":[{}]{more}}}}},"networks":{{"default":{{"name":"kuma_default","ipam":{{}}}}}}{top}}}"#,
            mounts.join(",")
        )
    }

    fn setup() -> (TestContainers, CatalogPolicy) {
        let test = TestContainers::new(Arc::new(NoAudit), Arc::new(FixedClock));
        let policy = CatalogPolicy::new(test.containers.clone());
        (test, policy)
    }

    fn place() -> TemplatePlace {
        TemplatePlace { project: " kuma ".into(), share_id: 1, folder: "docker/kuma".into() }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn a_template_keeping_to_the_policy_passes_as_compose_reads_it() {
        let (test, policy) = setup();
        *test.engine.normalized.lock().unwrap() = Some(printed(&[DATA], "", ""));

        let problems = policy.problems(&kuma(PLAIN), &place(), &alice(), &RequestContext::default()).await;

        assert_eq!(problems.unwrap(), Vec::<String>::new());
        assert!(test.engine.calls().contains(&"check kuma /volume1/docker/docker/kuma".to_owned()));
        let read = test.engine.read.lock().unwrap().clone();
        assert_eq!(read.len(), 1);
        for filled in ["/zendiloos-stand-in/app-data/data:/app/data", "61001:3001", "zendiloos-stand-in-tz"] {
            assert!(read[0].contains(filled), "Compose reads the stand-ins, {filled}: {}", read[0]);
        }
        assert!(!read[0].contains("${"), "every variable filled in");
    }

    /// The file as written passes the index's checks; what Compose makes
    /// of it would give the container the machine.
    #[tokio::test]
    async fn what_compose_makes_of_a_template_against_the_policy_is_named() {
        let (test, policy) = setup();
        let compose = format!(
            "{PLAIN}      - ${{APP_DATA}}/../../../@docker:/var/lib/d\n      - /var/run/docker.sock:/var/run/docker.sock\n      \
             - host:/host\n    privileged: true\nvolumes:\n  host:\n    driver_opts:\n      type: none\n      o: rbind\n      \
             device: /\n"
        );
        let mounts = [
            DATA,
            r#"{"type":"bind","source":"/zendiloos-stand-in/app-data/../../../@docker","target":"/var/lib/d",
                "bind":{"create_host_path":true}}"#,
            r#"{"type":"bind","source":"/var/run/docker.sock","target":"/var/run/docker.sock",
                "bind":{"create_host_path":true}}"#,
            r#"{"type":"volume","source":"host","target":"/host","volume":{}}"#,
        ];
        let top = r#","volumes":{"host":{"name":"kuma_host","driver_opts":{"type":"none","o":"rbind","device":"/"}}}"#;
        *test.engine.normalized.lock().unwrap() = Some(printed(&mounts, r#","privileged":true"#, top));

        let problems =
            policy.problems(&kuma(&compose), &place(), &alice(), &RequestContext::default()).await.unwrap();

        let all = problems.join("\n");
        for named in [
            "services.uptime-kuma.privileged",
            "services.uptime-kuma.volumes[1].source: must be an absolute path without `..`",
            "services.uptime-kuma.volumes[2]: the Docker socket needs `permissions.docker_socket: true`",
            "services.uptime-kuma.volumes[3]: Docker volumes keep data outside the NAS's volumes",
            "volumes.host: Docker volumes",
        ] {
            assert!(all.contains(named), "{named} in:\n{all}");
        }
    }

    /// So development fills apps in as a NAS does: the community catalog's
    /// apps keep to the policy as the simulation's stand-in for Compose
    /// reads them, as they do with Docker's.
    #[tokio::test]
    async fn the_community_catalogs_apps_pass_as_the_simulation_reads_them() {
        use zendiloos_containers::application::ContainersPorts;
        use zendiloos_containers::testing::{FakeBackupShares, FakeRegistries, MemoryUpdateSettings};

        let test = TestContainers::new(Arc::new(NoAudit), Arc::new(FixedClock));
        let agent = Arc::new(zendiloos_sim::SimulatedAgent::new());
        let containers = Containers::new(ContainersPorts {
            engine: Arc::new(crate::agent::AgentDockerEngine::new(agent)),
            projects: test.projects.clone(),
            storage: test.storage.clone(),
            registries: Arc::new(FakeRegistries),
            updates: Arc::new(MemoryUpdateSettings::default()),
            data: test.data.clone(),
            archiver: test.archiver.clone(),
            backups: test.backups.clone(),
            backup_shares: Arc::new(FakeBackupShares),
            jobs: zendiloos_jobs::Jobs::new(test.jobs.clone(), Arc::new(FixedClock)),
            notifier: zendiloos_notify::Notifier::new(test.notifications.clone(), Arc::new(FixedClock)),
            audit: Arc::new(NoAudit),
            clock: Arc::new(FixedClock),
        });
        let policy = CatalogPolicy::new(containers);
        let read = read_index(include_str!("fixtures/community-index.json")).unwrap();

        let mut broken = Vec::new();
        for app in &read.apps {
            let place =
                TemplatePlace { project: app.id.clone(), share_id: 1, folder: format!("docker/{}", app.id) };
            let problems = policy.problems(app, &place, &alice(), &RequestContext::default()).await.unwrap();
            broken.extend(problems.into_iter().map(|problem| format!("{}: {problem}", app.id)));
        }

        assert_eq!(broken, Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_place_the_administrator_may_not_use_is_no_place_to_check_in() {
        let (_, policy) = setup();
        let context = RequestContext::default();
        let bob = Actor { account_id: 2, name: "bob".into() };

        let denied = policy.problems(&kuma(PLAIN), &place(), &bob, &context).await;
        assert!(matches!(denied, Err(PolicyError::Place(_))), "{denied:?}");
        let odd = TemplatePlace { project: "Kuma".into(), ..place() };
        let refused = policy.problems(&kuma(PLAIN), &odd, &alice(), &context).await;
        assert!(matches!(refused, Err(PolicyError::Place(_))), "{refused:?}");
        let broken = kuma(
            &PLAIN.replace("restart: unless-stopped", "restart: unless-stopped\n    labels: {broken: x}"),
        );
        let unreadable = policy.problems(&broken, &place(), &alice(), &context).await;
        assert!(matches!(unreadable, Err(PolicyError::Unreadable(_))), "{unreadable:?}");
    }
}
