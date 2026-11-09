//! In-memory adapters for every port of this context, for tests here and
//! in the API crate (feature `testing`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::MemoryJobs;
use zendiloos_kernel::{Actor, AuditLog, Clock, PortError, RequestContext, Timestamp};

use crate::application::ports::{
    BackupRepository, BackupShares, CheckedCompose, DataPlace, DockerData, DockerEngine, EngineError, Lines,
    NewBackup, ProjectAction, ProjectPlace, ProjectRepository, ProjectStorage, Registries, SharePlace, Steps,
    StorageError, TerminalInput, TerminalSession, UpdateSettingsRepository, VolumeArchiver,
};
use crate::application::{Containers, ContainersPorts};
use crate::domain::{
    AttachedNetwork, Backup, BackupFile, BackupOutcome, BackupPlan, ComposeListing, ComposeModel,
    ComposeService, Container, ContainerAction, ContainerDetails, ContainerProcess, ContainerState,
    ContainerUsage, DiskUsage, Image, Mount, Network, OwedStart, OwnProject, PROJECT_LABEL, Privileges,
    ProjectName, PublishedPort, RegistryImage, ResourceLimits, ServiceMount, UpdateSettings, UsageSample,
    Volume, VolumeSize, WrittenBackup,
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

/// What inspecting `container` tells: one network, a password in its
/// environment, no limits.
pub fn details(container: Container) -> ContainerDetails {
    ContainerDetails {
        command: vec!["nginx".into(), "-g".into(), "daemon off;".into()],
        working_dir: None,
        user: None,
        hostname: Some(container.id.chars().take(12).collect()),
        environment: vec![("TZ".into(), "Europe/Berlin".into()), ("DB_PASSWORD".into(), "secret".into())],
        network_mode: "bridge".into(),
        networks: vec![AttachedNetwork {
            name: "bridge".into(),
            ipv4: Some("172.17.0.2".into()),
            ipv6: None,
            gateway: Some("172.17.0.1".into()),
            mac: None,
            aliases: vec![],
        }],
        privileges: Privileges { capabilities_dropped: vec!["ALL".into()], ..Privileges::default() },
        limits: ResourceLimits::default(),
        restart_retries: 0,
        healthcheck: None,
        health_output: None,
        container,
    }
}

/// `container` mounting the volume `name` at `/data`.
pub fn mounting(mut container: Container, name: &str) -> Container {
    container.mounts.push(Mount {
        kind: "volume".into(),
        source: name.into(),
        destination: "/data".into(),
        read_only: false,
    });
    container
}

/// A local volume, of `project` if any.
pub fn volume(name: &str, project: Option<&str>) -> Volume {
    Volume {
        name: name.to_owned(),
        driver: "local".into(),
        labels: project
            .map(|p| BTreeMap::from([(PROJECT_LABEL.to_owned(), p.to_owned())]))
            .unwrap_or_default(),
        mounts_elsewhere: false,
        created_at: Some(Timestamp::from_unix_seconds(1_790_000_000)),
    }
}

/// A Docker engine with two projects: `kuma`, an app of a catalog, and
/// `stack`, made elsewhere; images and networks to match. Volumes:
/// `kuma_data` (the running `kuma-1` uses it), `stack_db` (the stopped
/// `stack-db`), `orphan` (nothing) and `nfs_media` (mounted from
/// elsewhere).
pub struct FakeEngine {
    pub containers: Mutex<Vec<Container>>,
    pub images: Mutex<Vec<Image>>,
    pub networks: Mutex<Vec<Network>>,
    pub volumes: Mutex<Vec<Volume>>,
    /// Containers, by ID, that will not start.
    pub stubborn: Mutex<Vec<String>>,
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
                mounting(container("kuma-1", Some("kuma"), "kuma", true), "kuma_data"),
                container("stack-web", Some("stack"), "nginx", true),
                mounting(container("stack-db", Some("stack"), "postgres", false), "stack_db"),
                container("lonely", None, "alpine", false),
            ]),
            stubborn: Mutex::new(vec![]),
            volumes: Mutex::new(vec![
                volume("kuma_data", Some("kuma")),
                volume("stack_db", Some("stack")),
                volume("orphan", None),
                Volume { mounts_elsewhere: true, ..volume("nfs_media", None) },
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
        if action == ContainerAction::Start && self.stubborn.lock().unwrap().iter().any(|s| s == id) {
            return Err(EngineError::Failed("it exited at once".into()));
        }
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

    async fn inspect_container(&self, id: &str, _: &RequestContext) -> Result<ContainerDetails, EngineError> {
        let container = self
            .containers
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| EngineError::Failed("Error: No such container".into()))?;
        Ok(details(container))
    }

    async fn container_processes(
        &self,
        id: &str,
        _: &RequestContext,
    ) -> Result<Vec<ContainerProcess>, EngineError> {
        self.call(format!("top {id}"));
        Ok(vec![ContainerProcess {
            pid: 4242,
            parent: 4200,
            user: "root".into(),
            cpu_percent: 0.5,
            memory_percent: 1.2,
            resident_bytes: 51_200 * 1024,
            elapsed: "01:02:03".into(),
            command: "nginx: master process".into(),
        }])
    }

    /// Answers each input with what was typed, after a greeting; a resize
    /// with the new size.
    async fn open_terminal(
        &self,
        id: &str,
        command: &[String],
        columns: u16,
        rows: u16,
        _: &RequestContext,
    ) -> Result<TerminalSession, EngineError> {
        self.call(format!("terminal {id} {} {columns}x{rows}", command.join(" ")));
        let (output, from_engine) = tokio::sync::mpsc::channel(16);
        let (input, mut typed) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _ = output.send(b"$ ".to_vec()).await;
            while let Some(input) = typed.recv().await {
                let answer = match input {
                    TerminalInput::Data(data) if data == b"exit\r" => break,
                    TerminalInput::Data(data) => data,
                    TerminalInput::Resize { columns, rows } => format!("[{columns}x{rows}]").into_bytes(),
                };
                if output.send(answer).await.is_err() {
                    break;
                }
            }
        });
        Ok(TerminalSession { output: from_engine, input })
    }

    fn read_config(&self, config: &str) -> Result<ComposeModel, EngineError> {
        // `bind <source> <target>` per line, under the service `app`.
        let mounts = config
            .lines()
            .filter_map(|line| line.strip_prefix("bind ")?.split_once(' '))
            .map(|(source, target)| ServiceMount { source: source.into(), target: target.into() })
            .collect();
        Ok(ComposeModel {
            services: vec![ComposeService { name: "app".into(), mounts, ..ComposeService::default() }],
        })
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

    async fn volumes(&self, _: &RequestContext) -> Result<Vec<Volume>, EngineError> {
        Ok(self.volumes.lock().unwrap().clone())
    }

    async fn volume_sizes(&self, _: &RequestContext) -> Result<Vec<VolumeSize>, EngineError> {
        Ok(self
            .volumes
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v.backed_up_here())
            .map(|v| VolumeSize { name: v.name.clone(), bytes: 4096, complete: v.name != "orphan" })
            .collect())
    }

    async fn remove_volume(&self, name: &str, _: &RequestContext) -> Result<(), EngineError> {
        self.call(format!("volume rm {name}"));
        self.volumes.lock().unwrap().retain(|v| v.name != name);
        Ok(())
    }

    async fn prune_volumes(&self, _: &RequestContext) -> Result<String, EngineError> {
        self.call("volume prune".into());
        Ok("Total reclaimed space: 4kB\n".into())
    }

    async fn create_volume(
        &self,
        name: &str,
        labels: &BTreeMap<String, String>,
        _: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("volume create {name} {}", labels.keys().cloned().collect::<Vec<_>>().join(",")));
        self.volumes.lock().unwrap().push(Volume { labels: labels.clone(), ..volume(name, None) });
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
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        self.call(format!("deploy {project} pull={pull} {config}"));
        if pull {
            lines.line(" app Pulled");
        }
        lines.line(&format!(" Container {project}-app  Started"));
        self.control_project(project, ProjectAction::Up, context).await
    }

    async fn control_project_reporting(
        &self,
        project: &str,
        action: ProjectAction,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        lines.line(&format!("{} {project}", action.as_str()));
        self.control_project(project, action, context).await
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

/// Share 1 is `docker` on volume 1; no other.
pub struct FakeBackupShares;

#[async_trait]
impl BackupShares for FakeBackupShares {
    async fn locate(&self, share_id: i64) -> Result<Option<SharePlace>, PortError> {
        Ok((share_id == 1).then(|| SharePlace { volume: 1, share: "docker".into() }))
    }
}

/// Backups as the agent would make them, recorded as calls; `fail` makes
/// the next one fail with its reason. Files removed are those of the
/// archives written, by inode. Ending a volume's work says nothing ran,
/// unless `unanswering`.
#[derive(Default)]
pub struct FakeArchiver {
    pub calls: Mutex<Vec<String>>,
    pub fail: Mutex<Option<String>>,
    /// Inodes of the files there are.
    pub files: Mutex<Vec<u64>>,
    /// The agent cannot be reached: ending work fails.
    pub unanswering: std::sync::atomic::AtomicBool,
}

impl FakeArchiver {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl VolumeArchiver for FakeArchiver {
    async fn back_up(
        &self,
        volume: &str,
        place: &SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<WrittenBackup, EngineError> {
        self.calls.lock().unwrap().push(format!("back up {volume} to {}/{folder} as {stem}", place.share));
        steps.step("backing_up", Some(50));
        if let Some(reason) = self.fail.lock().unwrap().take() {
            return Err(EngineError::Failed(reason));
        }
        let mut files = self.files.lock().unwrap();
        let inode = files.len() as u64 * 2 + 1;
        files.extend([inode, inode + 1]);
        let file = |name: String, inode: u64| BackupFile {
            path: format!("{folder}/{volume}/{name}"),
            bytes: 100,
            device: 7,
            inode,
        };
        Ok(WrittenBackup {
            archive: file(format!("{stem}.tar.zst"), inode),
            manifest: file(format!("{stem}.json"), inode + 1),
            sha256: "ab".repeat(32),
            data_bytes: 4096,
            manifest_json: format!(
                r#"{{"volume":"{volume}","driver":"local","labels":{{"com.docker.compose.project":"kuma"}}}}"#
            ),
        })
    }

    async fn restore(
        &self,
        volume: &str,
        place: &SharePlace,
        archive: &str,
        sha256: &str,
        _: &dyn Steps,
        _: &RequestContext,
    ) -> Result<(), EngineError> {
        let short = &sha256[..4.min(sha256.len())];
        self.calls.lock().unwrap().push(format!("restore {volume} from {}/{archive} {short}", place.share));
        match self.fail.lock().unwrap().take() {
            Some(reason) => Err(EngineError::Refused(reason)),
            None => Ok(()),
        }
    }

    async fn remove(
        &self,
        place: &SharePlace,
        file: &BackupFile,
        _: &RequestContext,
    ) -> Result<bool, EngineError> {
        self.calls.lock().unwrap().push(format!("remove {}/{}", place.share, file.path));
        let mut files = self.files.lock().unwrap();
        let before = files.len();
        files.retain(|&inode| inode != file.inode);
        Ok(files.len() < before)
    }

    async fn end_work(&self, volume: &str, _: &RequestContext) -> Result<bool, EngineError> {
        self.calls.lock().unwrap().push(format!("end work on {volume}"));
        if self.unanswering.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(EngineError::Port(PortError::new("agent", "not answering")));
        }
        Ok(false)
    }
}

/// Plans, runs and the starts owed in memory.
#[derive(Default)]
pub struct MemoryBackups {
    pub plans: Mutex<Vec<BackupPlan>>,
    pub runs: Mutex<Vec<Backup>>,
    pub owed: Mutex<Vec<OwedStart>>,
}

/// The labels a manifest of the fake archiver gives.
fn manifest_labels(json: &str) -> BTreeMap<String, String> {
    json.split_once(r#""com.docker.compose.project":""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(project, _)| BTreeMap::from([(PROJECT_LABEL.to_owned(), project.to_owned())]))
        .unwrap_or_default()
}

#[async_trait]
impl BackupRepository for MemoryBackups {
    async fn plans(&self) -> Result<Vec<BackupPlan>, PortError> {
        let mut plans = self.plans.lock().unwrap().clone();
        plans.sort_by(|a, b| a.volume.cmp(&b.volume));
        Ok(plans)
    }

    async fn save_plan(&self, plan: &BackupPlan) -> Result<(), PortError> {
        let mut plans = self.plans.lock().unwrap();
        plans.retain(|p| p.volume != plan.volume);
        plans.push(plan.clone());
        Ok(())
    }

    async fn delete_plan(&self, volume: &str) -> Result<bool, PortError> {
        let mut plans = self.plans.lock().unwrap();
        let before = plans.len();
        plans.retain(|p| p.volume != volume);
        Ok(plans.len() < before)
    }

    async fn ran(&self, volume: &str, at: Timestamp) -> Result<(), PortError> {
        for plan in self.plans.lock().unwrap().iter_mut().filter(|p| p.volume == volume) {
            plan.since = at;
        }
        Ok(())
    }

    async fn start(&self, backup: &NewBackup) -> Result<i64, PortError> {
        let mut runs = self.runs.lock().unwrap();
        let id = runs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        runs.push(Backup {
            id,
            volume: backup.volume.clone(),
            share_id: backup.share_id,
            started_at: backup.started_at,
            finished_at: None,
            outcome: BackupOutcome::Running,
            driver: "local".into(),
            labels: BTreeMap::new(),
            by: backup.by.clone(),
        });
        Ok(id)
    }

    async fn finish(
        &self,
        id: i64,
        at: Timestamp,
        outcome: Result<&WrittenBackup, &str>,
    ) -> Result<(), PortError> {
        for run in self.runs.lock().unwrap().iter_mut().filter(|r| r.id == id) {
            run.finished_at = Some(at);
            run.outcome = match outcome {
                Ok(written) => {
                    run.labels = manifest_labels(&written.manifest_json);
                    BackupOutcome::Succeeded(written.clone())
                }
                Err(reason) => BackupOutcome::Failed(reason.to_owned()),
            };
        }
        Ok(())
    }

    async fn backups(&self, volume: Option<&str>) -> Result<Vec<Backup>, PortError> {
        let mut runs: Vec<Backup> = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|r| volume.is_none_or(|v| r.volume == v))
            .cloned()
            .collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        Ok(runs)
    }

    async fn backup(&self, id: i64) -> Result<Option<Backup>, PortError> {
        Ok(self.runs.lock().unwrap().iter().find(|r| r.id == id).cloned())
    }

    async fn forget(&self, id: i64) -> Result<(), PortError> {
        self.runs.lock().unwrap().retain(|r| r.id != id);
        Ok(())
    }

    async fn interrupted(&self, at: Timestamp) -> Result<Vec<Backup>, PortError> {
        let mut cut = Vec::new();
        for run in self.runs.lock().unwrap().iter_mut().filter(|r| r.outcome == BackupOutcome::Running) {
            cut.push(run.clone());
            run.outcome = BackupOutcome::Failed("interrupted".into());
            run.finished_at = Some(at);
        }
        Ok(cut)
    }

    async fn owed_starts(&self) -> Result<Vec<OwedStart>, PortError> {
        Ok(self.owed.lock().unwrap().clone())
    }

    async fn owe_start(&self, container: &str, volume: &str, at: Timestamp) -> Result<(), PortError> {
        let mut owed = self.owed.lock().unwrap();
        if !owed.iter().any(|o| o.container == container && o.volume == volume) {
            owed.push(OwedStart { container: container.to_owned(), volume: volume.to_owned(), since: at });
        }
        Ok(())
    }

    async fn started(&self, container: &str) -> Result<(), PortError> {
        self.owed.lock().unwrap().retain(|o| o.container != container);
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
    pub archiver: Arc<FakeArchiver>,
    pub backups: Arc<MemoryBackups>,
    pub notifications: Arc<zendiloos_notify::testing::MemoryNotifications>,
}

impl TestContainers {
    pub fn new(audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        let engine = Arc::new(FakeEngine::default());
        let projects = Arc::new(MemoryProjects::default());
        let storage = Arc::new(MemoryStorage::default());
        let jobs = Arc::new(MemoryJobs::default());
        let data = Arc::new(FakeDockerData::default());
        let archiver = Arc::new(FakeArchiver::default());
        let backups = Arc::new(MemoryBackups::default());
        let notifications = Arc::new(zendiloos_notify::testing::MemoryNotifications::default());
        let containers = Containers::new(ContainersPorts {
            engine: engine.clone(),
            projects: projects.clone(),
            storage: storage.clone(),
            registries: Arc::new(FakeRegistries),
            updates: Arc::new(MemoryUpdateSettings::default()),
            data: data.clone(),
            archiver: archiver.clone(),
            backups: backups.clone(),
            backup_shares: Arc::new(FakeBackupShares),
            jobs: Jobs::new(jobs.clone(), clock.clone()),
            notifier: zendiloos_notify::Notifier::new(notifications.clone(), clock.clone()),
            audit,
            clock,
        });
        Self { containers, engine, projects, storage, jobs, data, archiver, backups, notifications }
    }
}
