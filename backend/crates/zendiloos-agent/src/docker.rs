//! The Docker engine as administrators see it: containers, images,
//! networks and Compose projects, whoever started them. The agent runs
//! `docker` with fixed arguments and checked names and hands back what it
//! printed; zendiloosd reads it.
//!
//! Administrators may run any Compose file of theirs (docs/architecture.md,
//! 4.9). The apps of catalogs are the exception: their Compose files in
//! `/etc/zendiloos` passed the catalog's policy, and only the App Center
//! changes them, so projects whose file lies there are only started,
//! stopped and restarted here, and their files are not shown: they hold
//! generated settings.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value};
use zendiloos_agent_proto::{
    ComposeText, ContainerAction, ContainerId, ImageId, ImageReference, NetworkName, ProjectAction,
    ProjectDirectory, ProjectName, Response, Subnet,
};
use zendiloos_exec::{Limits, Output, Program, Surroundings};

use crate::apps::DOCKER_ENVIRONMENT;
use crate::operations::OperationError;

/// Reading: inspecting every container can be a few megabytes.
const QUICK: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 16 * 1024 * 1024 };
/// Fetching images and recreating containers.
const LONG: Limits = Limits { timeout: Duration::from_secs(30 * 60), max_stdout: 1024 * 1024 };
const LOGS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 8 * 1024 * 1024 };
const MAX_LOG_LINES: u32 = 10_000;
/// A Compose file shown here is at most this large.
const MAX_FILE: u64 = 256 * 1024;
/// Where the App Center keeps the checked Compose files of catalog apps.
const KEPT_BY_ZENDILOOS: &str = "/etc/zendiloos/";
const PROJECT_LABEL: &str = "label=com.docker.compose.project=";
/// Networks Docker itself has; they cannot be removed.
const BUILT_IN_NETWORKS: [&str; 3] = ["bridge", "host", "none"];

fn surroundings() -> Surroundings<'static> {
    Surroundings {
        variables: DOCKER_ENVIRONMENT.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect(),
        directory: None,
    }
}

/// The end of what `docker` wrote to stderr, for a message.
fn complaint(output: &Output) -> String {
    let lines: Vec<&str> = output.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    if tail.is_empty() { format!("it ended with {}", output.status) } else { tail }
}

fn text(stdout: Vec<u8>) -> Result<String, OperationError> {
    String::from_utf8(stdout).map_err(|_| OperationError::NotUtf8)
}

#[derive(Debug)]
pub(crate) struct Docker {
    docker: Program,
}

impl Docker {
    pub(crate) fn new(docker: Program) -> Self {
        Self { docker }
    }

    async fn run(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        limits: Limits,
    ) -> Result<Output, OperationError> {
        Ok(zendiloos_exec::run_with(&self.docker, args, input, &surroundings(), limits).await?)
    }

