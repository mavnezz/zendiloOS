use zendiloos_backup::application::{CloudCredentials, Crypt};
use zendiloos_backup::domain::Filters;
use zendiloos_kernel::Timestamp;

use super::super::scripted::Scripted;
use super::*;

struct Heard(std::sync::Mutex<Vec<(String, Option<u8>)>>);

impl Steps for Heard {
    fn step(&self, step: &str, percent: Option<u8>) {
        self.0.lock().unwrap().push((step.to_owned(), percent));
    }
}

fn context() -> RequestContext {
    RequestContext::default()
}

fn no_secrets() -> Secrets {
    Secrets::default()
}

fn password(password: &str) -> Secrets {
    Secrets { password: Some(Secret::new(password)), ..Secrets::default() }
}

fn daemon() -> Server {
    Server {
        transport: Transport::Daemon { module: "nas".into() },
        host: "nas2.lan".into(),
        port: 873,
        user: "zendi".into(),
    }
}

#[test]
fn listings_are_read_as_rsync_writes_them() {
    let listing = "drwxr-xr-x           4096 2026/11/11 03:00:00 .\n\
                   drwxr-xr-x           4096 2026/11/11 03:00:00 2026-11-11_030000Z\n\
                   -rw-r--r--          1,024 2026/11/10 12:30:05 holiday photos.zip\n\
                   lrwxrwxrwx              9 2026/11/10 12:30:05 latest -> 2026-11-11\n\
                   -rw-r--r--              3 2026/11/10 12:30:05 tab\\#011name\n\
                   sending incremental file list";
    let entries = entries(listing);
    assert_eq!(entries.len(), 5);
    assert_eq!((entries[1].name.as_str(), entries[1].kind), ("2026-11-11_030000Z", EntryKind::Folder));
    assert_eq!((entries[2].name.as_str(), entries[2].bytes), ("holiday photos.zip", 1024));
    assert_eq!(entries[2].modified, Some(Timestamp::from_unix_seconds(1_794_313_805)));
    assert_eq!((entries[3].name.as_str(), entries[3].kind), ("latest", EntryKind::Link));
    assert_eq!(entries[4].name, "tab\tname");
}

#[test]
fn changes_and_host_keys_are_read() {
    assert_eq!(
        transfer(">f+++++++++ a b.jpg"),
        Some(Transfer { change: ">f+++++++++".into(), path: "a b.jpg".into() })
    );
    assert_eq!(
        transfer("*deleting   old.txt"),
        Some(Transfer { change: "*deleting".into(), path: "old.txt".into() })
    );
    assert_eq!(transfer("short"), None);
    let keyscan = "# nas2.lan:22 SSH-2.0-OpenSSH_10.0\n\
                   nas2.lan ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDx\n\
                   nas2.lan ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n\
                   nas2.lan ssh-dss AAAAB3NzaC1kc3MAAACBAP\n";
    let keys = host_keys(keyscan);
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys[0].line.starts_with("ssh-ed25519 "), "ed25519 first");
    // As `ssh-keygen -lf` computes it.
    assert_eq!(keys[0].fingerprint, "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU");
}

#[tokio::test]
async fn a_run_goes_to_the_agent_as_its_types_and_comes_back_read() {
    let scripted = Arc::new(Scripted::reporting(
        vec![Response::Progress { step: "transferring".into(), percent: Some(40) }],
        Response::BackupRan {
            files: 2,
            bytes: 20,
            partial: None,
            transfers: vec![">f+++++++++ a.jpg".into()],
            transfers_total: 2,
            listing: "drwxr-xr-x           4096 2026/11/11 03:00:00 2026-11-11_030000Z".into(),
            token: None,
        },
    ));
    let host = AgentBackup::new(scripted.clone());
    let plan = RunPlan {
        task: 3,
        source: Location::Share { volume: 1, share: "photos".into(), path: "albums".into() },
        target: Location::Remote { server: daemon(), path: "nas/Photos nightly".into() },
        mode: RunMode::Versioned,
        stamp: "2026-11-11_030000Z".into(),
        filters: Filters { excludes: vec!["*.tmp".into()], hidden: false },
        bwlimit_kib: 100,
        encrypted: false,
    };
    let heard = Heard(Default::default());
    let report = host.run(&plan, &password("s3cret"), &heard, &context()).await.unwrap();
    assert_eq!((report.files, report.transfers.len(), report.entries.len()), (2, 1, 1));
    assert_eq!(*heard.0.lock().unwrap(), [("transferring".to_owned(), Some(40))]);
    let Request::RunBackup { run, secrets } = scripted.only() else { panic!() };
    assert_eq!(secrets.password.unwrap().expose(), "s3cret");
    assert_eq!((run.task.get(), run.folder.as_str()), (3, "Photos nightly"));
    let BackupEnd::Remote { path, .. } = &run.target else { panic!() };
    assert_eq!(path.as_str(), "nas");
    assert!(!run.hidden && run.bwlimit_kib == 100);

    // Values the protocol would not take never reach the agent.
    let mut bad = plan.clone();
    bad.filters.excludes = vec!["- *".into()];
    assert!(matches!(host.run(&bad, &no_secrets(), &heard, &context()).await, Err(HostError::Port(_))));
}

