//! In-memory adapters for every port of this context, for tests here and
//! in the API crate (feature `testing`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::MemoryJobs;
use zendiloos_kernel::{Actor, AuditLog, Clock, PortError, RequestContext, Timestamp};

use crate::application::ports::{
    CheckedCompose, DataPlace, DockerData, DockerEngine, EngineError, ProjectAction, ProjectPlace,
    ProjectRepository, ProjectStorage, Registries, StorageError, UpdateSettingsRepository,
};
use crate::application::{Containers, ContainersPorts};
use crate::domain::{
    ComposeListing, ComposeModel, ComposeService, Container, ContainerAction, ContainerState, ContainerUsage,
    DiskUsage, Image, Network, OwnProject, ProjectName, PublishedPort, RegistryImage, UpdateSettings,
    UsageSample,
};

/// A container of `project` (if any) from `image`.
pub fn container(name: &str, project: Option<&str>, image: &str, running: bool) -> Container {
    Container {
        id: format!("{name}-id"),
        name: name.to_owned(),
        image: image.to_owned(),
        image_id: format!("sha256:{image}"),
        state: if running { ContainerState::Running } else { ContainerState::Exited },
        health: None,
        exit_code: (!running).then_some(0),
        created_at: Timestamp::from_unix_seconds(1_800_000_000),
        started_at: running.then(|| Timestamp::from_unix_seconds(1_800_000_100)),
        project: project.map(str::to_owned),
        service: project.map(|_| name.to_owned()),
        ports: vec![PublishedPort {
            host_ip: None,
            host_port: 8080,
            container_port: 80,
            protocol: "tcp".into(),
        }],
        networks: vec!["bridge".into()],
        mounts: vec![],
        restart_policy: "unless-stopped".into(),
    }
}

/// A Docker engine with two projects: `kuma`, an app of a catalog, and
/// `stack`, made elsewhere; images and networks to match.
pub struct FakeEngine {
    pub containers: Mutex<Vec<Container>>,
    pub images: Mutex<Vec<Image>>,
    pub networks: Mutex<Vec<Network>>,
    pub listings: Mutex<Vec<ComposeListing>>,
    /// What Compose reads from a file containing `privileged: true`; plain
    /// otherwise; a file containing `broken` it cannot read.
    pub calls: Mutex<Vec<String>>,
}

impl Default for FakeEngine {
    fn default() -> Self {
        let network = |name: &str, project: Option<&str>| Network {
            id: format!("{name}-id"),
            name: name.to_owned(),
            driver: "bridge".into(),
            internal: false,
            subnets: vec!["172.17.0.0/16".into()],
            gateways: vec!["172.17.0.1".into()],
            containers: vec![],
            project: project.map(str::to_owned),
            created_at: None,
        };
        Self {
            containers: Mutex::new(vec![
                container("kuma-1", Some("kuma"), "kuma", true),
                container("stack-web", Some("stack"), "nginx", true),
                container("stack-db", Some("stack"), "postgres", false),
                container("lonely", None, "alpine", false),
            ]),
            images: Mutex::new(
                ["kuma", "nginx", "postgres", "alpine", "unused"]
                    .iter()
                    .enumerate()
                    .map(|(n, name)| Image {
                        id: format!("sha256:{name}"),
                        references: vec![format!("{name}:latest")],
                        digests: vec![format!("{name}@sha256:old")],
                        size_bytes: 1_000_000,
                        created_at: Some(Timestamp::from_unix_seconds(1_700_000_000 + n as i64)),
                    })
                    .collect(),
            ),
            networks: Mutex::new(vec![network("bridge", None), network("stack_default", Some("stack"))]),
            listings: Mutex::new(vec![
                ComposeListing { name: "kuma".into(), files: vec!["/etc/zendiloos/apps/kuma.json".into()] },
                ComposeListing { name: "stack".into(), files: vec!["/opt/stack/compose.yml".into()] },
            ]),
            calls: Mutex::new(vec![]),
        }
    }
}

