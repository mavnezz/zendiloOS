//! A Docker engine for the simulation: the catalog apps the simulation
//! installed, a project made elsewhere (`stack`), a lone container, the
//! projects administrators deploy, and their images and networks. It
//! answers in the formats `docker` prints.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use zendiloos_agent_proto::{ContainerAction, ErrorCode, ProjectAction, Request, Response};

#[derive(Debug, Clone)]
struct SimContainer {
    id: String,
    name: String,
    image: String,
    project: Option<String>,
    service: Option<String>,
    running: bool,
    host_port: Option<u16>,
    container_port: u16,
    created: i64,
}

#[derive(Debug)]
pub(crate) struct SimDocker {
    containers: Vec<SimContainer>,
    images: Vec<(String, String)>,
    networks: Vec<(String, Option<String>, Option<String>)>,
    /// Projects deployed from an administrator's file, with that file.
    own: BTreeMap<String, String>,
    next: u64,
}

fn hex(seed: u64) -> String {
    format!("{:016x}", seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)).repeat(4)
}

fn refused(message: &str) -> Response {
    Response::Error { code: ErrorCode::Forbidden, message: format!("refused: {message}") }
}

fn failed(message: &str) -> Response {
    Response::Error { code: ErrorCode::Failed, message: message.to_owned() }
}

impl Default for SimDocker {
    fn default() -> Self {
        let container =
            |seed: u64, name: &str, image: &str, project: Option<&str>, running: bool, port: Option<u16>| {
                SimContainer {
                    id: hex(seed),
                    name: name.to_owned(),
                    image: image.to_owned(),
                    project: project.map(str::to_owned),
                    service: project.map(|_| name.rsplit('-').nth(1).unwrap_or(name).to_owned()),
                    running,
                    host_port: port,
                    container_port: port.map_or(80, |_| 80),
                    created: 1_793_000_000 + i64::try_from(seed).unwrap_or(0) * 3600,
                }
            };
        Self {
            containers: vec![
                container(1, "stack-web-1", "nginx:1.27", Some("stack"), true, Some(8080)),
                container(2, "stack-db-1", "postgres:17", Some("stack"), true, None),
                container(3, "lonely", "alpine:3.20", None, false, None),
            ],
            images: ["nginx:1.27", "postgres:17", "alpine:3.20", "redis:7-alpine"]
                .iter()
                .enumerate()
                .map(|(n, name)| (format!("sha256:{}", hex(100 + n as u64)), (*name).to_owned()))
                .collect(),
            networks: vec![
                ("bridge".into(), Some("172.17.0.0/16".into()), None),
                ("host".into(), None, None),
                ("none".into(), None, None),
                ("stack_default".into(), Some("172.19.0.0/16".into()), Some("stack".into())),
            ],
            own: BTreeMap::new(),
            next: 1000,
        }
    }
}

impl SimDocker {
    fn image_id(&mut self, image: &str) -> String {
        if let Some((id, _)) = self.images.iter().find(|(_, name)| name == image) {
            return id.clone();
        }
        self.next += 1;
        let id = format!("sha256:{}", hex(self.next));
        self.images.push((id.clone(), image.to_owned()));
        id
    }

    /// The simulation's containers and one for each catalog app.
    fn all(&mut self, apps: &BTreeMap<String, bool>) -> Vec<SimContainer> {
        let mut all = self.containers.clone();
        for (n, (app, running)) in apps.iter().enumerate() {
            all.push(SimContainer {
                id: hex(500 + n as u64),
                name: format!("{app}-app-1"),
                image: format!("catalog/{app}:latest"),
                project: Some(app.clone()),
                service: Some("app".into()),
                running: *running,
                host_port: None,
                container_port: 80,
                created: 1_793_500_000,
            });
        }
        for container in &all {
            self.image_id(&container.image.clone());
        }
        all
    }

    fn inspect(&mut self, apps: &BTreeMap<String, bool>) -> Value {
        let all = self.all(apps);
        let entries: Vec<Value> = all
            .iter()
            .map(|c| {
                let mut labels = json!({});
                if let Some(project) = &c.project {
                    labels = json!({"com.docker.compose.project": project, "com.docker.compose.service": c.service});
                }
                let ports = match c.host_port {
                    Some(port) => json!({format!("{}/tcp", c.container_port): [
                        {"HostIp": "0.0.0.0", "HostPort": port.to_string()},
                        {"HostIp": "::", "HostPort": port.to_string()}]}),
                    None => json!({}),
                };
                let network = c.project.as_ref().map_or_else(|| "bridge".to_owned(), |p| format!("{p}_default"));
                json!({
                    "Id": c.id,
                    "Created": jiff::Timestamp::from_second(c.created).map(|t| t.to_string()).unwrap_or_default(),
                    "Name": format!("/{}", c.name),
                    "Image": self.image_id(&c.image),
                    "State": {
                        "Status": if c.running { "running" } else { "exited" },
                        "ExitCode": if c.running { 0 } else { 137 },
                        "StartedAt": if c.running { "2026-11-06T08:00:00Z" } else { "0001-01-01T00:00:00Z" },
                    },
                    "Config": {"Image": c.image, "Labels": labels},
                    "HostConfig": {"RestartPolicy": {"Name": "unless-stopped"}},
                    "NetworkSettings": {"Ports": ports, "Networks": {network: {}}},
                    "Mounts": [],
                })
            })
            .collect();
        Value::from(entries)
    }

