//! Docker's volumes (docs/architecture.md, 4.9): listed as Docker inspects
//! them, their data counted in Docker's data root, removed, made, and
//! backed up and restored.
//!
//! The agent checks what a backup or a restore is asked for itself: the
//! volume is a local one whose data lies in Docker's data root (`docker
//! volume inspect`, against `docker info`), the share is mounted, no running
//! container uses a volume being restored. The work then runs in a
//! transient unit (`systemd-run`), this binary as `backup-volume`,
//! `restore-volume`, `swap-volume` or `remove-volume-backup`
//! ([`crate::volume_archive`]), outside the agent's sandbox, which can write
//! neither shares nor Docker's data, in one of its own: no network, no
//! devices, the whole system read only except the share (backups) or the
//! volume's folder (restores), and only the capabilities that work needs.
//!
//! Each unit has a name of its own (`zendiloos-volume-<work>-<volume>`), so
//! systemd refuses a second one for the same volume, and a time it may run
//! for that ends before the agent stops waiting. The agent answers only once
//! its unit ended: should the client hang up, the agent be asked to stop or
//! the time run out, it stops the unit and waits for it first. A restore is
//! two units: the first extracts beside the data, then the agent looks for
//! running containers once more, and only if none uses the volume does the
//! second swap the data in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zendiloos_agent_proto::{
    ArchiveDigest, BackupStem, Response, ShareLocation, SharePath, VolumeLabels, VolumeName, VolumeSize,
};
use zendiloos_exec::{ExecError, Limits, Program, Surroundings};

use crate::docker::{complaint, surroundings};
use crate::operations::OperationError;
use crate::pools::HostPaths;
use crate::volume_archive::{
    BackupDone, BackupJob, Extracted, RemoveJob, RestoreJob, SwapJob, Swapped, UnitLine, VolumeJob, encode,
};

const QUICK: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 16 * 1024 * 1024 };
/// Removing many volumes, or large ones, takes a while.
const LONG: Limits = Limits { timeout: Duration::from_secs(30 * 60), max_stdout: 1024 * 1024 };
/// What a backup or a restore of a large volume may take in all, its
/// units' time included; zendiloosd waits a little longer.
const WORK_FOR: Duration = Duration::from_secs(6 * 3600);
/// What a unit leaves of that, to be stopped and answered: it may run
/// until then.
const STOP_MARGIN: Duration = Duration::from_secs(10 * 60);
/// What a restore's extraction leaves to the swap after it.
const SWAP_RESERVE: Duration = Duration::from_secs(30 * 60);
/// Removing a backup's file.
const REMOVE_FOR: Duration = Duration::from_secs(30 * 60);
/// A unit asked to stop is killed after this.
const STOP_TIMEOUT: &str = "--property=TimeoutStopSec=60";
/// `systemctl stop`, which waits for the unit to end.
const STOPPING: Limits = Limits { timeout: Duration::from_secs(3 * 60), max_stdout: 64 * 1024 };
/// Counting every volume's data stops after this…
const COUNT_FOR: Duration = Duration::from_secs(20);
/// …or after this many entries in one volume.
const MAX_COUNTED: u64 = 1_000_000;

/// What every unit's sandbox has.
const SANDBOX: [&str; 19] = [
    "--property=NoNewPrivileges=yes",
    "--property=ProtectSystem=strict",
    "--property=ProtectHome=yes",
    "--property=PrivateTmp=yes",
    "--property=PrivateDevices=yes",
    "--property=PrivateNetwork=yes",
    "--property=ProtectKernelTunables=yes",
    "--property=ProtectKernelModules=yes",
    "--property=ProtectKernelLogs=yes",
    "--property=ProtectControlGroups=yes",
    "--property=ProtectClock=yes",
    "--property=ProtectHostname=yes",
    "--property=RestrictNamespaces=yes",
    "--property=RestrictRealtime=yes",
    "--property=LockPersonality=yes",
    "--property=MemoryDenyWriteExecute=yes",
    "--property=RestrictAddressFamilies=AF_UNIX",
    "--property=SystemCallArchitectures=native",
    "--property=MemoryMax=1G",
];
/// A backup reads everything in the volume and writes the share, whoever
/// owns what is there.
const BACKUP_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE";
/// A restore gives each file its owner, mode, times and attributes back.
const RESTORE_CAPABILITIES: &str =
    "CAP_CHOWN CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH CAP_FOWNER CAP_FSETID CAP_SETFCAP";

