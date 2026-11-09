//! Every request reaches its operation, in the agent of its role, and
//! nowhere else: each is sent as zendiloosd would (JSON), with every
//! program a script that records its call and every place on the host a
//! folder of the test's. What each operation does is tested where it
//! lives; here only that it is the one that answers, and that a request
//! for the other agent is refused before anything runs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zendiloos_agent::config::Tools;
use zendiloos_agent::{
    AccountFiles, AcmePaths, Erasable, ErasePolicy, FirewallPaths, HostPaths, NetworkPaths, Operations,
    Paths, SshPaths, TimePaths, WebPaths, WebServerPaths,
};
use zendiloos_agent_proto::{ErrorCode, Request, Response, Role};

struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        fs::write(dir.path().join("group"), "root:x:0:\nzendiloos-users:x:990:\n").unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A program that records its name and arguments and exits with 1,
    /// unless the file `<name>.ok` is there.
    fn program(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        let log = self.path("calls.log");
        let ok = self.path(&format!("{name}.ok"));
        fs::write(
            &path,
            format!("#!/bin/sh\necho {name} \"$@\" >> {}\n[ -f {} ]\n", log.display(), ok.display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn operations(&self, role: Role) -> Operations {
        let p = |name: &str| self.program(name);
        let tools = Tools {
            systemd_mount: "/bin/false".into(),
            umount: "/bin/false".into(),
            rmdir: "/bin/false".into(),
            systemctl: p("systemctl"),
            lsblk: p("lsblk"),
            smartctl: p("smartctl"),
            zpool: p("zpool"),
            zfs: p("zfs"),
            sgdisk: p("sgdisk"),
            wipefs: p("wipefs"),
            udevadm: p("udevadm"),
            testparm: p("testparm"),
            exportfs: p("exportfs"),
            setfacl: p("setfacl"),
            useradd: p("useradd"),
            userdel: p("userdel"),
            groupadd: p("groupadd"),
            groupdel: p("groupdel"),
            gpasswd: p("gpasswd"),
            smbpasswd: p("smbpasswd"),
            busctl: p("busctl"),
            timedatectl: p("timedatectl"),
            sshd: p("sshd"),
            systemd_run: p("systemd-run"),
            docker: p("docker"),
            setsid: "/usr/bin/setsid".into(),
            openssl: "/bin/false".into(),
            nginx: "/bin/false".into(),
            rm: "/bin/false".into(),
            nft: "/bin/false".into(),
            dockerd: p("dockerd"),
        };
        let at = |name: &str| self.path(name);
        let host = HostPaths {
            dev: at("dev"),
            by_id: at("by-id"),
            sys_block: at("sys-block"),
            dmi: at("dmi"),
            mountinfo: at("mountinfo"),
            swaps: at("swaps"),
            units: at("units"),
            volumes: at("volumes"),
            samba_config: at("samba/zendiloos.conf"),
            nfs_exports: at("exports.d/zendiloos.exports"),
            console_issue: at("issue.d/zendiloos.issue"),
            docker_config: at("docker/daemon.json"),
            apps: at("apps"),
            projects: at("projects"),
            localtime: at("localtime"),
            app_shares: at("app-shares"),
            project_shares: at("project-shares"),
            removable: at("removable"),
            agent: at("agent"),
            packages: at("packages"),
            docker_socket: at("docker.sock"),
            update_spool: at("spool"),
            release_keys: vec![at("release.pub")],
            share_mounts: true,
        };
        let paths = Paths {
            accounts: AccountFiles { passwd: at("passwd"), group: at("group"), high_water: at("high-water") },
            host,
            network: NetworkPaths { directory: at("network") },
            time: TimePaths { servers: at("timesyncd.conf") },
            ssh: SshPaths { config: at("sshd_config.d/00-zendiloos.conf") },
            web: WebPaths { tls: at("nginx/tls.conf") },
            acme: AcmePaths {
                script: at("acme/acme.sh"),
                state: at("acme-state"),
                runtime: at("acme-run"),
                certificates: at("web/certificates"),
            },
            web_server: WebServerPaths {
                directory: at("web"),
                certificates: at("web/certificates"),
                own_certificate: at("tls/zendiloos.crt"),
                own_key: at("tls/zendiloos.key"),
                webroot: at("acme-state/webroot"),
            },
            firewall: FirewallPaths { directory: at("firewall") },
        };
        Operations::with_paths(role, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap()
    }
}

/// Requests as zendiloosd sends them, one of each kind that needs no
/// structure of its own; `op` is also what `Request::name` says.
fn requests() -> Vec<Value> {
    let tank = json!("tank");
    vec![
        json!({ "op": "list_block_devices" }),
        json!({ "op": "machine" }),
        json!({ "op": "read_burst", "device": "sdz", "millis": 700 }),
        json!({ "op": "smart_data", "device": "sdz" }),
        json!({ "op": "start_self_test", "device": "sdz", "kind": "short" }),
        json!({ "op": "start_self_test", "device": "sdz", "kind": "long" }),
        json!({ "op": "abort_self_test", "device": "sdz" }),
        json!({ "op": "zfs_status" }),
        json!({ "op": "destroy_pool", "name": tank, "volume": 1 }),
        json!({ "op": "scrub_pool", "pool": tank }),
        json!({ "op": "stop_scrub", "pool": tank }),
        json!({ "op": "create_snapshot", "pool": tank, "volume": 1, "share": "media", "label": "zdl-2026-11-07_08.00.00" }),
        json!({ "op": "destroy_snapshot", "pool": tank, "volume": 1, "share": "media", "label": "zdl-2026-11-07_08.00.00" }),
        json!({ "op": "rollback_snapshot", "pool": tank, "volume": 1, "share": "media", "label": "zdl-2026-11-07_08.00.00" }),
        json!({ "op": "list_snapshots" }),
        json!({ "op": "docker_state" }),
        json!({ "op": "enable_docker", "pool": tank, "volume": 1 }),
        json!({ "op": "move_docker", "from_pool": tank, "from_volume": 1, "to_pool": "cold", "to_volume": 2 }),
        json!({ "op": "start_app", "app": "shm" }),
        json!({ "op": "stop_app", "app": "shm" }),
        json!({ "op": "remove_app", "app": "shm", "pool": tank, "volume": 1 }),
        json!({ "op": "delete_app_account", "app": "shm" }),
        json!({ "op": "apps_status" }),
        json!({ "op": "app_logs", "app": "shm", "lines": 10 }),
        json!({ "op": "start_packages", "task": "check" }),
        json!({ "op": "packages_state" }),
        json!({ "op": "docker_containers" }),
        json!({ "op": "docker_stats" }),
        json!({ "op": "docker_images" }),
        json!({ "op": "docker_networks" }),
        json!({ "op": "docker_disk_usage" }),
        json!({ "op": "remove_image", "image": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" }),
        json!({ "op": "pull_image", "reference": "nginx:1.27" }),
        json!({ "op": "prune_images" }),
        json!({ "op": "remove_network", "name": "proxy" }),
        json!({ "op": "control_project", "project": "tesla", "action": "down" }),
        json!({ "op": "control_project", "project": "tesla", "action": "restart" }),
        json!({ "op": "project_logs", "project": "tesla", "lines": 10 }),
        json!({ "op": "project_files", "project": "tesla" }),
        json!({ "op": "network_state" }),
        json!({ "op": "confirm_network" }),
        json!({ "op": "ssh_state" }),
        json!({ "op": "web_tls_state" }),
        json!({ "op": "set_web_tls", "minimum": "tls1_3" }),
        json!({ "op": "acme_state" }),
        json!({ "op": "issue_certificate", "order": {
            "certificate": 3, "account": 1, "ca": "lets_encrypt_staging", "email": "admin@example.org",
            "domains": ["nas.example.org"], "key": "ec256", "challenge": { "method": "http" },
        } }),
        json!({ "op": "remove_certificate", "certificate": 3 }),
        json!({ "op": "remove_acme_account", "account": 1 }),
        json!({ "op": "web_server_state" }),
        json!({ "op": "apply_web_server", "config": { "http": 80, "https": 443, "certificate": null }, "confirm_within": 90 }),
        json!({ "op": "confirm_web_server" }),
        json!({ "op": "reload_web_certificate" }),
        json!({ "op": "firewall_state" }),
        json!({ "op": "apply_firewall", "config": { "rules": [{
            "interface": "enp1s0", "source": { "kind": "network", "address": "192.168.1.0", "prefix": 24 },
            "traffic": { "kind": "tcp", "ports": [{ "from": 445, "to": 445 }] }, "forward": false, "verdict": "accept",
        }], "fallback": "drop" }, "confirm_within": 90 }),
        json!({ "op": "apply_firewall", "config": null, "confirm_within": null }),
        json!({ "op": "confirm_firewall" }),
        json!({ "op": "time_state" }),
        json!({ "op": "share_usage" }),
        json!({ "op": "show_on_console" }),
        json!({ "op": "create_share", "pool": tank, "volume": 1, "name": "media" }),
        json!({ "op": "destroy_share", "pool": tank, "volume": 1, "name": "media" }),
        json!({ "op": "prepare_homes", "pool": tank, "volume": 1 }),
        json!({ "op": "set_home_quota", "pool": tank, "volume": 1, "user": "anna", "quota_bytes": null }),
        json!({ "op": "create_user", "name": "anna" }),
        json!({ "op": "delete_user", "name": "anna" }),
        json!({ "op": "set_user_enabled", "name": "anna", "enabled": false }),
        json!({ "op": "create_group", "name": "family" }),
        json!({ "op": "delete_group", "name": "family" }),
        json!({ "op": "set_group_members", "group": "family", "members": ["anna"] }),
        // The files agent's.
        json!({ "op": "make_home", "volume": 1, "user": "anna" }),
        json!({ "op": "set_share_access", "volume": 1, "name": "media", "access": [], "removed": [], "recursive": false }),
        json!({ "op": "list_directory", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": "" } }),
        json!({ "op": "open_file", "user": "anna", "location": { "volume": 1, "root": "home", "path": "a.txt" } }),
        json!({ "op": "make_directory", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": "new" } }),
        json!({ "op": "remove_entry", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": "a" }, "recycle": true }),
        json!({ "op": "create_upload", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": "x.bin" } }),
        json!({ "op": "discard_upload", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": ".zendiloos-upload-1" } }),
        json!({ "op": "restore_entry", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": ".recycle/anna/a" } }),
        json!({ "op": "archive_folder", "user": "anna", "location": { "volume": 1, "root": { "share": "media" }, "path": "Films" } }),
        json!({ "op": "move_entry", "user": "anna", "from": { "volume": 1, "root": { "share": "media" }, "path": "a" },
                "to": { "volume": 1, "root": "home", "path": "a" }, "replace": true }),
        json!({ "op": "copy_entry", "user": "anna", "from": { "volume": 1, "root": { "share": "media" }, "path": "a" },
                "to": { "volume": 2, "root": { "share": "backup" }, "path": "a" } }),
        // The system agent's, with more to say.
        json!({ "op": "create_pool", "name": "cold", "layout": "mirror", "disks": ["ata-ONE", "ata-TWO"], "volume": 2 }),
        json!({ "op": "replace_disk", "pool": "tank", "device": 1234, "disk": "ata-NEW" }),
        json!({ "op": "control_container", "container": "d853eed323e7", "action": "restart" }),
        json!({ "op": "container_logs", "container": "teslamate_db", "lines": 20 }),
        json!({ "op": "create_network", "name": "proxy", "subnet": "172.30.0.0/24", "gateway": "172.30.0.1" }),
        json!({ "op": "docker_volumes" }),
        json!({ "op": "docker_volume_sizes" }),
        json!({ "op": "remove_volume", "name": "teslamate_db" }),
        json!({ "op": "prune_volumes" }),
        json!({ "op": "create_volume", "name": "teslamate_db", "labels": { "com.docker.compose.project": "tesla" } }),
        json!({ "op": "back_up_volume", "volume": "teslamate_db", "share": { "volume": 1, "share": "media" },
                "folder": "backups", "name": "teslamate_db-20261108-030000" }),
        json!({ "op": "restore_volume", "volume": "teslamate_db", "share": { "volume": 1, "share": "media" },
                "archive": "backups/teslamate_db/teslamate_db-20261108-030000.tar.zst", "sha256": "ab".repeat(32),
                "data_bytes": 4096, "entries": 12 }),
        json!({ "op": "end_volume_work", "volume": "teslamate_db" }),
        json!({ "op": "remove_volume_backup", "share": { "volume": 1, "share": "media" },
                "path": "backups/teslamate_db/teslamate_db-20261108-030000.json", "device": 41, "inode": 7 }),
        json!({ "op": "check_compose", "project": "tesla", "directory": "/volume1/docker/tesla", "compose": "services: {}\n" }),
        json!({ "op": "deploy_project", "project": "tesla", "config": "{\"services\":{}}", "pull": true }),
        json!({ "op": "set_time", "zone": "Europe/Berlin", "network_time": true, "servers": ["ptbtime1.ptb.de"] }),
        json!({ "op": "set_share_properties", "pool": tank, "volume": 1, "share": "media", "quota_bytes": 1_073_741_824u64,
                "compression": "zstd", "record_size": "m1" }),
        json!({ "op": "show_on_console", "setup_code": "ABCD-EFGH-JKLM" }),
        json!({ "op": "show_on_console", "open_setup": true }),
        json!({ "op": "set_smb_password", "name": "anna", "password": "correct horse battery" }),
        json!({ "op": "start_packages", "task": "update", "release": "zendiloos_9.9.9_amd64.deb" }),
    ]
}

#[tokio::test]
async fn every_request_is_answered_by_its_own_operation() {
    let host = Host::new();
    let (system, files) = (host.operations(Role::System), host.operations(Role::Files));
    for json in requests() {
        let op = json["op"].as_str().unwrap().to_owned();
        let request: Request = serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("{op}: {e}"));
        assert_eq!(request.name(), op, "{json}");
        // And back, as the agent answers in its log: the same request.
        let written = serde_json::to_value(&request).unwrap();
        assert_eq!(serde_json::from_value::<Request>(written).unwrap(), request, "{op}");
        let (own, other) = match request.role() {
            Role::System => (&system, &files),
            Role::Files => (&files, &system),
        };
        let refused = other.execute(&request).await;
        assert!(
            matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.contains("is not for")),
            "{op} in the other agent: {refused:?}"
        );
        // Answered, whatever the stand-in programs make of it.
        let _ = own.execute(&request).await;
    }
}

#[tokio::test]
async fn an_app_whose_containers_stay_keeps_its_data() {
    let host = Host::new();
    let system = host.operations(Role::System);
    fs::create_dir_all(host.path("apps")).unwrap();
    fs::write(host.path("apps/shm.json"), "{}").unwrap();
    let remove: Request =
        serde_json::from_value(json!({ "op": "remove_app", "app": "shm", "pool": "tank", "volume": 1 }))
            .unwrap();

    // Compose could not take it down: its dataset is not touched.
    assert!(matches!(system.execute(&remove).await, Response::Error { .. }));
    let calls = fs::read_to_string(host.path("calls.log")).unwrap();
    assert!(calls.contains("down --rmi all") && !calls.contains("zfs"), "{calls}");
}

#[tokio::test]
async fn a_request_for_the_other_agent_runs_nothing() {
    let host = Host::new();
    let files = host.operations(Role::Files);
    for op in ["reboot", "power_off", "list_block_devices", "docker_state"] {
        let request: Request = serde_json::from_value(json!({ "op": op })).unwrap();
        assert!(matches!(files.execute(&request).await, Response::Error { code: ErrorCode::Forbidden, .. }));
    }
    assert!(!Path::new(&host.path("calls.log")).exists(), "nothing ran");
    let hello = files.execute(&serde_json::from_value(json!({ "op": "hello" })).unwrap()).await;
    assert!(matches!(hello, Response::Hello { .. }), "the handshake works with both");
}

#[tokio::test]
async fn files_are_only_handled_for_users_zendiloos_manages() {
    let host = Host::new();
    let files = host.operations(Role::Files);
    fs::write(
        host.path("passwd"),
        "root:x:0:0:root:/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\nanna:x:2001:990::/nonexistent:/usr/sbin/nologin\n",
    )
    .unwrap();
    let location = json!({ "volume": 1, "root": { "share": "media" }, "path": "" });
    for user in ["root", "debian", "nobody"] {
        let request: Request =
            serde_json::from_value(json!({ "op": "list_directory", "user": user, "location": location }))
                .unwrap();
        let answer = files.execute(&request).await;
        assert!(matches!(&answer, Response::Error { code: ErrorCode::Forbidden, .. }), "{user}: {answer:?}");
    }
    // A managed user, but the share is not there.
    let request: Request =
        serde_json::from_value(json!({ "op": "list_directory", "user": "anna", "location": location }))
            .unwrap();
    assert!(matches!(files.execute(&request).await, Response::Error { .. }));
    // Snapshots cannot be changed.
    let snapshot = json!({ "volume": 1, "root": { "share": "media" }, "path": "x", "snapshot": "zdl-2026-11-07_08.00.00" });
    for op in ["make_directory", "remove_entry", "create_upload", "discard_upload", "restore_entry"] {
        let mut json = json!({ "op": op, "user": "anna", "location": snapshot });
        if op == "remove_entry" {
            json["recycle"] = json!(false);
        }
        let answer = files.execute(&serde_json::from_value(json).unwrap()).await;
        assert!(
            matches!(&answer, Response::Error { code: ErrorCode::InvalidRequest, message } if message.contains("snapshot")),
            "{op}: {answer:?}"
        );
    }
}
