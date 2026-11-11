//! An app installed from a catalog the earlier way, from its start to its
//! removal, with Docker Compose and systemctl replaced by scripts that record
//! what they are asked. Such an app is what the agent kept then: its
//! normalized compose file, which the tests write as it was kept.
//!
//! The agent wants the app's data and its shares on mounted file systems;
//! without root a test cannot mount one, so it borrows `/dev/shm`, a file
//! system of its own below `/dev` on every Linux, as both. What the app
//! mounts lies in a folder of the test's own in `/dev/shm`, removed when it
//! ends. The volume folders are links to `/dev`, which the agent never
//! takes for a mounted file system.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zendiloos_agent_proto::{AppName, DevicePath, Response, UserName};
use zendiloos_exec::Program;

use super::{Apps, EXTENSION};
use crate::accounts::{APP_COMMENT, AccountFiles};
use crate::pools::HostPaths;

const IMAGE: &str =
    "example/app:1.2.3@sha256:fea863d0cd9ec445985d9c75fc6ed64e38a1d2766cf0c250a33d553f4fc1b771";

/// A folder of the test's own in `/dev/shm`; removed when dropped.
struct Shm {
    name: String,
}

impl Shm {
    fn new(tag: &str) -> Self {
        let name = format!("zendiloos-test-{}-{tag}", std::process::id());
        fs::create_dir_all(Path::new("/dev/shm").join(&name)).unwrap();
        Self { name }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(Path::new("/dev/shm").join(&self.name));
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    apps: Apps,
    shm: Shm,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shm = Shm::new(tag);
        let root = dir.path();
        // The app's data and the share, as mounted file systems.
        fs::create_dir_all(root.join("volumes/volume1")).unwrap();
        std::os::unix::fs::symlink("/dev", root.join("volumes/volume1/@apps")).unwrap();
        std::os::unix::fs::symlink("/dev", root.join("volumes/volume2")).unwrap();
        fs::create_dir_all(root.join("units")).unwrap();
        fs::create_dir_all(root.join("dev")).unwrap();
        std::os::unix::fs::symlink("/usr/share/zoneinfo/Europe/Berlin", root.join("localtime")).unwrap();
        fs::write(
            root.join("passwd"),
            format!("root:x:0:0:root:/root:/bin/bash\napp-shm:x:2100:2100:{APP_COMMENT}:/nonexistent:/usr/sbin/nologin\n"),
        )
        .unwrap();
        fs::write(root.join("group"), "root:x:0:\napp-shm:x:2100:\n").unwrap();
        let log = root.join("calls.log");
        let script = |name: &str, body: &str| {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let fail = root.join("fail");
        let docker = script(
            "docker",
            &format!(
                r#"[ -f {fail} ] && {{ echo 'it went wrong' >&2; exit 1; }}
case "$*" in
  *" ls --all --format json") echo '[{{"Name":"shm","Status":"running(1)"}}]' ;;
  *" logs "*) echo 'app-1  | ready' ;;
esac"#,
                fail = fail.display(),
            ),
        );
        let systemctl = script("systemctl", "");
        let paths = HostPaths {
            volumes: root.join("volumes"),
            apps: root.join("apps"),
            units: root.join("units"),
            app_shares: root.join("app-shares"),
            localtime: root.join("localtime"),
            dev: root.join("dev"),
            docker_socket: root.join("docker.sock"),
            ..HostPaths::default()
        };
        let accounts = AccountFiles {
            passwd: root.join("passwd"),
            group: root.join("group"),
            high_water: root.join("high-water"),
        };
        let apps =
            Apps::new(Program::new(&docker).unwrap(), Program::new(&systemctl).unwrap(), paths, accounts);
        Self { dir, apps, shm }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.path("calls.log")).unwrap_or_default()
    }

    fn data_folder(&self) -> PathBuf {
        Path::new("/dev/shm").join(&self.shm.name)
    }

    fn kept_path(&self) -> PathBuf {
        self.path("apps/shm.json")
    }

    /// The compose file as the agent kept it when installing: its data
    /// folder and its share both `/dev/shm`, the app's folder in it bound.
    fn installed(&self) -> Value {
        let data = self.data_folder().display().to_string();
        json!({
            "name": "shm",
            "services": {
                "app": {
                    "image": IMAGE,
                    "restart": "unless-stopped",
                    "environment": { "TZ": "Europe/Berlin", "ADMIN_NAME": "anna" },
                    "ports": [{ "mode": "ingress", "target": 80, "published": "18123", "protocol": "tcp" }],
                    "volumes": [
                        { "type": "bind", "source": data, "target": "/config", "bind": {} },
                        { "type": "bind", "source": "/dev/shm", "target": "/media", "read_only": true, "bind": {} }
                    ]
                }
            },
            EXTENSION: { "app_data": "/dev/shm", "shares": ["/dev/shm"], "pins": [], "docker_socket": false }
        })
    }

    fn keep(&self, kept: &Value) {
        fs::create_dir_all(self.path("apps")).unwrap();
        fs::write(self.kept_path(), kept.to_string()).unwrap();
    }
}

