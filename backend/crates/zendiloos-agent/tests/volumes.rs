//! Docker's volumes against a fake `docker`, and their backups and
//! restores end to end: the system agent checks the request and starts its
//! unit through a `systemd-run` that records what it was asked and runs
//! this crate's own binary, which reads and writes the test's folders. A
//! fake `systemctl` says a unit runs while `units/<name>` exists.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio::net::UnixListener;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, HostPaths, Operations, Paths, Server, serve};
use zendiloos_agent_proto::{
    AgentTransport, ArchiveDigest, BackupStem, ErrorCode, Request, Response, Role, ShareLocation, ShareName,
    SharePath, UnixTransport, VolumeLabels, VolumeName, VolumeNumber, WrittenFile,
};

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    fn new(share_mounts: bool) -> Self {
        Self::with(share_mounts, "")
    }

    /// `unit_first`: shell that `systemd-run` runs before the unit's
    /// command, `{root}` the test's folder.
    fn with(share_mounts: bool, unit_first: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let data = root.join("docker/volumes/db/_data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("PG_VERSION"), "17\n").unwrap();
        fs::create_dir_all(root.join("volume1/backups")).unwrap();
        let log = root.join("calls.log");
        let docker_root = root.join("docker");
        let docker = root.join("docker-cli");
        let script = format!(
            r#"#!/bin/sh
echo "docker $*" >> {log}
case "$*" in
  "info --format {{{{.DockerRootDir}}}}") echo {docker_root} ;;
  "volume ls --quiet") printf 'db\nnfs\n'; [ -f {root}/gone ] && echo gone ;;
  "volume inspect gone") echo 'Error response from daemon: get gone: no such volume' >&2; exit 1 ;;
  "volume inspect db") echo '[{{db}}]' ;;
  "volume inspect nfs") echo '[{{nfs}}]' ;;
  "volume inspect "*gone*) echo '[{{db}},{{nfs}}]'; echo 'Error response from daemon: get gone: no such volume' >&2; exit 1 ;;
  "volume inspect "*) echo '[{{db}},{{nfs}}]' ;;
  "ps --quiet --no-trunc --filter volume=db") [ -f {root}/running ] && echo d853eed323e7 ;;
  "volume prune --all --force") echo 'Total reclaimed space: 1.2MB' ;;
  "volume rm busy") echo 'Error response from daemon: remove busy: volume is in use' >&2; exit 1 ;;
esac
exit 0
"#,
            log = log.display(),
            docker_root = docker_root.display(),
            root = root.display(),
        );
        let db = format!(
            r#"{{"Name":"db","Driver":"local","Options":null,"Mountpoint":"{}/volumes/db/_data","Labels":{{"com.docker.compose.project":"tesla"}},"CreatedAt":"2026-11-01T10:00:00Z"}}"#,
            docker_root.display()
        );
        let nfs = format!(
            r#"{{"Name":"nfs","Driver":"local","Options":{{"type":"nfs"}},"Mountpoint":"{}/volumes/nfs/_data","Labels":null}}"#,
            docker_root.display()
        );
        let script = script.replace("{db}", &db).replace("{nfs}", &nfs);
        fs::write(&docker, script).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        // Records what the unit was to be, then runs its command here.
        let systemd_run = root.join("systemd-run");
        let script = format!(
            "#!/bin/sh\necho \"systemd-run $*\" >> {log}\n{first}\nwhile [ \"${{1#--}}\" != \"$1\" ]; do shift; done\nexec \"$@\"\n",
            log = log.display(),
            first = unit_first.replace("{root}", &root.display().to_string()),
        );
        fs::write(&systemd_run, script).unwrap();
        fs::set_permissions(&systemd_run, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(root.join("units")).unwrap();
        let systemctl = root.join("systemctl");
        let script = format!(
            r#"#!/bin/sh
echo "systemctl $*" >> {log}
case "$1" in
  show) shift 3; for unit in "$@"; do if [ -f {root}/units/$unit ]; then echo active; else echo inactive; fi; done ;;
  list-units) for unit in $(ls {root}/units); do echo "$unit loaded active running a volume's work"; done ;;
  stop) rm -f {root}/units/$2 ;;