/// A local volume whose data lies in Docker's data root, as Docker inspected it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalVolume {
    /// `<data root>/volumes/<name>/_data`.
    data: PathBuf,
    driver: String,
    labels: BTreeMap<String, String>,
    created: String,
}

/// Only paths systemd and the agent read as they are: letters, digits and
/// `/._-@+`.
fn plain(path: &Path) -> bool {
    path.is_absolute()
        && path
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-@+".contains(b))
}

fn refused(reason: String) -> OperationError {
    OperationError::Refused(reason)
}

/// What a unit's step is called for the interface.
fn step_name(step: &str) -> &'static str {
    match step {
        "backing_up" => "backing_up",
        "verifying" => "verifying",
        "restoring" => "restoring",
        _ => "working",
    }
}

/// The unit `work` of `volume` runs in: one at a time, as systemd refuses a
/// second unit of a name.
fn unit_name(work: VolumeJob, volume: &VolumeName) -> String {
    format!("zendiloos-volume-{}-{}.service", work.word(), volume.as_str())
}

/// Every unit that works on `volume`'s data.
fn units_of(volume: &VolumeName) -> [String; 3] {
    [VolumeJob::BackUp, VolumeJob::Restore, VolumeJob::Swap].map(|work| unit_name(work, volume))
}

/// A unit to run: what it is, where it may write, with which
/// capabilities, until when.
struct Unit<'a> {
    work: VolumeJob,
    name: String,
    writable: &'a Path,
    capabilities: &'a str,
    description: String,
    until: Instant,
}

#[derive(Debug)]
pub(crate) struct Volumes {
    docker: Program,
    systemd_run: Program,
    systemctl: Program,
    paths: HostPaths,
}

impl Volumes {
    pub(crate) fn new(docker: Program, systemd_run: Program, systemctl: Program, paths: HostPaths) -> Self {
        Self { docker, systemd_run, systemctl, paths }
    }

