//! The App Center as a quick way into Docker: an app of a catalog is the
//! template of an administrator's Compose project. Its file is filled in
//! here with what the administrator gave; the project is then made like
//! any other, through the Containers context, in a folder of a share the
//! administrator picks, and is theirs from then on: changed, updated and
//! removed in the Docker app (docs/architecture.md, 4.9).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use zendiloos_kernel::{Actor, RequestContext};

use super::ports::{
    AppProjects, ComposeFiller, InstalledRepository, ListeningPorts, Secrets, TemplateHost, TemplateProject,
    UsableShare, UsableShares,
};
use super::{AppsError, Catalogs};
use crate::domain::{
    Access, CatalogApp, InvalidValue, ValueProblem, ValueRequest, WebAddress, install_values,
    share_and_folder, template_variables, web_address,
};

/// What filling in an app's template takes from the administrator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FillRequest {
    pub source: i64,
    pub app: String,
    /// Share id → the share picked, or a folder in it (`media/Films`).
    pub shares: BTreeMap<String, String>,
    /// Port id → host port; missing ones get their default.
    pub ports: BTreeMap<String, u16>,
    /// Setting id → value; missing ones get their default, generated ones
    /// are made.
    pub settings: BTreeMap<String, String>,
}

/// A project made from an app of a catalog, with what its catalog says of
/// the app now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppProjectView {
    pub project: TemplateProject,
    /// `None` once the app or its catalog is gone.
    pub app: Option<CatalogApp>,
    /// Where its web interface answers, while it is published.
    pub web: Option<WebAddress>,
}

#[derive(Clone)]
pub struct TemplatePorts {
    pub catalogs: Catalogs,
    /// Apps installed the earlier way: their ports are taken.
    pub installed: Arc<dyn InstalledRepository>,
    pub shares: Arc<dyn UsableShares>,
    pub listening: Arc<dyn ListeningPorts>,
    pub secrets: Arc<dyn Secrets>,
    pub host: Arc<dyn TemplateHost>,
    pub filler: Arc<dyn ComposeFiller>,
    pub projects: Arc<dyn AppProjects>,
}

#[derive(Clone)]
pub struct Templates {
    ports: TemplatePorts,
}

fn invalid(field: String, problem: ValueProblem) -> AppsError {
    AppsError::InvalidValue(InvalidValue { field, problem })
}

