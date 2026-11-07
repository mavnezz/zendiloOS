//! An app from configuring to removing, with Docker Compose and systemctl
//! replaced by scripts that record what they are asked.
//!
//! The agent wants the app's data and its shares on mounted file systems;
//! without root a test cannot mount one, so it borrows `/dev/shm`, a
//! file system of its own below `/dev` on every Linux: the volume folders
//! are links, `volume1/@apps` to `/dev` and `volume2` to `/dev`, and the
//! app and the share are both called `shm`. What the app mounts lies in a
//! folder of the test's own in `/dev/shm`, removed when it ends. Where the
//! agent opens a mounted root without following a link at its end (before
//! a start, and to pin), the links do not pass: those tests keep the app
//! with `/dev/shm` itself as its data.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zendiloos_agent_proto::{
    AppCompose, AppFolder, AppManifest, AppName, AppSettingValue, AppShareChoice, AppValues, DevicePath,
    Response, ShareName, UserName, VolumeNumber,
};
use zendiloos_exec::Program;

use super::{Apps, EXTENSION};
use crate::accounts::{APP_COMMENT, AccountFiles};
use crate::pools::HostPaths;

const IMAGE: &str =
    "example/app:1.2.3@sha256:fea863d0cd9ec445985d9c75fc6ed64e38a1d2766cf0c250a33d553f4fc1b771";

/// The app's manifest: a port, a share it reads and a setting.
fn manifest(id: &str) -> AppManifest {
    manifest_with(id, |_| {})
}

/// [`manifest`], changed by `change`.
fn manifest_with(id: &str, change: impl FnOnce(&mut Value)) -> AppManifest {
    let mut manifest = json!({
        "id": id,
        "name": "Shm",
        "tagline": "Shows what an app needs",
        "description": "A longer text about it.",
        "categories": ["productivity"],
        "developer": "Demo developers",
        "website": "https://example.org",
        "source": "https://github.com/example/shm",
        "license": "MIT",
        "maintainers": ["mavnezz"],
        "architectures": ["amd64"],
        "ports": [{ "id": "web", "label": "Web", "default": 8080 }],
        "shares": [{ "id": "media", "label": "Media", "access": "read-only" }],
        "settings": [
            { "id": "ADMIN_NAME", "label": "Administrator", "type": "text", "required": true },
            { "id": "WORKERS", "label": "Workers", "type": "number" },
            { "id": "VERBOSE", "label": "Verbose", "type": "boolean" }
        ]
    });
    change(&mut manifest);
    AppManifest::new(manifest.to_string()).unwrap()
}

fn compose(folder: &str) -> AppCompose {
    AppCompose::new(format!(
        "services:
  app:
    image: {IMAGE}
    restart: unless-stopped
    environment:
      TZ: ${{TZ}}
      ADMIN_NAME: ${{ADMIN_NAME}}
      WORKERS: ${{WORKERS}}
      VERBOSE: ${{VERBOSE}}
    ports:
      - ${{PORT_WEB}}:80
    volumes:
      - ${{APP_DATA}}/{folder}:/config
      - ${{SHARE_MEDIA}}:/media:ro
"
    ))
    .unwrap()
}

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
        let config = format!(
            r#"cat > /dev/null
[ -f {fail} ] && {{ echo 'no such service' >&2; exit 1; }}
[ -f {garbage} ] && {{ echo 'not json'; exit 0; }}
printf '{{"name":"shm","networks":{{"default":{{"name":"shm_default","ipam":{{}}}}}},"services":{{"app":{{"image":"{IMAGE}","restart":"unless-stopped","command":null,"entrypoint":null,"networks":{{"default":null}},"environment":{{"TZ":"%s","ADMIN_NAME":"%s","WORKERS":"%s","VERBOSE":"%s"}},"ports":[{{"mode":"ingress","target":80,"published":"%s","protocol":"tcp"}}],"volumes":[{{"type":"bind","source":"%s/{folder}","target":"/config","bind":{{}}}},{{"type":"bind","source":"%s","target":"/media","read_only":true,"bind":{{}}}}]}}}}}}' "$TZ" "$ADMIN_NAME" "$WORKERS" "$VERBOSE" "$PORT_WEB" "$APP_DATA" "$SHARE_MEDIA"
env > {environment}"#,
            fail = fail.display(),
            garbage = root.join("garbage").display(),
            environment = root.join("environment").display(),
            folder = shm.name,
        );
        let docker = script(
            "docker",
            &format!(
                r#"case "$*" in
  *" config --format json") {config} ;;
  *) [ -f {fail} ] && {{ echo 'it went wrong' >&2; exit 1; }} ;;