    /// What `docker` printed, if it succeeded.
    async fn printed(&self, args: &[&str], limits: Limits) -> Result<String, OperationError> {
        let output = self.run(args, None, limits).await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker: {}", complaint(&output))));
        }
        text(output.stdout)
    }

    async fn done(&self, args: &[&str], limits: Limits) -> Result<Response, OperationError> {
        self.printed(args, limits).await?;
        Ok(Response::Done)
    }

    /// Like [`Docker::done`], reporting what `docker` writes line by line
    /// ([`crate::progress::line`]) as it comes.
    async fn reported(&self, args: &[&str], limits: Limits) -> Result<Response, OperationError> {
        let output = zendiloos_exec::run_lines(
            &self.docker,
            args,
            &surroundings(),
            limits,
            &mut crate::progress::line,
        )
        .await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker: {}", complaint(&output))));
        }
        Ok(Response::Done)
    }

    fn output(text: String) -> Response {
        Response::DockerOutput { text }
    }

    /// `docker <object> inspect` of every ID `docker <object> ls --quiet`
    /// lists: a JSON array, empty if there is nothing.
    async fn inspect_all(&self, list: &[&str], inspect: &[&str]) -> Result<Response, OperationError> {
        let ids = self.printed(list, QUICK).await?;
        let ids: Vec<&str> = ids.split_whitespace().collect();
        if ids.is_empty() {
            return Ok(Self::output("[]".to_owned()));
        }
        let args: Vec<&str> = inspect.iter().copied().chain(ids).collect();
        Ok(Self::output(self.printed(&args, QUICK).await?))
    }

    pub(crate) async fn containers(&self) -> Result<Response, OperationError> {
        self.inspect_all(&["ps", "--all", "--quiet", "--no-trunc"], &["container", "inspect"]).await
    }

    pub(crate) async fn stats(&self) -> Result<Response, OperationError> {
        let args = ["stats", "--all", "--no-stream", "--no-trunc", "--format", "json"];
        Ok(Self::output(self.printed(&args, QUICK).await?))
    }

    pub(crate) async fn images(&self) -> Result<Response, OperationError> {
        let args = ["image", "ls", "--digests", "--no-trunc", "--format", "json"];
        Ok(Self::output(self.printed(&args, QUICK).await?))
    }

    pub(crate) async fn networks(&self) -> Result<Response, OperationError> {
        self.inspect_all(&["network", "ls", "--quiet", "--no-trunc"], &["network", "inspect"]).await
    }

    pub(crate) async fn disk_usage(&self) -> Result<Response, OperationError> {
        Ok(Self::output(self.printed(&["system", "df", "--format", "json"], QUICK).await?))
    }

    pub(crate) async fn control_container(
        &self,
        container: &ContainerId,
        action: ContainerAction,
    ) -> Result<Response, OperationError> {
        // Without --force: Docker refuses to remove a running container.
        self.done(&[action.docker_command(), container.as_str()], LONG).await
    }

    pub(crate) async fn container_logs(
        &self,
        container: &ContainerId,
        lines: u32,
    ) -> Result<Response, OperationError> {
        let tail = lines.clamp(1, MAX_LOG_LINES).to_string();
        let args = ["logs", "--timestamps", "--tail", &tail, container.as_str()];
        let output = zendiloos_exec::run_combined(&self.docker, args, &surroundings(), LOGS).await?;
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            return Err(OperationError::Failed(format!(
                "Docker: {}",
                printed.lines().last().unwrap_or_default()
            )));
        }
        Ok(Self::output(printed))
    }

    pub(crate) async fn remove_image(&self, image: &ImageId) -> Result<Response, OperationError> {
        self.done(&["image", "rm", image.as_str()], LONG).await
    }

    pub(crate) async fn pull_image(&self, reference: &ImageReference) -> Result<Response, OperationError> {
        self.done(&["pull", "--quiet", reference.as_str()], LONG).await
    }

    pub(crate) async fn prune_images(&self) -> Result<Response, OperationError> {
        Ok(Self::output(self.printed(&["image", "prune", "--all", "--force"], LONG).await?))
    }

    pub(crate) async fn create_network(
        &self,
        name: &NetworkName,
        subnet: Option<&Subnet>,
        gateway: Option<std::net::IpAddr>,
    ) -> Result<Response, OperationError> {
        if gateway.is_some() && subnet.is_none() {
            return Err(OperationError::Refused("a gateway needs a subnet".into()));
        }
        let gateway = gateway.map(|ip| ip.to_string());
        let mut args = vec!["network", "create", "--driver", "bridge"];
        if let Some(subnet) = subnet {
            args.extend(["--subnet", subnet.as_str()]);
        }
        if let Some(gateway) = &gateway {
            args.extend(["--gateway", gateway.as_str()]);
        }
        args.push(name.as_str());
        self.done(&args, QUICK).await
    }

    pub(crate) async fn remove_network(&self, name: &NetworkName) -> Result<Response, OperationError> {
        if BUILT_IN_NETWORKS.contains(&name.as_str()) {
            return Err(OperationError::Refused(format!("{} is Docker's own network", name.as_str())));
        }
        self.done(&["network", "rm", name.as_str()], QUICK).await
    }

    /// The Compose file as Compose understands it, read from stdin with
    /// `directory` as where relative paths start. A `.env` there does not
    /// count: what comes out is what runs (`DeployProject`), and the share
    /// could change it in between.
    pub(crate) async fn check_compose(
        &self,
        project: &ProjectName,
        directory: &ProjectDirectory,
        compose: &ComposeText,
    ) -> Result<Response, OperationError> {
        let args = [
            "compose",
            "--file",
            "-",
            "--project-directory",
            directory.as_str(),
            "--project-name",
            project.as_str(),
            "--env-file",
            "/dev/null",
            "config",
            "--format",
            "json",
        ];
        let output = self.run(&args, Some(compose.as_str().as_bytes()), QUICK).await?;
        if !output.status.success() {
            return Err(OperationError::Refused(format!("Docker Compose: {}", complaint(&output))));
        }
        Ok(Self::output(text(output.stdout)?))
    }

    /// The files Compose reports for `project`; empty if it reports none.
    async fn reported_files(&self, project: &ProjectName) -> Result<Vec<String>, OperationError> {
        let listed = self.printed(&["compose", "ls", "--all", "--format", "json"], QUICK).await?;
        let projects: Vec<Value> = serde_json::from_str(&listed)
            .map_err(|e| OperationError::Failed(format!("Docker Compose lists projects unreadably: {e}")))?;
        let files = projects
            .iter()
            .find(|entry| entry.get("Name").and_then(Value::as_str) == Some(project.as_str()))
            .and_then(|entry| entry.get("ConfigFiles")?.as_str())
            .unwrap_or_default();
        Ok(files.split(',').map(str::trim).filter(|file| !file.is_empty()).map(str::to_owned).collect())
    }

    /// The reported files of a project zendiloOS does not keep: neither an
    /// app of a catalog's nor an administrator's project made here.
    async fn foreign_files(&self, project: &ProjectName) -> Result<Vec<String>, OperationError> {
        let files = self.reported_files(project).await?;
        if files.iter().any(|file| file.starts_with(KEPT_BY_ZENDILOOS)) {
            return Err(OperationError::Refused(format!(
                "{} is kept by zendiloOS: an app of a catalog, or a project made here",
                project.as_str()
            )));
        }
        Ok(files)
    }

    /// Acts on a project made outside zendiloOS (an administrator's own go
    /// through `Projects`).
    pub(crate) async fn control_project(
        &self,
        project: &ProjectName,
        action: ProjectAction,
    ) -> Result<Response, OperationError> {
        let name = project.as_str();
        match action {
            ProjectAction::Start | ProjectAction::Stop | ProjectAction::Restart => {
                // By the containers' label, which works without the file.
                let filter = format!("{PROJECT_LABEL}{name}");
                let ids = self
                    .printed(&["ps", "--all", "--quiet", "--no-trunc", "--filter", &filter], QUICK)
                    .await?;
                let ids: Vec<&str> = ids.split_whitespace().collect();
                if ids.is_empty() {
                    return Err(OperationError::Failed(format!("{name} has no containers")));
                }
                let command = match action {
                    ProjectAction::Start => "start",
                    ProjectAction::Stop => "stop",
                    _ => "restart",
                };
                let args: Vec<&str> = std::iter::once(command).chain(ids).collect();
                self.done(&args, LONG).await
            }
            ProjectAction::Down => {
                self.foreign_files(project).await?;
                self.done(&["compose", "--project-name", name, "down", "--remove-orphans"], LONG).await
            }
            ProjectAction::Up | ProjectAction::Pull => {
                let files = self.foreign_files(project).await?;
                // Whoever may write the share could change such a file
                // before it runs, as root.
                if let Some(file) = files.iter().find(|file| in_a_volume(file)) {
                    return Err(OperationError::Refused(format!(
                        "{file} lies in a share, where others may change it: make {name} a project of \
                         zendiloOS's, or keep its files outside the shares"
                    )));
                }
                let Some(first) = files.first() else {
                    return Err(OperationError::Failed(format!("Compose reports no file for {name}")));
                };
                let directory = Path::new(first).parent().map(|p| p.to_string_lossy().into_owned());
                let mut args = vec!["compose"];
                for file in &files {
                    args.extend(["--file", file.as_str()]);
                }
                if let Some(directory) = &directory {
                    args.extend(["--project-directory", directory.as_str()]);
                }
                args.extend(["--project-name", name]);
                if action == ProjectAction::Up {
                    args.extend(["up", "--detach", "--remove-orphans"]);
                } else {
                    args.extend(["pull"]);
                }
                self.reported(&args, LONG).await
            }
        }
    }

    pub(crate) async fn project_logs(
        &self,
        project: &ProjectName,
        lines: u32,
    ) -> Result<Response, OperationError> {
        let tail = lines.clamp(1, MAX_LOG_LINES).to_string();
        let args = [
            "compose",
            "--project-name",
            project.as_str(),
            "logs",
            "--no-color",
            "--timestamps",
            "--tail",
            &tail,
        ];
        let output = zendiloos_exec::run_combined(&self.docker, args, &surroundings(), LOGS).await?;
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            return Err(OperationError::Failed(format!(
                "Docker Compose: {}",
                printed.lines().last().unwrap_or_default()
            )));
        }
        Ok(Self::output(printed))
    }

    pub(crate) async fn project_files(&self, project: &ProjectName) -> Result<Response, OperationError> {
        let mut files = Map::new();
        for path in self.foreign_files(project).await? {
            files.insert(path.clone(), Value::String(read_small(Path::new(&path))?));
        }
        Ok(Self::output(Value::Object(files).to_string()))
    }
}

/// Whether `path` lies in a volume, `/volume<N>/…`.
fn in_a_volume(path: &str) -> bool {
    path.strip_prefix("/volume")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(number, _)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// A regular file of at most [`MAX_FILE`] bytes, as text; never through a
/// link at its end, for it is read as root.
fn read_small(path: &Path) -> Result<String, OperationError> {
    use std::os::unix::fs::OpenOptionsExt;
    let error = |e| OperationError::Read(path.to_owned(), e);
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32 | rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(error)?;
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(OperationError::Refused(format!(
            "{} is no Compose file of at most 256 KiB",
            path.display()
        )));
    }
    let mut text = String::new();
    file.take(MAX_FILE).read_to_string(&mut text).map_err(error)?;
    Ok(text)
}