#[tokio::test]
async fn refusals_and_a_changed_host_key_are_told_apart() {
    let at = Location::Remote { server: daemon(), path: String::new() };
    let refused = AgentBackup::new(Arc::new(Scripted::refusing(
        ErrorCode::Forbidden,
        &format!("refused: {BACKUP_HOST_KEY_CHANGED}: Host key verification failed."),
    )));
    assert!(
        matches!(refused.list(&at, "", &no_secrets(), &context()).await, Err(HostError::HostKeyChanged(detail)) if detail == "Host key verification failed.")
    );
    let other =
        AgentBackup::new(Arc::new(Scripted::refusing(ErrorCode::Forbidden, "refused: holds other files")));
    assert!(
        matches!(other.list(&at, "", &no_secrets(), &context()).await, Err(HostError::Refused(reason)) if reason == "holds other files")
    );
    let failed = AgentBackup::new(Arc::new(Scripted::refusing(ErrorCode::Failed, "rsync ended with 10")));
    assert!(matches!(failed.list(&at, "", &no_secrets(), &context()).await, Err(HostError::Failed(_))));
    let missing = AgentBackup::new(Arc::new(Scripted::answering([Response::BackupListing {
        listing: "x".into(),
        missing: true,
        token: None,
    }])));
    assert!(missing.list(&at, "", &no_secrets(), &context()).await.unwrap().entries.is_empty());
}

#[tokio::test]
async fn the_plugin_installs_through_apt_and_keys_are_asked_for() {
    use zendiloos_agent_proto::PackageRun;
    let run = |state| PackageRun {
        id: "ab01".into(),
        task: PackageTask::InstallBackup,
        state,
        step: String::new(),
        started_at: 0,
        finished_at: None,
        error: Some("apt said no".into()),
        report: None,
        release: None,
    };
    let scripted = Arc::new(Scripted::answering([
        Response::PackagesStarted { id: "ab01".into() },
        Response::Packages { run: Some(run(PackageRunState::Succeeded)), update: None },
    ]));
    let host = AgentBackup::new(scripted.clone());
    host.install(&context()).await.unwrap();
    assert!(matches!(scripted.asked()[0], Request::StartPackages { task: PackageTask::InstallBackup, .. }));

    let scripted = Arc::new(Scripted::answering([
        Response::BackupFacts { rsync: true, ssh: true, rclone: false, public_key: None },
        Response::BackupKey { public_key: "ssh-ed25519 AAAA".into() },
        Response::BackupHostKeys {
            keyscan: "h ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
                .into(),
        },
        Response::Done,
        Response::BackupPruned { token: None },
        Response::BackupRestored { files: 1, bytes: 2, partial: None, token: None },
    ]));
    let host = AgentBackup::new(scripted.clone());
    assert!(host.facts(&context()).await.unwrap().rsync);
    assert_eq!(host.make_key(&context()).await.unwrap(), "ssh-ed25519 AAAA");
    assert_eq!(host.host_keys("nas2.lan", 22, &context()).await.unwrap().len(), 1);
    host.stop(3, &context()).await.unwrap();
    let folder = Location::Drive { drive: "1A2B-3C4D".into(), path: "nas/photos".into() };
    host.prune(3, &folder, &["2026-11-01_030000Z".into()], &no_secrets(), &context()).await.unwrap();
    assert!(
        host.prune(3, &folder, &["current".into()], &no_secrets(), &context()).await.is_err(),
        "never what is no version"
    );
    let plan = RestorePlan {
        task: 3,
        from: folder.join("2026-11-01_030000Z"),
        paths: vec!["2024".into()],
        to: Location::Share { volume: 1, share: "photos".into(), path: String::new() },
        existing: Existing::Replace,
        encrypted: false,
    };
    let heard = Heard(Default::default());
    assert_eq!(host.restore(&plan, &no_secrets(), &heard, &context()).await.unwrap().files, 1);
    let Request::RestoreBackup { restore, .. } = scripted.asked().pop().unwrap() else { panic!() };
    assert_eq!(restore.existing, BackupExisting::Replace);
    let BackupEnd::Nas { path, .. } = &restore.from else { panic!() };
    assert_eq!((path.as_str(), restore.within.as_str()), ("nas/photos", "2026-11-01_030000Z"));
}

