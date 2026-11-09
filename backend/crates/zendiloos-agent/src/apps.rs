//! Apps installed from a catalog the earlier way, run by Docker Compose
//! (docs/architecture.md, 4.9). The App Center now fills an app in as an
//! administrator's own project; what was installed before keeps running
//! from the normalized compose file the agent checked and kept then
//! (`/etc/zendiloos/apps/<app>.json`, root's alone):
//!
//! - before every start, each folder a container mounts is opened name by
//!   name without following a link;
//! - a folder in a share (rather than the whole share) is mounted pinned
//!   (`app_shares`), never by its path, and checked to be the folder the
//!   walk reaches now;
//! - removing takes the containers, images, the pins' unit and the file.
//!
//! What filling in an app for a project takes from the machine (an
//! account's IDs, the time zone, the devices there) comes from here too.

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{Mode, OFlags, fstat, openat};
use serde_json::Value;
use zendiloos_agent_proto::{AppFolder, AppName, DevicePath, MAX_LOG_LINES, Response, UserName};
use zendiloos_exec::{Limits, Output, Program, Surroundings};

use crate::accounts::{AccountFiles, require_managed};
use crate::app_shares::{self, Owner, open_mounted};
use crate::operations::OperationError;
use crate::passwd;
use crate::pools::HostPaths;

/// What zendiloos keeps in an app's normalized compose file besides what
/// Compose reads: where the app's data is and which shares it mounts.
pub(crate) const EXTENSION: &str = "x-zendiloos";

/// The environment Docker itself needs; the app's values come on top.
/// `DOCKER_CONFIG` points nowhere, so no configuration of root's is read.
pub(crate) const DOCKER_ENVIRONMENT: [(&str, &str); 2] =
    [("PATH", "/usr/sbin:/usr/bin:/sbin:/bin"), ("DOCKER_CONFIG", "/nonexistent")];

/// Listing the projects: quick.
const LIMITS: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 4 * 1024 * 1024 };
/// Starting, stopping or removing containers.
const LONG: Limits = Limits { timeout: Duration::from_secs(30 * 60), max_stdout: 1024 * 1024 };
const LOGS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 8 * 1024 * 1024 };

fn refused(reason: impl Into<String>) -> OperationError {
    OperationError::Refused(reason.into())
}

/// The end of what a program wrote to stderr, for a message.
fn complaint(output: &Output) -> String {
    let lines: Vec<&str> = output.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    if tail.is_empty() { format!("it ended with {}", output.status) } else { tail }
}

#[derive(Debug)]
pub(crate) struct Apps {
    docker: Program,
    systemctl: Program,
    paths: HostPaths,
    accounts: AccountFiles,
    /// One removal of a kept compose file at a time.
    serial: tokio::sync::Mutex<()>,
}

impl Apps {
    pub(crate) fn new(docker: Program, systemctl: Program, paths: HostPaths, accounts: AccountFiles) -> Self {
        Self { docker, systemctl, paths, accounts, serial: tokio::sync::Mutex::new(()) }
    }

    fn config_path(&self, app: &AppName) -> PathBuf {
        self.paths.apps.join(format!("{}.json", app.as_str()))
    }

    async fn remove_pin_unit(&self, app: &AppName) -> Result<(), OperationError> {
        app_shares::remove_unit(&self.systemctl, &self.paths, Owner::App(app)).await
    }

    /// The machine's time zone, from where `/etc/localtime` leads; UTC if
    /// it is no link into the zone database.
    fn time_zone(&self) -> String {
        let zone = fs::read_link(&self.paths.localtime).ok().and_then(|target| {
            let target = target.to_string_lossy().into_owned();
            let (_, zone) = target.split_once("zoneinfo/")?;
            let valid = !zone.is_empty()
                && !zone.contains("..")
                && zone.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'));
            valid.then(|| zone.to_owned())
        });
        zone.unwrap_or_else(|| "UTC".to_owned())
    }