fn app() -> AppName {
    AppName::parse("shm").unwrap()
}

fn refusal(result: Result<Response, crate::operations::OperationError>) -> String {
    match result {
        Err(error) => error.to_string(),
        Ok(response) => panic!("not refused: {response:?}"),
    }
}

#[tokio::test]
async fn an_app_installed_earlier_is_run_and_removed() {
    let f = Fixture::new("life");
    f.keep(&f.installed());

    assert_eq!(f.apps.start(&app()).await.unwrap(), Response::Done);
    assert!(f.calls().contains("up --detach --remove-orphans --quiet-pull"), "{}", f.calls());
    assert!(
        f.calls().contains(&format!("--file {}", f.kept_path().display())),
        "the kept file, nothing else"
    );
    let Response::AppLogs { text } = f.apps.logs(&app(), 50).await.unwrap() else { panic!() };
    assert!(text.contains("ready"));
    assert!(f.calls().contains("logs --no-color --timestamps --tail 50"));
    let Response::Apps { compose_ls } = f.apps.status().await.unwrap() else { panic!() };
    assert!(compose_ls.contains("running"));
    assert_eq!(f.apps.stop(&app()).await.unwrap(), Response::Done);

    let unit = f.path("units/zendiloos-app-shm-shares.service");
    fs::write(&unit, "[Service]\n").unwrap();
    f.apps.remove(&app()).await.unwrap();
    assert!(f.calls().contains("down --rmi all --volumes --remove-orphans"));
    assert!(
        f.calls().contains("disable --no-reload --now zendiloos-app-shm-shares.service"),
        "{}",
        f.calls()
    );
    assert!(!unit.exists(), "the pins' unit goes");
    assert!(!f.kept_path().exists(), "the kept file goes");
    f.apps.remove(&app()).await.unwrap();
    assert_eq!(f.calls().matches(" down ").count(), 1, "nothing to remove a second time");
    assert!(refusal(f.apps.start(&app()).await).contains("not installed"));
}

#[tokio::test]
async fn data_that_is_not_mounted_is_not_started() {
    let f = Fixture::new("unmounted");
    let mut kept = f.installed();
    // Where the app's dataset is mounted when all is well; a link here.
    let app_data = f.path("volumes/volume1/@apps/shm").display().to_string();
    kept[EXTENSION]["app_data"] = json!(app_data);
    kept["services"]["app"]["volumes"][0]["source"] = json!(format!("{app_data}/config"));
    f.keep(&kept);

    assert!(refusal(f.apps.start(&app()).await).contains("not mounted"), "the links are no mounts");
    assert_eq!(f.calls().matches(" up ").count(), 0);
}

#[tokio::test]
async fn a_folder_of_a_share_starts_only_from_its_pin() {
    let f = Fixture::new("pin");
    let folder = format!("{}/Albums", f.shm.name);
    let at = f.path("app-shares/shm/media").display().to_string();
    let mut kept = f.installed();
    kept["services"]["app"]["volumes"][1]["source"] = json!(at);
    kept[EXTENSION]["shares"] = json!([]);
    kept[EXTENSION]["pins"] = json!([{ "share": "/dev/shm", "folder": folder, "at": at }]);
    f.keep(&kept);

    // The folder is gone: refused before anything is pinned again.
    let refused = refusal(f.apps.start(&app()).await);
    assert!(refused.contains("Albums"), "the folder is named: {refused}");
    assert!(!f.calls().contains("restart"), "{}", f.calls());

    // There, but nothing is mounted at the pin's place: Docker would bind
    // whatever is there.
    fs::create_dir_all(Path::new("/dev/shm").join(&folder)).unwrap();
    let refused = refusal(f.apps.start(&app()).await);
    assert!(f.calls().contains("restart zendiloos-app-shm-shares.service"), "pinned again: {}", f.calls());
    assert!(refused.contains("app-shares/shm/media"), "{refused}");
    assert_eq!(f.calls().matches(" up ").count(), 0, "nothing started: {}", f.calls());
}

