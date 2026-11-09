//! Administrators' Compose projects, deployed as they confirmed them
//! (docs/architecture.md, 4.9).
//!
//! zendiloosd hands over what Compose made of the file the administrator
//! confirmed, never a path to it: the file lies in a share, where whoever
//! may write the share could change it before it runs, and Compose would
//! read a `.env`, includes and extended files from there too. What it made
//! is complete; nothing else is read.
//!
//! Every bind whose source lies in a volume is pinned (`app_shares`): the
//! containers mount the folder or file that is there, never a link put in
//! its place later, also when Docker restarts them by itself. The result is
//! kept by root as `/etc/zendiloos/projects/<project>.json`, and Compose
//! runs that file.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use tokio::time::Instant;
use zendiloos_agent_proto::{AppFolder, DOCKER_WORK, ProjectConfig, ProjectName, Response};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::app_shares::{self, Found, Owner, Pin};
use crate::apps::{DOCKER_ENVIRONMENT, EXTENSION};
use crate::operations::OperationError;
use crate::pools::HostPaths;

/// What Compose writes, kept at most.
const MAX_STDOUT: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Projects {
    docker: Program,
    systemctl: Program,
    paths: HostPaths,
    /// One change to a kept file at a time.
    serial: tokio::sync::Mutex<()>,
}

impl Projects {
    pub(crate) fn new(docker: Program, systemctl: Program, paths: HostPaths) -> Self {
        Self { docker, systemctl, paths, serial: tokio::sync::Mutex::new(()) }
    }

    /// Whether `project` was deployed here: its file is kept.
    pub(crate) fn is_kept(&self, project: &ProjectName) -> bool {
        Owner::Project(project).kept(&self.paths).exists()
    }

    /// Pins the binds of `config`, keeps it, and brings the project up from
    /// it; with `pull`, its images are fetched first. Pulling and bringing
    /// up share one budget, [`DOCKER_WORK`].
    pub(crate) async fn deploy(
        &self,
        project: &ProjectName,
        config: &ProjectConfig,
        pull: bool,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let until = Instant::now() + DOCKER_WORK;
        let owner = Owner::Project(project);
        let mut normalized: Value = serde_json::from_str(config.as_str())
            .map_err(|_| OperationError::Refused("the project cannot be read".into()))?;
        let pins = {
            let volumes = Volumes {
                root: &self.paths.volumes,
                mounted: &|path| app_shares::open_mounted(path).is_ok(),
                find: &app_shares::find,
            };
            let places = |id: &str| app_shares::pin_path(&self.paths, owner, id);
            pin_binds(&mut normalized, &volumes, &places)?
        };
        if let Some(root) = normalized.as_object_mut() {
            root.insert(EXTENSION.to_owned(), json!({ "pins": pins }));
        }
        keep(&self.paths.projects, &owner.kept(&self.paths), &normalized)?;
        app_shares::place_unit(&self.systemctl, &self.paths, owner, &pins).await?;
        if !pins.is_empty() {
            app_shares::repin(&self.systemctl, owner).await?;
        }
        // Not quiet: what Compose writes goes to whoever deploys, line by
        // line, as it pulls and makes containers.
        if pull {
            self.compose(project, &["pull"], until).await?;
        }
        self.compose(project, &["up", "--detach", "--remove-orphans"], until).await
    }

    /// Takes the project's containers and networks down and removes what
    /// deploying it kept: its file, its pins and their unit.
    pub(crate) async fn down(&self, project: &ProjectName) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let owner = Owner::Project(project);
        self.compose(project, &["down", "--remove-orphans"], Instant::now() + DOCKER_WORK).await?;
        app_shares::remove_unit(&self.systemctl, &self.paths, owner).await?;
        let kept = owner.kept(&self.paths);
        match fs::remove_file(&kept) {
            Ok(()) => Ok(Response::Done),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Response::Done),
            Err(error) => Err(OperationError::Write(kept, error)),
        }
    }

    /// `docker compose` on the kept file, nothing else read, given up at
    /// `until`; what it writes is reported line by line
    /// ([`crate::progress::line`]).
    async fn compose(
        &self,
        project: &ProjectName,
        args: &[&str],
        until: Instant,
    ) -> Result<Response, OperationError> {
        let timeout = until.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Err(OperationError::Failed(format!(
                "Docker Compose did not finish within {} minutes",
                DOCKER_WORK.as_secs() / 60
            )));
        }
        let path = Owner::Project(project).kept(&self.paths).to_string_lossy().into_owned();
        let mut all =
            vec!["compose", "--file", &path, "--env-file", "/dev/null", "--project-name", project.as_str()];
        all.extend_from_slice(args);
        let surroundings = Surroundings {
            variables: DOCKER_ENVIRONMENT.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect(),
            directory: None,
        };
        let limits = Limits { timeout, max_stdout: MAX_STDOUT };
        let output =
            zendiloos_exec::run_lines(&self.docker, &all, &surroundings, limits, &mut crate::progress::line)
                .await?;
        if output.status.success() {
            return Ok(Response::Done);
        }
        let lines: Vec<&str> = output.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
        let tail = lines[lines.len().saturating_sub(5)..].join("\n");
        Err(OperationError::Failed(format!("Docker Compose: {tail}")))
    }
}