/// A line of a comment: one line, without control characters.
fn comment(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

impl Templates {
    pub fn new(ports: TemplatePorts) -> Self {
        Self { ports }
    }

    /// The Compose file of `request.app` filled in for `actor`: their
    /// account runs the containers, the shares picked are ones they may
    /// use as the app needs, the ports are free, the devices the machine
    /// lacks are left out. Nothing is made yet.
    pub async fn fill(
        &self,
        request: FillRequest,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<String, AppsError> {
        let listed = self.ports.catalogs.app(request.source, &request.app).await?;
        let app = listed.app;
        let usable = self.ports.shares.usable(actor).await?;
        let by_name: BTreeMap<&str, &UsableShare> =
            usable.iter().map(|share| (share.name.as_str(), share)).collect();
        let existing: BTreeSet<String> = usable.iter().map(|share| share.name.clone()).collect();
        let other_apps: BTreeMap<u16, String> = self
            .ports
            .installed
            .list()
            .await?
            .into_iter()
            .flat_map(|other| {
                let id = other.id.clone();
                other.values.ports.into_values().map(move |port| (port, id.clone()))
            })
            .collect();
        let busy = match self.ports.listening.listening().await {
            Ok(busy) => busy,
            Err(error) => {
                tracing::warn!(%error, "apps: cannot tell which ports are in use");
                BTreeSet::new()
            }
        };
        let given = ValueRequest {
            shares: &request.shares,
            ports: &request.ports,
            settings: &request.settings,
            before: None,
            existing_shares: &existing,
            other_apps: &other_apps,
            busy: &busy,
        };
        let values = install_values(&app, &given, |setting| {
            self.ports.secrets.generate(setting.length, setting.alphabet)
        })?;

        let mut paths = BTreeMap::new();
        for share in &app.shares {
            let Some(picked) = values.shares.get(&share.id) else { continue };
            let (name, folder) = share_and_folder(picked);
            let Some(usable) = by_name.get(name) else {
                return Err(invalid(format!("shares.{}", share.id), ValueProblem::NoSuchShare));
            };
            if share.access == Access::ReadWrite && !usable.writable {
                return Err(invalid(format!("shares.{}", share.id), ValueProblem::NotWritable));
            }
            let root = format!("/volume{}/{name}", usable.volume);
            paths.insert(share.id.clone(), if folder.is_empty() { root } else { format!("{root}/{folder}") });
        }

        let facts = self.ports.host.facts(&actor.name, &app.permissions.devices, context).await?;
        let absent: Vec<String> = app
            .permissions
            .devices
            .iter()
            .filter(|device| !facts.devices.contains(device))
            .cloned()
            .collect();
        let variables = template_variables(&app, &values, &paths, &facts);
        let filled =
            self.ports.filler.fill(&app.compose, &variables, &absent).map_err(AppsError::Template)?;
        let origin = if listed.source.official {
            "the community catalog".to_owned()
        } else {
            comment(&listed.source.index)
        };
        Ok(format!(
            "# {name}, filled in by the zendiloOS App Center from {origin}.\n\
             # The file is yours now: change it here as in any project.\n{filled}",
            name = comment(&app.name),
        ))
    }

    /// The projects made from apps of a catalog, by name.
    pub async fn projects(&self, context: &RequestContext) -> Result<Vec<AppProjectView>, AppsError> {
        let projects = self.ports.projects.projects(context).await?;
        if projects.is_empty() {
            return Ok(Vec::new());
        }
        let listed = match self.ports.catalogs.apps().await {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(%error, "apps: cannot read the catalogs");
                Vec::new()
            }
        };
        let mut views: Vec<AppProjectView> = projects
            .into_iter()
            .map(|project| {
                let app = listed
                    .iter()
                    .find(|l| l.source.id == project.source && l.app.id == project.app)
                    .map(|l| l.app.clone());
                let web = app.as_ref().and_then(|app| web_address(app, &project.published));
                AppProjectView { project, app, web }
            })
            .collect();
        views.sort_by(|a, b| a.project.name.cmp(&b.project.name));
        Ok(views)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::{AuditEntry, AuditLog, Clock, PortError, Timestamp};

    use super::*;
    use crate::application::ports::AppHostError;
    use crate::domain::{
        Containers, HostFacts, OFFICIAL_INDEX, Permissions, SECRET_LENGTH, SecretAlphabet, Setting,
        SettingKind, Share, Web, WebScheme,
    };
    use crate::testing::{CountingSecrets, FakeListening, FakeReader, MemoryInstalled, MemorySources, app};

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_793_836_800)
        }
    }

    struct NoAudit;

    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    /// `media` (alice reads) and `docker` (alice writes) on volume 1.
    struct Shares;

    #[async_trait]
    impl UsableShares for Shares {
        async fn usable(&self, actor: &Actor) -> Result<Vec<UsableShare>, PortError> {
            Ok(if actor.name == "alice" {
                vec![
                    UsableShare { name: "media".into(), volume: 1, writable: false },
                    UsableShare { name: "docker".into(), volume: 1, writable: true },
                ]
            } else {
                vec![]
            })
        }
    }

    /// Alice is 2001:1500 in Berlin; the machine has a graphics card.
    #[derive(Default)]
    struct Host(Mutex<Vec<String>>);

    #[async_trait]
    impl TemplateHost for Host {
        async fn facts(
            &self,
            user: &str,
            devices: &[String],
            _: &RequestContext,
        ) -> Result<HostFacts, AppHostError> {
            self.0.lock().unwrap().push(format!("{user} {}", devices.join(",")));
            Ok(HostFacts {
                uid: 2001,
                gid: 1500,
                time_zone: "Europe/Berlin".into(),
                devices: devices.iter().filter(|d| *d == "/dev/dri").cloned().collect(),
            })
        }
    }

    /// Writes the variables and what is left out instead of filling in.
    struct Filler;

    impl ComposeFiller for Filler {
        fn fill(
            &self,
            compose: &str,
            variables: &BTreeMap<String, String>,
            absent: &[String],
        ) -> Result<String, String> {
            if compose.contains("unfillable") {
                return Err("line 3: no".into());
            }
            let pairs: Vec<String> = variables.iter().map(|(n, v)| format!("{n}={v}")).collect();
            Ok(format!("{}\nabsent: {}\n", pairs.join("\n"), absent.join(",")))
        }
    }

    #[derive(Default)]
    struct Projects(Mutex<Vec<TemplateProject>>);

    #[async_trait]
    impl AppProjects for Projects {
        async fn projects(&self, _: &RequestContext) -> Result<Vec<TemplateProject>, PortError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    fn jellyfin() -> CatalogApp {
        let mut jellyfin = app("jellyfin", "Jellyfin");
        jellyfin.compose = "services:\n  jellyfin:\n    ports:\n      - ${PORT_WEB}:8096\n".into();
        jellyfin.web = Some(Web { port: "web".into(), path: "/".into(), scheme: WebScheme::Http });
        jellyfin.shares = vec![
            Share { id: "media".into(), label: "Media".into(), description: None, access: Access::ReadOnly },
            Share { id: "cache".into(), label: "Cache".into(), description: None, access: Access::ReadWrite },
        ];
        jellyfin.settings = vec![Setting {
            id: "API_KEY".into(),
            label: "API key".into(),
            description: None,
            kind: SettingKind::Password,
            default: None,
            required: true,
            generate: true,
            length: SECRET_LENGTH,
            alphabet: SecretAlphabet::Alphanumeric,
        }];
        jellyfin.permissions =
            Permissions { devices: vec!["/dev/dri".into(), "/dev/ttyUSB0".into()], ..Permissions::default() };
        jellyfin
    }

    async fn setup(apps: Vec<CatalogApp>) -> (Templates, Arc<Host>, Arc<Projects>) {
        let reader = Arc::new(FakeReader::default());
        reader.serve(OFFICIAL_INDEX, apps);
        let catalogs =
            Catalogs::new(Arc::new(MemorySources::default()), reader, Arc::new(NoAudit), Arc::new(Fixed));
        catalogs.refresh().await.unwrap();
        let host = Arc::new(Host::default());
        let projects = Arc::new(Projects::default());
        let templates = Templates::new(TemplatePorts {
            catalogs,
            installed: Arc::new(MemoryInstalled::default()),
            shares: Arc::new(Shares),
            listening: Arc::new(FakeListening),
            secrets: Arc::new(CountingSecrets::default()),
            host: host.clone(),
            filler: Arc::new(Filler),
            projects: projects.clone(),
        });
        (templates, host, projects)
    }

    fn request(shares: &[(&str, &str)]) -> FillRequest {
        FillRequest {
            source: 1,
            app: "jellyfin".into(),
            shares: shares.iter().map(|(id, picked)| ((*id).to_owned(), (*picked).to_owned())).collect(),
            ..FillRequest::default()
        }
    }

    #[tokio::test]
    async fn a_template_is_filled_in_for_the_administrator() {
        let (templates, host, _) = setup(vec![jellyfin()]).await;
        let context = RequestContext::default();

        let filled = templates
            .fill(request(&[("media", "media/Films"), ("cache", "docker")]), &alice(), &context)
            .await
            .unwrap();

        assert_eq!(
            filled,
            "# Jellyfin, filled in by the zendiloOS App Center from the community catalog.\n\
             # The file is yours now: change it here as in any project.\n\
             API_KEY=secret-1\nAPP_DATA=.\nPGID=1500\nPORT_WEB=8096\nPUID=2001\n\
             SHARE_CACHE=/volume1/docker\nSHARE_MEDIA=/volume1/media/Films\nTZ=Europe/Berlin\n\
             absent: /dev/ttyUSB0\n"
        );
        assert_eq!(*host.0.lock().unwrap(), ["alice /dev/dri,/dev/ttyUSB0"]);
    }

    #[tokio::test]
    async fn the_administrator_must_be_able_to_use_the_shares_as_the_app_does() {
        let (templates, _, _) = setup(vec![jellyfin()]).await;
        let context = RequestContext::default();
        let refused = |result: Result<String, AppsError>| match result {
            Err(AppsError::InvalidValue(invalid)) => invalid.to_string(),
            other => panic!("{other:?}"),
        };

        let read_only = request(&[("media", "media"), ("cache", "media")]);
        let read_only = templates.fill(read_only, &alice(), &context).await;
        assert_eq!(refused(read_only), "shares.cache: the share can only be read");
        let bob = Actor { account_id: 2, name: "bob".into() };
        let foreign =
            templates.fill(request(&[("media", "media"), ("cache", "docker")]), &bob, &context).await;
        assert_eq!(refused(foreign), "shares.media: there is no such share");
        let busy = FillRequest {
            ports: BTreeMap::from([("web".to_owned(), 9000)]),
            ..request(&[("media", "media"), ("cache", "docker")])
        };
        assert_eq!(
            refused(templates.fill(busy, &alice(), &context).await),
            "ports.web: port 9000 is in use on the NAS"
        );
        let unknown = FillRequest { app: "plex".into(), ..request(&[]) };
        assert!(matches!(templates.fill(unknown, &alice(), &context).await, Err(AppsError::AppNotFound)));
    }

    #[tokio::test]
    async fn a_template_that_cannot_be_filled_in_says_why() {
        let mut broken = app("jellyfin", "Jellyfin");
        broken.compose = "services:\n  unfillable: {}\n".into();
        let (templates, _, _) = setup(vec![broken]).await;

        let failed = templates.fill(request(&[]), &alice(), &RequestContext::default()).await;

        assert!(
            matches!(failed, Err(AppsError::Template(ref reason)) if reason == "line 3: no"),
            "{failed:?}"
        );
        assert_eq!(comment("Other\nCatalog"), "Other Catalog");
    }

    #[tokio::test]
    async fn projects_made_from_templates_say_where_they_answer() {
        let (templates, _, projects) = setup(vec![jellyfin()]).await;
        let context = RequestContext::default();
        assert!(templates.projects(&context).await.unwrap().is_empty());
        let project = |name: &str, app: &str, published: Vec<(u16, u16)>| TemplateProject {
            name: name.into(),
            source: 1,
            app: app.into(),
            containers: Containers { running: 1, ..Containers::default() },
            published,
            created_at: Timestamp::from_unix_seconds(1_793_000_000),
        };
        projects.0.lock().unwrap().extend([
            project("media", "jellyfin", vec![(8096, 8099)]),
            project("gone", "plex", vec![(32400, 32400)]),
        ]);

        let views = templates.projects(&context).await.unwrap();

        let names: Vec<_> = views.iter().map(|v| v.project.name.as_str()).collect();
        assert_eq!(names, ["gone", "media"]);
        assert_eq!((views[0].app.as_ref(), views[0].web.as_ref()), (None, None), "no longer in its catalog");
        assert_eq!(views[1].app.as_ref().map(|a| a.name.as_str()), Some("Jellyfin"));
        assert_eq!(views[1].web.as_ref().map(|w| w.port), Some(8099));
    }
}
