//! [`DockerEngine`] through the system agent: it runs `docker` and hands
//! back what it printed; this reads it.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_agent_proto::{self as proto, AgentTransport, ErrorCode, Request, Response};
use zendiloos_containers::application::ports::{
    CheckedCompose, DockerEngine, EngineError, Lines, ProjectAction, TerminalInput, TerminalSession,
};
use zendiloos_containers::domain::{
    AttachedNetwork, ComposeListing, ComposeModel, ComposeNetwork, ComposeService, ComposeVolume, Container,
    ContainerAction, ContainerDetails, ContainerProcess, ContainerState, ContainerUsage, DeviceMapping,
    DiskUsage, Healthcheck, Image, Mount, Network, Privileges, ProjectName, PublishedPort, ResourceLimits,
    ServiceMount, UsageSample, Volume, VolumeSize,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use super::{WHAT, call_raw, call_raw_reporting, unexpected};

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

    /// As [`Self::done`], passing the lines Docker writes on to `lines`.
    async fn done_reporting(
        &self,
        request: Request,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        match call_raw_reporting(self.transport.as_ref(), request, &ToLines(lines), context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
                Err(EngineError::Refused(message))
            }
            Response::Error { message, .. } => Err(EngineError::Failed(message)),
            other => Err(unexpected(other).into()),
        }
    }
}

/// The agent's lines of output to [`Lines`]; its steps are not used here.
struct ToLines<'a>(&'a dyn Lines);

impl proto::Report for ToLines<'_> {
    fn step(&self, _: &str, _: Option<u8>) {}

    fn line(&self, line: &str) {
        self.0.line(line);
    }
}

fn unreadable(what: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::Port(PortError::new(WHAT, format!("Docker's {what} cannot be read: {error}")))
}

fn invalid(error: proto::InvalidDockerName) -> EngineError {
    EngineError::Refused(error.to_string())
}

/// The request for `action` on `project`.
fn control(project: &str, action: ProjectAction) -> Result<Request, EngineError> {
    let action = match action {
        ProjectAction::Start => proto::ProjectAction::Start,
        ProjectAction::Stop => proto::ProjectAction::Stop,
        ProjectAction::Restart => proto::ProjectAction::Restart,
        ProjectAction::Up => proto::ProjectAction::Up,
        ProjectAction::Pull => proto::ProjectAction::Pull,
        ProjectAction::Down => proto::ProjectAction::Down,
    };
    Ok(Request::ControlProject { project: proto::ProjectName::parse(project).map_err(invalid)?, action })
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

/// Text that may be missing or empty: `None` for both.
fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// Nanoseconds as Docker keeps durations, in whole seconds; 0 is unset.
fn seconds(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0).map(|n| n / 1_000_000_000)
}