esac
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

    fn kept(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.path("apps/shm.json")).unwrap()).unwrap()
    }

    fn data_folder(&self) -> PathBuf {
        Path::new("/dev/shm").join(&self.shm.name)
    }

    /// The kept app with `/dev/shm` itself as its data and its share.
    fn on_dev_shm(&self) {
        let mut kept = self.kept();
        kept[EXTENSION]["app_data"] = json!("/dev/shm");
        kept[EXTENSION]["shares"] = json!(["/dev/shm"]);
        let data = self.data_folder().display().to_string();
        kept["services"]["app"]["volumes"][0]["source"] = json!(data);
        kept["services"]["app"]["volumes"][1]["source"] = json!("/dev/shm");
        fs::write(self.path("apps/shm.json"), kept.to_string()).unwrap();
    }
}

fn app() -> AppName {
    AppName::parse("shm").unwrap()
}

fn volume(number: u16) -> VolumeNumber {
    VolumeNumber::new(number).unwrap()
}

fn values(folder: &str) -> AppValues {
    let setting = |value: &str| AppSettingValue::new(value).unwrap();
    AppValues {
        shares: BTreeMap::from([(
            "media".to_owned(),
            AppShareChoice {
                volume: volume(2),
                name: ShareName::parse("shm").unwrap(),
                folder: AppFolder::parse(folder).unwrap(),
            },
        )]),
        ports: BTreeMap::from([("web".to_owned(), 18_123)]),
        settings: BTreeMap::from([
            ("ADMIN_NAME".to_owned(), setting("anna")),
            ("WORKERS".to_owned(), setting("4")),
            ("VERBOSE".to_owned(), setting("true")),
        ]),
    }
}

fn refusal(result: Result<Response, crate::operations::OperationError>) -> String {
    match result {
        Err(error) => error.to_string(),
        Ok(response) => panic!("not refused: {response:?}"),
    }
}

