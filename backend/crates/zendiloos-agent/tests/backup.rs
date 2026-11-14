//! Backup tasks against a fake host: `systemd-run` and `systemctl`
//! replaced by scripts that record their arguments and what came on
//! standard input, and answer as a backup's unit would; the volumes, the
//! drives' place and the plugin's state in a temporary directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{BackupPaths, Erasable, ErasePolicy, HostPaths, Operations, Paths};
use zendiloos_agent_proto::{
    BackupCloud, BackupCloudAccess, BackupCrypt, BackupEnd, BackupExisting, BackupFolder, BackupHostKey,
    BackupHostName, BackupMode, BackupPassword, BackupPattern, BackupPruned, BackupRemotePath, BackupRestore,
    BackupRoot, BackupRun, BackupSecret, BackupSecrets, BackupServer, BackupStamp, BackupTaskId,
    BackupTransport, BackupWord, ErrorCode, RemovableId, Request, Response, Role, ShareLocation, ShareName,
    SharePath, VolumeNumber,
};

const DRIVE: &str = "1A2B-3C4D";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    operations: Operations,
}

impl Host {
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.path("calls.log")).unwrap_or_default();
        log.lines().map(|line| line.replace(&self.root.display().to_string(), "")).collect()
    }

    /// What the last unit was given on standard input.
    fn input(&self) -> String {
        fs::read_to_string(self.path("stdin")).unwrap_or_default()
    }

    async fn run(&self, request: Request) -> Response {
        self.operations.execute(&request).await
    }
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let log = root.join("calls.log");
    fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Shares `photos` (volume 1) and `backups` (volume 2), a drive's place,
/// rsync and the SSH tools there unless `installed` is false. What a unit
/// answers comes from `answer` in the folder (the unit's lines).
fn host(installed: bool) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in ["volume1/photos", "volume2/backups", "removable/1A2B-3C4D", "state", "units"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    let systemd_run = script(
        &root,
        "systemd-run",
        &format!(
            "cat > {root}/stdin\n\
             case \"$*\" in\n\
             *ssh-keyscan*) echo '# nas2.lan:22 SSH-2.0-OpenSSH_10.0'; echo 'nas2.lan ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl' ;;\n\
             *ssh-keygen*) eval last=\\${{$#}}; echo 'private' > \"$last\"; echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv1zDdNJ6P8b8HFqUvNiOXIdk9JKLy6vJwDGP1vbTqL zendiloos-backup' > \"$last.pub\" ;;\n\
             *backup-unit*) cat {root}/answer ;;\n\
             esac",
            root = root.display()
        ),
    );
    let tools = Tools {
        systemd_run,
        systemctl: script(&root, "systemctl", ""),
        rsync: if installed { script(&root, "rsync", "") } else { root.join("no-rsync") },
        rclone: if installed { script(&root, "rclone", "") } else { root.join("no-rclone") },
        ssh: script(&root, "ssh", ""),
        ssh_keygen: script(&root, "ssh-keygen", ""),
        ssh_keyscan: script(&root, "ssh-keyscan", ""),
        upsd: "/bin/false".into(),
        upsmon: "/bin/false".into(),
        nut_scanner: "/bin/false".into(),
        ..Tools::default()
    };
    let paths = Paths {
        host: HostPaths {
            units: root.join("units"),
            volumes: root.clone(),
            removable: root.join("removable"),
            agent: PathBuf::from("/usr/libexec/zendiloos/zendiloos-agent"),
            share_mounts: false,
            ..HostPaths::default()
        },
        backup: BackupPaths {
            state: root.join("state"),
            credentials: PathBuf::from("/run/credentials"),
            transient: root.join("transient"),
        },
        ..Paths::default()
    };
    let operations =
        Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
    Host { _dir: dir, root, operations }
}

/// What a unit answers: its lines.
fn answer(host: &Host, lines: &[&str]) {
    fs::write(host.path("answer"), lines.join("\n") + "\n").unwrap();
}