/// One container as `docker container inspect` writes it: tolerant of
/// what is missing, as older engines leave fields out.
fn container_details(value: &Value) -> ContainerDetails {
    let config = &value["Config"];
    let host = &value["HostConfig"];
    let command = std::iter::once(string(value, "Path"))
        .filter(|path| !path.is_empty())
        .chain(texts(&value["Args"]))
        .collect();
    let environment = texts(&config["Env"])
        .into_iter()
        .map(|entry| match entry.split_once('=') {
            Some((name, value)) => (name.to_owned(), value.to_owned()),
            None => (entry, String::new()),
        })
        .collect();
    let mut networks: Vec<AttachedNetwork> = value["NetworkSettings"]["Networks"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, network)| AttachedNetwork {
            name: name.clone(),
            ipv4: optional(network, "IPAddress"),
            ipv6: optional(network, "GlobalIPv6Address"),
            gateway: optional(network, "Gateway"),
            mac: optional(network, "MacAddress"),
            aliases: texts(&network["Aliases"]),
        })
        .collect();
    networks.sort_by(|a, b| a.name.cmp(&b.name));
    let devices = host["Devices"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|device| DeviceMapping {
            host: string(device, "PathOnHost"),
            container: string(device, "PathInContainer"),
            permissions: string(device, "CgroupPermissions"),
        })
        .collect();
    let positive = |key: &str| host[key].as_u64().filter(|n| *n > 0);
    // Either a share of the processors (NanoCpus) or a quota per period.
    let cpus = positive("NanoCpus").map(|nano| nano as f64 / 1e9).or_else(|| {
        let (quota, period) = (host["CpuQuota"].as_i64()?, host["CpuPeriod"].as_i64()?);
        (quota > 0 && period > 0).then(|| quota as f64 / period as f64)
    });
    let health = &config["Healthcheck"];
    let healthcheck = health["Test"].as_array().filter(|test| !test.is_empty()).map(|_| Healthcheck {
        test: texts(&health["Test"]),
        interval_seconds: seconds(&health["Interval"]),
        timeout_seconds: seconds(&health["Timeout"]),
        retries: health["Retries"].as_u64().filter(|n| *n > 0),
    });
    let health_output = value["State"]["Health"]["Log"]
        .as_array()
        .and_then(|log| log.last())
        .and_then(|last| optional(last, "Output"))
        .map(|output| output.trim().to_owned());
    ContainerDetails {
        container: container(value),
        command,
        working_dir: optional(config, "WorkingDir"),
        user: optional(config, "User"),
        hostname: optional(config, "Hostname"),
        environment,
        network_mode: string(host, "NetworkMode"),
        networks,
        privileges: Privileges {
            privileged: host["Privileged"].as_bool().unwrap_or(false),
            capabilities_added: texts(&host["CapAdd"]),
            capabilities_dropped: texts(&host["CapDrop"]),
            devices,
            security_options: texts(&host["SecurityOpt"]),
            read_only_root: host["ReadonlyRootfs"].as_bool().unwrap_or(false),
            pid_mode: string(host, "PidMode"),
            ipc_mode: string(host, "IpcMode"),
        },
        limits: ResourceLimits {
            cpus,
            memory_bytes: positive("Memory"),
            memory_reservation_bytes: positive("MemoryReservation"),
            processes: positive("PidsLimit"),
        },
        restart_retries: host["RestartPolicy"]["MaximumRetryCount"].as_u64().unwrap_or(0),
        healthcheck,
        health_output,
    }
}

/// `docker top` with the agent's columns (pid, ppid, user, pcpu, pmem,
/// rss, etime, args): a header, then a process per line, its command last
/// and with spaces.
fn processes(text: &str) -> Vec<ContainerProcess> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().unwrap_or(0);
            let user = fields.next()?.to_owned();
            let cpu_percent = fields.next()?.parse().unwrap_or(0.0);
            let memory_percent = fields.next()?.parse().unwrap_or(0.0);
            // ps counts in KiB.
            let resident_bytes = fields.next()?.parse::<u64>().unwrap_or(0).saturating_mul(1024);
            let elapsed = fields.next()?.to_owned();
            let command = fields.collect::<Vec<_>>().join(" ");
            Some(ContainerProcess {
                pid,
                parent,
                user,
                cpu_percent,
                memory_percent,
                resident_bytes,
                elapsed,
                command,
            })
        })
        .collect()
}

/// How much of a terminal's output is read at once.
const TERMINAL_CHUNK: usize = 16 * 1024;