    fn kept(&self, app: &AppName) -> Result<(PathBuf, Value), OperationError> {
        let path = self.config_path(app);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(refused(format!("{} is not installed", app.as_str())));
            }
            Err(error) => return Err(OperationError::Read(path, error)),
        };
        let normalized =
            serde_json::from_str(&text).map_err(|_| refused(format!("{} cannot be read", path.display())))?;
        Ok((path, normalized))
    }

    /// `docker compose` on the kept file, with the given arguments after it.
    async fn compose(&self, app: &AppName, args: &[&str], limits: Limits) -> Result<Output, OperationError> {
        let (path, _) = self.kept(app)?;
        self.compose_file(app, &path, args, limits).await
    }

    async fn compose_file(
        &self,
        app: &AppName,
        path: &Path,
        args: &[&str],
        limits: Limits,
    ) -> Result<Output, OperationError> {
        let path = path.to_string_lossy().into_owned();
        let mut all =
            vec!["compose", "--file", &path, "--env-file", "/dev/null", "--project-name", app.as_str()];
        all.extend_from_slice(args);
        let surroundings = Surroundings {
            variables: DOCKER_ENVIRONMENT.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect(),
            directory: None,
        };
        Ok(zendiloos_exec::run_with(&self.docker, &all, None, &surroundings, limits).await?)
    }

    async fn compose_done(
        &self,
        app: &AppName,
        args: &[&str],
        limits: Limits,
    ) -> Result<Response, OperationError> {
        let output = self.compose(app, args, limits).await?;
        if output.status.success() {
            Ok(Response::Done)
        } else {
            Err(OperationError::Failed(format!("Docker Compose: {}", complaint(&output))))
        }
    }

    pub(crate) async fn start(&self, app: &AppName) -> Result<Response, OperationError> {
        let (_, normalized) = self.kept(app)?;
        let pins = app_shares::pins_of(&normalized)?;
        if !pins.is_empty() {
            // Walked here first, for a refusal that names the folder.
            for pin in &pins {
                app_shares::walk(Path::new(&pin.share), &pin.folder)?;
            }
            // Pinned again: a folder that was missing at boot may be back.
            app_shares::repin(&self.systemctl, Owner::App(app)).await?;
        }
        self.check_mounts(&normalized)?;
        self.compose_done(app, &["up", "--detach", "--remove-orphans", "--quiet-pull"], LONG).await
    }

    pub(crate) async fn stop(&self, app: &AppName) -> Result<Response, OperationError> {
        self.compose_done(app, &["stop"], LONG).await
    }

    /// Containers, images and the kept file; nothing if the app never got
    /// that far.
    pub(crate) async fn remove(&self, app: &AppName) -> Result<(), OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let path = self.config_path(app);
        if !path.exists() {
            return Ok(());
        }
        let output = self
            .compose_file(app, &path, &["down", "--rmi", "all", "--volumes", "--remove-orphans"], LONG)
            .await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker Compose: {}", complaint(&output))));
        }
        self.remove_pin_unit(app).await?;
        fs::remove_file(&path).map_err(|e| OperationError::Write(path, e))
    }

    /// What filling in an app of a catalog for `user`'s project takes from
    /// the machine: the account's UID (one zendiloOS manages) and the GID of
    /// `zendiloos-files`, the group whose access every share's folder gives,
    /// so the containers reach the shares they mount; the time zone, and
    /// which of `devices` are there.
    pub(crate) fn facts(&self, user: &UserName, devices: &[DevicePath]) -> Result<Response, OperationError> {
        if devices.len() > DevicePath::MAX_COUNT {
            return Err(refused("too many devices at once"));
        }
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        require_managed(&passwd, &group, user)?;
        let (uid, _) = passwd::lookup(&passwd, user.as_str())
            .ok_or_else(|| OperationError::NotManaged(format!("user {}", user.as_str())))?;
        let files = crate::accounts::FILES_GROUP;
        let gid = passwd::group_id(&group, files)
            .ok_or_else(|| OperationError::Setup(format!("the group {files} is missing")))?;
        let devices =
            devices.iter().filter(|device| self.paths.dev.join(device.inside()).exists()).cloned().collect();
        Ok(Response::HostFacts { uid, gid, time_zone: self.time_zone(), devices })
    }

    /// Every Compose project with its containers' states, as Compose
    /// reports them; zendiloosd picks the apps' (their file is in
    /// `/etc/zendiloos/apps`).
    pub(crate) async fn status(&self) -> Result<Response, OperationError> {
        let surroundings = Surroundings {
            variables: DOCKER_ENVIRONMENT.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect(),
            directory: None,
        };
        let args = ["compose", "ls", "--all", "--format", "json"];
        let output = zendiloos_exec::run_with(&self.docker, args, None, &surroundings, LIMITS).await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker Compose: {}", complaint(&output))));
        }
        let compose_ls = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(Response::Apps { compose_ls })
    }

    pub(crate) async fn logs(&self, app: &AppName, lines: u32) -> Result<Response, OperationError> {
        let tail = lines.clamp(1, MAX_LOG_LINES).to_string();
        let output =
            self.compose(app, &["logs", "--no-color", "--timestamps", "--tail", &tail], LOGS).await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker Compose: {}", complaint(&output))));
        }
        Ok(Response::AppLogs { text: String::from_utf8_lossy(&output.stdout).into_owned() })
    }

    /// Each folder a container mounts: below the app's data folder, reached
    /// name by name without a link or another file system on the way; or a
    /// share the app was given, mounted. Anything else is refused.
    fn check_mounts(&self, normalized: &Value) -> Result<(), OperationError> {
        let extension = normalized.get(EXTENSION);
        let app_data = extension.and_then(|e| e.get("app_data")).and_then(Value::as_str).unwrap_or_default();
        let shares: Vec<&str> = extension
            .and_then(|e| e.get("shares"))
            .and_then(Value::as_array)
            .map(|shares| shares.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if app_data.is_empty() {
            return Err(refused("the kept compose file does not say where the app's data is"));
        }
        let pins = app_shares::pins_of(normalized)?;
        let docker_socket =
            extension.and_then(|e| e.get("docker_socket")).and_then(Value::as_bool).unwrap_or(false);
        let root = open_mounted(Path::new(app_data))?;
        for source in bind_sources(normalized) {
            if let Some(folder) = below(&source, app_data) {
                let folder =
                    AppFolder::parse(folder).map_err(|_| refused(format!("{source} is no plain path")))?;
                reach(&root, &folder).map_err(|reason| refused(format!("{source}: {reason}")))?;
            } else if let Some(pin) = pins.iter().find(|pin| pin.at == source) {
                app_shares::is_pinned(Path::new(&pin.at), Path::new(&pin.share), &pin.folder)?;
            } else if shares.contains(&source.as_str()) {
                open_mounted(Path::new(&source))?;
            } else if DOCKER_SOCKETS.contains(&source.as_str()) {
                if !docker_socket {
                    return Err(refused(format!("{source}: the app was not given the Docker socket")));
                }
                is_socket(&self.paths.docker_socket)?;
            } else {
                return Err(refused(format!("{source} is neither the app's data nor one of its shares")));
            }
        }
        Ok(())
    }
}