    /// What `docker` printed, if it succeeded.
    async fn printed(&self, args: &[&str], limits: Limits) -> Result<String, OperationError> {
        let output = zendiloos_exec::run_with(&self.docker, args, None, &surroundings(), limits).await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!("Docker: {}", complaint(&output))));
        }
        String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)
    }

    /// `docker volume inspect` of every volume: a JSON array.
    async fn inspected(&self) -> Result<String, OperationError> {
        let names = self.printed(&["volume", "ls", "--quiet"], QUICK).await?;
        let names: Vec<&str> = names.split_whitespace().collect();
        if names.is_empty() {
            return Ok("[]".to_owned());
        }
        let args: Vec<&str> = ["volume", "inspect"].into_iter().chain(names).collect();
        self.printed(&args, QUICK).await
    }

    pub(crate) async fn list(&self) -> Result<Response, OperationError> {
        Ok(Response::DockerOutput { text: self.inspected().await? })
    }

    /// Docker's data root, as the engine says.
    async fn data_root(&self) -> Result<PathBuf, OperationError> {
        let printed = self.printed(&["info", "--format", "{{.DockerRootDir}}"], QUICK).await?;
        let root = PathBuf::from(printed.trim());
        if !plain(&root) {
            return Err(refused(format!("Docker's data root {} is no plain path", root.display())));
        }
        Ok(root)
    }

    /// Where a local volume's data is, if it is one whose data lies in
    /// Docker's data root.
    fn local(root: &Path, value: &Value) -> Result<LocalVolume, String> {
        let name = value["Name"].as_str().unwrap_or_default();
        let driver = value["Driver"].as_str().unwrap_or_default();
        if driver != "local" {
            return Err(format!(
                "{name} is a volume of the driver {driver}: only local volumes are backed up"
            ));
        }
        if value["Options"].as_object().is_some_and(|options| !options.is_empty()) {
            return Err(format!(
                "{name} mounts something else (it has options): its data is not in Docker's folder"
            ));
        }
        let data = root.join("volumes").join(name).join("_data");
        if value["Mountpoint"].as_str() != Some(data.to_string_lossy().as_ref()) || !plain(&data) {
            return Err(format!("{name}'s data is not in Docker's data root"));
        }
        let labels = value["Labels"]
            .as_object()
            .map(|labels| {
                labels.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect()
            })
            .unwrap_or_default();
        let created = value["CreatedAt"].as_str().unwrap_or_default().to_owned();
        Ok(LocalVolume { data, driver: driver.to_owned(), labels, created })
    }

    async fn inspect(&self, volume: &VolumeName) -> Result<LocalVolume, OperationError> {
        let root = self.data_root().await?;
        let text = self.printed(&["volume", "inspect", volume.as_str()], QUICK).await?;
        let values: Vec<Value> = serde_json::from_str(&text)
            .map_err(|e| OperationError::Failed(format!("Docker inspects the volume unreadably: {e}")))?;
        let value = values
            .iter()
            .find(|value| value["Name"].as_str() == Some(volume.as_str()))
            .ok_or_else(|| OperationError::Failed(format!("Docker: no such volume: {}", volume.as_str())))?;
        Self::local(&root, value).map_err(refused)
    }

    /// What each local volume's data takes up: a bounded walk each.
    pub(crate) async fn sizes(&self) -> Result<Response, OperationError> {
        let root = self.data_root().await?;
        let values: Vec<Value> = serde_json::from_str(&self.inspected().await?)
            .map_err(|e| OperationError::Failed(format!("Docker inspects the volumes unreadably: {e}")))?;
        let volumes: Vec<(String, PathBuf)> = values
            .iter()
            .filter_map(|value| {
                let local = Self::local(&root, value).ok()?;
                Some((value["Name"].as_str()?.to_owned(), local.data))
            })
            .collect();
        let sizes = tokio::task::spawn_blocking(move || {
            let until = Instant::now() + COUNT_FOR;
            volumes
                .into_iter()
                .map(|(name, data)| {
                    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                    let counted = rustix::fs::open(&data, flags, Mode::empty())
                        .map(|folder| crate::volume_archive::count(&folder, MAX_COUNTED, Some(until)))
                        .unwrap_or_default();
                    VolumeSize { name, bytes: counted.bytes, complete: counted.complete }
                })
                .collect()
        })
        .await
        .map_err(|e| OperationError::Failed(format!("counting the volumes failed: {e}")))?;
        Ok(Response::VolumeSizes { sizes })
    }

    pub(crate) async fn remove(&self, name: &VolumeName) -> Result<Response, OperationError> {
        // Without --force: Docker refuses a volume a container uses.
        self.printed(&["volume", "rm", name.as_str()], LONG).await?;
        Ok(Response::Done)
    }

    pub(crate) async fn prune(&self) -> Result<Response, OperationError> {
        let text = self.printed(&["volume", "prune", "--all", "--force"], LONG).await?;
        Ok(Response::DockerOutput { text })
    }

    pub(crate) async fn create(
        &self,
        name: &VolumeName,
        labels: &VolumeLabels,
    ) -> Result<Response, OperationError> {
        let labels: Vec<String> =
            labels.as_map().iter().map(|(key, value)| format!("--label={key}={value}")).collect();
        let mut args = vec!["volume", "create", "--driver", "local"];
        args.extend(labels.iter().map(String::as_str));
        args.push(name.as_str());
        self.printed(&args, QUICK).await?;
        Ok(Response::Done)
    }

    /// The share's root, `/volume<N>/<share>`: only while its file system
    /// is mounted there, or what is written would land on the volume.
    fn share_root(&self, share: &ShareLocation) -> Result<PathBuf, OperationError> {
        if self.paths.share_mounts {
            zendiloos_fileops::ShareRoot::open(&self.paths.volumes, share.volume, &share.share, true)
                .map_err(|e| refused(format!("the share {} cannot be used: {e}", share.share.as_str())))?;
        }
        let root =
            self.paths.volumes.join(format!("volume{}", share.volume.get())).join(share.share.as_str());
        if !plain(&root) {
            return Err(refused(format!("{} is no plain path", root.display())));
        }
        Ok(root)
    }

    /// The units of `names` that run (starting, running or stopping).
    async fn running(&self, names: &[String]) -> Result<Vec<String>, OperationError> {
        let mut args = vec!["show", "--property=ActiveState", "--value"];
        args.extend(names.iter().map(String::as_str));
        let output =
            zendiloos_exec::run_with(&self.systemctl, &args, None, &Surroundings::default(), QUICK).await?;
        if !output.status.success() {
            return Err(OperationError::Failed(format!(
                "systemd does not say whether the volume's work runs: {}",
                output.stderr.trim()
            )));
        }
        let states = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        let states: Vec<&str> = states.lines().map(str::trim).collect();
        if states.len() != names.len() {
            return Err(OperationError::Failed("systemd answered for other units than asked".into()));
        }
        Ok(names
            .iter()
            .zip(states)
            .filter(|(_, state)| !matches!(*state, "inactive" | "failed" | ""))
            .map(|(name, _)| name.clone())
            .collect())
    }

    /// Refused while a backup, restore or swap of `volume` still runs:
    /// one zendiloosd gave up on, or one a restart of the agent left.
    async fn not_busy(&self, volume: &VolumeName) -> Result<(), OperationError> {
        let running = self.running(&units_of(volume)).await?;
        if running.is_empty() {
            return Ok(());
        }
        Err(OperationError::Busy(format!(
            "a backup or restore of the volume {} still runs ({})",
            volume.as_str(),
            running.join(", ")
        )))
    }

    /// Stops the unit `name` and waits until it ended; a unit that is not
    /// there is no matter.
    async fn stop_unit(&self, name: &str) {
        let stopped = zendiloos_exec::run_with(
            &self.systemctl,
            ["stop", name],
            None,
            &Surroundings::default(),
            STOPPING,
        )
        .await;
        match stopped {
            Ok(output) if output.status.success() => {
                tracing::info!(unit = name, "a volume's unit was stopped")
            }
            Ok(output) => tracing::warn!(
                unit = name,
                complaint = %output.stderr.trim(),
                "cannot stop a volume's unit"
            ),
            Err(error) => tracing::warn!(unit = name, %error, "cannot stop a volume's unit"),
        }
    }

    /// Stops whatever still works on `volume`'s data and answers once
    /// nothing does.
    pub(crate) async fn end_work(&self, volume: &VolumeName) -> Result<Response, OperationError> {
        let names = units_of(volume);
        let running = self.running(&names).await?;
        for name in &running {
            self.stop_unit(name).await;
        }
        let still = self.running(&names).await?;
        if !still.is_empty() {
            return Err(OperationError::Failed(format!("{} still runs", still.join(", "))));
        }
        Ok(Response::VolumeWorkEnded { stopped: !running.is_empty() })
    }

    /// Refused while a running container uses `volume`.
    async fn none_running(&self, volume: &VolumeName) -> Result<(), OperationError> {
        let filter = format!("volume={}", volume.as_str());
        let running = self.printed(&["ps", "--quiet", "--no-trunc", "--filter", &filter], QUICK).await?;
        if running.trim().is_empty() {
            return Ok(());
        }
        Err(OperationError::Busy(format!(
            "running containers use the volume {}: stop them first",
            volume.as_str()
        )))
    }

    /// Runs `job` in the transient unit `unit` says; what it did, as it
    /// says. The unit ends before the agent answers: should the client hang
    /// up, the agent be asked to stop, or the time run out, it is stopped
    /// and waited for first.
    async fn unit<T: DeserializeOwned>(
        &self,
        unit: Unit<'_>,
        job: &impl Serialize,
    ) -> Result<T, OperationError> {
        let left = unit.until.saturating_duration_since(Instant::now());
        let Some(runs_for) = left.checked_sub(STOP_MARGIN).filter(|time| *time >= Duration::from_secs(60))
        else {
            return Err(OperationError::Failed("no time is left for the work".into()));
        };
        let agent = self.paths.agent.to_string_lossy().into_owned();
        let name = format!("--unit={}", unit.name);
        let description = format!("--description=zendiloOS: {}", unit.description);
        let writable = format!("--property=ReadWritePaths={}", unit.writable.display());
        let capabilities = format!("--property=CapabilityBoundingSet={}", unit.capabilities);
        let runtime = format!("--property=RuntimeMaxSec={}", runs_for.as_secs());
        let hex = encode(job);
        let mut args = vec![name.as_str(), "--wait", "--pipe", "--collect", "--quiet", description.as_str()];
        args.extend(SANDBOX);
        args.extend([
            writable.as_str(),
            capabilities.as_str(),
            runtime.as_str(),
            STOP_TIMEOUT,
            agent.as_str(),
            unit.work.command(),
            hex.as_str(),
        ]);
        let mut said: Option<UnitLine> = None;
        let ran = {
            let mut heard = |line: &str| match serde_json::from_str::<UnitLine>(line) {
                Ok(UnitLine::Progress { step, percent }) => {
                    crate::progress::step(step_name(&step), Some(percent))
                }
                Ok(other) => said = Some(other),
                Err(_) => tracing::debug!(line, "a volume unit wrote"),
            };
            let limits = Limits { timeout: left, max_stdout: 1024 * 1024 };
            let surroundings = Surroundings::default();
            let running =
                zendiloos_exec::run_lines(&self.systemd_run, &args, &surroundings, limits, &mut heard);
            tokio::select! {
                ran = running => Some(ran),
                () = crate::waiting::given_up() => None,
            }
        };
        let output = match ran {
            Some(Ok(output)) => output,
            Some(Err(ExecError::Timeout { .. })) => {
                self.stop_unit(&unit.name).await;
                return Err(OperationError::Failed(format!("{} took too long and was stopped", unit.name)));
            }
            Some(Err(error)) => return Err(error.into()),
            None => {
                self.stop_unit(&unit.name).await;
                return Err(OperationError::Failed(format!(
                    "{} was stopped: zendiloosd gave up waiting, or the agent stops",
                    unit.name
                )));
            }
        };
        match said {
            Some(UnitLine::Done(value)) if output.status.success() => serde_json::from_value(value)
                .map_err(|e| OperationError::Failed(format!("the unit's answer cannot be read: {e}"))),
            Some(UnitLine::Failed { message, refused: true }) => Err(OperationError::Refused(message)),
            Some(UnitLine::Failed { message, .. }) => Err(OperationError::Failed(message)),
            _ => {
                let tail =
                    output.stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
                Err(OperationError::Failed(format!("the unit ended with {}: {tail}", output.status)))
            }
        }
    }

    pub(crate) async fn back_up(
        &self,
        volume: &VolumeName,
        share: &ShareLocation,
        folder: &SharePath,
        name: &BackupStem,
    ) -> Result<Response, OperationError> {
        let local = self.inspect(volume).await?;
        self.not_busy(volume).await?;
        let share_root = self.share_root(share)?;
        let job = BackupJob {
            source: local.data,
            share: share_root.clone(),
            folder: folder.clone(),
            volume: volume.clone(),
            name: name.clone(),
            driver: local.driver,
            labels: local.labels,
            created: local.created,
        };
        let unit = Unit {
            work: VolumeJob::BackUp,
            name: unit_name(VolumeJob::BackUp, volume),
            writable: &share_root,
            capabilities: BACKUP_CAPABILITIES,
            description: format!("back up the volume {}", volume.as_str()),
            until: Instant::now() + WORK_FOR,
        };
        let done: BackupDone = self.unit(unit, &job).await?;
        Ok(Response::VolumeBackedUp {
            archive: done.archive,
            manifest: done.manifest,
            sha256: done.sha256,
            data_bytes: done.data_bytes,
            manifest_json: done.manifest_json,
        })
    }

    pub(crate) async fn restore(
        &self,
        volume: &VolumeName,
        share: &ShareLocation,
        archive: &SharePath,
        sha256: &ArchiveDigest,
    ) -> Result<Response, OperationError> {
        let until = Instant::now() + WORK_FOR;
        let local = self.inspect(volume).await?;
        self.not_busy(volume).await?;
        self.none_running(volume).await?;
        let share_root = self.share_root(share)?;
        let volume_dir = local.data.parent().map(Path::to_owned).unwrap_or_default();
        let job = RestoreJob {
            volume_dir: volume_dir.clone(),
            share: share_root,
            archive: archive.clone(),
            sha256: sha256.clone(),
        };
        let unit = |work: VolumeJob, description: String, until: Instant| Unit {
            work,
            name: unit_name(work, volume),
            writable: &volume_dir,
            capabilities: RESTORE_CAPABILITIES,
            description,
            until,
        };
        let extract = unit(
            VolumeJob::Restore,
            format!("restore the volume {}", volume.as_str()),
            until.checked_sub(SWAP_RESERVE).unwrap_or(until),
        );
        let extracted: Extracted = self.unit(extract, &job).await?;
        if crate::waiting::is_given_up() {
            return Err(OperationError::Failed(
                "not swapped in: zendiloosd gave up waiting, or the agent stops; the old contents stay"
                    .into(),
            ));
        }
        // Right before the swap: a container started meanwhile keeps the old
        // contents, and what was extracted goes.
        let started = self.none_running(volume).await.err();
        let swap =
            SwapJob { volume_dir: volume_dir.clone(), fresh: extracted.fresh, discard: started.is_some() };
        let description = if swap.discard {
            format!("discard what was extracted for the volume {}", volume.as_str())
        } else {
            format!("swap the restored data of the volume {} in", volume.as_str())
        };
        let swapped: Swapped = self.unit(unit(VolumeJob::Swap, description, until), &swap).await?;
        if let Some(started) = started {
            return Err(started);
        }
        if let Some(left) = swapped.left_over {
            tracing::warn!(
                volume = volume.as_str(),
                left,
                "restored; the old contents are partly left beside the data"
            );
        }
        Ok(Response::Done)
    }

    pub(crate) async fn remove_backup(
        &self,
        share: &ShareLocation,
        path: &SharePath,
        device: u64,
        inode: u64,
    ) -> Result<Response, OperationError> {
        let share_root = self.share_root(share)?;
        let job = RemoveJob { share: share_root.clone(), path: path.clone(), device, inode };
        let mut random = [0u8; 8];
        rustix::rand::getrandom(&mut random, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| OperationError::Failed(format!("no random name: {e}")))?;
        let unit = Unit {
            work: VolumeJob::RemoveBackup,
            name: format!("zendiloos-volume-remove-{:016x}.service", u64::from_le_bytes(random)),
            writable: &share_root,
            capabilities: BACKUP_CAPABILITIES,
            description: format!("remove a backup in {}", share.share.as_str()),
            until: Instant::now() + REMOVE_FOR,
        };
        let removed: bool = self.unit(unit, &job).await?;
        Ok(Response::BackupRemoved { removed })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_local_volumes_in_the_data_root_are_backed_up() {
        let root = Path::new("/volume1/@docker");
        let volume = |name: &str, driver: &str, options: Value, mountpoint: &str| {
            json!({"Name": name, "Driver": driver, "Options": options, "Mountpoint": mountpoint,
                   "Labels": {"com.docker.compose.project": "tesla"}, "CreatedAt": "2026-11-01T10:00:00Z"})
        };
        let local =
            Volumes::local(root, &volume("db", "local", Value::Null, "/volume1/@docker/volumes/db/_data"))
                .unwrap();
        assert_eq!(local.data, Path::new("/volume1/@docker/volumes/db/_data"));
        assert_eq!(local.labels["com.docker.compose.project"], "tesla");
        assert_eq!(local.created, "2026-11-01T10:00:00Z");
        assert!(
            Volumes::local(root, &volume("db", "local", json!({}), "/volume1/@docker/volumes/db/_data"))
                .is_ok()
        );
        for (bad, why) in [
            (volume("db", "nfs-plugin", Value::Null, "/volume1/@docker/volumes/db/_data"), "driver"),
            (volume("db", "local", json!({"type": "nfs"}), "/volume1/@docker/volumes/db/_data"), "options"),
            (volume("db", "local", Value::Null, "/elsewhere/db"), "data root"),
        ] {
            let refused = Volumes::local(root, &bad).unwrap_err();
            assert!(refused.contains(why), "{refused}");
        }
        assert!(plain(Path::new("/volume1/@docker/volumes/a_b-c.d/_data")));
        assert!(!plain(Path::new("/volume1/my docker")) && !plain(Path::new("relative")));
        assert_eq!(step_name("verifying"), "verifying");
        assert_eq!(step_name("anything"), "working");
        let db = VolumeName::parse("tesla_db").unwrap();
        assert_eq!(
            units_of(&db),
            [
                "zendiloos-volume-backup-tesla_db.service",
                "zendiloos-volume-restore-tesla_db.service",
                "zendiloos-volume-swap-tesla_db.service"
            ]
        );
    }
}