/// How binds' sources in volumes are looked at: through the file system,
/// or in tests through stand-ins.
struct Volumes<'a> {
    /// Where the `/volume<N>` directories are: `/`.
    root: &'a Path,
    /// Whether a file system is mounted at the path.
    mounted: &'a dyn Fn(&Path) -> bool,
    /// What is at a folder of a mounted share or volume.
    find: &'a dyn Fn(&Path, &AppFolder) -> Result<Found, OperationError>,
}

/// Points every bind of `normalized` whose source lies in a volume at a pin
/// of it, and says which. The same source is pinned once. A missing source
/// is made at pinning when the bind may create it, as Docker would.
fn pin_binds(
    normalized: &mut Value,
    volumes: &Volumes<'_>,
    places: &dyn Fn(&str) -> Result<PathBuf, OperationError>,
) -> Result<Vec<Pin>, OperationError> {
    let mut pins: Vec<Pin> = Vec::new();
    let mut by_source: BTreeMap<String, String> = BTreeMap::new();
    let mut pin = |source: &str, create: bool| -> Result<Option<String>, OperationError> {
        if let Some(at) = by_source.get(source) {
            return Ok(Some(at.clone()));
        }
        // What the kernel would make of it, a folder in a share among it,
        // is not what it says: a relative path starts where Docker runs
        // (`/`), and `..` may lead from the system into a volume.
        let path = Path::new(source);
        if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
            return Err(OperationError::Refused(format!(
                "{source} cannot be pinned: write it as a path from / without .."
            )));
        }
        let Some((share, folder)) = locate(source, volumes)? else { return Ok(None) };
        let (file, create) = match (volumes.find)(&share, &folder)? {
            Found::Folder => (false, false),
            Found::File => (true, false),
            Found::Missing if create => (false, true),
            Found::Missing => return Err(OperationError::Refused(format!("{source} does not exist"))),
        };
        let at = places(&pin_id(&share, &folder, file))?.to_string_lossy().into_owned();
        if pins.iter().any(|pin| pin.at == at) {
            return Err(OperationError::Refused(format!("two binds would share the place {at}")));
        }
        pins.push(Pin { share: share.to_string_lossy().into_owned(), folder, at: at.clone(), file, create });
        by_source.insert(source.to_owned(), at.clone());
        Ok(Some(at))
    };

    for service in
        normalized.get_mut("services").and_then(Value::as_object_mut).into_iter().flat_map(|s| s.values_mut())
    {
        for volume in service.get_mut("volumes").and_then(Value::as_array_mut).into_iter().flatten() {
            if volume.get("type").and_then(Value::as_str) != Some("bind") {
                continue;
            }
            let Some(source) = volume.get("source").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let create = volume.pointer("/bind/create_host_path").and_then(Value::as_bool).unwrap_or(true);
            let pinned = pin(&source, create)?;
            let on_a_volume = pinned.is_some() || app_shares::on_a_volume(&source);
            if let Some(at) = pinned {
                volume["source"] = Value::String(at);
            }
            // A pin is made before; a share's or volume's root is there once
            // mounted, and Docker must not make it on the system's disk.
            if on_a_volume {
                let bind = volume.as_object_mut().map(|v| v.entry("bind").or_insert_with(|| json!({})));
                if let Some(bind) = bind.and_then(Value::as_object_mut) {
                    bind.insert("create_host_path".into(), Value::Bool(false));
                }
            }
        }
    }
    // A named volume that is a bind in disguise: the local driver with
    // `o: bind` or `rbind` mounts its `device` as a bind mount does. Compose
    // makes a relative device absolute only for `o: bind` itself.
    for volume in
        normalized.get_mut("volumes").and_then(Value::as_object_mut).into_iter().flat_map(|v| v.values_mut())
    {
        let Some(options) = volume.get_mut("driver_opts").and_then(Value::as_object_mut) else { continue };
        let binds = options.get("o").and_then(Value::as_str).is_some_and(binds_a_folder);
        let Some(device) = options.get("device").and_then(Value::as_str).map(str::to_owned) else { continue };
        if binds && let Some(at) = pin(&device, false)? {
            options.insert("device".into(), Value::String(at));
        }
    }
    // Secrets and configs from files are bound into the containers too.
    for kind in ["secrets", "configs"] {
        for entry in
            normalized.get_mut(kind).and_then(Value::as_object_mut).into_iter().flat_map(|v| v.values_mut())
        {
            let Some(file) = entry.get("file").and_then(Value::as_str).map(str::to_owned) else { continue };
            if let Some(at) = pin(&file, false)? {
                entry["file"] = Value::String(at);
            }
        }
    }
    Ok(pins)
}