#[tokio::test]
async fn an_app_is_checked_kept_run_and_removed() {
    let f = Fixture::new("life");
    let (manifest, compose) = (manifest("shm"), compose(&f.shm.name));

    let configured = f.apps.configure(&app(), volume(1), &manifest, &compose, &values("")).await.unwrap();

    let Response::AppFolders { folders } = configured else { panic!("{configured:?}") };
    assert_eq!(folders, [AppFolder::parse(&f.shm.name).unwrap()], "the folder for the files agent to make");
    let kept = f.kept();
    let app_data = f.path("volumes/volume1/@apps/shm").display().to_string();
    assert_eq!(kept[EXTENSION]["app_data"], app_data);
    assert_eq!(kept[EXTENSION]["shares"], json!([f.path("volumes/volume2/shm").display().to_string()]));
    let service = &kept["services"]["app"];
    assert_eq!(service["environment"]["TZ"], "Europe/Berlin", "the machine's zone");
    assert_eq!(service["environment"]["ADMIN_NAME"], "anna");
    assert_eq!(service["ports"][0]["published"], "18123");
    let mode = fs::metadata(f.path("apps/shm.json")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "settings may be passwords");
    // Compose saw nothing of the agent's environment: only Docker's own and the app's values.
    let environment = fs::read_to_string(f.path("environment")).unwrap();
    assert!(environment.contains("DOCKER_CONFIG=/nonexistent") && environment.contains("PORT_WEB=18123"));
    assert!(!environment.contains("HOME="), "{environment}");
    assert_eq!(f.calls().matches("config --format json").count(), 2, "stand-ins first, then the values");

    assert_eq!(f.apps.pull(&app()).await.unwrap(), Response::Done);
    assert!(refusal(f.apps.start(&app()).await).contains("not mounted"), "the links are no mounts");
    f.on_dev_shm();
    assert_eq!(f.apps.start(&app()).await.unwrap(), Response::Done);
    assert!(f.calls().contains("up --detach --remove-orphans --quiet-pull"), "{}", f.calls());
    let Response::AppLogs { text } = f.apps.logs(&app(), 50).await.unwrap() else { panic!() };
    assert!(text.contains("ready"));
    assert!(f.calls().contains("logs --no-color --timestamps --tail 50"));
    let Response::Apps { compose_ls } = f.apps.status().await.unwrap() else { panic!() };
    assert!(compose_ls.contains("running"));
    assert_eq!(f.apps.stop(&app()).await.unwrap(), Response::Done);

    f.apps.remove(&app()).await.unwrap();
    assert!(f.calls().contains("down --rmi all --volumes --remove-orphans"));
    assert!(!f.path("apps/shm.json").exists(), "the kept file goes");
    f.apps.remove(&app()).await.unwrap();
    assert_eq!(f.calls().matches(" down ").count(), 1, "nothing to remove a second time");
    assert!(refusal(f.apps.start(&app()).await).contains("not installed"));
}

#[tokio::test]
async fn a_folder_of_a_share_is_pinned_only_from_a_mounted_share() {
    let f = Fixture::new("pin");
    let inside = format!("{}/Albums", f.shm.name);
    fs::create_dir_all(Path::new("/dev/shm").join(&inside)).unwrap();

    // The share's root is reached through a link here: no pin from it.
    let refused =
        f.apps.configure(&app(), volume(1), &manifest("shm"), &compose(&f.shm.name), &values(&inside)).await;
    assert!(refusal(refused).contains("not mounted"));
    assert!(!f.path("apps/shm.json").exists());
}

#[tokio::test]
async fn what_the_values_do_not_fit_is_refused_before_compose_runs() {
    let f = Fixture::new("values");
    let (manifest, compose) = (manifest("shm"), compose(&f.shm.name));
    let configure = |values: AppValues| {
        let (f, manifest, compose) = (&f, &manifest, &compose);
        async move { refusal(f.apps.configure(&app(), volume(1), manifest, compose, &values).await) }
    };
    let setting = |value: &str| AppSettingValue::new(value).unwrap();

    let mut no_port = values("");
    no_port.ports.clear();
    assert!(configure(no_port).await.contains("ports"));
    let mut extra_share = values("");
    let share = extra_share.shares["media"].clone();
    extra_share.shares.insert("other".into(), share);
    assert!(configure(extra_share).await.contains("shares"));
    let mut reserved = values("");
    reserved.ports.insert("web".into(), 443);
    assert!(configure(reserved).await.contains("not available"));
    let mut empty = values("");
    empty.settings.insert("ADMIN_NAME".into(), setting(""));
    assert!(configure(empty).await.contains("needs a value"));
    let mut number = values("");
    number.settings.insert("WORKERS".into(), setting("many"));
    assert!(configure(number).await.contains("wrong kind"));
    let mut boolean = values("");
    boolean.settings.insert("VERBOSE".into(), setting("yes"));
    assert!(configure(boolean).await.contains("wrong kind"));
    let mut unmounted = values("");
    unmounted.shares.get_mut("media").unwrap().name = ShareName::parse("photos").unwrap();
    assert!(configure(unmounted).await.contains("not mounted"));
    assert_eq!(f.calls(), "", "Compose never ran");
}

