//! Apps from a catalog, run by Docker Compose (docs/architecture.md, 4.9).
//!
//! zendiloosd sends an app's manifest, its compose file as written and the
//! values for it; the agent trusts none of it. It reads both files itself
//! and checks them with the catalog's own code (`zendiloos-catalog`), as
//! the catalog's format asks of zendiloOS (format.md, section 8):
//!
//! - `check_source` before Compose sees the file, which would otherwise
//!   read what `env_file`, `extends` or `include` name;
//! - the file normalized twice by `docker compose config`, with stand-in
//!   values for `check_variables` and with the real ones for
//!   `check_normalized`, each time with nothing but those values in the
//!   environment, `--env-file /dev/null` and an empty project folder;
//! - only that normalized form is kept and ever started;
//! - before every start, each folder a container mounts is opened name by
//!   name without following a link;
//! - a folder in a share (rather than the whole share) is mounted pinned
//!   (`app_shares`), never by its path.
//!
//! The account (`app-<id>`), the time zone and the data folder
//! (`/volume<N>/@apps/<id>`) the agent fills in itself.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{Mode, OFlags, fstat, openat};
use serde_json::Value;
use zendiloos_agent_proto::{
    AppCompose, AppFolder, AppManifest, AppName, AppShareChoice, AppValues, MAX_LOG_LINES, Response,
    VolumeNumber,
};
use zendiloos_catalog::compose::{check_normalized, check_source, check_variables};
use zendiloos_catalog::{Manifest, Placement, Problem, RESERVED_HOST_PORTS, SettingKind};
use zendiloos_exec::{Limits, Output, Program, Surroundings};

use crate::accounts::{AccountFiles, app_account};
use crate::app_shares::{self, Owner, Pin, open_mounted};
use crate::operations::OperationError;
use crate::pools::{APPS_DIRECTORY, HostPaths};

/// What zendiloos keeps in an app's normalized compose file besides what
/// Compose reads: where the app's data is and which shares it mounts.
pub(crate) const EXTENSION: &str = "x-zendiloos";

/// The environment Docker itself needs; the app's values come on top.
/// `DOCKER_CONFIG` points nowhere, so no configuration of root's is read.
pub(crate) const DOCKER_ENVIRONMENT: [(&str, &str); 2] =
    [("PATH", "/usr/sbin:/usr/bin:/sbin:/bin"), ("DOCKER_CONFIG", "/nonexistent")];

/// Names a setting must not have: Docker, Compose, the Go runtime or the C
/// library would read them from the environment `docker compose` runs in,
/// as root.
const RESERVED_VARIABLES: [&str; 33] = [
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SHELL",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TZDIR",
    "HOSTALIASES",
    "LOCALDOMAIN",
    "GCONV_PATH",
    "GETCONF_DIR",
    "LOCPATH",
    "NLSPATH",
    "NIS_PATH",
    "GODEBUG",
    "GOGC",
    "GOMAXPROCS",
    "GOMEMLIMIT",
    "GOTRACEBACK",
    "GOCOVERDIR",
    "NO_COLOR",
    "FORCE_COLOR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "FTP_PROXY",
    "SSL_CERT_FILE",
];
const RESERVED_PREFIXES: [&str; 15] = [
    "DOCKER_",
    "COMPOSE_",
    "BUILDKIT_",
    "BUILDX_",
    "LD_",
    "LC_",
    "XDG_",
    "OTEL_",
    "SSH_",
    "GIT_",
    "NOTARY_",
    "MALLOC_",
    "GLIBC_",
    "RES_",
    "SSL_",
];

/// Reading and checking: quick.
const LIMITS: Limits = Limits { timeout: Duration::from_secs(120), max_stdout: 4 * 1024 * 1024 };
/// Fetching images and starting or removing containers.
const LONG: Limits = Limits { timeout: Duration::from_secs(30 * 60), max_stdout: 1024 * 1024 };
const LOGS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 8 * 1024 * 1024 };

fn refused(reason: impl Into<String>) -> OperationError {
    OperationError::Refused(reason.into())
}