fn report(json: &str) -> Vec<String> {
    // In two parts, as a long answer comes.
    let at = json.len() / 2;
    vec![
        serde_json::json!({ "part": &json[..at] }).to_string(),
        serde_json::json!({ "part": &json[at..] }).to_string(),
        "\"done\"".to_owned(),
    ]
}

fn share(volume: u16, name: &str, path: &str) -> BackupEnd {
    BackupEnd::Nas {
        root: BackupRoot::Share {
            location: ShareLocation {
                volume: VolumeNumber::new(volume).unwrap(),
                share: ShareName::parse(name).unwrap(),
            },
        },
        path: SharePath::parse(path).unwrap(),
    }
}

fn server(transport: BackupTransport, path: &str) -> BackupEnd {
    BackupEnd::Remote {
        server: BackupServer {
            transport,
            host: BackupHostName::parse("nas2.lan").unwrap(),
            port: 873,
            user: BackupWord::parse("zendi").unwrap(),
        },
        path: BackupRemotePath::parse(path).unwrap(),
    }
}

fn run(source: BackupEnd, target: BackupEnd) -> BackupRun {
    BackupRun {
        task: BackupTaskId::new(7).unwrap(),
        source,
        target,
        folder: BackupFolder::parse("photos").unwrap(),
        mode: BackupMode::Versioned,
        stamp: BackupStamp::parse("2026-11-11_030000Z").unwrap(),
        excludes: vec![BackupPattern::parse("*.tmp").unwrap()],
        hidden: true,
        bwlimit_kib: 0,
    }
}

fn unit_call(host: &Host) -> String {
    host.calls().into_iter().rev().find(|call| call.contains("backup-unit")).unwrap()
}

#[tokio::test]
async fn the_plugin_says_what_is_installed_and_makes_its_key_once() {
    let host = host(true);
    let Response::BackupFacts { rsync, ssh, rclone, public_key } = host.run(Request::BackupFacts {}).await
    else {
        panic!()
    };
    assert!(rsync && ssh && rclone && public_key.is_none());
    let Response::BackupKey { public_key } = host.run(Request::MakeBackupKey {}).await else { panic!() };
    assert!(public_key.starts_with("ssh-ed25519 "));
    let keygen = host.calls().into_iter().find(|call| call.contains("ssh-keygen")).unwrap();
    for property in
        ["StateDirectory=zendiloos-backup", "StateDirectoryMode=0700", "PrivateNetwork=yes", "UMask=0077"]
    {
        assert!(keygen.contains(property), "{property}: {keygen}");
    }
    let made = host.calls().len();
    host.run(Request::MakeBackupKey {}).await;
    assert_eq!(host.calls().len(), made, "made once");

    let missing = host_without_rsync().await;
    assert!(matches!(missing, Response::Error { code: ErrorCode::Failed, .. }), "{missing:?}");
}

async fn host_without_rsync() -> Response {
    let host = host(false);
    let Response::BackupFacts { rsync, .. } = host.run(Request::BackupFacts {}).await else { panic!() };
    assert!(!rsync);
    host.run(Request::ListBackup {
        at: share(1, "photos", ""),
        inside: SharePath::default(),
        secrets: BackupSecrets::default(),
    })
    .await
}

#[tokio::test]
async fn host_keys_are_read_in_a_unit_with_the_network_as_nobody() {
    let host = host(true);
    let request = Request::ScanBackupHostKeys { host: BackupHostName::parse("nas2.lan").unwrap(), port: 22 };
    let Response::BackupHostKeys { keyscan } = host.run(request).await else { panic!() };
    assert!(keyscan.contains("ssh-ed25519 AAAA"));
    let call = host.calls().into_iter().find(|call| call.contains("ssh-keyscan")).unwrap();
    for property in ["DynamicUser=yes", "PrivateNetwork=no", "CapabilityBoundingSet= ", "-p 22 -- nas2.lan"] {
        assert!(call.contains(property), "{property}: {call}");
    }
}

