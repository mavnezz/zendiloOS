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
    };
    let heard = Heard(Default::default());
    let report = host.run(&plan, Some(&Secret::new("s3cret")), &heard, &context()).await.unwrap();
    assert_eq!((report.files, report.transfers.len(), report.entries.len()), (2, 1, 1));
    assert_eq!(*heard.0.lock().unwrap(), [("transferring".to_owned(), Some(40))]);
    let Request::RunBackup { run, password } = scripted.only() else { panic!() };
    assert_eq!(password.unwrap().expose(), "s3cret");
    assert_eq!((run.task.get(), run.folder.as_str()), (3, "Photos nightly"));
    let BackupEnd::Remote { path, .. } = &run.target else { panic!() };
    assert_eq!(path.as_str(), "nas");
    assert!(!run.hidden && run.bwlimit_kib == 100);

    // Values the protocol would not take never reach the agent.
    let mut bad = plan.clone();
    bad.filters.excludes = vec!["- *".into()];
    assert!(matches!(host.run(&bad, None, &heard, &context()).await, Err(HostError::Port(_))));
}

#[tokio::test]
async fn refusals_and_a_changed_host_key_are_told_apart() {
    let at = Location::Remote { server: daemon(), path: String::new() };
    let refused = AgentBackup::new(Arc::new(Scripted::refusing(
        ErrorCode::Forbidden,
        &format!("refused: {BACKUP_HOST_KEY_CHANGED}: Host key verification failed."),
    )));
    assert!(
        matches!(refused.list(&at, None, &context()).await, Err(HostError::HostKeyChanged(detail)) if detail == "Host key verification failed.")
    );
    let other =
        AgentBackup::new(Arc::new(Scripted::refusing(ErrorCode::Forbidden, "refused: holds other files")));
    assert!(
        matches!(other.list(&at, None, &context()).await, Err(HostError::Refused(reason)) if reason == "holds other files")
    );
    let failed = AgentBackup::new(Arc::new(Scripted::refusing(ErrorCode::Failed, "rsync ended with 10")));
    assert!(matches!(failed.list(&at, None, &context()).await, Err(HostError::Failed(_))));
    let missing = AgentBackup::new(Arc::new(Scripted::answering([Response::BackupListing {
        listing: "x".into(),
        missing: true,
    }])));
    assert!(missing.list(&at, None, &context()).await.unwrap().is_empty());
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
        Response::BackupFacts { rsync: true, ssh: true, public_key: None },
        Response::BackupKey { public_key: "ssh-ed25519 AAAA".into() },
        Response::BackupHostKeys {
            keyscan: "h ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
                .into(),
        },
        Response::Done,
        Response::Done,
        Response::BackupRestored { files: 1, bytes: 2, partial: None },
    ]));
    let host = AgentBackup::new(scripted.clone());
    assert!(host.facts(&context()).await.unwrap().rsync);
    assert_eq!(host.make_key(&context()).await.unwrap(), "ssh-ed25519 AAAA");
    assert_eq!(host.host_keys("nas2.lan", 22, &context()).await.unwrap().len(), 1);
    host.stop(3, &context()).await.unwrap();
    let folder = Location::Drive { drive: "1A2B-3C4D".into(), path: "nas/photos".into() };
    host.prune(3, &folder, &["2026-11-01_030000Z".into()], None, &context()).await.unwrap();
    assert!(
        host.prune(3, &folder, &["current".into()], None, &context()).await.is_err(),
        "never what is no version"
    );
    let plan = RestorePlan {
        task: 3,
        from: folder.join("2026-11-01_030000Z"),
        paths: vec!["2024".into()],
        to: Location::Share { volume: 1, share: "photos".into(), path: String::new() },
        existing: Existing::Replace,
    };
    let heard = Heard(Default::default());
    assert_eq!(host.restore(&plan, None, &heard, &context()).await.unwrap().files, 1);
    let Request::RestoreBackup { restore, .. } = scripted.asked().pop().unwrap() else { panic!() };
    assert_eq!(restore.existing, BackupExisting::Replace);
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