/// Whether the local driver's mount options `o` make it a bind mount, as
/// Docker reads them: `bind`, or `rbind` with what is mounted beneath.
pub(crate) fn binds_a_folder(options: &str) -> bool {
    options.split(',').any(|option| matches!(option.trim(), "bind" | "rbind"))
}

/// What `normalized` binds that a deployment here would pin or refuse:
/// a folder or file below the root of a share or volume (`/volume1/media/
/// Films`), which whoever may write there could swap for a link Docker
/// follows, and a path the kernel would not take as it reads. A project
/// made elsewhere runs from its own files, by those paths.
pub(crate) fn needing_pins(normalized: &Value) -> Vec<String> {
    let services = normalized.get("services").and_then(Value::as_object).into_iter().flat_map(|s| s.values());
    let binds = services
        .flat_map(|service| service.get("volumes").and_then(Value::as_array).into_iter().flatten())
        .filter(|volume| volume.get("type").and_then(Value::as_str) == Some("bind"))
        .filter_map(|volume| volume.get("source").and_then(Value::as_str));
    let volumes = normalized.get("volumes").and_then(Value::as_object).into_iter().flat_map(|v| v.values());
    let bound = volumes.filter_map(|volume| {
        let options = volume.get("driver_opts")?;
        options.get("o")?.as_str().filter(|o| binds_a_folder(o))?;
        options.get("device")?.as_str()
    });
    let files = ["secrets", "configs"]
        .into_iter()
        .flat_map(|kind| normalized.get(kind).and_then(Value::as_object).into_iter().flat_map(|v| v.values()))
        .filter_map(|entry| entry.get("file").and_then(Value::as_str));
    let mut found: Vec<String> = Vec::new();
    for source in binds.chain(bound).chain(files) {
        let path = Path::new(source);
        let odd = !path.is_absolute() || path.components().any(|part| part == Component::ParentDir);
        let beneath = source
            .strip_prefix("/volume")
            .and_then(|rest| rest.split_once('/'))
            .filter(|(number, _)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
            .is_some_and(|(_, rest)| rest.split('/').filter(|name| !name.is_empty()).count() > 1);
        if (odd || beneath) && !found.iter().any(|f| f == source) {
            found.push(source.to_owned());
        }
    }
    found
}

/// A pin's name: from what it pins, so that a bind whose source changed gets
/// another pin and Docker Compose, seeing another source, makes its
/// container again instead of keeping it on what it had mounted.
fn pin_id(share: &Path, folder: &AppFolder, file: bool) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(share.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    hasher.update(folder.as_str().as_bytes());
    hasher.update(if file { "\0file" } else { "\0folder" });
    hasher.finalize()[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The mounted file system a source in a volume lies on (the share's
/// dataset, else the volume's) and the path in it; `None` for a source
/// outside the volumes, or the root of a share or volume, which nobody but
/// root can swap.
fn locate(source: &str, volumes: &Volumes<'_>) -> Result<Option<(PathBuf, AppFolder)>, OperationError> {
    let Ok(inside) = Path::new(source).strip_prefix(volumes.root) else { return Ok(None) };
    let names: Vec<&str> = inside.iter().filter_map(|n| n.to_str()).collect();
    let Some((volume, rest)) = names.split_first() else { return Ok(None) };
    let numbered =
        volume.strip_prefix("volume").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if !numbered || names.len() != inside.iter().count() {
        return Ok(None);
    }
    let volume = volumes.root.join(volume);
    let (share, folder) = match rest.split_first() {
        None => return Ok(None),
        Some((share, folder)) if (volumes.mounted)(&volume.join(share)) => (volume.join(share), folder),
        Some(_) => (volume.clone(), rest),
    };
    if folder.is_empty() {
        return Ok(None);
    }
    if !(volumes.mounted)(&share) {
        return Err(OperationError::Refused(format!("{} is not mounted", share.display())));
    }
    let folder = AppFolder::parse(&folder.join("/"))
        .map_err(|_| OperationError::Refused(format!("{source} cannot be pinned: an odd path")))?;
    Ok(Some((share, folder)))
}

/// Writes `normalized` to `path` in `folder`, readable by root only, whole
/// or not at all.
fn keep(folder: &Path, path: &Path, normalized: &Value) -> Result<(), OperationError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|e| OperationError::Write(folder.to_owned(), e))?;
    let temporary = path.with_extension("json.new");
    let text = serde_json::to_string_pretty(normalized).unwrap_or_default();
    let written = (|| {
        use std::io::Write;
        let mut file =
            fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    written.map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))?;
    if let Ok(directory) = fs::File::open(folder) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Volume 1 with the shares `docker` and `media` mounted; what is in
    /// them by path.
    fn pinned(mut normalized: Value) -> Result<(Value, Vec<Pin>), OperationError> {
        let mounted =
            |path: &Path| ["/volume1", "/volume1/docker", "/volume1/media"].contains(&path.to_str().unwrap());
        let looked = RefCell::new(Vec::new());
        let find = |share: &Path, folder: &AppFolder| {
            looked.borrow_mut().push(format!("{}/{}", share.display(), folder.as_str()));
            Ok(match folder.as_str() {
                "app/nginx.conf" | "app/secret.txt" => Found::File,
                "app/new" | "missing" => Found::Missing,
                "app/link" => return Err(OperationError::Refused("the folder app/link is a link".into())),
                _ => Found::Folder,
            })
        };
        let volumes = Volumes { root: Path::new("/"), mounted: &mounted, find: &find };
        let places = |id: &str| Ok(PathBuf::from(format!("/run/zendiloos-project-shares/app/{id}")));
        let pins = pin_binds(&mut normalized, &volumes, &places)?;
        Ok((normalized, pins))
    }

    fn bind(source: &str) -> Value {
        json!({ "type": "bind", "source": source, "target": "/x", "bind": { "create_host_path": true } })
    }

    #[test]
    fn binds_in_shares_mount_their_pins() {
        let (normalized, pins) = pinned(json!({ "services": {
            "web": { "image": "nginx", "volumes": [
                bind("/volume1/docker/app/data"),
                bind("/volume1/docker/app/nginx.conf"),
                bind("/volume1/media/Films"),
                bind("/volume1/docker/app/data"),
            ] },
            "db": { "image": "postgres", "volumes": [
                { "type": "volume", "source": "db", "target": "/var/lib/postgresql/data" },
                bind("/etc/localtime"),
                bind("/volume1/docker"),
            ] },
        } }))
        .unwrap();
        let sources: Vec<&str> = normalized["services"]["web"]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["source"].as_str().unwrap())
            .collect();
        let at: Vec<&str> = pins.iter().map(|pin| pin.at.as_str()).collect();
        assert_eq!(sources, [at[0], at[1], at[2], at[0]], "the same source is pinned once");
        assert!(at.iter().all(|at| at.starts_with("/run/zendiloos-project-shares/app/")), "{at:?}");
        assert_eq!(normalized["services"]["web"]["volumes"][0]["bind"]["create_host_path"], false);
        let db = &normalized["services"]["db"]["volumes"];
        assert_eq!(db[1]["source"], "/etc/localtime", "outside the volumes: left alone");
        assert_eq!(db[2]["source"], "/volume1/docker", "a share's root cannot be swapped");
        assert_eq!(db[2]["bind"]["create_host_path"], false, "nor made by Docker");
        assert_eq!(db[1]["bind"]["create_host_path"], true, "outside the volumes as it was");
        assert_eq!(pins.len(), 3);
        assert_eq!(
            (pins[0].share.as_str(), pins[0].folder.as_str(), pins[0].file),
            ("/volume1/docker", "app/data", false)
        );
        assert!(pins[1].file, "a file is pinned as a file");
        assert_eq!(pins[2].share, "/volume1/media");
    }

    #[test]
    fn a_pin_is_named_by_what_it_pins_not_by_its_place_in_the_file() {
        let at = |sources: &[&str]| -> Vec<String> {
            let volumes = sources.iter().map(|source| bind(source)).collect::<Vec<_>>();
            let (_, pins) =
                pinned(json!({ "services": { "web": { "image": "nginx", "volumes": volumes } } })).unwrap();
            pins.into_iter().map(|pin| pin.at).collect()
        };
        let before = at(&["/volume1/docker/app/data", "/volume1/media/Films"]);
        let changed = at(&["/volume1/docker/app/other", "/volume1/media/Films"]);
        let reordered = at(&["/volume1/media/Films", "/volume1/docker/app/data"]);
        assert_ne!(before[0], changed[0], "another source, another pin: the container is made again");
        assert_eq!(before[1], changed[1]);
        assert_eq!((before[0].as_str(), before[1].as_str()), (reordered[1].as_str(), reordered[0].as_str()));
    }

    #[test]
    fn a_missing_source_is_made_only_where_the_bind_may_create_it() {
        let (_, pins) =
            pinned(json!({ "services": { "web": { "volumes": [bind("/volume1/docker/app/new")] } } }))
                .unwrap();
        assert!(pins[0].create && !pins[0].file);

        let strict = json!({ "type": "bind", "source": "/volume1/docker/missing", "target": "/x",
                              "bind": { "create_host_path": false } });
        let error = pinned(json!({ "services": { "web": { "volumes": [strict] } } })).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
    }

    #[test]
    fn a_link_in_a_share_is_refused() {
        let error =
            pinned(json!({ "services": { "web": { "volumes": [bind("/volume1/docker/app/link")] } } }))
                .unwrap_err();
        assert!(error.to_string().contains("is a link"), "{error}");
    }

    #[test]
    fn folders_on_the_volume_itself_are_pinned_from_the_volume() {
        let (_, pins) =
            pinned(json!({ "services": { "web": { "volumes": [bind("/volume1/stuff/data")] } } })).unwrap();
        assert_eq!((pins[0].share.as_str(), pins[0].folder.as_str()), ("/volume1", "stuff/data"));
        let error =
            pinned(json!({ "services": { "web": { "volumes": [bind("/volume2/x/y")] } } })).unwrap_err();
        assert!(error.to_string().contains("/volume2 is not mounted"), "{error}");
    }

    #[test]
    fn named_volumes_secrets_and_configs_from_shares_are_pinned_too() {
        let (normalized, pins) = pinned(json!({
            "services": { "web": { "image": "nginx" } },
            "volumes": {
                "data": { "driver": "local", "driver_opts": { "type": "none", "o": "bind", "device": "/volume1/docker/app/data" } },
                "nfs": { "driver": "local", "driver_opts": { "type": "nfs", "o": "addr=10.0.0.2", "device": ":/export" } },
            },
            "secrets": { "token": { "file": "/volume1/docker/app/secret.txt" } },
            "configs": { "site": { "file": "/volume1/docker/app/nginx.conf" } },
        }))
        .unwrap();
        assert_eq!(pins.len(), 3);
        assert_eq!(normalized["volumes"]["data"]["driver_opts"]["device"], pins[0].at.as_str());
        assert_eq!(normalized["volumes"]["nfs"]["driver_opts"]["device"], ":/export");
        assert_eq!(normalized["secrets"]["token"]["file"], pins[1].at.as_str());
        assert_eq!(normalized["configs"]["site"]["file"], pins[2].at.as_str());
    }

    #[test]
    fn named_volumes_that_bind_recursively_are_pinned_as_binds_are() {
        let volume =
            |o: &str, device: &str| json!({ "driver_opts": { "type": "none", "o": o, "device": device } });
        let (normalized, pins) = pinned(json!({
            "services": { "web": { "image": "nginx" } },
            "volumes": {
                "data": volume("rbind", "/volume1/docker/app/data"),
                "films": volume("ro, rbind", "/volume1/media/Films"),
                "root": volume("rbind", "/"),
            },
        }))
        .unwrap();
        assert_eq!(pins.len(), 2, "{pins:?}");
        assert_eq!(normalized["volumes"]["data"]["driver_opts"]["device"], pins[0].at.as_str());
        assert_eq!(normalized["volumes"]["films"]["driver_opts"]["device"], pins[1].at.as_str());
        assert_eq!(
            normalized["volumes"]["root"]["driver_opts"]["device"], "/",
            "outside the volumes: confirmed"
        );
        assert!(binds_a_folder("bind") && binds_a_folder("rbind,ro") && !binds_a_folder("rbinds"));
    }

    #[test]
    fn what_reaches_a_volume_some_other_way_is_refused() {
        // A relative device starts where Docker runs, `/`; `..` leads from
        // the system into a share.
        for device in ["volume1/media/Films", "/etc/../volume1/media/Films"] {
            let named = json!({
                "services": { "web": { "image": "nginx" } },
                "volumes": { "v": { "driver_opts": { "type": "none", "o": "rbind", "device": device } } },
            });
            let error = pinned(named).unwrap_err();
            assert!(error.to_string().contains("cannot be pinned"), "{device}: {error}");
        }
        let error =
            pinned(json!({ "services": { "web": { "volumes": [bind("/srv/../volume1/media/x")] } } }))
                .unwrap_err();
        assert!(error.to_string().contains("cannot be pinned"), "{error}");
    }

    #[test]
    fn what_a_project_made_elsewhere_would_mount_unpinned_is_found() {
        let normalized = json!({
            "services": {
                "web": { "volumes": [
                    bind("/volume1/media"),
                    bind("/volume1/media/Films"),
                    bind("/etc/localtime"),
                    { "type": "volume", "source": "data", "target": "/data" },
                ] },
                "db": { "volumes": [bind("/volume1/media/Films"), bind("/srv/../volume2/x/y")] },
            },
            "volumes": {
                "data": {},
                "music": { "driver_opts": { "type": "none", "o": "rbind", "device": "/volume1/media/Music" } },
                "root": { "driver_opts": { "type": "none", "o": "bind", "device": "/" } },
                "nfs": { "driver_opts": { "type": "nfs", "o": "addr=10.0.0.2", "device": ":/volume1/a/b" } },
            },
            "secrets": { "token": { "file": "/volume1/docker/app/token" } },
        });

        assert_eq!(
            needing_pins(&normalized),
            [
                "/volume1/media/Films",
                "/srv/../volume2/x/y",
                "/volume1/media/Music",
                "/volume1/docker/app/token"
            ],
            "a share's root, the system's folders and another machine's export are left as they are"
        );
    }

    #[test]
    fn odd_paths_are_not_volumes() {
        let volumes = Volumes { root: Path::new("/"), mounted: &|_| true, find: &|_, _| Ok(Found::Folder) };
        for outside in ["/etc/hosts", "/volumes/x", "/volume/x", "/volumeX/x", "/volume1", "relative/x"] {
            assert!(locate(outside, &volumes).unwrap().is_none(), "{outside}");
        }
        assert!(locate("/volume1/docker/a/../b", &volumes).is_err(), "no way out with ..");
    }

    #[tokio::test]
    async fn pull_and_up_share_one_budget() {
        use std::os::unix::fs::PermissionsExt;

        let folder = tempfile::tempdir().unwrap();
        let log = folder.path().join("calls.log");
        let docker = folder.path().join("docker");
        fs::write(&docker, format!("#!/bin/sh\necho \"$*\" >> {}\nsleep 5\n", log.display())).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        let paths = HostPaths { projects: folder.path().join("projects"), ..HostPaths::default() };
        let projects =
            Projects::new(Program::new(&docker).unwrap(), Program::new("/bin/true").unwrap(), paths);
        let project = ProjectName::parse("web").unwrap();

        // A pull that takes the budget up: given up when it is spent, and
        // nothing is left to bring the containers up with.
        let began = std::time::Instant::now();
        let until = Instant::now() + std::time::Duration::from_millis(300);
        assert!(projects.compose(&project, &["pull"], until).await.is_err());
        assert!(began.elapsed() < std::time::Duration::from_secs(4), "not waited for");
        let up = projects.compose(&project, &["up", "--detach"], until).await.unwrap_err();
        assert!(up.to_string().contains("did not finish within 30 minutes"), "{up}");
        let calls = fs::read_to_string(&log).unwrap();
        assert_eq!(calls.lines().filter(|call| call.ends_with(" up --detach")).count(), 0, "{calls}");
    }

    #[test]
    fn the_kept_file_is_roots_alone() {
        let folder = tempfile::tempdir().unwrap();
        let projects = folder.path().join("projects");
        let path = projects.join("app.json");
        keep(&projects, &path, &json!({ "services": {} })).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&projects).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!projects.join("app.json.new").exists());
    }
}
