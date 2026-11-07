//! [`DockerEngine`] through the system agent: it runs `docker` and hands
//! back what it printed; this reads it.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_agent_proto::{self as proto, AgentTransport, ErrorCode, Request, Response};
use zendiloos_containers::application::ports::{CheckedCompose, DockerEngine, EngineError, ProjectAction};
use zendiloos_containers::domain::{
    ComposeListing, ComposeModel, ComposeService, Container, ContainerAction, ContainerState, ContainerUsage,
    DiskUsage, Image, Mount, Network, ProjectName, PublishedPort, UsageSample,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use super::{WHAT, call_raw, unexpected};

pub struct AgentDockerEngine {
    transport: Arc<dyn AgentTransport>,
}

impl AgentDockerEngine {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    async fn ask(&self, request: Request, context: &RequestContext) -> Result<Response, EngineError> {
        match call_raw(self.transport.as_ref(), request, context).await? {
            Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
                Err(EngineError::Refused(message))
            }
            Response::Error { message, .. } => Err(EngineError::Failed(message)),
            response => Ok(response),
        }
    }

    async fn printed(&self, request: Request, context: &RequestContext) -> Result<String, EngineError> {
        match self.ask(request, context).await? {
            Response::DockerOutput { text } => Ok(text),
            other => Err(unexpected(other).into()),
        }
    }

    async fn done(&self, request: Request, context: &RequestContext) -> Result<(), EngineError> {
        match self.ask(request, context).await? {
            Response::Done => Ok(()),
            other => Err(unexpected(other).into()),
        }
    }
}

fn unreadable(what: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::Port(PortError::new(WHAT, format!("Docker's {what} cannot be read: {error}")))
}

fn invalid(error: proto::InvalidDockerName) -> EngineError {
    EngineError::Refused(error.to_string())
}

fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// RFC 3339 as Docker writes it; its zero time is no time.
fn timestamp(text: &str) -> Option<Timestamp> {
    let parsed: jiff::Timestamp = text.parse().ok()?;
    (parsed.as_second() > 0).then(|| Timestamp::from_unix_seconds(parsed.as_second()))
}

/// `2026-10-31 15:59:34 +0000 UTC`, as `docker image ls` writes it.
fn listed_time(text: &str) -> Option<Timestamp> {
    let text = text.strip_suffix(" UTC").unwrap_or(text);
    let parsed = jiff::fmt::strtime::parse("%Y-%m-%d %H:%M:%S %z", text).ok()?.to_timestamp().ok()?;
    Some(Timestamp::from_unix_seconds(parsed.as_second()))
}

/// A size as Docker prints it: `1.74GB`, `145.4MiB`, `0B`.
fn size(text: &str) -> u64 {
    let text = text.trim();
    let split = text.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let Ok(number) = number.trim().parse::<f64>() else { return 0 };
    let factor: f64 = match unit {
        "B" | "" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return 0,
    };
    (number * factor).round() as u64
}

/// The two sizes of `145.4MiB / 1.928GiB`.
fn pair(text: &str) -> (u64, u64) {
    let (first, second) = text.split_once('/').unwrap_or((text, ""));
    (size(first), size(second))
}

fn json_lines(text: &str, what: &str) -> Result<Vec<Value>, EngineError> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| unreadable(what, e)))
        .collect()
}