impl FakeEngine {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn call(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl DockerEngine for FakeEngine {
    async fn containers(&self, _: &RequestContext) -> Result<Vec<Container>, EngineError> {
        Ok(self.containers.lock().unwrap().clone())
    }

    async fn usage(&self, _: &RequestContext) -> Result<UsageSample, EngineError> {
        let containers = self.containers.lock().unwrap();
        Ok(UsageSample {
            at: Timestamp::from_unix_seconds(1_800_000_000),
            containers: containers
                .iter()
                .filter(|c| c.state.is_up())
                .map(|c| ContainerUsage {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    cpu_percent: 10.0,
                    memory_bytes: 100 * 1024 * 1024,
                    memory_limit_bytes: 8 * 1024 * 1024 * 1024,
                    received_bytes: 1000,
                    sent_bytes: 500,
                })
                .collect(),
            processors: 4,
        })
    }

    async fn images(&self, _: &RequestContext) -> Result<Vec<Image>, EngineError> {
        Ok(self.images.lock().unwrap().clone())
    }

    async fn networks(&self, _: &RequestContext) -> Result<Vec<Network>, EngineError> {
        Ok(self.networks.lock().unwrap().clone())
    }

    async fn disk_usage(&self, _: &RequestContext) -> Result<DiskUsage, EngineError> {
        Ok(DiskUsage {
            images_bytes: 5_000_000,
            containers_bytes: 1000,
            volumes_bytes: 0,
            build_cache_bytes: 0,
        })
    }

    async fn compose_projects(&self, _: &RequestContext) -> Result<Vec<ComposeListing>, EngineError> {
        Ok(self.listings.lock().unwrap().clone())
    }

    async fn control_container(
        &self,
        id: &str,
        action: ContainerAction,
        _: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("{} {id}", action.as_str()));
        let mut containers = self.containers.lock().unwrap();
        let container =
            containers.iter_mut().find(|c| c.id == id).ok_or_else(|| EngineError::Failed("gone".into()))?;
        match action {
            ContainerAction::Start | ContainerAction::Restart => container.state = ContainerState::Running,
            ContainerAction::Stop => container.state = ContainerState::Exited,
            ContainerAction::Remove if container.state.is_up() => {
                return Err(EngineError::Failed("cannot remove a running container".into()));
            }
            ContainerAction::Remove => containers.retain(|c| c.id != id),
        }
        Ok(())
    }

    async fn container_logs(&self, id: &str, lines: u32, _: &RequestContext) -> Result<String, EngineError> {
        Ok(format!("2026-11-06T10:00:00Z {id} said {lines} lines\n"))
    }

    async fn remove_image(&self, id: &str, _: &RequestContext) -> Result<(), EngineError> {
        if self.containers.lock().unwrap().iter().any(|c| c.image_id == id) {
            return Err(EngineError::Failed(format!("image {id} is being used")));
        }
        self.images.lock().unwrap().retain(|i| i.id != id);
        Ok(())
    }

    async fn pull_image(&self, reference: &str, _: &RequestContext) -> Result<(), EngineError> {
        self.call(format!("pull {reference}"));
        Ok(())
    }

    async fn prune_images(&self, _: &RequestContext) -> Result<String, EngineError> {
        self.call("prune".into());
        Ok("Total reclaimed space: 1MB".into())
    }

    async fn create_network(
        &self,
        name: &str,
        subnet: Option<&str>,
        gateway: Option<&str>,
        _: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("network create {name} {subnet:?} {gateway:?}"));
        self.networks.lock().unwrap().push(Network {
            id: format!("{name}-id"),
            name: name.to_owned(),
            driver: "bridge".into(),
            internal: false,
            subnets: subnet.into_iter().map(str::to_owned).collect(),
            gateways: gateway.into_iter().map(str::to_owned).collect(),
            containers: vec![],
            project: None,
            created_at: None,
        });
        Ok(())
    }

    async fn remove_network(&self, name: &str, _: &RequestContext) -> Result<(), EngineError> {
        self.call(format!("network rm {name}"));
        self.networks.lock().unwrap().retain(|n| n.name != name);
        Ok(())
    }

    async fn check_compose(
        &self,
        project: &ProjectName,
        directory: &str,
        compose: &str,
        _: &RequestContext,
    ) -> Result<CheckedCompose, EngineError> {
        self.call(format!("check {project} {directory}"));
        if compose.contains("broken") {
            return Err(EngineError::Refused("yaml: line 2: mapping values are not allowed".into()));
        }
        let privileged = compose.contains("privileged: true");
        let image = compose.lines().find_map(|l| l.trim().strip_prefix("image: ")).unwrap_or("nginx");
        // Relative binds (`- ./config:/config`) start in the project's folder.
        let binds = compose
            .lines()
            .filter_map(|l| l.trim().strip_prefix("- ./")?.split_once(':'))
            .map(|(source, _)| format!("{directory}/{source}"))
            .collect();
        Ok(CheckedCompose {
            model: ComposeModel {
                services: vec![ComposeService {
                    name: "app".into(),
                    image: Some(image.into()),
                    privileged,
                    binds,
                    ..ComposeService::default()
                }],
            },
            config: format!(
                r#"{{"name":"{project}","services":{{"app":{{"image":"{image}","privileged":{privileged}}}}}}}"#
            ),
        })
    }