/// Where compose files name Docker's socket; `/var/run` is a link to `/run`.
const DOCKER_SOCKETS: [&str; 2] = ["/var/run/docker.sock", "/run/docker.sock"];

/// Docker's socket is there, a socket, root's.
fn is_socket(path: &Path) -> Result<(), OperationError> {
    use std::os::unix::fs::FileTypeExt;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| refused(format!("{} is missing", path.display())))?;
    if metadata.file_type().is_socket() && metadata.uid() == 0 {
        Ok(())
    } else {
        Err(refused(format!("{} is not Docker's socket", path.display())))
    }
}

fn read(path: &Path) -> Result<String, OperationError> {
    fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
}

/// The part of `path` below `root` (empty for `root` itself).
fn below<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if path == root {
        return Some("");
    }
    path.strip_prefix(root)?.strip_prefix('/')
}

/// The sources of every bind mount of every service.
fn bind_sources(normalized: &Value) -> Vec<String> {
    let services = normalized.get("services").and_then(Value::as_object).into_iter().flat_map(|s| s.values());
    services
        .flat_map(|service| service.get("volumes").and_then(Value::as_array).into_iter().flatten())
        .filter(|mount| mount.get("type").and_then(Value::as_str) == Some("bind"))
        .filter_map(|mount| mount.get("source").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Walks `folder` from `root` name by name: each must be a folder on the
/// same file system, never a link.
fn reach(root: &OwnedFd, folder: &AppFolder) -> Result<(), &'static str> {
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file_system = fstat(root).map_err(|_| "cannot be opened")?.st_dev;
    let mut current = openat(root, ".", flags, Mode::empty()).map_err(|_| "cannot be opened")?;
    for name in folder.names() {
        current =
            openat(&current, name, flags, Mode::empty()).map_err(|_| "is missing, a link or no folder")?;
        if fstat(&current).map_err(|_| "cannot be opened")?.st_dev != file_system {
            return Err("another file system is mounted on the way");
        }
    }
    Ok(())
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn normalized() -> Value {
        json!({
            "name": "demo",
            "services": {
                "app": {
                    "image": "x",
                    "devices": [
                        { "source": "/dev/dri", "target": "/dev/dri", "permissions": "rwm" },
                        { "source": "/dev/net/tun", "target": "/dev/net/tun", "permissions": "rwm" }
                    ],
                    "volumes": [
                        { "type": "bind", "source": "/volume1/@apps/demo/config", "target": "/config" },
                        { "type": "bind", "source": "/volume1/@apps/demo", "target": "/data" },
                        { "type": "bind", "source": "/volume1/media", "target": "/media", "read_only": true },
                        { "type": "tmpfs", "target": "/tmp" }
                    ]
                },
                "db": {
                    "image": "y",
                    "volumes": [{ "type": "bind", "source": "/volume1/@apps/demo/db", "target": "/var/lib/db" }]
                }
            }
        })
    }

    #[test]
    fn the_binds_are_what_is_checked_before_a_start() {
        let sources = bind_sources(&normalized());
        assert_eq!(
            sources,
            ["/volume1/@apps/demo/config", "/volume1/@apps/demo", "/volume1/media", "/volume1/@apps/demo/db"]
        );
        assert_eq!(below("/volume1/@apps/demo/config", "/volume1/@apps/demo"), Some("config"));
        assert_eq!(below("/volume1/@apps/demo", "/volume1/@apps/demo"), Some(""));
        assert_eq!(below("/volume1/@apps/demo2", "/volume1/@apps/demo"), None);
        assert!(bind_sources(&json!({ "services": "none" })).is_empty());
    }

    #[test]
    fn a_folder_that_became_a_link_is_not_reached() {
        use rustix::fs::open;
        let data = tempfile::tempdir().unwrap();
        fs::create_dir_all(data.path().join("config/cache")).unwrap();
        std::os::unix::fs::symlink("/etc", data.path().join("evil")).unwrap();
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let root = open(data.path(), flags, Mode::empty()).unwrap();
        assert_eq!(reach(&root, &AppFolder::parse("config/cache").unwrap()), Ok(()));
        assert!(reach(&root, &AppFolder::parse("evil").unwrap()).is_err());
        assert!(reach(&root, &AppFolder::parse("missing").unwrap()).is_err());
    }

    #[test]
    fn another_file_system_on_the_way_is_not_reached() {
        use rustix::fs::open;
        // /dev/shm is a file system of its own below /dev.
        let root = open("/dev", OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).unwrap();
        assert_eq!(
            reach(&root, &AppFolder::parse("shm").unwrap()),
            Err("another file system is mounted on the way")
        );
    }
}