#[tokio::test]
async fn an_app_that_is_not_what_it_says_is_refused() {
    let f = Fixture::new("odd");
    let compose = compose(&f.shm.name);
    let refused = |result| refusal(result);

    let other = f.apps.configure(&app(), volume(1), &manifest("other"), &compose, &values("")).await;
    assert!(refused(other).contains("another app's"));
    let broken = AppManifest::new("{").unwrap();
    assert!(
        refused(f.apps.configure(&app(), volume(1), &broken, &compose, &values("")).await)
            .contains("cannot be read")
    );
    let reads = AppCompose::new("services:\n  app:\n    image: x\n    env_file: secrets.env\n").unwrap();
    let refused_source = f.apps.configure(&app(), volume(1), &manifest("shm"), &reads, &values("")).await;
    assert!(refused(refused_source).contains("checks"));
    // No data folder mounted for it on volume 3.
    let unmounted = f.apps.configure(&app(), volume(3), &manifest("shm"), &compose, &values("")).await;
    assert!(refused(unmounted).contains("not mounted"));
    // The app's account is missing.
    fs::write(f.path("passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
    let no_account = f.apps.configure(&app(), volume(1), &manifest("shm"), &compose, &values("")).await;
    assert!(refused(no_account).contains("app-shm is missing"));
}

#[tokio::test]
async fn what_compose_cannot_read_or_answers_oddly_is_refused() {
    let f = Fixture::new("compose");
    let (manifest, compose) = (manifest("shm"), compose(&f.shm.name));

    fs::write(f.path("fail"), "").unwrap();
    let failed = f.apps.configure(&app(), volume(1), &manifest, &compose, &values("")).await;
    assert!(refusal(failed).contains("no such service"), "the end of what Compose said");
    fs::remove_file(f.path("fail")).unwrap();
    fs::write(f.path("garbage"), "").unwrap();
    let garbage = f.apps.configure(&app(), volume(1), &manifest, &compose, &values("")).await;
    assert!(refusal(garbage).contains("unexpected"));
    assert!(!f.path("apps/shm.json").exists(), "nothing kept");
}

#[tokio::test]
async fn a_failing_compose_is_reported_in_its_words() {
    let f = Fixture::new("fails");
    f.apps.configure(&app(), volume(1), &manifest("shm"), &compose(&f.shm.name), &values("")).await.unwrap();
    fs::write(f.path("fail"), "").unwrap();

    let logs = f.apps.logs(&app(), 10).await;
    for result in [f.apps.pull(&app()).await, f.apps.stop(&app()).await, f.apps.status().await, logs] {
        assert!(matches!(&result, Err(e) if e.to_string().contains("it went wrong")), "{result:?}");
    }
    let removed = f.apps.remove(&app()).await.unwrap_err();
    assert!(removed.to_string().contains("it went wrong"));
    assert!(f.path("apps/shm.json").exists(), "kept until Compose took it down");
}

#[tokio::test]
async fn a_start_reaches_every_mount_first() {
    let f = Fixture::new("mounts");
    f.apps.configure(&app(), volume(1), &manifest("shm"), &compose(&f.shm.name), &values("")).await.unwrap();
    f.on_dev_shm();
    let kept = f.kept();
    let write = |changed: &Value| fs::write(f.path("apps/shm.json"), changed.to_string()).unwrap();
    assert_eq!(f.apps.start(&app()).await.unwrap(), Response::Done);

    // The app's folder became a link to the system.
    fs::remove_dir(f.data_folder()).unwrap();
    std::os::unix::fs::symlink("/etc", f.data_folder()).unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("is missing, a link or no folder"));
    fs::remove_file(f.data_folder()).unwrap();
    fs::create_dir(f.data_folder()).unwrap();

    let mut elsewhere = kept.clone();
    elsewhere["services"]["app"]["volumes"][1]["source"] = json!("/etc");
    write(&elsewhere);
    assert!(refusal(f.apps.start(&app()).await).contains("neither the app's data nor one of its shares"));

    let mut socket = kept.clone();
    socket["services"]["app"]["volumes"][1]["source"] = json!("/run/docker.sock");
    write(&socket);
    assert!(refusal(f.apps.start(&app()).await).contains("not given the Docker socket"));
    socket[EXTENSION]["docker_socket"] = json!(true);
    write(&socket);
    assert!(refusal(f.apps.start(&app()).await).contains("missing"), "no socket at the place");
    fs::write(f.path("docker.sock"), "").unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("not Docker's socket"));

    let mut nowhere = kept.clone();
    nowhere[EXTENSION]["app_data"] = json!("");
    write(&nowhere);
    assert!(refusal(f.apps.start(&app()).await).contains("does not say where"));
    fs::write(f.path("apps/shm.json"), "not json").unwrap();
    assert!(refusal(f.apps.start(&app()).await).contains("cannot be read"));
    fs::remove_file(f.path("apps/shm.json")).unwrap();
    fs::create_dir(f.path("apps/shm.json")).unwrap();
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

#[tokio::test]
async fn ports_and_settings_the_manifest_rules_out_are_refused() {
    let f = Fixture::new("manifest");
    let compose = compose(&f.shm.name);
    let configure = |manifest: AppManifest, values: AppValues| {
        let (f, compose) = (&f, &compose);
        async move { refusal(f.apps.configure(&app(), volume(1), &manifest, compose, &values).await) }
    };

    // Only an app on the host's network has fixed ports.
    let fixed = manifest_with("shm", |m| {
        m["ports"][0]["fixed"] = json!(true);
        m["permissions"] = json!({ "host_network": true });
    });
    let refused = configure(fixed, values("")).await;
    assert!(refused.contains("fixed at 8080"), "{refused}");
    let two = manifest_with("shm", |m| {
        m["ports"] = json!([{ "id": "web", "label": "Web", "default": 8080 }, { "id": "api", "label": "API", "default": 8081 }]);
    });
    let mut same = values("");
    same.ports.insert("api".into(), 18_123);
    assert!(configure(two, same).await.contains("given twice"));
    let reserved = manifest_with("shm", |m| {
        m["settings"] = json!([{ "id": "LD_PRELOAD", "label": "Library", "type": "text" }]);
    });
    let mut preload = values("");
    preload.settings = BTreeMap::from([("LD_PRELOAD".to_owned(), AppSettingValue::new("/x.so").unwrap())]);
    let using = compose.as_str().replace(
        "      ADMIN_NAME: ${ADMIN_NAME}\n      WORKERS: ${WORKERS}\n      VERBOSE: ${VERBOSE}\n",
        "      LIBRARY: ${LD_PRELOAD}\n",
    );
    let refused = refusal(
        f.apps.configure(&app(), volume(1), &reserved, &AppCompose::new(using).unwrap(), &preload).await,
    );
    assert!(refused.contains("Docker reads itself"), "{refused}");
    assert_eq!(f.calls(), "", "Compose never ran");
}

#[test]
fn the_facts_a_template_needs_come_from_the_machine() {
    let f = Fixture::new("facts");
    let users = "zendiloos-users:x:1500:\napp-shm:x:2100:\n";
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
        gid: 1500,
        time_zone: "Europe/Berlin".into(),
        devices: vec![device("/dev/dri")],
    };
    assert_eq!(facts, expected);
    let unmanaged = f.apps.facts(&UserName::parse("owner").unwrap(), &asked);
    assert!(unmanaged.is_err(), "the owner's own user is no account of zendiloOS");
    let many = vec![device("/dev/dri"); DevicePath::MAX_COUNT + 1];
    assert!(f.apps.facts(&UserName::parse("alice").unwrap(), &many).is_err());
}