#[tokio::test]
async fn a_local_run_sees_only_its_roots_and_writes_only_its_target() {
    let host = host(true);
    let json = serde_json::json!({
        "files": 3, "bytes": 300, "partial": null, "transfers": [">f+++++++++ a.jpg"],
        "transfers_total": 3, "listing": "drwxr-xr-x 4096 2026/11/11 03:00:00 2026-11-11_030000Z", "missing": false
    })
    .to_string();
    answer(&host, &report(&json).iter().map(String::as_str).collect::<Vec<_>>());
    let request = Request::RunBackup {
        run: run(share(1, "photos", "albums"), share(2, "backups", "")),
        secrets: BackupSecrets::default(),
    };
    let Response::BackupRan { files, transfers, listing, .. } = host.run(request).await else { panic!() };
    assert_eq!((files, transfers.len()), (3, 1));
    assert!(listing.contains("2026-11-11_030000Z"));
    let call = unit_call(&host);
    for expected in [
        "--unit=zendiloos-backup-run-7.service",
        "--property=TemporaryFileSystem=/volume1:ro",
        "--property=TemporaryFileSystem=/volume2:ro",
        "--property=TemporaryFileSystem=/removable:ro",
        "--property=BindReadOnlyPaths=/volume1/photos",
        "--property=BindPaths=/volume2/backups",
        "--property=PrivateNetwork=yes",
        "--property=RestrictAddressFamilies=AF_UNIX ",
        "--property=NoNewPrivileges=yes",
        "--property=ProtectSystem=strict",
        "--property=InaccessiblePaths=-/etc/shadow",
        "-/var/lib/zendiloos ",
        "--property=CapabilityBoundingSet=CAP_CHOWN CAP_DAC_OVERRIDE",
        "backup-unit ",
    ] {
        assert!(call.contains(expected), "{expected}: {call}");
    }
    assert!(!call.contains("LoadCredential"), "no key without a server: {call}");
}