esac
exit 0
"#,
            log = log.display(),
            root = root.display(),
        );
        fs::write(&systemctl, script).unwrap();
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Tools { docker, systemd_run, systemctl, ..Tools::default() };
        let host = HostPaths {
            volumes: root.clone(),
            agent: PathBuf::from(env!("CARGO_BIN_EXE_zendiloos-agent")),
            share_mounts,
            ..HostPaths::default()
        };
        let paths = Paths { host, ..Paths::default() };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        Self { _dir: dir, root, agent }
    }

    /// This host's agent behind a socket of its own, as zendiloosd talks to
    /// it.
    fn serve(self) -> (PathBuf, PathBuf) {
        let socket = self.root.join("agent.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let uid = fs::metadata(&self.root).unwrap().uid();
        let root = self.root.clone();
        let server = Server::new(self.agent, uid);
        let dir = self._dir;
        tokio::spawn(async move {
            let _dir = dir;
            serve(listener, server, std::future::pending::<()>()).await;
        });
        (socket, root)
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn share() -> ShareLocation {
    ShareLocation { volume: VolumeNumber::new(1).unwrap(), share: ShareName::parse("backups").unwrap() }
}

fn volume(name: &str) -> VolumeName {
    VolumeName::parse(name).unwrap()
}

fn backup(name: &str) -> Request {
    Request::BackUpVolume {
        volume: volume(name),
        share: share(),
        folder: SharePath::parse("docker").unwrap(),
        name: BackupStem::parse(&format!("{name}-20261108-030000")).unwrap(),
    }
}

fn calls_in(root: &std::path::Path) -> Vec<String> {
    fs::read_to_string(root.join("calls.log")).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn restore(archive: &WrittenFile, sha256: &ArchiveDigest) -> Request {
    Request::RestoreVolume {
        volume: volume("db"),
        share: share(),
        archive: archive.path.clone(),
        sha256: sha256.clone(),
        data_bytes: 1 << 20,
        entries: Some(64),
    }
}

#[tokio::test]
async fn volumes_are_listed_counted_made_and_removed() {
    let host = Host::new(false);

    let Response::DockerOutput { text } = host.agent.execute(&Request::DockerVolumes {}).await else {
        panic!()
    };
    assert!(text.contains(r#""Name":"db""#) && text.contains(r#""Name":"nfs""#), "{text}");
    let Response::VolumeSizes { sizes } = host.agent.execute(&Request::DockerVolumeSizes {}).await else {
        panic!()
    };
    assert_eq!(sizes.len(), 1, "only volumes whose data lies in the data root: {sizes:?}");
    assert_eq!((sizes[0].name.as_str(), sizes[0].bytes, sizes[0].complete), ("db", 3, true));

    let labels =
        VolumeLabels::parse([("com.docker.compose.project".to_owned(), "tesla".to_owned())].into()).unwrap();
    assert_eq!(
        host.agent.execute(&Request::CreateVolume { name: volume("fresh"), labels }).await,
        Response::Done
    );
    assert_eq!(host.agent.execute(&Request::RemoveVolume { name: volume("fresh") }).await, Response::Done);
    let busy = host.agent.execute(&Request::RemoveVolume { name: volume("busy") }).await;
    assert!(
        matches!(&busy, Response::Error { code: ErrorCode::Failed, message } if message.contains("in use"))
    );
    let Response::DockerOutput { text } = host.agent.execute(&Request::PruneVolumes {}).await else {
        panic!()
    };
    assert!(text.contains("reclaimed"));
    let calls = host.calls();
    assert!(
        calls.contains(
            &"docker volume create --driver local --label=com.docker.compose.project=tesla fresh".to_owned()
        ),
        "{calls:?}"
    );
    assert!(calls.contains(&"docker volume rm fresh".to_owned()));
    assert!(calls.contains(&"docker volume prune --all --force".to_owned()));
}

#[tokio::test]
async fn a_volume_is_backed_up_into_the_share_and_restored_from_it() {
    let host = Host::new(false);

    let answer = host.agent.execute(&backup("db")).await;
    let Response::VolumeBackedUp { archive, manifest, sha256, data_bytes, entries, manifest_json } = answer
    else {
        panic!("{answer:?}")
    };
    assert_eq!(archive.path.as_str(), "docker/db/db-20261108-030000.tar.zst");
    assert_eq!(manifest.path.as_str(), "docker/db/db-20261108-030000.json");
    assert_eq!((data_bytes, entries), (3, 2), "the folder and PG_VERSION");
    assert!(
        manifest_json.contains(r#""volume": "db""#) && manifest_json.contains(sha256.as_str()),
        "{manifest_json}"
    );
    assert!(host.root.join("volume1/backups").join(archive.path.as_str()).is_file());
    let unit = host.calls().into_iter().find(|call| call.starts_with("systemd-run")).unwrap();
    let share_root = host.root.join("volume1/backups");
    for wanted in [
        "--unit=zendiloos-volume-backup-db.service --wait --pipe --collect --quiet".to_owned(),
        "--description=zendiloOS: back up the volume db".to_owned(),
        "--property=RuntimeMaxSec=".to_owned(),
        "--property=TimeoutStopSec=".to_owned(),
        "--property=ProtectSystem=strict".to_owned(),
        "--property=PrivateNetwork=yes".to_owned(),
        format!("--property=ReadWritePaths={}", share_root.display()),
        "--property=CapabilityBoundingSet=CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE".to_owned(),
        " backup-volume ".to_owned(),
    ] {
        assert!(unit.contains(&wanted), "{wanted} in {unit}");
    }

    // The data changes; the backup brings it back.
    let data = host.root.join("docker/volumes/db/_data");
    fs::write(data.join("PG_VERSION"), "18\n").unwrap();
    fs::write(data.join("later"), "x").unwrap();
    assert_eq!(
        host.agent.execute(&restore(&archive, &sha256)).await,
        Response::VolumeRestored { left_over: None }
    );
    assert_eq!(fs::read_to_string(data.join("PG_VERSION")).unwrap(), "17\n");
    assert!(!data.join("later").exists());
    let unit = host.calls().into_iter().filter(|call| call.starts_with("systemd-run")).nth(1).unwrap();
    assert!(
        unit.contains(&format!(
            "--property=ReadWritePaths={}",
            host.root.join("docker/volumes/db").display()
        ))
    );
    assert!(
        unit.contains("CAP_CHOWN") && unit.contains("CAP_SETFCAP") && unit.contains(" restore-volume "),
        "{unit}"
    );
    let swap = host.calls().into_iter().filter(|call| call.starts_with("systemd-run")).nth(2).unwrap();
    assert!(
        swap.contains("--unit=zendiloos-volume-swap-db.service") && swap.contains(" swap-volume "),
        "the data is swapped in by a unit of its own: {swap}"
    );
    let checks = host.calls().into_iter().filter(|call| call.starts_with("docker ps")).count();
    assert_eq!(checks, 2, "running containers are looked for before extracting and before the swap");

    // Not the archive recorded: refused, the data as it is.
    fs::write(data.join("PG_VERSION"), "18\n").unwrap();
    let wrong = ArchiveDigest::parse(&"0".repeat(64)).unwrap();
    let refused = host.agent.execute(&restore(&archive, &wrong)).await;
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.contains("SHA-256")),
        "{refused:?}"
    );
    assert_eq!(fs::read_to_string(data.join("PG_VERSION")).unwrap(), "18\n");
    // Not while a container runs on it.
    fs::write(host.root.join("running"), "").unwrap();
    let busy = host.agent.execute(&restore(&archive, &sha256)).await;
    assert!(matches!(&busy, Response::Error { code: ErrorCode::Conflict, .. }), "{busy:?}");
    fs::remove_file(host.root.join("running")).unwrap();

    // Retention removes the files it wrote, and only those.
    let remove = |file: &WrittenFile| Request::RemoveVolumeBackup {
        share: share(),
        path: file.path.clone(),
        inode: file.inode,
        bytes: file.bytes,
        modified: Some(file.modified),
    };
    assert_eq!(host.agent.execute(&remove(&archive)).await, Response::BackupRemoved { removed: true });
    assert_eq!(host.agent.execute(&remove(&archive)).await, Response::BackupRemoved { removed: false });
    let other = WrittenFile { inode: manifest.inode + 1, ..manifest.clone() };
    assert_eq!(host.agent.execute(&remove(&other)).await, Response::BackupRemoved { removed: false });
    assert!(host.root.join("volume1/backups").join(manifest.path.as_str()).exists());
}

/// A ZFS dataset's device number is handed out as it is mounted: after a
/// reboot the same archive has another one. Retention finds it by its
/// inode, size and time of writing on the share as it is mounted now.
#[tokio::test]
async fn retention_removes_an_archive_whatever_device_its_share_was_mounted_with() {
    let host = Host::new(false);
    let Response::VolumeBackedUp { archive, .. } = host.agent.execute(&backup("db")).await else { panic!() };
    // As recorded before the share was mounted again.
    let recorded = WrittenFile { device: archive.device + 1, ..archive.clone() };
    let remove = Request::RemoveVolumeBackup {
        share: share(),
        path: recorded.path.clone(),
        inode: recorded.inode,
        bytes: recorded.bytes,
        modified: Some(recorded.modified),
    };
    assert_eq!(host.agent.execute(&remove).await, Response::BackupRemoved { removed: true });
    assert!(!host.root.join("volume1/backups").join(archive.path.as_str()).exists());
}

/// What a restore cut off left beside the data (its unit stopped or
/// killed, the machine off) goes before the next restore of the volume,
/// and as the agent starts.
#[tokio::test]
async fn what_a_restore_cut_off_left_beside_the_data_goes() {
    let host = Host::new(false);
    let Response::VolumeBackedUp { archive, sha256, .. } = host.agent.execute(&backup("db")).await else {
        panic!()
    };
    let left = host.root.join("docker/volumes/db/_data.zendiloos-restore-0123456789abcdef");
    fs::create_dir(&left).unwrap();
    fs::write(left.join("big"), vec![0u8; 1 << 20]).unwrap();
    assert!(matches!(host.agent.execute(&restore(&archive, &sha256)).await, Response::VolumeRestored { .. }));
    assert!(!left.exists(), "removed before the restore");

    // As the agent starts: the units nobody waits for stop, then a sweep.
    fs::create_dir(&left).unwrap();
    fs::write(left.join("big"), "x").unwrap();
    fs::write(host.root.join("units/zendiloos-volume-backup-db.service"), "").unwrap();
    host.agent.recover().await;
    let started = Instant::now();
    while left.exists() {
        assert!(started.elapsed() < Duration::from_secs(20), "never swept: {:?}", host.calls());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let calls = host.calls();
    assert!(calls.contains(&"systemctl stop zendiloos-volume-backup-db.service".to_owned()), "{calls:?}");
    assert!(calls.iter().any(|call| call.contains("--unit=zendiloos-volume-sweep.service")), "{calls:?}");
    // Meanwhile, backups wait for the sweep, then run.
    assert!(matches!(
        host.agent.execute(&backup("db")).await,
        Response::Error { .. } | Response::VolumeBackedUp { .. }
    ));
}

/// A volume removed between the listing and the inspection is left out of
/// the list, which does not fail.
#[tokio::test]
async fn volumes_that_go_while_they_are_listed_are_left_out() {
    let host = Host::new(false);
    fs::write(host.root.join("gone"), "").unwrap();
    let Response::DockerOutput { text } = host.agent.execute(&Request::DockerVolumes {}).await else {
        panic!()
    };
    let listed: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = listed.iter().filter_map(|v| v["Name"].as_str()).collect();
    assert_eq!(names, ["db", "nfs"]);
    let Response::VolumeSizes { sizes } = host.agent.execute(&Request::DockerVolumeSizes {}).await else {
        panic!()
    };
    assert_eq!(sizes.len(), 1);
}

#[tokio::test]
async fn only_local_volumes_and_mounted_shares_are_used() {
    let host = Host::new(false);
    let refused = host.agent.execute(&backup("nfs")).await;
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.contains("options")),
        "{refused:?}"
    );

    let host = Host::new(true);
    let refused = host.agent.execute(&backup("db")).await;
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.contains("cannot be used")),
        "{refused:?}"
    );
    assert!(!host.calls().iter().any(|call| call.starts_with("systemd-run")), "no unit started");
}

/// A container started once the agent had looked (by hand, a redeploy, an
/// image update) has the old data mounted: it is not swapped away under it.
#[tokio::test]
async fn a_restore_is_not_swapped_in_under_a_container_started_meanwhile() {
    let host = Host::with(false, "case \"$*\" in *restore-volume*) : > {root}/running ;; esac");
    let Response::VolumeBackedUp { archive, sha256, .. } = host.agent.execute(&backup("db")).await else {
        panic!()
    };
    let data = host.root.join("docker/volumes/db/_data");
    fs::write(data.join("written-since"), "the app's newest data").unwrap();

    let answer = host.agent.execute(&restore(&archive, &sha256)).await;

    assert!(matches!(&answer, Response::Error { code: ErrorCode::Conflict, .. }), "{answer:?}");
    assert!(data.join("written-since").exists(), "the old contents stay");
    let beside: Vec<_> = fs::read_dir(host.root.join("docker/volumes/db"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(beside, ["_data"], "what was extracted is discarded");
    let swap =
        host.calls().into_iter().find(|call| call.contains("--unit=zendiloos-volume-swap-db")).unwrap();
    assert!(swap.contains("discard what was extracted"), "{swap}");
}

/// Work a client gave up on, or a restart of the agent left, refuses new
/// work on the volume until it is ended; ending it stops it.
#[tokio::test]
async fn work_left_running_on_a_volume_refuses_more_until_it_is_ended() {
    let host = Host::new(false);
    let left = host.root.join("units/zendiloos-volume-restore-db.service");
    fs::write(&left, "").unwrap();

    let refused = host.agent.execute(&backup("db")).await;
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Conflict, message } if message.contains("still runs")),
        "{refused:?}"
    );
    let end = Request::EndVolumeWork { volume: volume("db") };
    assert_eq!(host.agent.execute(&end).await, Response::VolumeWorkEnded { stopped: true });
    assert!(host.calls().contains(&"systemctl stop zendiloos-volume-restore-db.service".to_owned()));
    assert_eq!(host.agent.execute(&end).await, Response::VolumeWorkEnded { stopped: false });
    assert!(matches!(host.agent.execute(&backup("db")).await, Response::VolumeBackedUp { .. }));
}