    fn stats(&mut self, apps: &BTreeMap<String, bool>, now: u64) -> String {
        let wave = |seed: usize, scale: f64| {
            let phase = (now as f64 / 60.0 + seed as f64 * 1.7).sin().abs();
            (phase * scale * 100.0).round() / 100.0
        };
        self.all(apps)
            .iter()
            .enumerate()
            .filter(|(_, c)| c.running)
            .map(|(n, c)| {
                let memory = 60.0 + wave(n, 180.0);
                let received = (now % 100_000) as f64 * (n as f64 + 1.0) * 3.0 / 1000.0;
                json!({
                    "ID": c.id,
                    "Name": c.name,
                    "CPUPerc": format!("{:.2}%", wave(n, 4.0)),
                    "MemUsage": format!("{memory:.1}MiB / 7.66GiB"),
                    "NetIO": format!("{received:.1}kB / {:.1}kB", received / 2.0),
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn compose_ls(&self, apps: &BTreeMap<String, bool>) -> Value {
        let mut projects: Vec<Value> = apps
            .iter()
            .map(|(app, running)| {
                json!({"Name": app, "Status": if *running { "running(1)" } else { "exited(1)" },
                       "ConfigFiles": format!("/etc/zendiloos/apps/{app}.json")})
            })
            .collect();
        projects
            .push(json!({"Name": "stack", "Status": "running(2)", "ConfigFiles": "/opt/stack/compose.yml"}));
        for (project, file) in &self.own {
            projects.push(json!({"Name": project, "Status": "running(1)", "ConfigFiles": file}));
        }
        Value::from(projects)
    }

    /// The simulation's answer to a request about Docker, if it is one.
    pub(crate) fn handle(
        &mut self,
        request: &Request,
        apps: &mut BTreeMap<String, bool>,
        now: u64,
    ) -> Option<Response> {
        let output = |text: String| Some(Response::DockerOutput { text });
        match request {
            Request::DockerContainers {} => output(self.inspect(apps).to_string()),
            Request::DockerStats {} => output(self.stats(apps, now)),
            Request::DockerImages {} => {
                let lines: Vec<String> = self
                    .images
                    .iter()
                    .map(|(id, name)| {
                        let (repository, tag) = name.split_once(':').unwrap_or((name, "latest"));
                        json!({"ID": id, "Repository": repository, "Tag": tag, "Digest": format!("sha256:{}", &id[7..]),
                               "Size": "187MB", "CreatedAt": "2026-10-20 10:00:00 +0000 UTC"})
                        .to_string()
                    })
                    .collect();
                output(lines.join("\n"))
            }
            Request::DockerNetworks {} => {
                let all = self.all(apps);
                let networks: Vec<Value> = self
                    .networks
                    .iter()
                    .map(|(name, subnet, project)| {
                        let members: serde_json::Map<String, Value> = all
                            .iter()
                            .filter(|c| c.running && project.is_some() && c.project == *project)
                            .map(|c| (c.id.clone(), json!({"Name": c.name})))
                            .collect();
                        let config: Vec<Value> = subnet.iter().map(|s| json!({"Subnet": s})).collect();
                        json!({"Id": hex(name.len() as u64), "Name": name,
                               "Driver": if name == "host" || name == "none" { name.as_str() } else { "bridge" },
                               "Internal": false, "IPAM": {"Config": config}, "Containers": members,
                               "Labels": project.as_ref().map_or(json!({}), |p| json!({"com.docker.compose.project": p})),
                               "Created": "2026-10-01T10:00:00Z"})
                    })
                    .collect();
                output(Value::from(networks).to_string())
            }
            Request::DockerDiskUsage {} => output(
                [
                    ("Images", "1.2GB"),
                    ("Containers", "4.1MB"),
                    ("Local Volumes", "0B"),
                    ("Build Cache", "0B"),
                ]
                .iter()
                .map(|(kind, size)| json!({"Type": kind, "Size": size}).to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            ),
            Request::AppsStatus {} => Some(Response::Apps { compose_ls: self.compose_ls(apps).to_string() }),
            Request::ControlContainer { container, action } => {
                let Some(found) = self
                    .containers
                    .iter_mut()
                    .find(|c| c.id == container.as_str() || c.name == container.as_str())
                else {
                    return Some(failed("Docker: no such container"));
                };
                match action {
                    ContainerAction::Start | ContainerAction::Restart => found.running = true,
                    ContainerAction::Stop => found.running = false,
                    ContainerAction::Remove if found.running => {
                        return Some(failed("Docker: you cannot remove a running container"));
                    }
                    ContainerAction::Remove => {
                        let id = found.id.clone();
                        self.containers.retain(|c| c.id != id);
                    }
                }
                Some(Response::Done)
            }
            Request::ContainerLogs { container, .. } => output(format!(
                "2026-11-06T08:00:00.000000000Z {} simulated: started\n2026-11-06T08:00:01.000000000Z listening on :80\n",
                container.as_str()
            )),
            Request::RemoveImage { image } => {
                let used = self
                    .all(apps)
                    .iter()
                    .any(|c| self.images.iter().any(|(id, name)| id == image.as_str() && *name == c.image));
                if used {
                    return Some(failed("Docker: image is being used by a container"));
                }
                self.images.retain(|(id, _)| id != image.as_str());
                Some(Response::Done)
            }
            Request::PullImage { reference } => {
                self.image_id(reference.as_str());
                Some(Response::Done)
            }
            Request::PruneImages {} => {
                let used: Vec<String> = self.all(apps).iter().map(|c| c.image.clone()).collect();
                let before = self.images.len();
                self.images.retain(|(_, name)| used.contains(name));
                output(format!("Total reclaimed space: {}MB\n", (before - self.images.len()) * 187))
            }
            Request::CreateNetwork { name, subnet, .. } => {
                if self.networks.iter().any(|(n, _, _)| n == name.as_str()) {
                    return Some(failed("Docker: network with that name already exists"));
                }
                self.networks.push((
                    name.as_str().to_owned(),
                    subnet.as_ref().map(|s| s.as_str().to_owned()),
                    None,
                ));
                Some(Response::Done)
            }
            Request::RemoveNetwork { name } => {
                if ["bridge", "host", "none"].contains(&name.as_str()) {
                    return Some(refused("Docker's own network"));
                }
                self.networks.retain(|(n, _, _)| n != name.as_str());
                Some(Response::Done)
            }
            Request::CheckCompose { compose, .. } => {
                let text = compose.as_str();
                if text.contains("broken") || !text.contains("services") {
                    return Some(refused("Docker Compose: yaml: line 2: did not find expected key"));
                }
                let mut volumes =
                    vec![json!({"type": "bind", "source": "/volume1/docker/app/data", "target": "/data"})];
                if text.contains("docker.sock") {
                    volumes.push(json!({"type": "bind", "source": "/var/run/docker.sock", "target": "/var/run/docker.sock"}));
                }
                let mut service = json!({"image": "nginx:1.27", "volumes": volumes});
                if text.contains("privileged: true") {
                    service["privileged"] = json!(true);
                }
                if text.contains("network_mode: host") {
                    service["network_mode"] = json!("host");
                }
                output(json!({"name": "sim", "services": {"app": service}}).to_string())
            }
            Request::ControlProject { project, action, file } => {
                let name = project.as_str();
                if apps.contains_key(name) {
                    return Some(match action {
                        ProjectAction::Start | ProjectAction::Stop | ProjectAction::Restart => {
                            apps.insert(name.to_owned(), !matches!(action, ProjectAction::Stop));
                            Response::Done
                        }
                        _ => refused(&format!("{name} is an app of a catalog: the App Center changes it")),
                    });
                }
                match action {
                    ProjectAction::Up => {
                        if let Some(file) = file {
                            self.own.insert(name.to_owned(), file.as_str().to_owned());
                        }
                        if !self.containers.iter().any(|c| c.project.as_deref() == Some(name)) {
                            self.next += 1;
                            let id = hex(self.next);
                            self.containers.push(SimContainer {
                                id,
                                name: format!("{name}-app-1"),
                                image: "nginx:1.27".into(),
                                project: Some(name.to_owned()),
                                service: Some("app".into()),
                                running: true,
                                host_port: None,
                                container_port: 80,
                                created: i64::try_from(now).unwrap_or(0),
                            });
                        }
                        self.containers
                            .iter_mut()
                            .filter(|c| c.project.as_deref() == Some(name))
                            .for_each(|c| c.running = true);
                    }
                    ProjectAction::Start | ProjectAction::Restart | ProjectAction::Stop => {
                        let running = !matches!(action, ProjectAction::Stop);
                        let members =
                            self.containers.iter_mut().filter(|c| c.project.as_deref() == Some(name));
                        let mut any = false;
                        for c in members {
                            c.running = running;
                            any = true;
                        }
                        if !any {
                            return Some(failed(&format!("{name} has no containers")));
                        }
                    }
                    ProjectAction::Down => {
                        self.containers.retain(|c| c.project.as_deref() != Some(name));
                        self.own.remove(name);
                    }
                    ProjectAction::Pull => {}
                }
                Some(Response::Done)
            }
            Request::ProjectLogs { project, .. } => output(format!(
                "{0}-app-1  | 2026-11-06T08:00:00.000000000Z simulated: started\n{0}-app-1  | 2026-11-06T08:00:01.000000000Z ready\n",
                project.as_str()
            )),
            Request::ProjectFiles { project } => {
                if apps.contains_key(project.as_str()) {
                    return Some(refused("an app of a catalog: the App Center changes it"));
                }
                if project.as_str() == "stack" {
                    let file = "services:\n  web:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n  db:\n    image: postgres:17\n";
                    return output(json!({"/opt/stack/compose.yml": file}).to_string());
                }
                Some(failed("Compose reports no file"))
            }
            _ => None,
        }
    }
}