    async fn deploy_project(
        &self,
        project: &str,
        config: &str,
        pull: bool,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("deploy {project} pull={pull} {config}"));
        self.control_project(project, ProjectAction::Up, context).await
    }

    async fn control_project(
        &self,
        project: &str,
        action: ProjectAction,
        _: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("project {} {project}", action.as_str()));
        let mut containers = self.containers.lock().unwrap();
        match action {
            ProjectAction::Up => {
                if !containers.iter().any(|c| c.project.as_deref() == Some(project)) {
                    containers.push(container(&format!("{project}-app"), Some(project), "nginx", true));
                }
                for c in containers.iter_mut().filter(|c| c.project.as_deref() == Some(project)) {
                    c.state = ContainerState::Running;
                }
            }
            ProjectAction::Start | ProjectAction::Restart => {
                for c in containers.iter_mut().filter(|c| c.project.as_deref() == Some(project)) {
                    c.state = ContainerState::Running;
                }
            }
            ProjectAction::Stop => {
                for c in containers.iter_mut().filter(|c| c.project.as_deref() == Some(project)) {
                    c.state = ContainerState::Exited;
                }
            }
            ProjectAction::Down => containers.retain(|c| c.project.as_deref() != Some(project)),
            ProjectAction::Pull => {}
        }
        Ok(())
    }

    async fn project_logs(&self, project: &str, _: u32, _: &RequestContext) -> Result<String, EngineError> {
        Ok(format!("{project}-web  | 2026-11-06T10:00:00Z ready\n"))
    }

    async fn project_files(
        &self,
        project: &str,
        _: &RequestContext,
    ) -> Result<BTreeMap<String, String>, EngineError> {
        let listings = self.listings.lock().unwrap();
        let listing =
            listings.iter().find(|l| l.name == project).ok_or_else(|| EngineError::Failed("none".into()))?;
        Ok(listing
            .files
            .iter()
            .map(|f| (f.clone(), "services:\n  web:\n    image: nginx\n".to_owned()))
            .collect())
    }
}

#[derive(Default)]
pub struct MemoryProjects(pub Mutex<Vec<OwnProject>>);

#[async_trait]
impl ProjectRepository for MemoryProjects {
    async fn list(&self) -> Result<Vec<OwnProject>, PortError> {
        let mut projects = self.0.lock().unwrap().clone();
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(projects)
    }

    async fn create(&self, project: &OwnProject) -> Result<bool, PortError> {
        let mut projects = self.0.lock().unwrap();
        if projects.iter().any(|p| p.name == project.name) {
            return Ok(false);
        }
        projects.push(project.clone());
        Ok(true)
    }

    async fn confirm(&self, name: &ProjectName, compose: &str, config: &str) -> Result<bool, PortError> {
        let mut projects = self.0.lock().unwrap();
        let Some(project) = projects.iter_mut().find(|p| &p.name == name) else { return Ok(false) };
        project.compose = Some(compose.to_owned());
        project.config = Some(config.to_owned());
        Ok(true)
    }

    async fn delete(&self, name: &ProjectName) -> Result<bool, PortError> {
        let mut projects = self.0.lock().unwrap();
        let before = projects.len();
        projects.retain(|p| &p.name != name);
        Ok(projects.len() < before)
    }
}

/// One share, `docker` (id 1) on volume 1, writable by `alice` only; files
/// by share and path.
#[derive(Default)]
pub struct MemoryStorage {
    pub files: Mutex<BTreeMap<(i64, String), String>>,
    /// The folders made, by share and path, with who made them.
    pub folders: Mutex<Vec<(i64, String, String)>>,
}

impl MemoryStorage {
    fn allowed(actor: &Actor, share_id: i64) -> Result<(), StorageError> {
        if actor.name == "alice" && share_id == 1 { Ok(()) } else { Err(StorageError::Denied) }
    }
}

#[async_trait]
impl ProjectStorage for MemoryStorage {
    async fn places(&self, actor: &Actor) -> Result<Vec<ProjectPlace>, StorageError> {
        Ok(if actor.name == "alice" {
            vec![ProjectPlace { share_id: 1, share: "docker".into() }]
        } else {
            vec![]
        })
    }

    async fn directory(&self, actor: &Actor, share_id: i64, folder: &str) -> Result<String, StorageError> {
        Self::allowed(actor, share_id)?;
        Ok(format!("/volume1/docker/{folder}"))
    }