#[tokio::test]
async fn a_password_comes_on_stdin_and_never_on_a_command_line() {
    let host = host(true);
    answer(&host, &report(r#"{"files":0,"bytes":0,"partial":null,"transfers":[],"transfers_total":0,"listing":"","missing":true}"#).iter().map(String::as_str).collect::<Vec<_>>());
    let daemon = BackupTransport::Daemon { module: BackupWord::parse("nas").unwrap() };
    let request = Request::ListBackup {
        at: server(daemon.clone(), "photos"),
        inside: SharePath::default(),
        secrets: BackupSecrets {
            password: Some(BackupPassword::new("s3cret").unwrap()),
            ..BackupSecrets::default()
        },
    };
    let Response::BackupListing { missing, .. } = host.run(request).await else { panic!() };
    assert!(missing);
    assert!(host.calls().iter().all(|call| !call.contains("s3cret")));
    assert_eq!(host.input().trim(), r#"{"password":"s3cret","crypt":null,"cloud":null}"#);
    let call = unit_call(&host);
    for expected in ["--property=PrivateNetwork=no", "AF_INET AF_INET6", "--property=CapabilityBoundingSet= "]
    {
        assert!(call.contains(expected), "{expected}: {call}");
    }

    // A push reads the share and needs nothing else.
    answer(&host, &report(r#"{"files":0,"bytes":0,"partial":null,"transfers":[],"transfers_total":0,"listing":"","missing":false}"#).iter().map(String::as_str).collect::<Vec<_>>());
    let push = Request::RunBackup {
        run: run(share(1, "photos", ""), server(daemon, "")),
        secrets: BackupSecrets::default(),
    };
    assert!(matches!(host.run(push).await, Response::BackupRan { .. }));
    let call = unit_call(&host);
    assert!(call.contains("--property=CapabilityBoundingSet=CAP_DAC_READ_SEARCH "), "{call}");
    assert!(
        call.contains("--property=BindReadOnlyPaths=/volume1/photos") && !call.contains("BindPaths="),
        "{call}"
    );
}

#[tokio::test]
async fn ssh_needs_the_key_and_a_changed_host_key_is_refused_as_such() {
    let host = host(true);
    let ssh = BackupTransport::Ssh {
        host_key: BackupHostKey::parse(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        )
        .unwrap(),
    };
    let request = Request::ListBackup {
        at: server(ssh.clone(), "/srv"),
        inside: SharePath::default(),
        secrets: BackupSecrets::default(),
    };
    let Response::Error { message, .. } = host.run(request.clone()).await else { panic!() };
    assert!(message.contains("no SSH key"), "{message}");
    host.run(Request::MakeBackupKey {}).await;
    answer(&host, &[r#"{"failed":{"message":"Host key verification failed.","kind":"host_key"}}"#]);
    let Response::Error { code, message } = host.run(request).await else { panic!() };
    assert_eq!(code, ErrorCode::Forbidden);
    assert!(message.contains("the server's host key changed: Host key verification failed."), "{message}");
    let call = unit_call(&host);
    assert!(call.contains("--property=LoadCredential=backup-key:/state/id_ed25519"), "{call}");
}

#[tokio::test]
async fn drives_restores_retention_and_stops() {
    let host = host(true);
    let drive = BackupEnd::Nas {
        root: BackupRoot::Drive { drive: RemovableId::parse(DRIVE).unwrap() },
        path: SharePath::parse("nas").unwrap(),
    };
    answer(&host, &report(r#"{"files":2,"bytes":20,"partial":null,"transfers":[],"transfers_total":0,"listing":"","missing":false}"#).iter().map(String::as_str).collect::<Vec<_>>());
    let restore = Request::RestoreBackup {
        restore: BackupRestore {
            task: BackupTaskId::new(7).unwrap(),
            from: drive.clone(),
            within: SharePath::parse("current").unwrap(),
            paths: vec![SharePath::parse("2024").unwrap()],
            to: share(1, "photos", "restored"),
            existing: BackupExisting::Skip,
        },
        secrets: BackupSecrets::default(),
    };
    let Response::BackupRestored { files, .. } = host.run(restore).await else { panic!() };
    assert_eq!(files, 2);
    let call = unit_call(&host);
    assert!(call.contains("--unit=zendiloos-backup-restore-7.service"), "{call}");
    assert!(
        call.contains("BindReadOnlyPaths=/removable/1A2B-3C4D") && call.contains("BindPaths=/volume1/photos"),
        "{call}"
    );

    let prune = Request::PruneBackup {
        task: BackupTaskId::new(7).unwrap(),
        at: drive.clone(),
        names: vec![BackupPruned::parse("2026-11-01_030000Z").unwrap()],
        secrets: BackupSecrets::default(),
    };
    assert_eq!(host.run(prune).await, Response::BackupPruned { token: None });
    assert!(unit_call(&host).contains("--unit=zendiloos-backup-prune-7.service"));

    // Into a drive from a server: refused where a backup cannot go.
    let from_drive =
        Request::RunBackup { run: run(drive, share(2, "backups", "")), secrets: BackupSecrets::default() };
    let Response::Error { code, .. } = host.run(from_drive).await else { panic!() };
    assert_eq!(code, ErrorCode::Forbidden);

    assert_eq!(host.run(Request::StopBackup { task: BackupTaskId::new(7).unwrap() }).await, Response::Done);
    assert!(host.calls().iter().any(|call| call == "systemctl stop zendiloos-backup-run-7.service"));
}

#[tokio::test]
async fn units_left_running_are_stopped_as_the_agent_starts() {
    let host = host(true);
    host.operations.recover().await;
    assert!(
        !host.calls().iter().any(|call| call.contains("zendiloos-backup")),
        "none there: {:?}",
        host.calls()
    );
    fs::create_dir_all(host.path("transient")).unwrap();
    for name in ["zendiloos-backup-run-7.service", "zendiloos-backup-look-a1b2.service", "other.service"] {
        fs::write(host.path("transient").join(name), "").unwrap();
    }
    host.operations.recover().await;
    assert!(
        host.calls().contains(
            &"systemctl stop zendiloos-backup-look-a1b2.service zendiloos-backup-run-7.service".to_owned()
        ),
        "{:?}",
        host.calls()
    );
}

#[tokio::test]
async fn a_unit_that_ends_without_an_answer_fails() {
    let host = host(true);
    answer(&host, &[]);
    let Response::Error { code, message } = host
        .run(Request::ListBackup {
            at: share(1, "photos", ""),
            inside: SharePath::default(),
            secrets: BackupSecrets::default(),
        })
        .await
    else {
        panic!()
    };
    assert_eq!(code, ErrorCode::Failed);
    assert!(message.contains("ended unexpectedly"), "{message}");
}

#[tokio::test]
async fn a_cloud_is_backed_up_with_rclone_its_secrets_on_stdin_and_a_refreshed_token_answered() {
    let host = host(true);
    let answered = r#"{"files":1,"bytes":5,"partial":null,"transfers":[],"transfers_total":0,"listing":"","missing":false,"token":"{\"access_token\":\"fresh\"}"}"#;
    answer(&host, &report(answered).iter().map(String::as_str).collect::<Vec<_>>());
    let cloud = BackupEnd::Cloud { cloud: BackupCloud::GoogleDrive, path: SharePath::parse("nas").unwrap() };
    let secrets = BackupSecrets {
        password: None,
        crypt: Some(BackupCrypt { password: BackupSecret::new("correct horse").unwrap(), salt: None }),
        cloud: Some(BackupCloudAccess {
            token: BackupSecret::new(r#"{"access_token":"old"}"#).unwrap(),
            client_id: None,
            client_secret: Some(BackupSecret::new("own client secret").unwrap()),
            drive_id: None,
            drive_type: None,
        }),
    };
    let request =
        Request::RunBackup { run: run(share(1, "photos", ""), cloud.clone()), secrets: secrets.clone() };
    let Response::BackupRan { token, .. } = host.run(request).await else { panic!() };
    assert_eq!(token.map(|t| t.expose().to_owned()).as_deref(), Some(r#"{"access_token":"fresh"}"#));
    for secret in ["correct horse", "own client secret", "access_token"] {
        assert!(host.calls().iter().all(|call| !call.contains(secret)), "{secret} on a command line");
        assert!(host.input().contains(secret), "{secret} on stdin");
    }
    let call = unit_call(&host);
    for expected in [
        "--property=PrivateNetwork=no",
        "--property=CapabilityBoundingSet=CAP_DAC_READ_SEARCH ",
        "--property=BindReadOnlyPaths=/volume1/photos",
    ] {
        assert!(call.contains(expected), "{expected}: {call}");
    }
    assert!(!call.contains("BindPaths=") && !call.contains("LoadCredential"), "{call}");

    // Not from a cloud, nor encrypted to an rsync daemon.
    let from_cloud =
        Request::RunBackup { run: run(cloud, share(2, "backups", "")), secrets: secrets.clone() };
    let Response::Error { code, .. } = host.run(from_cloud).await else { panic!() };
    assert_eq!(code, ErrorCode::Forbidden);
    let daemon = BackupTransport::Daemon { module: BackupWord::parse("nas").unwrap() };
    let to_daemon = Request::RunBackup { run: run(share(1, "photos", ""), server(daemon, "")), secrets };
    let Response::Error { code, message } = host.run(to_daemon).await else { panic!() };
    assert_eq!(code, ErrorCode::Forbidden, "{message}");
}

#[tokio::test]
async fn without_rclone_a_cloud_cannot_be_reached() {
    let host = host(false);
    let cloud = BackupEnd::Cloud { cloud: BackupCloud::OneDrive, path: SharePath::default() };
    let request =
        Request::ListBackup { at: cloud, inside: SharePath::default(), secrets: BackupSecrets::default() };
    let Response::Error { message, .. } = host.run(request).await else { panic!() };
    assert!(message.contains("rclone is not installed"), "{message}");
}