/// A client that hangs up (zendiloosd gave up, or stopped) gets no unit
/// left running behind its back: the agent stops it before it is done.
#[tokio::test]
async fn a_restore_whose_client_hangs_up_is_stopped_before_the_agent_answers() {
    let host =
        Host::with(false, "case \"$*\" in *restore-volume*) [ -f {root}/slow ] && /bin/sleep 20 ;; esac");
    let Response::VolumeBackedUp { archive, sha256, .. } = host.agent.execute(&backup("db")).await else {
        panic!()
    };
    fs::write(host.root.join("slow"), "").unwrap();
    let (socket, root) = host.serve();

    let started = Instant::now();
    let client = UnixTransport::new(&socket, Duration::from_millis(1500));
    assert!(client.call(restore(&archive, &sha256), None).await.is_err(), "the client gave up");
    let stop = "systemctl stop zendiloos-volume-restore-db.service".to_owned();
    while !calls_in(&root).contains(&stop) {
        assert!(started.elapsed() < Duration::from_secs(15), "never stopped: {:?}", calls_in(&root));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(fs::read_to_string(root.join("docker/volumes/db/_data/PG_VERSION")).unwrap(), "17\n");
    assert!(!calls_in(&root).iter().any(|call| call.contains("swap-volume")), "never swapped in");
}

/// Backups and restores take places of their own: hours of them never
/// keep the long requests (deploys, pulls, drives, pools, sizes) waiting.
#[tokio::test]
async fn backups_leave_the_long_places_to_other_requests() {
    let host =
        Host::with(false, "case \"$*\" in *backup-volume*) [ -f {root}/slow ] && /bin/sleep 3 ;; esac");
    fs::write(host.root.join("slow"), "").unwrap();
    let (socket, _root) = host.serve();

    // Into folders of their own, as backups of two volumes are: one unit
    // of a volume runs at a time, which the fake systemd-run here does not
    // see to.
    let backups: Vec<_> = [("docker", "db-20261108-030000"), ("elsewhere", "db-20261108-030001")]
        .into_iter()
        .map(|(folder, stem)| {
            let client = UnixTransport::new(&socket, Duration::from_secs(60));
            let request = Request::BackUpVolume {
                volume: volume("db"),
                share: share(),
                folder: SharePath::parse(folder).unwrap(),
                name: BackupStem::parse(stem).unwrap(),
            };
            tokio::spawn(async move { client.call(request, None).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = Instant::now();
    let client = UnixTransport::new(&socket, Duration::from_millis(1600));
    let sizes = client.call(Request::DockerVolumeSizes {}, None).await;
    let waited = started.elapsed();
    for backup in backups {
        assert!(matches!(backup.await.unwrap(), Ok(Response::VolumeBackedUp { .. })));
    }
    assert!(
        matches!(sizes, Ok(Response::VolumeSizes { .. })),
        "after {waited:?} behind two backups: {sizes:?}"
    );
}