    async fn write(
        &self,
        actor: &Actor,
        share_id: i64,
        folder: &str,
        name: &str,
        content: &str,
        _: &RequestContext,
    ) -> Result<(), StorageError> {
        Self::allowed(actor, share_id)?;
        self.files.lock().unwrap().insert((share_id, format!("{folder}/{name}")), content.to_owned());
        Ok(())
    }

    async fn read(
        &self,
        actor: &Actor,
        share_id: i64,
        path: &str,
        _: &RequestContext,
    ) -> Result<String, StorageError> {
        Self::allowed(actor, share_id)?;
        self.files.lock().unwrap().get(&(share_id, path.to_owned())).cloned().ok_or(StorageError::NotFound)
    }

    async fn make_folder(
        &self,
        actor: &Actor,
        share_id: i64,
        path: &str,
        _: &RequestContext,
    ) -> Result<(), StorageError> {
        Self::allowed(actor, share_id)?;
        self.folders.lock().unwrap().push((share_id, path.to_owned(), actor.name.clone()));
        Ok(())
    }
}

/// Registries that publish something newer for `nginx` only, and know
/// nothing of `alpine`.
#[derive(Default)]
pub struct FakeRegistries;

#[async_trait]
impl Registries for FakeRegistries {
    async fn digest(&self, image: &RegistryImage) -> Result<String, String> {
        match image.repository.as_str() {
            "library/nginx" => Ok("sha256:new".into()),
            "library/alpine" => Err("the registry does not answer".into()),
            _ => Ok("sha256:old".into()),
        }
    }
}

#[derive(Default)]
pub struct MemoryUpdateSettings(Mutex<Option<UpdateSettings>>);

#[async_trait]
impl UpdateSettingsRepository for MemoryUpdateSettings {
    async fn settings(&self) -> Result<UpdateSettings, PortError> {
        Ok(self.0.lock().unwrap().unwrap_or_default())
    }

    async fn save(&self, settings: &UpdateSettings) -> Result<(), PortError> {
        *self.0.lock().unwrap() = Some(*settings);
        Ok(())
    }
}

/// Docker's data on `tank` (volume 1); `backup` (volume 2) has room for
/// it, `tiny` (volume 3) has not.
pub struct FakeDockerData {
    pub current: Mutex<Option<DataPlace>>,
}

impl Default for FakeDockerData {
    fn default() -> Self {
        Self {
            current: Mutex::new(Some(DataPlace {
                pool: "tank".into(),
                volume: 1,
                available_bytes: Some(1 << 40),
            })),
        }
    }
}

#[async_trait]
impl DockerData for FakeDockerData {
    async fn places(&self, _: &RequestContext) -> Result<(Option<DataPlace>, Vec<DataPlace>), PortError> {
        let places = vec![
            DataPlace { pool: "tank".into(), volume: 1, available_bytes: Some(1 << 40) },
            DataPlace { pool: "backup".into(), volume: 2, available_bytes: Some(1 << 40) },
            DataPlace { pool: "tiny".into(), volume: 3, available_bytes: Some(1000) },
        ];
        Ok((self.current.lock().unwrap().clone(), places))
    }

    async fn move_to(&self, _: &DataPlace, to: &DataPlace, _: &RequestContext) -> Result<(), EngineError> {
        *self.current.lock().unwrap() = Some(to.clone());
        Ok(())
    }
}

/// The use cases on in-memory adapters, with handles to look into them.
pub struct TestContainers {
    pub containers: Containers,
    pub engine: Arc<FakeEngine>,
    pub projects: Arc<MemoryProjects>,
    pub storage: Arc<MemoryStorage>,
    pub jobs: Arc<MemoryJobs>,
    pub data: Arc<FakeDockerData>,
}

impl TestContainers {
    pub fn new(audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        let engine = Arc::new(FakeEngine::default());
        let projects = Arc::new(MemoryProjects::default());
        let storage = Arc::new(MemoryStorage::default());
        let jobs = Arc::new(MemoryJobs::default());
        let data = Arc::new(FakeDockerData::default());
        let containers = Containers::new(ContainersPorts {
            engine: engine.clone(),
            projects: projects.clone(),
            storage: storage.clone(),
            registries: Arc::new(FakeRegistries),
            updates: Arc::new(MemoryUpdateSettings::default()),
            data: data.clone(),
            jobs: Jobs::new(jobs.clone(), clock.clone()),
            audit,
            clock,
        });
        Self { containers, engine, projects, storage, jobs, data }
    }
}