#[test]
fn a_folder_on_the_root_of_a_server_keeps_its_slash() {
    let at = Location::Remote { server: daemon(), path: "/backups".into() };
    let (parent, folder) = split_folder(&at).unwrap();
    assert_eq!((parent.path(), folder.as_str()), ("/", "backups"));
    let at = Location::Share { volume: 1, share: "photos".into(), path: "a/b".into() };
    let (parent, folder) = split_folder(&at).unwrap();
    assert_eq!((parent.path(), folder.as_str()), ("a", "b"));
}

#[tokio::test]
async fn an_encrypted_cloud_run_carries_its_secrets_and_brings_back_a_refreshed_token() {
    let scripted = Arc::new(Scripted::reporting(
        Vec::new(),
        Response::BackupRan {
            files: 1,
            bytes: 1,
            partial: None,
            transfers: Vec::new(),
            transfers_total: 0,
            listing: String::new(),
            token: Some(BackupSecret::new(r#"{"access_token":"fresh"}"#).unwrap()),
        },
    ));
    let host = AgentBackup::new(scripted.clone());
    let plan = RunPlan {
        task: 4,
        source: Location::Share { volume: 1, share: "photos".into(), path: String::new() },
        target: Location::Cloud { provider: Provider::OneDrive, account: 2, path: "nas/Photos".into() },
        mode: RunMode::Versioned,
        stamp: "2026-11-11_030000Z".into(),
        filters: Filters { excludes: Vec::new(), hidden: true },
        bwlimit_kib: 0,
        encrypted: true,
    };
    let mut secrets = Secrets {
        password: None,
        crypt: Some(Crypt { password: Secret::new("correct horse"), salt: Some(Secret::new("pepper")) }),
        cloud: Some(CloudCredentials {
            provider: Provider::OneDrive,
            token: Secret::new(r#"{"access_token":"old"}"#),
            client_id: None,
            client_secret: None,
            drive_id: Some("b!drive".into()),
            drive_type: Some("business".into()),
        }),
    };
    let heard = Heard(Default::default());
    let report = host.run(&plan, &secrets, &heard, &context()).await.unwrap();
    assert_eq!(report.token.map(|t| t.expose().to_owned()).as_deref(), Some(r#"{"access_token":"fresh"}"#));
    let Request::RunBackup { run, secrets: sent } = scripted.only() else { panic!() };
    assert_eq!(sent.crypt.unwrap().salt.unwrap().expose(), "pepper");
    assert_eq!(sent.cloud.unwrap().drive_id.unwrap().as_str(), "b!drive");
    let BackupEnd::Cloud { cloud, path } = &run.target else { panic!() };
    assert_eq!((*cloud, path.as_str(), run.folder.as_str()), (BackupCloud::OneDrive, "nas", "Photos"));

    // Encrypted without its password: nothing is asked.
    secrets.crypt = None;
    assert!(matches!(host.run(&plan, &secrets, &heard, &context()).await, Err(HostError::Refused(_))));
    // Not encrypted: the crypt stays behind.
    let mut plain = plan.clone();
    plain.encrypted = false;
    secrets.crypt = Some(Crypt { password: Secret::new("x"), salt: None });
    let scripted = Arc::new(Scripted::answering([Response::BackupListing {
        listing: String::new(),
        missing: false,
        token: None,
    }]));
    let host = AgentBackup::new(scripted.clone());
    host.list(&plain.target, "2026-11-10_030000Z/2024", &secrets, &context()).await.unwrap();
    let Request::ListBackup { inside, secrets: sent, .. } = scripted.only() else { panic!() };
    assert_eq!(inside.as_str(), "2026-11-10_030000Z/2024");
    assert!(sent.crypt.is_some(), "a listing decrypts when a crypt is given");
}