fn container(value: &Value) -> Container {
    let state = &value["State"];
    let labels = &value["Config"]["Labels"];
    let label = |key: &str| labels.get(key).and_then(Value::as_str).map(str::to_owned);
    let mut ports: Vec<PublishedPort> = Vec::new();
    if let Some(published) = value["NetworkSettings"]["Ports"].as_object() {
        for (inside, bindings) in published {
            let (port, protocol) = inside.split_once('/').unwrap_or((inside, "tcp"));
            let Ok(container_port) = port.parse() else { continue };
            for binding in bindings.as_array().into_iter().flatten() {
                let Ok(host_port) = string(binding, "HostPort").parse() else { continue };
                let ip = string(binding, "HostIp");
                let host_ip = (!ip.is_empty() && ip != "0.0.0.0" && ip != "::").then_some(ip);
                let port =
                    PublishedPort { host_ip, host_port, container_port, protocol: protocol.to_owned() };
                // Docker lists the same port for IPv4 and IPv6.
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    ports.sort_by_key(|p| (p.host_port, p.container_port));
    let networks = value["NetworkSettings"]["Networks"]
        .as_object()
        .map(|networks| networks.keys().cloned().collect())
        .unwrap_or_default();
    let mounts = value["Mounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|mount| Mount {
            kind: string(mount, "Type"),
            source: if string(mount, "Type") == "volume" {
                string(mount, "Name")
            } else {
                string(mount, "Source")
            },
            destination: string(mount, "Destination"),
            read_only: !mount["RW"].as_bool().unwrap_or(true),
        })
        .collect();
    let status = string(state, "Status");
    Container {
        id: string(value, "Id"),
        name: string(value, "Name").trim_start_matches('/').to_owned(),
        image: string(&value["Config"], "Image"),
        image_id: string(value, "Image"),
        state: ContainerState::parse(&status),
        health: state["Health"]["Status"].as_str().map(str::to_owned),
        exit_code: (status == "exited").then(|| state["ExitCode"].as_i64()).flatten(),
        created_at: timestamp(&string(value, "Created")).unwrap_or(Timestamp::from_unix_seconds(0)),
        started_at: timestamp(&string(state, "StartedAt")),
        project: label("com.docker.compose.project"),
        service: label("com.docker.compose.service"),
        ports,
        networks,
        mounts,
        restart_policy: string(&value["HostConfig"]["RestartPolicy"], "Name"),
    }
}

fn containers(text: &str) -> Result<Vec<Container>, EngineError> {
    let values: Vec<Value> = serde_json::from_str(text).map_err(|e| unreadable("containers", e))?;
    Ok(values.iter().map(container).collect())
}

fn usage(text: &str) -> Result<Vec<ContainerUsage>, EngineError> {
    Ok(json_lines(text, "statistics")?
        .iter()
        .map(|line| {
            let (memory, limit) = pair(&string(line, "MemUsage"));
            let (received, sent) = pair(&string(line, "NetIO"));
            ContainerUsage {
                id: string(line, "ID"),
                name: string(line, "Name"),
                cpu_percent: string(line, "CPUPerc").trim_end_matches('%').parse().unwrap_or(0.0),
                memory_bytes: memory,
                memory_limit_bytes: limit,
                received_bytes: received,
                sent_bytes: sent,
            }
        })
        .collect())
}

/// `docker image ls` lists an image once per name; one entry per image.
fn images(text: &str) -> Result<Vec<Image>, EngineError> {
    let mut images: Vec<Image> = Vec::new();
    for line in json_lines(text, "images")? {
        let id = string(&line, "ID");
        let (repository, tag, digest) =
            (string(&line, "Repository"), string(&line, "Tag"), string(&line, "Digest"));
        let index = match images.iter().position(|image| image.id == id) {
            Some(index) => index,
            None => {
                images.push(Image {
                    id: id.clone(),
                    references: vec![],
                    digests: vec![],
                    size_bytes: size(&string(&line, "Size")),
                    created_at: listed_time(&string(&line, "CreatedAt")),
                });
                images.len() - 1
            }
        };
        let image = &mut images[index];
        let named = repository != "<none>" && !repository.is_empty();
        if named && tag != "<none>" && !tag.is_empty() {
            let reference = format!("{repository}:{tag}");
            if !image.references.contains(&reference) {
                image.references.push(reference);
            }
        }
        if named && digest.starts_with("sha256:") {
            let pinned = format!("{repository}@{digest}");
            if !image.digests.contains(&pinned) {
                image.digests.push(pinned);
            }
        }
    }
    Ok(images)
}

fn networks(text: &str) -> Result<Vec<Network>, EngineError> {
    let values: Vec<Value> = serde_json::from_str(text).map_err(|e| unreadable("networks", e))?;
    Ok(values
        .iter()
        .map(|value| {
            let config = value["IPAM"]["Config"].as_array().cloned().unwrap_or_default();
            let mut members: Vec<String> = value["Containers"]
                .as_object()
                .map(|members| members.values().map(|m| string(m, "Name")).collect())
                .unwrap_or_default();
            members.sort();
            Network {
                id: string(value, "Id"),
                name: string(value, "Name"),
                driver: string(value, "Driver"),
                internal: value["Internal"].as_bool().unwrap_or(false),
                subnets: config.iter().map(|c| string(c, "Subnet")).filter(|s| !s.is_empty()).collect(),
                gateways: config.iter().map(|c| string(c, "Gateway")).filter(|s| !s.is_empty()).collect(),
                containers: members,
                project: value["Labels"]["com.docker.compose.project"].as_str().map(str::to_owned),
                created_at: timestamp(&string(value, "Created")),
            }
        })
        .collect())
}

fn disk_usage(text: &str) -> Result<DiskUsage, EngineError> {
    let mut usage = DiskUsage::default();
    for line in json_lines(text, "disk usage")? {
        let bytes = size(&string(&line, "Size"));
        match string(&line, "Type").as_str() {
            "Images" => usage.images_bytes = bytes,
            "Containers" => usage.containers_bytes = bytes,
            "Local Volumes" => usage.volumes_bytes = bytes,
            "Build Cache" => usage.build_cache_bytes = bytes,
            _ => {}
        }
    }
    Ok(usage)
}

pub(crate) fn compose_listings(text: &str) -> Result<Vec<ComposeListing>, EngineError> {
    let values: Vec<Value> = serde_json::from_str(text).map_err(|e| unreadable("Compose projects", e))?;
    Ok(values
        .iter()
        .map(|value| ComposeListing {
            name: string(value, "Name"),
            files: string(value, "ConfigFiles")
                .split(',')
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
                .collect(),
        })
        .collect())
}

/// The host's side of a volume entry or device, in either of Compose's
/// forms.
fn source(entry: &Value) -> Option<String> {
    match entry {
        Value::String(text) => text.split(':').next().map(str::to_owned),
        Value::Object(_) => entry.get("source").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// The files of the secrets or configs a service uses, by their names.
fn used(entries: &Value, files: &BTreeMap<String, String>) -> Vec<String> {
    entries
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("source").and_then(Value::as_str).or_else(|| entry.as_str()))
        .filter_map(|name| files.get(name).cloned())
        .collect()
}

fn texts(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn compose_model(text: &str) -> Result<ComposeModel, EngineError> {
    let value: Value = serde_json::from_str(text).map_err(|e| unreadable("Compose file", e))?;
    let services = value["services"].as_object().cloned().unwrap_or_default();
    // Binds in disguise, by name: a named volume of the local driver with
    // `o: bind` mounts its `device`; a secret or config from a file is bound.
    let named_binds: BTreeMap<String, String> = value["volumes"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, volume)| {
            let options = volume.get("driver_opts")?;
            let binds = options.get("o")?.as_str()?.split(',').any(|o| o.trim() == "bind");
            binds.then(|| Some((name.clone(), options.get("device")?.as_str()?.to_owned())))?
        })
        .collect();
    let files = |kind: &str| -> BTreeMap<String, String> {
        value[kind]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(name, entry)| Some((name.clone(), entry.get("file")?.as_str()?.to_owned())))
            .collect()
    };
    let (secrets, configs) = (files("secrets"), files("configs"));
    Ok(ComposeModel {
        services: services
            .iter()
            .map(|(name, service)| {
                let mode = |key: &str| service.get(key).and_then(Value::as_str).map(str::to_owned);
                ComposeService {
                    name: name.clone(),
                    image: mode("image"),
                    privileged: service["privileged"].as_bool().unwrap_or(false),
                    network_mode: mode("network_mode"),
                    pid: mode("pid"),
                    ipc: mode("ipc"),
                    userns_mode: mode("userns_mode"),
                    binds: service["volumes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|volume| match volume.get("type").and_then(Value::as_str) {
                            Some("bind") => source(volume),
                            Some("volume") => volume
                                .get("source")
                                .and_then(Value::as_str)
                                .and_then(|n| named_binds.get(n))
                                .cloned(),
                            _ => None,
                        })
                        .chain(used(&service["secrets"], &secrets))
                        .chain(used(&service["configs"], &configs))
                        .collect(),
                    devices: service["devices"].as_array().into_iter().flatten().filter_map(source).collect(),
                    cap_add: texts(&service["cap_add"]),
                    security_opt: texts(&service["security_opt"]),
                    keys: service.as_object().map(|s| s.keys().cloned().collect()).unwrap_or_default(),
                }
            })
            .collect(),
    })
}

/// Output as people read it: without the terminal's control sequences
/// Compose writes even with `--no-color`.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters, then one final letter.
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn container_id(id: &str) -> Result<proto::ContainerId, EngineError> {
    proto::ContainerId::parse(id).map_err(invalid)
}

#[async_trait]
impl DockerEngine for AgentDockerEngine {
    async fn containers(&self, context: &RequestContext) -> Result<Vec<Container>, EngineError> {
        containers(&self.printed(Request::DockerContainers {}, context).await?)
    }

    async fn usage(&self, context: &RequestContext) -> Result<UsageSample, EngineError> {
        let text = self.printed(Request::DockerStats {}, context).await?;
        let processors =
            std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1));
        let at = Timestamp::from_unix_seconds(jiff::Timestamp::now().as_second());
        Ok(UsageSample { at, containers: usage(&text)?, processors })
    }

    async fn images(&self, context: &RequestContext) -> Result<Vec<Image>, EngineError> {
        images(&self.printed(Request::DockerImages {}, context).await?)
    }

    async fn networks(&self, context: &RequestContext) -> Result<Vec<Network>, EngineError> {
        networks(&self.printed(Request::DockerNetworks {}, context).await?)
    }

    async fn disk_usage(&self, context: &RequestContext) -> Result<DiskUsage, EngineError> {
        disk_usage(&self.printed(Request::DockerDiskUsage {}, context).await?)
    }

    async fn compose_projects(&self, context: &RequestContext) -> Result<Vec<ComposeListing>, EngineError> {
        match self.ask(Request::AppsStatus {}, context).await? {
            Response::Apps { compose_ls } => compose_listings(&compose_ls),
            other => Err(unexpected(other).into()),
        }
    }

    async fn control_container(
        &self,
        id: &str,
        action: ContainerAction,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let action = match action {
            ContainerAction::Start => proto::ContainerAction::Start,
            ContainerAction::Stop => proto::ContainerAction::Stop,
            ContainerAction::Restart => proto::ContainerAction::Restart,
            ContainerAction::Remove => proto::ContainerAction::Remove,
        };
        self.done(Request::ControlContainer { container: container_id(id)?, action }, context).await
    }

    async fn container_logs(
        &self,
        id: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, EngineError> {
        let text =
            self.printed(Request::ContainerLogs { container: container_id(id)?, lines }, context).await?;
        Ok(plain(&text))
    }

    async fn remove_image(&self, id: &str, context: &RequestContext) -> Result<(), EngineError> {
        let image = proto::ImageId::parse(id).map_err(invalid)?;
        self.done(Request::RemoveImage { image }, context).await
    }

    async fn pull_image(&self, reference: &str, context: &RequestContext) -> Result<(), EngineError> {
        let reference = proto::ImageReference::parse(reference).map_err(invalid)?;
        self.done(Request::PullImage { reference }, context).await
    }

    async fn prune_images(&self, context: &RequestContext) -> Result<String, EngineError> {
        Ok(plain(&self.printed(Request::PruneImages {}, context).await?))
    }

    async fn create_network(
        &self,
        name: &str,
        subnet: Option<&str>,
        gateway: Option<&str>,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let name = proto::NetworkName::parse(name).map_err(invalid)?;
        let subnet = subnet.map(proto::Subnet::parse).transpose().map_err(invalid)?;
        let gateway = gateway
            .map(|g| g.parse().map_err(|_| EngineError::Refused(format!("{g:?} is no IP address"))))
            .transpose()?;
        self.done(Request::CreateNetwork { name, subnet, gateway }, context).await
    }

    async fn remove_network(&self, name: &str, context: &RequestContext) -> Result<(), EngineError> {
        let name = proto::NetworkName::parse(name).map_err(invalid)?;
        self.done(Request::RemoveNetwork { name }, context).await
    }

    async fn check_compose(
        &self,
        project: &ProjectName,
        directory: &str,
        compose: &str,
        context: &RequestContext,
    ) -> Result<CheckedCompose, EngineError> {
        let request = Request::CheckCompose {
            project: proto::ProjectName::parse(project.as_str()).map_err(invalid)?,
            directory: proto::ProjectDirectory::parse(directory).map_err(invalid)?,
            compose: proto::ComposeText::parse(compose).map_err(invalid)?,
        };
        let config = self.printed(request, context).await?;
        Ok(CheckedCompose { model: compose_model(&config)?, config: config.trim_end().to_owned() })
    }

    async fn deploy_project(
        &self,
        project: &str,
        config: &str,
        pull: bool,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let request = Request::DeployProject {
            project: proto::ProjectName::parse(project).map_err(invalid)?,
            config: proto::ProjectConfig::parse(config).map_err(invalid)?,
            pull,
        };
        self.done(request, context).await
    }

    async fn control_project(
        &self,
        project: &str,
        action: ProjectAction,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let action = match action {
            ProjectAction::Start => proto::ProjectAction::Start,
            ProjectAction::Stop => proto::ProjectAction::Stop,
            ProjectAction::Restart => proto::ProjectAction::Restart,
            ProjectAction::Up => proto::ProjectAction::Up,
            ProjectAction::Pull => proto::ProjectAction::Pull,
            ProjectAction::Down => proto::ProjectAction::Down,
        };
        let request =
            Request::ControlProject { project: proto::ProjectName::parse(project).map_err(invalid)?, action };
        self.done(request, context).await
    }

    async fn project_logs(
        &self,
        project: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, EngineError> {
        let project = proto::ProjectName::parse(project).map_err(invalid)?;
        Ok(plain(&self.printed(Request::ProjectLogs { project, lines }, context).await?))
    }

    async fn project_files(
        &self,
        project: &str,
        context: &RequestContext,
    ) -> Result<BTreeMap<String, String>, EngineError> {
        let project = proto::ProjectName::parse(project).map_err(invalid)?;
        let text = self.printed(Request::ProjectFiles { project }, context).await?;
        let files: BTreeMap<String, String> =
            serde_json::from_str(&text).map_err(|e| unreadable("files", e))?;
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;
    use crate::agent::scripted::Scripted;

    fn simulated() -> (AgentDockerEngine, Arc<SimulatedAgent>) {
        let agent = Arc::new(SimulatedAgent::new());
        (AgentDockerEngine::new(agent.clone()), agent)
    }

    #[tokio::test]
    async fn docker_is_read_through_the_agent() {
        let (engine, _) = simulated();
        let context = RequestContext::default();
        let containers = engine.containers(&context).await.unwrap();
        assert!(containers.iter().any(|c| c.name == "stack-web-1"), "{containers:?}");
        let sample = engine.usage(&context).await.unwrap();
        assert!(sample.processors >= 1);
        assert!(!engine.images(&context).await.unwrap().is_empty());
        assert!(engine.networks(&context).await.unwrap().iter().any(|n| n.name == "stack_default"));
        assert!(engine.disk_usage(&context).await.unwrap().images_bytes > 0);
        let _ = engine.compose_projects(&context).await.unwrap();
        let logs = engine.container_logs("lonely", 10, &context).await.unwrap();
        assert!(logs.contains("lonely"));
        let project_logs = engine.project_logs("stack", 10, &context).await.unwrap();
        assert!(project_logs.contains("ready"));
        let files = engine.project_files("stack", &context).await.unwrap();
        assert!(files.contains_key("/opt/stack/compose.yml"));
        assert!(matches!(engine.project_files("tesla", &context).await, Err(EngineError::Failed(_))));
    }

    #[tokio::test]
    async fn containers_images_networks_and_projects_are_changed_through_the_agent() {
        let (engine, agent) = simulated();
        let context = RequestContext::default();
        for action in
            [ContainerAction::Start, ContainerAction::Restart, ContainerAction::Stop, ContainerAction::Remove]
        {
            engine.control_container("lonely", action, &context).await.unwrap();
        }
        let gone = engine.control_container("lonely", ContainerAction::Start, &context).await;
        assert!(matches!(gone, Err(EngineError::Failed(_))), "{gone:?}");
        engine.pull_image("redis:7", &context).await.unwrap();
        let images = engine.images(&context).await.unwrap();
        let redis = images.iter().find(|i| i.references.iter().any(|t| t == "redis:7")).expect("pulled");
        engine.remove_image(&redis.id, &context).await.unwrap();
        assert!(engine.prune_images(&context).await.unwrap().contains("reclaimed"));
        engine.create_network("proxy", Some("172.30.0.0/24"), Some("172.30.0.1"), &context).await.unwrap();
        let again = engine.create_network("proxy", None, None, &context).await;
        assert!(matches!(again, Err(EngineError::Failed(_))), "{again:?}");
        engine.remove_network("proxy", &context).await.unwrap();
        let own = engine.remove_network("bridge", &context).await;
        assert!(matches!(own, Err(EngineError::Refused(_))), "{own:?}");

        let name = ProjectName::parse("tesla").unwrap();
        let checked = engine
            .check_compose(
                &name,
                "/volume1/docker/tesla",
                "services:\n  app:\n    privileged: true\n",
                &context,
            )
            .await
            .unwrap();
        assert!(checked.model.services[0].privileged);
        let broken = engine.check_compose(&name, "/volume1/docker/tesla", "broken: [", &context).await;
        assert!(matches!(broken, Err(EngineError::Refused(_))), "{broken:?}");
        engine.deploy_project("tesla", &checked.config, true, &context).await.unwrap();
        for action in [
            ProjectAction::Stop,
            ProjectAction::Start,
            ProjectAction::Restart,
            ProjectAction::Pull,
            ProjectAction::Up,
            ProjectAction::Down,
        ] {
            engine.control_project("tesla", action, &context).await.unwrap();
        }
        assert!(
            agent
                .received()
                .iter()
                .any(|r| matches!(r, Request::ControlProject { action: proto::ProjectAction::Down, .. }))
        );
    }

    #[tokio::test]
    async fn names_docker_would_misread_are_refused_before_they_are_sent() {
        let agent = Arc::new(Scripted::default());
        let engine = AgentDockerEngine::new(agent.clone());
        let context = RequestContext::default();
        let refused =
            |result: Result<(), EngineError>| assert!(matches!(result, Err(EngineError::Refused(_))));
        refused(engine.control_container("--rm", ContainerAction::Stop, &context).await);
        assert!(engine.container_logs("a b", 1, &context).await.is_err());
        refused(engine.remove_image("not an id", &context).await);
        refused(engine.pull_image("-x", &context).await);
        refused(engine.create_network("bad name", None, None, &context).await);
        refused(engine.create_network("proxy", Some("no subnet"), None, &context).await);
        refused(engine.create_network("proxy", None, Some("no address"), &context).await);
        refused(engine.remove_network("bad name", &context).await);
        refused(engine.deploy_project("Bad", "{}", false, &context).await);
        refused(engine.control_project("Bad", ProjectAction::Up, &context).await);
        assert!(engine.project_logs("Bad", 1, &context).await.is_err());
        assert!(engine.project_files("Bad", &context).await.is_err());
        let name = ProjectName::parse("tesla").unwrap();
        assert!(engine.check_compose(&name, "relative/folder", "services: {}", &context).await.is_err());
        assert!(agent.asked().is_empty(), "nothing odd was sent");
    }

    #[tokio::test]
    async fn the_agents_answers_are_told_apart() {
        let context = RequestContext::default();
        let answering =
            |response: Response| AgentDockerEngine::new(Arc::new(Scripted::answering([response])));
        let conflict = answering(Response::Error { code: ErrorCode::Conflict, message: "in use".into() });
        assert!(matches!(conflict.prune_images(&context).await, Err(EngineError::Refused(_))));
        let failed = answering(Response::Error { code: ErrorCode::Failed, message: "docker".into() });
        assert!(matches!(failed.prune_images(&context).await, Err(EngineError::Failed(_))));
        let odd = answering(Response::Done);
        assert!(matches!(odd.prune_images(&context).await, Err(EngineError::Port(_))), "not printed");
        let odd = answering(Response::DockerOutput { text: String::new() });
        assert!(matches!(odd.pull_image("redis:7", &context).await, Err(EngineError::Port(_))), "not done");
        let odd = answering(Response::Done);
        assert!(matches!(odd.compose_projects(&context).await, Err(EngineError::Port(_))));
        let odd = answering(Response::DockerOutput { text: "not json".into() });
        assert!(odd.project_files("stack", &context).await.is_err());
        let odd = answering(Response::DockerOutput { text: "not json".into() });
        assert!(odd.containers(&context).await.is_err());
        let odd = answering(Response::DockerOutput { text: "{not json".into() });
        assert!(odd.images(&context).await.is_err());
        let odd = answering(Response::DockerOutput { text: "{not json".into() });
        assert!(odd.networks(&context).await.is_err());
        let odd = answering(Response::DockerOutput { text: "{not json".into() });
        assert!(odd.disk_usage(&context).await.is_err());
        let odd = answering(Response::DockerOutput { text: "{not json".into() });
        assert!(odd.usage(&context).await.is_err());
    }

    const INSPECT: &str = r#"[{
        "Id": "d853eed323e74b49b6a84ea78784086ec720c3be10be36393589bfc80ad6ada1",
        "Created": "2026-11-06T07:33:22.5Z",
        "Name": "/uptime-kuma-uptime-kuma-1",
        "Image": "sha256:0772aa7e48ba0ed3cec5277b1df78865b7875fd275251d0797f0e793f0819912",
        "State": {"Status": "running", "ExitCode": 0, "StartedAt": "2026-11-06T14:30:00.1Z",
                  "Health": {"Status": "healthy"}},
        "Config": {"Image": "louislam/uptime-kuma:2.5.9",
                   "Labels": {"com.docker.compose.project": "uptime-kuma", "com.docker.compose.service": "uptime-kuma"}},
        "HostConfig": {"RestartPolicy": {"Name": "unless-stopped"}},
        "NetworkSettings": {
            "Ports": {"3001/tcp": [{"HostIp": "0.0.0.0", "HostPort": "3001"}, {"HostIp": "::", "HostPort": "3001"}],
                      "53/udp": [{"HostIp": "127.0.0.1", "HostPort": "5353"}], "9000/tcp": null},
            "Networks": {"uptime-kuma_default": {}}},
        "Mounts": [{"Type": "bind", "Source": "/volume1/@apps/uptime-kuma/data", "Destination": "/app/data", "RW": true},
                   {"Type": "volume", "Name": "cache", "Source": "/var/lib/docker/volumes/cache/_data",
                    "Destination": "/cache", "RW": false}]
    }, {
        "Id": "aa", "Created": "2026-11-06T07:33:22Z", "Name": "/lonely", "Image": "sha256:bb",
        "State": {"Status": "exited", "ExitCode": 137, "StartedAt": "0001-01-01T00:00:00Z"},
        "Config": {"Image": "alpine", "Labels": null}, "HostConfig": {"RestartPolicy": {"Name": "no"}},
        "NetworkSettings": {"Ports": {}, "Networks": {"bridge": {}}}, "Mounts": []
    }]"#;

    #[test]
    fn containers_read_as_docker_inspects_them() {
        let read = containers(INSPECT).unwrap();

        let kuma = &read[0];
        assert_eq!(
            (kuma.name.as_str(), kuma.image.as_str()),
            ("uptime-kuma-uptime-kuma-1", "louislam/uptime-kuma:2.5.9")
        );
        assert_eq!((kuma.state, kuma.health.as_deref()), (ContainerState::Running, Some("healthy")));
        assert_eq!(kuma.project.as_deref(), Some("uptime-kuma"));
        assert_eq!(kuma.started_at, Some(Timestamp::from_unix_seconds(1_793_975_400)));
        assert_eq!(
            kuma.ports,
            [
                PublishedPort {
                    host_ip: None,
                    host_port: 3001,
                    container_port: 3001,
                    protocol: "tcp".into()
                },
                PublishedPort {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: 5353,
                    container_port: 53,
                    protocol: "udp".into()
                },
            ],
            "IPv4 and IPv6 once; unpublished left out"
        );
        assert_eq!(
            kuma.mounts[1],
            Mount {
                kind: "volume".into(),
                source: "cache".into(),
                destination: "/cache".into(),
                read_only: true
            }
        );
        let lonely = &read[1];
        assert_eq!((lonely.exit_code, lonely.started_at, lonely.project.clone()), (Some(137), None, None));
    }

    #[test]
    fn statistics_sizes_and_listings_read_as_printed() {
        let stats = r#"{"CPUPerc":"12.50%","ID":"d853","MemUsage":"145.4MiB / 1.928GiB","Name":"kuma","NetIO":"17.5kB / 0B"}"#;
        let read = usage(stats).unwrap();
        assert_eq!(read[0].cpu_percent, 12.5);
        assert_eq!((read[0].memory_bytes, read[0].memory_limit_bytes), (152_462_950, 2_070_174_237));
        assert_eq!((read[0].received_bytes, read[0].sent_bytes), (17_500, 0));
        assert_eq!(size("1.74GB"), 1_740_000_000);
        assert_eq!(size("nonsense"), 0);

        let listed = concat!(
            r#"{"CreatedAt":"2026-10-31 15:59:34 +0000 UTC","Digest":"sha256:e884","ID":"sha256:0772","Repository":"louislam/uptime-kuma","Size":"1.74GB","Tag":"<none>"}"#,
            "\n",
            r#"{"CreatedAt":"2026-04-16 23:53:26 +0000 UTC","Digest":"sha256:d9e8","ID":"sha256:bf85","Repository":"alpine","Size":"7.81MB","Tag":"3.20"}"#,
            "\n",
            r#"{"CreatedAt":"2026-04-16 23:53:26 +0000 UTC","Digest":"sha256:d9e8","ID":"sha256:bf85","Repository":"alpine","Size":"7.81MB","Tag":"latest"}"#,
        );
        let read = images(listed).unwrap();
        assert_eq!(read.len(), 2, "one entry per image");
        assert_eq!(read[0].references, Vec::<String>::new());
        assert_eq!(read[0].digests, ["louislam/uptime-kuma@sha256:e884"]);
        assert_eq!(read[1].references, ["alpine:3.20", "alpine:latest"]);
        assert_eq!(read[1].created_at, Some(Timestamp::from_unix_seconds(1_776_383_606)));

        let df = "{\"Size\":\"1.753GB\",\"Type\":\"Images\"}\n{\"Size\":\"2kB\",\"Type\":\"Containers\"}\n";
        assert_eq!(disk_usage(df).unwrap().total_bytes(), 1_753_002_000);
    }

    #[test]
    fn networks_carry_their_subnets_and_members() {
        let inspected = r#"[{"Id":"1c6c","Name":"proxy_net","Created":"2026-10-01T10:00:00Z","Driver":"bridge",
            "Internal":false,"IPAM":{"Config":[{"Subnet":"172.30.0.0/24","Gateway":"172.30.0.1"}]},
            "Containers":{"x":{"Name":"vaultwarden"},"y":{"Name":"jellyfin"}},"Labels":{}},
            {"Id":"0","Name":"host","Driver":"host","IPAM":{"Config":[]},"Containers":{},"Labels":{}}]"#;
        let read = networks(inspected).unwrap();
        assert_eq!(
            (read[0].subnets.clone(), read[0].gateways.clone()),
            (vec!["172.30.0.0/24".to_owned()], vec!["172.30.0.1".to_owned()])
        );
        assert_eq!(read[0].containers, ["jellyfin", "vaultwarden"]);
        assert!(read[1].is_built_in() && read[1].subnets.is_empty());
    }

    #[test]
    fn binds_in_disguise_count_as_binds() {
        let config = r#"{"services":{"web":{"image":"nginx",
            "volumes":[{"type":"volume","source":"root","target":"/host"},{"type":"volume","source":"plain","target":"/d"}],
            "secrets":[{"source":"shadow","target":"/run/secrets/x"}],"configs":[{"source":"site"}]}},
            "volumes":{"root":{"driver":"local","driver_opts":{"type":"none","o":"bind","device":"/"}},"plain":{}},
            "secrets":{"shadow":{"file":"/etc/shadow"}},"configs":{"site":{"file":"/volume1/docker/web/site.conf"}}}"#;
        let model = compose_model(config).unwrap();
        assert_eq!(model.services[0].binds, ["/", "/etc/shadow", "/volume1/docker/web/site.conf"]);
    }

    #[test]
    fn compose_files_read_as_compose_normalizes_them() {
        let config = r#"{"name":"ha","services":{"homeassistant":{
            "image":"ghcr.io/home-assistant/home-assistant:stable","privileged":true,"network_mode":"host",
            "volumes":[{"type":"bind","source":"/volume1/docker/ha/config","target":"/config"},
                       {"type":"bind","source":"/run/dbus","target":"/run/dbus","read_only":true},
                       {"type":"volume","source":"cache","target":"/cache"}],
            "devices":[{"source":"/dev/ttyUSB0","target":"/dev/ttyUSB0","permissions":"rwm"}, "/dev/ttyACM0:/dev/ttyACM0"],
            "cap_add":["NET_ADMIN"],"security_opt":["apparmor:unconfined"]}}}"#;
        let model = compose_model(config).unwrap();
        let service = &model.services[0];
        assert!(service.privileged);
        assert_eq!(service.network_mode.as_deref(), Some("host"));
        assert_eq!(
            service.binds,
            ["/volume1/docker/ha/config", "/run/dbus"],
            "volumes of Docker's own are no binds"
        );
        assert_eq!(service.devices, ["/dev/ttyUSB0", "/dev/ttyACM0"]);
        assert_eq!(
            (service.cap_add.clone(), service.security_opt.clone()),
            (vec!["NET_ADMIN".into()], vec!["apparmor:unconfined".into()])
        );

        assert!(service.keys.contains(&"devices".to_owned()) && service.keys.contains(&"image".to_owned()));

        let listed = r#"[{"Name":"stack","Status":"running(2)","ConfigFiles":"/opt/a.yml,/opt/b.yml"}]"#;
        assert_eq!(compose_listings(listed).unwrap()[0].files, ["/opt/a.yml", "/opt/b.yml"]);
        assert_eq!(plain("\u{1b}[2Ks-1  | out\n"), "s-1  | out\n");
    }
}