#[tokio::test]
async fn a_failing_compose_is_reported_in_its_words() {
    let f = Fixture::new("fails");
    f.keep(&f.installed());
    fs::write(f.path("fail"), "").unwrap();

    let logs = f.apps.logs(&app(), 10).await;
    for result in [f.apps.start(&app()).await, f.apps.stop(&app()).await, f.apps.status().await, logs] {
        assert!(matches!(&result, Err(e) if e.to_string().contains("it went wrong")), "{result:?}");
    }
    let removed = f.apps.remove(&app()).await.unwrap_err();
    assert!(removed.to_string().contains("it went wrong"));
    assert!(f.kept_path().exists(), "kept until Compose took it down");
}

#[tokio::test]
async fn a_start_reaches_every_mount_first() {
    let f = Fixture::new("mounts");
    let kept = f.installed();
    f.keep(&kept);
    assert_eq!(f.apps.start(&app()).await.unwrap(), Response::Done);

    // The app's folder became a link to the system.
    fs::remove_dir(f.data_folder()).unwrap();
    std::os::unix::fs::symlink("/etc", f.data_folder()).unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("is missing, a link or no folder"));
    fs::remove_file(f.data_folder()).unwrap();
    fs::create_dir(f.data_folder()).unwrap();

    let mut elsewhere = kept.clone();
    elsewhere["services"]["app"]["volumes"][1]["source"] = json!("/etc");
    f.keep(&elsewhere);
    assert!(refusal(f.apps.start(&app()).await).contains("neither the app's data nor one of its shares"));

    let mut socket = kept.clone();
    socket["services"]["app"]["volumes"][1]["source"] = json!("/run/docker.sock");
    f.keep(&socket);
    assert!(refusal(f.apps.start(&app()).await).contains("not given the Docker socket"));
    socket[EXTENSION]["docker_socket"] = json!(true);
    f.keep(&socket);
    assert!(refusal(f.apps.start(&app()).await).contains("missing"), "no socket at the place");
    fs::write(f.path("docker.sock"), "").unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("not Docker's socket"));

    let mut nowhere = kept.clone();
    nowhere[EXTENSION]["app_data"] = json!("");
    f.keep(&nowhere);
    assert!(refusal(f.apps.start(&app()).await).contains("does not say where"));
    fs::write(f.kept_path(), "not json").unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("cannot be read"));
    fs::remove_file(f.kept_path()).unwrap();
    fs::create_dir(f.kept_path()).unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("cannot read"), "a folder in its place");
    assert_eq!(f.calls().matches(" up ").count(), 1, "nothing started again: {}", f.calls());
}

#[test]
fn the_time_zone_comes_from_where_localtime_leads() {
    let f = Fixture::new("zone");
    assert_eq!(f.apps.time_zone(), "Europe/Berlin");
    fs::remove_file(f.path("localtime")).unwrap();
    std::os::unix::fs::symlink("/usr/share/zoneinfo/../../../etc/passwd", f.path("localtime")).unwrap();
    assert_eq!(f.apps.time_zone(), "UTC", "no way out of the zone database");
    fs::remove_file(f.path("localtime")).unwrap();
    fs::write(f.path("localtime"), "a copy, not a link").unwrap();
    assert_eq!(f.apps.time_zone(), "UTC");
}

#[test]
fn the_facts_a_template_needs_come_from_the_machine() {
    let f = Fixture::new("facts");
    let users = "zendiloos-users:x:1500:\nzendiloos-files:x:995:\napp-shm:x:2100:\n";
    fs::write(f.path("group"), users).unwrap();
    let alice = "alice:x:2001:1500:Alice:/nonexistent:/usr/sbin/nologin\n";
    let owner = "owner:x:1000:1000::/home/owner:/bin/bash\n";
    fs::write(f.path("passwd"), format!("{alice}{owner}")).unwrap();
    fs::create_dir_all(f.path("dev/dri")).unwrap();
    let device = |path: &str| DevicePath::parse(path).unwrap();
    let asked = [device("/dev/dri"), device("/dev/ttyUSB0")];

    let facts = f.apps.facts(&UserName::parse("alice").unwrap(), &asked).unwrap();

    let expected = Response::HostFacts {
        uid: 2001,
        gid: 995,
        time_zone: "Europe/Berlin".into(),
        devices: vec![device("/dev/dri")],
    };
    assert_eq!(facts, expected, "the account, in the group whose access the shares give");
    let unmanaged = f.apps.facts(&UserName::parse("owner").unwrap(), &asked);
    assert!(unmanaged.is_err(), "the owner's own user is no account of zendiloOS");
    let many = vec![device("/dev/dri"); DevicePath::MAX_COUNT + 1];
    assert!(f.apps.facts(&UserName::parse("alice").unwrap(), &many).is_err());
}