/// Refuses with the problems, if there are any.
fn require_none(problems: Vec<Problem>) -> Result<(), OperationError> {
    if problems.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
    Err(refused(format!("the app does not pass the checks:\n{}", lines.join("\n"))))
}

/// Whether Docker, Compose, Go or the C library read a variable of this name.
fn is_reserved_variable(name: &str) -> bool {
    RESERVED_VARIABLES.contains(&name) || RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// The end of what a program wrote to stderr, for a message.
fn complaint(output: &Output) -> String {
    let lines: Vec<&str> = output.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    if tail.is_empty() { format!("it ended with {}", output.status) } else { tail }
}

/// A folder of its own in the agent's private `/tmp`, removed when dropped.
struct EmptyFolder(PathBuf);

impl EmptyFolder {
    fn new() -> Result<Self, OperationError> {
        let mut bytes = [0u8; 8];
        rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| OperationError::Write(std::env::temp_dir(), e.into()))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("zendiloos-compose-{name}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| OperationError::Write(path.clone(), e))?;
        Ok(Self(path))
    }
}

impl Drop for EmptyFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

use std::os::unix::fs::DirBuilderExt;

#[derive(Debug)]
pub(crate) struct Apps {
    docker: Program,
    systemctl: Program,
    paths: HostPaths,
    accounts: AccountFiles,
    /// One change to the kept compose files at a time.
    serial: tokio::sync::Mutex<()>,
}

impl Apps {
    pub(crate) fn new(docker: Program, systemctl: Program, paths: HostPaths, accounts: AccountFiles) -> Self {
        Self { docker, systemctl, paths, accounts, serial: tokio::sync::Mutex::new(()) }
    }

    fn config_path(&self, app: &AppName) -> PathBuf {
        self.paths.apps.join(format!("{}.json", app.as_str()))
    }

    /// `/volume<N>/@apps/<app>`, with the app's own dataset mounted there.
    fn app_data(&self, volume: VolumeNumber, app: &AppName) -> Result<PathBuf, OperationError> {
        let apps = self.paths.volumes.join(format!("volume{}", volume.get())).join(APPS_DIRECTORY);
        let path = apps.join(app.as_str());
        let mounted = match (fs::symlink_metadata(&path), fs::metadata(&apps)) {
            (Ok(own), Ok(above)) => own.is_dir() && own.dev() != above.dev(),
            _ => false,
        };
        if mounted { Ok(path) } else { Err(refused(format!("the data of {} is not mounted", app.as_str()))) }
    }

    /// `/volume<N>/<share>`, with the share's dataset mounted there.
    fn share_root(&self, choice: &AppShareChoice) -> Result<PathBuf, OperationError> {
        let volume = self.paths.volumes.join(format!("volume{}", choice.volume.get()));
        let path = volume.join(choice.name.as_str());
        let mounted = match (fs::symlink_metadata(&path), fs::metadata(&volume)) {
            (Ok(own), Ok(above)) => own.is_dir() && own.dev() != above.dev(),
            _ => false,
        };
        if mounted {
            Ok(path)
        } else {
            Err(refused(format!("the share {} is not mounted", choice.name.as_str())))
        }
    }

    /// What the app mounts for the share `id`: the share's root, or the pin
    /// of the folder picked in it, which must be reachable now.
    fn share_mount(
        &self,
        app: &AppName,
        id: &str,
        choice: &AppShareChoice,
    ) -> Result<(String, Option<Pin>), OperationError> {
        let root = self.share_root(choice)?;
        if choice.folder.is_whole() {
            return Ok((root.to_string_lossy().into_owned(), None));
        }
        app_shares::walk(&root, &choice.folder)?;
        let at = app_shares::pin_path(&self.paths, Owner::App(app), id)?.to_string_lossy().into_owned();
        let pin = Pin {
            share: root.to_string_lossy().into_owned(),
            folder: choice.folder.clone(),
            at: at.clone(),
            file: false,
            create: false,
        };
        Ok((at, Some(pin)))
    }

    /// The unit pinning the app's folders: written and enabled when there
    /// are any, stopped and removed when there are none.
    async fn place_pin_unit(&self, app: &AppName, pins: &[Pin]) -> Result<(), OperationError> {
        app_shares::place_unit(&self.systemctl, &self.paths, Owner::App(app), pins).await
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

    /// The values for `manifest`: exactly the shares, ports and settings it
    /// declares, each checked; and the folders of shares to pin.
    fn placement(
        &self,
        app: &AppName,
        manifest: &Manifest,
        app_data: &Path,
        account: (u32, u32),
        values: &AppValues,
    ) -> Result<(Placement, Vec<Pin>), OperationError> {
        let same_ids = |declared: BTreeSet<&str>, given: BTreeSet<&str>, what: &str| {
            if declared == given {
                Ok(())
            } else {
                Err(refused(format!("give a value for each of the app's {what}, and for no other")))
            }
        };
        same_ids(
            manifest.shares.iter().map(|s| s.id.as_str()).collect(),
            values.shares.keys().map(String::as_str).collect(),
            "shares",
        )?;
        same_ids(
            manifest.ports.iter().map(|p| p.id.as_str()).collect(),
            values.ports.keys().map(String::as_str).collect(),
            "ports",
        )?;
        same_ids(
            manifest.settings.iter().map(|s| s.id.as_str()).collect(),
            values.settings.keys().map(String::as_str).collect(),
            "settings",
        )?;

        let mut shares = BTreeMap::new();
        let mut pins = Vec::new();
        for (id, choice) in &values.shares {
            let (mounted, pin) = self.share_mount(app, id, choice)?;
            shares.insert(id.clone(), mounted);
            pins.extend(pin);
        }

        let mut taken = BTreeSet::new();
        for port in &manifest.ports {
            let chosen = values.ports[&port.id];
            if port.fixed && chosen != port.default {
                return Err(refused(format!("the port {} is fixed at {}", port.id, port.default)));
            }
            if chosen == 0 || RESERVED_HOST_PORTS.contains(&chosen) {
                return Err(refused(format!("port {chosen} is not available to apps")));
            }
            if !taken.insert(chosen) {
                return Err(refused(format!("port {chosen} is given twice")));
            }
        }

        for setting in &manifest.settings {
            let value = values.settings[&setting.id].expose();
            if is_reserved_variable(&setting.id) {
                return Err(refused(format!(
                    "the setting {} has a name Docker reads itself; the app cannot be installed",
                    setting.id
                )));
            }
            if setting.required && value.is_empty() {
                return Err(refused(format!("the setting {} needs a value", setting.id)));
            }
            let valid = value.is_empty()
                || match setting.kind {
                    SettingKind::Number => value.parse::<f64>().is_ok_and(f64::is_finite),
                    SettingKind::Boolean => matches!(value, "true" | "false"),
                    SettingKind::Text | SettingKind::Password => true,
                };
            if !valid {
                return Err(refused(format!("the setting {} has a value of the wrong kind", setting.id)));
            }
        }

        let placement = Placement {
            project: manifest.id.clone(),
            app_data: app_data.to_string_lossy().into_owned(),
            puid: account.0.to_string(),
            pgid: account.1.to_string(),
            tz: self.time_zone(),
            shares,
            ports: values.ports.clone(),
            settings: values
                .settings
                .iter()
                .map(|(id, value)| (id.clone(), value.expose().to_owned()))
                .collect(),
        };
        Ok((placement, pins))
    }

    /// `docker compose config --format json` of `compose` with `variables`
    /// and nothing else: no environment of the agent's, no `.env`, no
    /// files beside it.
    async fn normalize(
        &self,
        app: &AppName,
        compose: &str,
        variables: BTreeMap<String, String>,
    ) -> Result<Value, OperationError> {
        let folder = EmptyFolder::new()?;
        let folder_text = folder.0.to_string_lossy().into_owned();
        let mut environment: Vec<(String, String)> = DOCKER_ENVIRONMENT
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        environment.extend(variables);
        let surroundings = Surroundings { variables: environment, directory: Some(&folder.0) };
        let args = [
            "compose",
            "--file",
            "-",
            "--project-directory",
            &folder_text,
            "--env-file",
            "/dev/null",
            "--project-name",
            app.as_str(),
            "config",
            "--format",
            "json",
        ];
        let output =
            zendiloos_exec::run_with(&self.docker, args, Some(compose.as_bytes()), &surroundings, LIMITS)
                .await?;
        if !output.status.success() {
            return Err(refused(format!(
                "Docker Compose cannot read the compose file: {}",
                complaint(&output)
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| refused(format!("Docker Compose wrote something unexpected: {e}")))
    }

    /// Checks the app with `values` and keeps its normalized compose file.
    pub(crate) async fn configure(
        &self,
        app: &AppName,
        volume: VolumeNumber,
        manifest: &AppManifest,
        compose: &AppCompose,
        values: &AppValues,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let manifest: Manifest = serde_json::from_str(manifest.as_str())
            .map_err(|e| refused(format!("the manifest cannot be read: {e}")))?;
        if manifest.id != app.as_str() {
            return Err(refused("the manifest is another app's"));
        }
        require_none(manifest.check(&manifest.id))?;
        require_none(check_source(compose.as_str(), Some(&manifest)))?;

        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let account = app_account(&passwd, &group, app)
            .ok_or_else(|| refused(format!("the account {} is missing", app.account())))?;
        let app_data = self.app_data(volume, app)?;
        let (placement, pins) = self.placement(app, &manifest, &app_data, account, values)?;

        let stand_in = Placement::stand_in(&manifest);
        let checked = self.normalize(app, compose.as_str(), stand_in.variables()).await?;
        let mut problems = check_normalized(&checked, &manifest, &stand_in);
        problems.extend(check_variables(&checked, &manifest));
        require_none(problems)?;

        let mut normalized = self.normalize(app, compose.as_str(), placement.variables()).await?;
        require_none(check_normalized(&normalized, &manifest, &placement))?;
        leave_out_missing_devices(&mut normalized, &self.paths.dev);
        let folders = folders(&normalized, &placement.app_data)?;
        let pinned: Vec<&str> = pins.iter().map(|pin| pin.at.as_str()).collect();
        refuse_to_create(&mut normalized, &pinned);
        if let Some(root) = normalized.as_object_mut() {
            let shares: Vec<&String> = placement.shares.values().collect();
            root.insert(
                EXTENSION.to_owned(),
                serde_json::json!({
                    "app_data": placement.app_data,
                    "shares": shares,
                    "pins": pins,
                    // Checked against the manifest by check_normalized.
                    "docker_socket": manifest.permissions.docker_socket,
                }),
            );
        }
        self.keep(app, &normalized)?;
        self.place_pin_unit(app, &pins).await?;
        Ok(Response::AppFolders { folders })
    }

    /// Writes the normalized compose file, readable by root only: settings
    /// may be passwords.
    fn keep(&self, app: &AppName, normalized: &Value) -> Result<(), OperationError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.paths.apps)
            .map_err(|e| OperationError::Write(self.paths.apps.clone(), e))?;
        let path = self.config_path(app);
        let temporary = path.with_extension("json.new");
        let text = serde_json::to_string_pretty(normalized).unwrap_or_default();
        let written = (|| {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })();
        written.map_err(|e| OperationError::Write(temporary.clone(), e))?;
        fs::rename(&temporary, &path).map_err(|e| OperationError::Write(path, e))
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

    pub(crate) async fn pull(&self, app: &AppName) -> Result<Response, OperationError> {
        self.compose_done(app, &["pull", "--quiet"], LONG).await
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

/// Binds of `sources` fail when the source is missing, rather than Docker
/// making an empty folder there: a pin that could not be made.
fn refuse_to_create(normalized: &mut Value, sources: &[&str]) {
    let Some(services) = normalized.get_mut("services").and_then(Value::as_object_mut) else {
        return;
    };
    for service in services.values_mut() {
        let Some(mounts) = service.get_mut("volumes").and_then(Value::as_array_mut) else { continue };
        for mount in mounts {
            let pinned = mount.get("type").and_then(Value::as_str) == Some("bind")
                && mount.get("source").and_then(Value::as_str).is_some_and(|s| sources.contains(&s));
            if let (true, Some(mount)) = (pinned, mount.as_object_mut()) {
                mount.insert("bind".to_owned(), serde_json::json!({ "create_host_path": false }));
            }
        }
    }
}

/// The folders below `app_data` the containers mount, for the files agent
/// to make.
fn folders(normalized: &Value, app_data: &str) -> Result<Vec<AppFolder>, OperationError> {
    let mut folders = BTreeSet::new();
    for source in bind_sources(normalized) {
        if let Some(folder) = below(&source, app_data) {
            folders
                .insert(AppFolder::parse(folder).map_err(|_| refused(format!("{source} is no plain path")))?);
        }
    }
    if folders.len() > AppFolder::MAX_COUNT {
        return Err(refused("the app mounts too many folders"));
    }
    Ok(folders.into_iter().collect())
}

/// Devices the machine lacks, such as `/dev/dri` without a graphics
/// card, are left out: Docker would refuse to start the container.
fn leave_out_missing_devices(normalized: &mut Value, dev: &Path) {
    let Some(services) = normalized.get_mut("services").and_then(Value::as_object_mut) else {
        return;
    };
    for service in services.values_mut() {
        let Some(service) = service.as_object_mut() else { continue };
        let Some(devices) = service.get_mut("devices").and_then(Value::as_array_mut) else { continue };
        devices.retain(|device| {
            let source = device.get("source").and_then(Value::as_str).unwrap_or_default();
            source.strip_prefix("/dev/").is_some_and(|inside| dev.join(inside).exists())
        });
        if devices.is_empty() {
            service.remove("devices");
        }
    }
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
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn variables_docker_reads_itself_are_no_settings() {
        for reserved in ["PATH", "LD_PRELOAD", "DOCKER_HOST", "COMPOSE_PROFILES", "GODEBUG", "GLIBC_TUNABLES"]
        {
            assert!(is_reserved_variable(reserved), "{reserved}");
        }
        for fine in ["DB_PASSWORD", "ADMIN_TOKEN", "PATHFINDER", "GOOGLE_KEY"] {
            assert!(!is_reserved_variable(fine), "{fine}");
        }
    }

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
    fn pinned_folders_are_never_made_by_docker() {
        let mut normalized = normalized();
        refuse_to_create(&mut normalized, &["/volume1/media"]);
        let volumes = &normalized["services"]["app"]["volumes"];
        assert_eq!(volumes[2]["bind"], json!({ "create_host_path": false }));
        assert!(volumes[0].get("bind").is_none());
    }

    #[test]
    fn folders_are_the_mounts_below_the_data_folder() {
        let made = folders(&normalized(), "/volume1/@apps/demo").unwrap();
        let names: Vec<&str> = made.iter().map(AppFolder::as_str).collect();
        assert_eq!(names, ["", "config", "db"]);
        let mut odd = normalized();
        odd["services"]["db"]["volumes"][0]["source"] = json!("/volume1/@apps/demo/a/../../etc");
        assert!(folders(&odd, "/volume1/@apps/demo").is_err());
        assert_eq!(below("/volume1/@apps/demo2", "/volume1/@apps/demo"), None);
    }

    #[test]
    fn devices_the_machine_lacks_are_left_out() {
        let dev = tempfile::tempdir().unwrap();
        fs::create_dir_all(dev.path().join("net")).unwrap();
        fs::write(dev.path().join("net/tun"), "").unwrap();
        let mut normalized = normalized();
        leave_out_missing_devices(&mut normalized, dev.path());
        assert_eq!(normalized["services"]["app"]["devices"].as_array().unwrap().len(), 1);
        assert_eq!(normalized["services"]["app"]["devices"][0]["source"], "/dev/net/tun");

        fs::remove_file(dev.path().join("net/tun")).unwrap();
        leave_out_missing_devices(&mut normalized, dev.path());
        assert!(normalized["services"]["app"].get("devices").is_none());
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
}