/// Relays a terminal's master between the descriptor and the session's
/// channels: what the program writes to `output`, what is typed from
/// `input`, a new size set on the terminal. Dropping `input`'s sender
/// closes the master, which hangs up the program.
fn relay(master: std::os::fd::OwnedFd) -> std::io::Result<TerminalSession> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::unix::pipe;

    let size = master.try_clone()?;
    let mut reader = pipe::Receiver::from_owned_fd_unchecked(master.try_clone()?)?;
    let mut writer = pipe::Sender::from_owned_fd_unchecked(master)?;
    let (output, from_terminal) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (input, mut typed) = tokio::sync::mpsc::channel::<TerminalInput>(64);
    tokio::spawn(async move {
        let mut buffer = vec![0u8; TERMINAL_CHUNK];
        loop {
            tokio::select! {
                // EIO once the program ended and the slave side is gone.
                read = reader.read(&mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output.send(buffer[..n].to_vec()).await.is_err() {
                            break;
                        }
                    }
                },
                input = typed.recv() => match input {
                    Some(TerminalInput::Data(data)) => {
                        if writer.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Some(TerminalInput::Resize { columns, rows }) => {
                        let wanted = proto::TerminalSize::clamped(columns, rows);
                        let size_now = rustix::termios::Winsize {
                            ws_row: wanted.rows,
                            ws_col: wanted.columns,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        // The program learns of it by SIGWINCH.
                        let _ = rustix::termios::tcsetwinsize(&size, size_now);
                    }
                    // Hung up: the descriptors close as this ends.
                    None => break,
                },
            }
        }
    });
    Ok(TerminalSession { output: from_terminal, input })
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

/// `docker volume inspect` of every volume.
fn volumes(text: &str) -> Result<Vec<Volume>, EngineError> {
    let values: Vec<Value> = serde_json::from_str(text).map_err(|e| unreadable("volumes", e))?;
    Ok(values
        .iter()
        .map(|value| Volume {
            name: string(value, "Name"),
            driver: string(value, "Driver"),
            labels: value["Labels"]
                .as_object()
                .map(|labels| {
                    labels
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
            mounts_elsewhere: value["Options"].as_object().is_some_and(|options| !options.is_empty()),
            created_at: timestamp(&string(value, "CreatedAt")),
        })
        .collect())
}

fn volume_name(name: &str) -> Result<proto::VolumeName, EngineError> {
    proto::VolumeName::parse(name).map_err(invalid)
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

/// Whether Compose says a volume or network is made outside the project:
/// `true`, or the older `{name: …}`.
fn external(entry: &Value) -> bool {
    match entry.get("external") {
        Some(Value::Bool(external)) => *external,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

fn compose_volumes(value: &Value) -> Vec<ComposeVolume> {
    let option = |volume: &Value, key: &str| volume.pointer(&format!("/driver_opts/{key}")).map(texted);
    value["volumes"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, volume)| ComposeVolume {
            key: key.clone(),
            driver: volume.get("driver").and_then(Value::as_str).map(str::to_owned),
            external: external(volume),
            kind: option(volume, "type"),
            options: option(volume, "o"),
            device: option(volume, "device"),
        })
        .collect()
}

fn compose_networks(value: &Value) -> Vec<ComposeNetwork> {
    value["networks"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, network)| ComposeNetwork {
            key: key.clone(),
            name: network.get("name").and_then(Value::as_str).unwrap_or(key).to_owned(),
            driver: network.get("driver").and_then(Value::as_str).map(str::to_owned),
            external: external(network),
            options: network
                .get("driver_opts")
                .and_then(Value::as_object)
                .map(|options| options.keys().cloned().collect())
                .unwrap_or_default(),
        })
        .collect()
}

/// A string as it is, anything else as JSON: options may be numbers.
fn texted(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
}

fn compose_model(text: &str) -> Result<ComposeModel, EngineError> {
    let value: Value = serde_json::from_str(text).map_err(|e| unreadable("Compose file", e))?;
    let services = value["services"].as_object().cloned().unwrap_or_default();
    let (volumes, networks) = (compose_volumes(&value), compose_networks(&value));
    // Binds in disguise, by name: a named volume of the local driver with
    // `o: bind` or `rbind` mounts its `device`; a secret or config from a
    // file is bound.
    let named_binds: BTreeMap<String, String> =
        volumes.iter().filter_map(|volume| Some((volume.key.clone(), volume.bound()?.to_owned()))).collect();
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
                    mounts: service["volumes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|volume| volume.get("type").and_then(Value::as_str) == Some("bind"))
                        .filter_map(|volume| {
                            Some(ServiceMount {
                                source: volume.get("source")?.as_str()?.to_owned(),
                                target: volume.get("target")?.as_str()?.to_owned(),
                            })
                        })
                        .collect(),
                    devices: service["devices"].as_array().into_iter().flatten().filter_map(source).collect(),
                    cap_add: texts(&service["cap_add"]),
                    security_opt: texts(&service["security_opt"]),
                    volumes: service["volumes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|volume| volume.get("type").and_then(Value::as_str) == Some("volume"))
                        .filter_map(|volume| volume.get("source").and_then(Value::as_str).map(str::to_owned))
                        .collect(),
                    networks: match service.get("networks").and_then(Value::as_object) {
                        Some(networks) => networks.keys().cloned().collect(),
                        // Compose puts a service without either on the
                        // project's default network.
                        None if mode("network_mode").is_none() => vec!["default".to_owned()],
                        None => Vec::new(),
                    },
                    keys: service.as_object().map(|s| s.keys().cloned().collect()).unwrap_or_default(),
                }
            })
            .collect(),
        volumes,
        networks,
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

    async fn inspect_container(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<ContainerDetails, EngineError> {
        let text = self.printed(Request::InspectContainer { container: container_id(id)? }, context).await?;
        let values: Vec<Value> = serde_json::from_str(&text).map_err(|e| unreadable("container", e))?;
        values.first().map(container_details).ok_or_else(|| EngineError::Failed(format!("no container {id}")))
    }

    async fn container_processes(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Vec<ContainerProcess>, EngineError> {
        let text =
            self.printed(Request::ContainerProcesses { container: container_id(id)? }, context).await?;
        Ok(processes(&text))
    }

    async fn open_terminal(
        &self,
        id: &str,
        command: &[String],
        columns: u16,
        rows: u16,
        context: &RequestContext,
    ) -> Result<TerminalSession, EngineError> {
        let request = Request::OpenTerminal {
            container: container_id(id)?,
            command: proto::ExecCommand::parse(command.to_vec()).map_err(invalid)?,
            size: proto::TerminalSize::clamped(columns, rows),
        };
        let request_id = context.request_id.as_deref().and_then(|id| proto::RequestId::parse(id).ok());
        let answer =
            self.transport.call_with_file(request, request_id).await.map_err(|e| PortError::new(WHAT, e))?;
        match answer {
            (Response::Terminal {}, Some(master)) => relay(master)
                .map_err(|e| PortError::new(WHAT, format!("cannot relay the terminal: {e}")).into()),
            (Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message }, _) => {
                Err(EngineError::Refused(message))
            }
            (Response::Error { message, .. }, _) => Err(EngineError::Failed(message)),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    fn read_config(&self, config: &str) -> Result<ComposeModel, EngineError> {
        compose_model(config)
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

    async fn volumes(&self, context: &RequestContext) -> Result<Vec<Volume>, EngineError> {
        volumes(&self.printed(Request::DockerVolumes {}, context).await?)
    }

    async fn volume_sizes(&self, context: &RequestContext) -> Result<Vec<VolumeSize>, EngineError> {
        match self.ask(Request::DockerVolumeSizes {}, context).await? {
            Response::VolumeSizes { sizes } => Ok(sizes
                .into_iter()
                .map(|size| VolumeSize { name: size.name, bytes: size.bytes, complete: size.complete })
                .collect()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn remove_volume(&self, name: &str, context: &RequestContext) -> Result<(), EngineError> {
        self.done(Request::RemoveVolume { name: volume_name(name)? }, context).await
    }

    async fn prune_volumes(&self, context: &RequestContext) -> Result<String, EngineError> {
        Ok(plain(&self.printed(Request::PruneVolumes {}, context).await?))
    }

    async fn create_volume(
        &self,
        name: &str,
        labels: &BTreeMap<String, String>,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let labels = proto::VolumeLabels::parse(labels.clone()).map_err(invalid)?;
        self.done(Request::CreateVolume { name: volume_name(name)?, labels }, context).await
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
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let request = Request::DeployProject {
            project: proto::ProjectName::parse(project).map_err(invalid)?,
            config: proto::ProjectConfig::parse(config).map_err(invalid)?,
            pull,
        };
        self.done_reporting(request, lines, context).await
    }

    async fn control_project(
        &self,
        project: &str,
        action: ProjectAction,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        self.done(control(project, action)?, context).await
    }

    async fn control_project_reporting(
        &self,
        project: &str,
        action: ProjectAction,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        self.done_reporting(control(project, action)?, lines, context).await
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

    use zendiloos_containers::application::ports::NoLines;

    use super::*;
    use crate::agent::scripted::Scripted;

    /// The lines Docker wrote, as told.
    #[derive(Default)]
    struct Heard(std::sync::Mutex<Vec<String>>);

    impl Lines for Heard {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_owned());
        }
    }

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
        let heard = Heard::default();
        engine.deploy_project("tesla", &checked.config, true, &heard, &context).await.unwrap();
        let lines = heard.0.lock().unwrap().clone();
        assert_eq!(lines.first().map(String::as_str), Some(" app Pulling"), "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some(" Container tesla-app-1  Started"));
        let heard = Heard::default();
        engine.control_project_reporting("tesla", ProjectAction::Pull, &heard, &context).await.unwrap();
        assert!(heard.0.lock().unwrap().iter().any(|line| line == " app Pulled"));
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
    async fn volumes_are_read_made_and_removed_through_the_agent() {
        let (engine, agent) = simulated();
        let context = RequestContext::default();
        let volumes = engine.volumes(&context).await.unwrap();
        let cache = volumes.iter().find(|v| v.name == "stack_cache").unwrap();
        assert_eq!((cache.driver.as_str(), cache.project()), ("local", Some("stack")));
        assert!(cache.backed_up_here() && cache.created_at.is_some());
        assert_eq!(engine.volume_sizes(&context).await.unwrap().len(), volumes.len());
        let labels = BTreeMap::from([("com.docker.compose.project".to_owned(), "tesla".to_owned())]);
        engine.create_volume("tesla_db", &labels, &context).await.unwrap();
        let made = engine.volumes(&context).await.unwrap();
        assert_eq!(made.iter().find(|v| v.name == "tesla_db").and_then(|v| v.project()), Some("tesla"));
        engine.remove_volume("tesla_db", &context).await.unwrap();
        let used = engine.remove_volume("stack_cache", &context).await;
        assert!(matches!(used, Err(EngineError::Failed(_))), "{used:?}");
        assert!(engine.prune_volumes(&context).await.unwrap().contains("reclaimed"));
        assert!(agent.received().iter().any(|r| matches!(r, Request::PruneVolumes {})));

        let refused = |result: Result<(), EngineError>| {
            assert!(matches!(result, Err(EngineError::Refused(_))), "{result:?}")
        };
        refused(engine.remove_volume("../x", &context).await);
        refused(engine.create_volume("-x", &BTreeMap::new(), &context).await);
        let odd = BTreeMap::from([("a=b".to_owned(), String::new())]);
        refused(engine.create_volume("good", &odd, &context).await);
        let odd = AgentDockerEngine::new(Arc::new(Scripted::answering([Response::Done])));
        assert!(matches!(odd.volume_sizes(&context).await, Err(EngineError::Port(_))));
        let odd = AgentDockerEngine::new(Arc::new(Scripted::answering([Response::DockerOutput {
            text: "{".into(),
        }])));
        assert!(odd.volumes(&context).await.is_err());
    }

    #[test]
    fn volumes_read_as_docker_inspects_them() {
        let read = volumes(
            r#"[{"Name": "nfs", "Driver": "local", "Options": {"type": "nfs", "device": ":/export"},
                 "Labels": null, "CreatedAt": "2026-11-01T10:00:00+01:00"},
                {"Name": "plugin", "Driver": "rclone", "Options": {}, "Labels": {"a": "b"}}]"#,
        )
        .unwrap();
        assert!(read[0].mounts_elsewhere && !read[0].backed_up_here());
        assert_eq!(read[0].created_at, Some(zendiloos_kernel::Timestamp::from_unix_seconds(1_793_523_600)));
        assert!(!read[1].mounts_elsewhere && !read[1].backed_up_here());
        assert_eq!((read[1].labels["a"].as_str(), read[1].created_at), ("b", None));
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
        refused(engine.deploy_project("Bad", "{}", false, &NoLines, &context).await);
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

    /// Reads a terminal's output until `wanted` came, or it closed.
    async fn read_until(session: &mut TerminalSession, wanted: &str) -> String {
        let mut seen = String::new();
        while !seen.contains(wanted) {
            match tokio::time::timeout(std::time::Duration::from_secs(10), session.output.recv()).await {
                Ok(Some(chunk)) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                _ => break,
            }
        }
        seen
    }

    #[tokio::test]
    async fn one_container_reads_in_detail_and_a_terminal_relays_both_ways() {
        let (engine, agent) = simulated();
        let context = RequestContext::default();
        let containers = engine.containers(&context).await.unwrap();
        let id = |name: &str| containers.iter().find(|c| c.name == name).unwrap().id.clone();

        let web = engine.inspect_container(&id("stack-web-1"), &context).await.unwrap();
        assert_eq!(web.container.name, "stack-web-1");
        assert_eq!(web.command, ["/docker-entrypoint.sh", "nginx", "-g", "daemon off;"]);
        assert_eq!(web.privileges.capabilities_dropped, ["ALL"]);
        assert_eq!(web.privileges.capabilities_added, ["NET_BIND_SERVICE"]);
        assert_eq!(web.privileges.security_options, ["no-new-privileges:true"]);
        assert!(web.environment.contains(&("TZ".to_owned(), "Europe/Berlin".to_owned())));
        assert_eq!(
            (web.networks[0].name.as_str(), web.network_mode.as_str()),
            ("stack_default", "stack_default")
        );
        assert!(web.networks[0].ipv4.as_deref().is_some_and(|ip| ip.starts_with("172.19.")));
        assert_eq!(web.container.mounts.len(), 2);
        assert!(web.container.mounts[0].read_only);
        assert_eq!(web.limits, ResourceLimits::default());
        let db = engine.inspect_container(&id("stack-db-1"), &context).await.unwrap();
        assert_eq!((db.limits.memory_bytes, db.limits.cpus), (Some(536_870_912), Some(1.5)));
        assert_eq!(db.user.as_deref(), Some("999:999"));
        let check = db.healthcheck.unwrap();
        assert_eq!(
            (check.interval_seconds, check.timeout_seconds, check.retries),
            (Some(10), Some(5), Some(5))
        );
        assert!(
            db.environment
                .iter()
                .any(|(name, value)| name == "POSTGRES_PASSWORD" && value == "simulated-secret")
        );
        assert!(matches!(engine.inspect_container("ghost", &context).await, Err(EngineError::Failed(_))));

        let processes = engine.container_processes(&id("stack-web-1"), &context).await.unwrap();
        assert_eq!(processes.len(), 2);
        assert_eq!((processes[1].pid, processes[1].user.as_str()), (21040, "101"));
        assert_eq!(processes[1].command, "nginx: worker process");
        assert_eq!(processes[1].resident_bytes, 51_200 * 1024);

        let mut terminal = engine
            .open_terminal(&id("stack-web-1"), &["/bin/sh".to_owned()], 100, 30, &context)
            .await
            .unwrap();
        assert!(read_until(&mut terminal, "$ ").await.contains("running /bin/sh"));
        terminal.input.send(TerminalInput::Data(b"ls\n".to_vec())).await.unwrap();
        assert!(read_until(&mut terminal, "simulated: ls").await.contains("simulated: ls"));
        terminal.input.send(TerminalInput::Resize { columns: 120, rows: 40 }).await.unwrap();
        terminal.input.send(TerminalInput::Data(b"exit\n".to_vec())).await.unwrap();
        // The program ended: the output closes.
        let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while terminal.output.recv().await.is_some() {}
        })
        .await;
        assert!(ended.is_ok(), "the output closes once the program ended");
        assert!(agent.received().iter().any(|r| matches!(r, Request::OpenTerminal { size, .. }
            if size.columns == 100 && size.rows == 30)));
        let refused = engine.open_terminal(&id("stack-web-1"), &["-it".to_owned()], 80, 24, &context).await;
        assert!(matches!(refused, Err(EngineError::Refused(_))));
    }

    #[test]
    fn details_and_processes_read_whatever_docker_leaves_out() {
        let bare = container_details(&serde_json::json!({"Id": "abc", "Name": "/x"}));
        assert_eq!(bare.container.name, "x");
        assert!(bare.command.is_empty() && bare.environment.is_empty() && bare.networks.is_empty());
        assert_eq!(bare.healthcheck, None);
        let quota = container_details(&serde_json::json!({
            "HostConfig": {"CpuQuota": 50000, "CpuPeriod": 100000, "PidsLimit": 100,
                           "Devices": [{"PathOnHost": "/dev/dri", "PathInContainer": "/dev/dri", "CgroupPermissions": "rwm"}]},
            "Config": {"Env": ["FLAG"]},
            "State": {"Health": {"Log": [{"Output": "old"}, {"Output": " ready\n"}]}}
        }));
        assert_eq!((quota.limits.cpus, quota.limits.processes), (Some(0.5), Some(100)));
        assert_eq!(quota.privileges.devices[0].host, "/dev/dri");
        assert_eq!(quota.environment, [("FLAG".to_owned(), String::new())]);
        assert_eq!(quota.health_output.as_deref(), Some("ready"));
        let listed = processes(
            "PID PPID USER %CPU %MEM RSS ELAPSED COMMAND\n7 1 root x y z 1-00:00:01 sh -c a  b\nbroken\n",
        );
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].cpu_percent, listed[0].resident_bytes), (0.0, 0));
        assert_eq!(listed[0].command, "sh -c a b");
    }

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

    /// As `docker compose config --format json` prints them: what named
    /// volumes and networks give a container of the machine is named.
    #[test]
    fn named_volumes_and_networks_that_reach_the_machine_are_named() {
        let named = |config: &str| -> Vec<(&'static str, Option<String>)> {
            let model = compose_model(config).unwrap();
            zendiloos_containers::domain::risks(&model).iter().map(|r| (r.kind(), r.detail())).collect()
        };
        let rbind = r#"{"name":"p","services":{"app":{"image":"alpine:3","restart":"unless-stopped",
            "volumes":[{"type":"volume","source":"host","target":"/host","volume":{}}]}},
            "volumes":{"host":{"name":"p_host","driver_opts":{"type":"none","o":"rbind","device":"/"}}}}"#;
        assert_eq!(named(rbind), [("system_path", Some("/".to_owned()))], "rbind binds as bind does");

        let disk = r#"{"name":"p","services":{"app":{"image":"alpine:3",
            "volumes":[{"type":"volume","source":"disk","target":"/disk","volume":{}}]}},
            "volumes":{"disk":{"name":"p_disk","driver_opts":{"type":"ext4","device":"/dev/sda2"}}}}"#;
        assert_eq!(named(disk), [("devices", Some("/dev/sda2".to_owned()))]);

        for folder in ["/volume1/@docker", "/volume1/@docker/containers", "/volume1/@homes", "/volume1/@apps"]
        {
            let config = format!(
                r#"{{"name":"p","services":{{"app":{{"image":"alpine:3",
                "volumes":[{{"type":"bind","source":"{folder}","target":"/x","bind":{{"create_host_path":true}}}}]}}}}}}"#
            );
            assert_eq!(named(&config), [("system_path", Some(folder.to_owned()))], "{folder}");
        }

        let host = r#"{"name":"p","services":{"app":{"image":"alpine:3","networks":{"default":null}}},
            "networks":{"default":{"name":"host","external":true}}}"#;
        assert_eq!(named(host), [("host_network", None)], "the machine's network by its name");
        let unlisted = r#"{"name":"p","services":{"app":{"image":"alpine:3"}},
            "networks":{"default":{"name":"lan","driver":"macvlan","driver_opts":{"parent":"eth0"}}}}"#;
        assert_eq!(named(unlisted), [("unusual", Some("network default: driver macvlan".to_owned()))]);

        let plain = r#"{"name":"p","services":{"app":{"image":"alpine:3","networks":{"default":null},
            "volumes":[{"type":"volume","source":"data","target":"/data"}]}},
            "networks":{"default":{"name":"p_default","ipam":{}}},"volumes":{"data":{"name":"p_data"}}}"#;
        assert_eq!(named(plain), [], "a project's own volume and bridge");
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
