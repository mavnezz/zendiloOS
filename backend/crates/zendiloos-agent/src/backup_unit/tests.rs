//! The unit's work against real rsync where the machine has it (local
//! places in a temporary folder), and against a fake rsync that records
//! its arguments for servers.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::*;
use crate::volume_archive::encode;

const RSYNC: &str = "/usr/bin/rsync";

fn real_rsync() -> Option<PathBuf> {
    Path::new(RSYNC).is_file().then(|| PathBuf::from(RSYNC))
}

fn local(root: &Path, path: &str) -> End {
    End::Local { root: root.to_owned(), path: SharePath::parse(path).unwrap() }
}

fn job(work: Work, rsync: PathBuf) -> UnitJob {
    UnitJob {
        work,
        rsync,
        rclone: PathBuf::from("/usr/bin/rclone"),
        ssh: PathBuf::from("/usr/bin/ssh"),
        key: PathBuf::from("/run/credentials/u/backup-key"),
    }
}

/// Runs the unit: its lines, and its answer if it gave one.
async fn unit(job: &UnitJob, secrets: &str) -> (Vec<UnitSays>, Option<Report>) {
    let mut out = Vec::new();
    let mut input = std::io::Cursor::new(format!("{secrets}\n").into_bytes());
    run_backup_unit(&encode(job), &mut input, &mut out).await;
    let said: Vec<UnitSays> =
        String::from_utf8_lossy(&out).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let parts: String = said
        .iter()
        .filter_map(|line| match line {
            UnitSays::Part(part) => Some(part.as_str()),
            _ => None,
        })
        .collect();
    let report = matches!(said.last(), Some(UnitSays::Done)).then(|| serde_json::from_str(&parts).unwrap());
    (said, report)
}

fn failure(said: &[UnitSays]) -> (String, FailKind) {
    match said.last() {
        Some(UnitSays::Failed { message, kind }) => (message.clone(), *kind),
        other => panic!("not a failure: {other:?}"),
    }
}

fn run(source: End, target: End, mode: BackupMode, stamp: &str) -> Work {
    Work::Run(Box::new(RunJob {
        source,
        target,
        folder: BackupFolder::parse("photos").unwrap(),
        mode,
        stamp: BackupStamp::parse(stamp).unwrap(),
        excludes: vec![BackupPattern::parse("*.tmp").unwrap()],
        hidden: true,
        bwlimit_kib: 0,
    }))
}

/// Two "shares": `volume1/photos` with files, `volume2/backups` empty.
fn places() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("volume1/photos");
    let target = dir.path().join("volume2/backups");
    fs::create_dir_all(source.join("2024/summer")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("2024/summer/beach.jpg"), "beach").unwrap();
    fs::write(source.join("notes.txt"), "v1").unwrap();
    fs::write(source.join("scratch.tmp"), "skip me").unwrap();
    (dir, source, target)
}

fn inode(path: &Path) -> u64 {
    fs::metadata(path).unwrap().ino()
}

#[tokio::test]
async fn versions_link_unchanged_files_and_take_their_name_once_complete() {
    let Some(rsync) = real_rsync() else { return };
    let (_dir, source, target) = places();
    let first = job(
        run(local(&source, ""), local(&target, ""), BackupMode::Versioned, "2026-11-10_030000Z"),
        rsync.clone(),
    );
    let (said, report) = unit(&first, "").await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    assert!(report.partial.is_none());
    assert!(report.transfers_total >= 2, "{:?}", report.transfers);
    assert!(report.transfers.iter().any(|t| t.ends_with("notes.txt")));
    let version = target.join("photos/2026-11-10_030000Z");
    assert_eq!(fs::read_to_string(version.join("2024/summer/beach.jpg")).unwrap(), "beach");
    assert!(!version.join("scratch.tmp").exists(), "excluded");
    assert!(target.join("photos/.zendiloos-backup").is_file(), "marked");
    assert!(!target.join("photos/2026-11-10_030000Z.incomplete").exists());
    assert!(report.listing.contains("2026-11-10_030000Z"), "{}", report.listing);

    // Another size: rsync's quick check would take a file of the same size
    // written in the same second for unchanged.
    fs::write(source.join("notes.txt"), "v2, longer").unwrap();
    let second =
        job(run(local(&source, ""), local(&target, ""), BackupMode::Versioned, "2026-11-11_030000Z"), rsync);
    let (said, report) = unit(&second, "").await;
    assert!(report.is_some(), "{said:?}");
    let newer = target.join("photos/2026-11-11_030000Z");
    assert_eq!(fs::read_to_string(newer.join("notes.txt")).unwrap(), "v2, longer");
    assert_eq!(
        fs::read_to_string(version.join("notes.txt")).unwrap(),
        "v1",
        "the older version stays as it was"
    );
    // Unchanged: the same file, linked.
    assert_eq!(inode(&newer.join("2024/summer/beach.jpg")), inode(&version.join("2024/summer/beach.jpg")));
    assert_ne!(inode(&newer.join("notes.txt")), inode(&version.join("notes.txt")));
}

#[tokio::test]
async fn a_run_cut_off_is_taken_over_by_the_next() {
    let Some(rsync) = real_rsync() else { return };
    let (_dir, source, target) = places();
    let cut_off = target.join("photos/2026-11-09_030000Z.incomplete");
    fs::create_dir_all(&cut_off).unwrap();
    fs::write(target.join("photos/.zendiloos-backup"), "").unwrap();
    fs::write(cut_off.join("notes.txt"), "v1").unwrap();
    fs::write(cut_off.join("gone since.txt"), "x").unwrap();
    let next =
        job(run(local(&source, ""), local(&target, ""), BackupMode::Versioned, "2026-11-10_030000Z"), rsync);
    let (said, report) = unit(&next, "").await;
    assert!(report.is_some(), "{said:?}");
    assert!(!cut_off.exists(), "taken over");
    let version = target.join("photos/2026-11-10_030000Z");
    assert!(version.join("notes.txt").is_file());
    assert!(!version.join("gone since.txt").exists(), "what the source no longer has goes");
}

#[tokio::test]
async fn a_mirror_keeps_what_it_deletes_or_replaces_when_asked() {
    let Some(rsync) = real_rsync() else { return };
    let (_dir, source, target) = places();
    let mirror = BackupMode::Mirror { keep_changed: true };
    let first = job(run(local(&source, ""), local(&target, ""), mirror, "2026-11-10_030000Z"), rsync.clone());
    assert!(unit(&first, "").await.1.is_some());
    fs::remove_file(source.join("notes.txt")).unwrap();
    fs::write(source.join("2024/summer/beach.jpg"), "beach, edited").unwrap();
    let second = job(run(local(&source, ""), local(&target, ""), mirror, "2026-11-11_030000Z"), rsync);
    let (said, report) = unit(&second, "").await;
    assert!(report.is_some(), "{said:?}");
    let current = target.join("photos/current");
    assert!(!current.join("notes.txt").exists(), "deleted in the mirror too");
    assert_eq!(fs::read_to_string(current.join("2024/summer/beach.jpg")).unwrap(), "beach, edited");
    let removed = target.join("photos/removed/2026-11-11_030000Z");
    assert_eq!(fs::read_to_string(removed.join("notes.txt")).unwrap(), "v1");
    assert_eq!(fs::read_to_string(removed.join("2024/summer/beach.jpg")).unwrap(), "beach");
}

#[tokio::test]
async fn a_folder_that_holds_other_files_is_not_written() {
    let Some(rsync) = real_rsync() else { return };
    let (_dir, source, target) = places();
    fs::create_dir_all(target.join("photos")).unwrap();
    fs::write(target.join("photos/thesis.odt"), "years of work").unwrap();
    let mirror = job(
        run(
            local(&source, ""),
            local(&target, ""),
            BackupMode::Mirror { keep_changed: false },
            "2026-11-10_030000Z",
        ),
        rsync,
    );
    let (said, report) = unit(&mirror, "").await;
    assert!(report.is_none());
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::Refused);
    assert!(message.contains("holds other files"), "{message}");
    assert_eq!(fs::read_to_string(target.join("photos/thesis.odt")).unwrap(), "years of work");
}

#[tokio::test]
async fn links_never_lead_out_of_the_source_or_the_target() {
    let Some(rsync) = real_rsync() else { return };
    let (dir, source, target) = places();
    let secret = dir.path().join("secret");
    fs::write(&secret, "root's").unwrap();
    // A link to a file outside, and a folder of the source that is a link.
    symlink(&secret, source.join("shadow")).unwrap();
    symlink(dir.path(), source.join("escape")).unwrap();
    symlink("2024/summer/beach.jpg", source.join("inside")).unwrap();
    let mirror = job(
        run(
            local(&source, ""),
            local(&target, ""),
            BackupMode::Mirror { keep_changed: false },
            "2026-11-10_030000Z",
        ),
        rsync.clone(),
    );
    let (said, report) = unit(&mirror, "").await;
    assert!(report.is_some(), "{said:?}");
    let current = target.join("photos/current");
    assert!(fs::symlink_metadata(current.join("shadow")).is_err(), "an absolute link is left out");
    assert!(fs::symlink_metadata(current.join("escape")).is_err());
    assert!(fs::symlink_metadata(current.join("inside")).unwrap().file_type().is_symlink(), "kept as a link");
    assert!(!target.join("photos/current/escape/secret").exists());

    // A source folder reached through a link is refused.
    symlink(&source, dir.path().join("volume1/via-link")).unwrap();
    let through = job(
        run(
            local(&dir.path().join("volume1"), "via-link"),
            local(&target, ""),
            BackupMode::Versioned,
            "2026-11-11_030000Z",
        ),
        rsync.clone(),
    );
    let (said, _) = unit(&through, "").await;
    assert_eq!(failure(&said).1, FailKind::Refused);
    // So is a target folder.
    symlink(dir.path(), target.join("elsewhere")).unwrap();
    let into = job(
        run(local(&source, ""), local(&target, "elsewhere"), BackupMode::Versioned, "2026-11-11_030000Z"),
        rsync,
    );
    let (said, _) = unit(&into, "").await;
    assert_eq!(failure(&said).1, FailKind::Refused);
    assert!(!dir.path().join("photos").exists(), "nothing written through the link");
}

#[tokio::test]
async fn retention_removes_only_what_it_names() {
    let Some(rsync) = real_rsync() else { return };
    let (_dir, _, target) = places();
    let folder = target.join("photos");
    for name in [
        "2026-11-01_030000Z",
        "2026-11-02_030000Z",
        "2026-11-03_030000Z.incomplete",
        "current",
        "removed/2026-11-01_030000Z",
        "removed/2026-11-05_030000Z",
    ] {
        fs::create_dir_all(folder.join(name)).unwrap();
        fs::write(folder.join(name).join("file"), "x").unwrap();
    }
    fs::write(folder.join(".zendiloos-backup"), "").unwrap();
    let names = ["2026-11-01_030000Z", "2026-11-03_030000Z.incomplete", "removed/2026-11-01_030000Z"]
        .map(|name| BackupPruned::parse(name).unwrap())
        .to_vec();
    let prune = job(Work::Prune { at: local(&target, "photos"), names }, rsync);
    let (said, report) = unit(&prune, "").await;
    assert!(report.is_some(), "{said:?}");
    let mut left: Vec<String> = walk(&folder);
    left.sort();
    assert_eq!(
        left,
        [
            ".zendiloos-backup",
            "2026-11-02_030000Z",
            "2026-11-02_030000Z/file",
            "current",
            "current/file",
            "removed",
            "removed/2026-11-05_030000Z",
            "removed/2026-11-05_030000Z/file"
        ]
    );
}

fn walk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            found.push(path.strip_prefix(root).unwrap().display().to_string());
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn restores_skip_or_replace_and_make_the_folder() {
    let Some(rsync) = real_rsync() else { return };
    let (dir, source, target) = places();
    let make = job(
        run(local(&source, ""), local(&target, ""), BackupMode::Versioned, "2026-11-10_030000Z"),
        rsync.clone(),
    );
    assert!(unit(&make, "").await.1.is_some());
    let restored = dir.path().join("volume1/photos");
    fs::write(restored.join("notes.txt"), "changed since").unwrap();
    fs::remove_dir_all(restored.join("2024")).unwrap();

    let restore = |paths: Vec<&str>, to: End, existing| {
        job(
            Work::Restore(Box::new(RestoreJob {
                from: local(&target, "photos/2026-11-10_030000Z"),
                within: SharePath::default(),
                paths: paths.into_iter().map(|p| SharePath::parse(p).unwrap()).collect(),
                to,
                existing,
                into_share: true,
            })),
            rsync.clone(),
        )
    };
    let (said, report) = unit(&restore(vec![], local(&source, ""), BackupExisting::Skip), "").await;
    assert!(report.is_some(), "{said:?}");
    assert_eq!(fs::read_to_string(restored.join("notes.txt")).unwrap(), "changed since", "skipped");
    assert_eq!(fs::read_to_string(restored.join("2024/summer/beach.jpg")).unwrap(), "beach");
    let (said, report) =
        unit(&restore(vec!["notes.txt"], local(&source, ""), BackupExisting::Replace), "").await;
    assert!(report.is_some(), "{said:?}");
    assert_eq!(fs::read_to_string(restored.join("notes.txt")).unwrap(), "v1", "replaced");

    // Some of it, elsewhere, into a folder made for it.
    let (said, report) =
        unit(&restore(vec!["2024/summer"], local(&target, "restored/here"), BackupExisting::Skip), "").await;
    assert!(report.is_some(), "{said:?}");
    assert!(target.join("restored/here/2024/summer/beach.jpg").is_file());
    assert!(!target.join("restored/here/notes.txt").exists());
}

/// A fake rsync that records how it was called and what its password
/// file held, and lists one version.
fn fake_rsync(dir: &Path) -> PathBuf {
    let path = dir.join("rsync");
    let log = dir.join("rsync.log");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> {log}\n\
         for a in \"$@\"; do case \"$a\" in --password-file=*) cat \"${{a#--password-file=}}\" >> {log}.password ;; --rsh=*) echo \"$a\" >> {log}.rsh ;; esac; done\n\
         case \"$*\" in *--list-only*) echo 'drwxr-xr-x           4096 2026/11/10 03:00:00 .'; echo '-rw-r--r--             10 2026/11/10 03:00:00 .zendiloos-backup'; echo 'drwxr-xr-x           4096 2026/11/10 03:00:00 2026-11-10_030000Z' ;; esac\n\
         case \"$*\" in *--info=progress2*) printf '        100  50%%    1.00MB/s    0:00:01 (xfr#1, to-chk=1/3)\\r'; echo '>f+++++++++ a.jpg'; echo 'Number of regular files transferred: 1'; echo 'Total transferred file size: 100 bytes' ;; esac\n\
         exit 0\n",
        log = log.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn daemon(path: &str) -> End {
    End::Daemon {
        host: BackupHostName::parse("nas2.lan").unwrap(),
        port: 873,
        user: BackupWord::parse("zendi").unwrap(),
        module: BackupWord::parse("nas").unwrap(),
        path: BackupRemotePath::parse(path).unwrap(),
    }
}

fn ssh(path: &str) -> End {
    End::Ssh {
        host: BackupHostName::parse("fe80::2").unwrap(),
        port: 2222,
        user: BackupWord::parse("backup").unwrap(),
        path: BackupRemotePath::parse(path).unwrap(),
        host_key: BackupHostKey::parse(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        )
        .unwrap(),
    }
}

#[tokio::test]
async fn a_push_to_a_daemon_reads_the_password_from_a_file_and_names_the_version_twice() {
    let (dir, source, _) = places();
    let rsync = fake_rsync(dir.path());
    let push =
        job(run(local(&source, ""), daemon("nas"), BackupMode::Versioned, "2026-11-11_030000Z"), rsync);
    let (said, report) = unit(&push, r#"{"password":"s3cret pass"}"#).await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    assert_eq!((report.files, report.bytes, report.transfers_total), (1, 100, 1));
    assert!(said.iter().any(|line| matches!(line, UnitSays::Progress { percent: 50, .. })));
    let calls = fs::read_to_string(dir.path().join("rsync.log")).unwrap();
    assert!(!calls.contains("s3cret"), "never on a command line: {calls}");
    let passwords = fs::read_to_string(dir.path().join("rsync.log.password")).unwrap();
    assert!(passwords.lines().all(|line| line == "s3cret pass"), "{passwords}");
    let lines: Vec<&str> = calls.lines().collect();
    // Listed, uploaded into the incomplete name linked to the version
    // before, copied as links under its name, the incomplete one removed.
    assert!(
        lines[0].contains("--list-only") && lines[0].ends_with("rsync://zendi@nas2.lan:873/nas/nas/photos/"),
        "{calls}"
    );
    let upload = lines.iter().find(|l| l.contains("--info=progress2")).unwrap();
    assert!(upload.contains("--link-dest=../2026-11-10_030000Z"), "{upload}");
    assert!(
        upload.contains("--delete") && upload.contains("--exclude=*.tmp") && upload.contains("--safe-links")
    );
    assert!(upload.contains("--one-file-system") && upload.contains("--contimeout=30"));
    assert!(
        upload.ends_with("rsync://zendi@nas2.lan:873/nas/nas/photos/2026-11-11_030000Z.incomplete/"),
        "{upload}"
    );
    let named = lines.iter().find(|l| l.ends_with("photos/2026-11-11_030000Z/")).unwrap();
    assert!(named.contains("--link-dest=../2026-11-11_030000Z.incomplete"), "{named}");
    let removed = lines.iter().find(|l| l.contains("--include=/2026-11-11_030000Z.incomplete/***")).unwrap();
    assert!(removed.contains("--delete") && removed.contains("--exclude=*"), "{removed}");
    assert!(!dir.path().join("rsync.log.rsh").exists());
}

#[tokio::test]
async fn ssh_runs_with_the_pinned_key_only() {
    let (dir, source, _) = places();
    let rsync = fake_rsync(dir.path());
    let push = job(
        run(
            local(&source, ""),
            ssh("/srv/backups"),
            BackupMode::Mirror { keep_changed: true },
            "2026-11-11_030000Z",
        ),
        rsync,
    );
    let (said, report) = unit(&push, "").await;
    assert!(report.is_some(), "{said:?}");
    let rsh = fs::read_to_string(dir.path().join("rsync.log.rsh")).unwrap();
    for option in [
        "-F /dev/null",
        "-i /run/credentials/u/backup-key",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=/dev/null",
        "BatchMode=yes",
        "PasswordAuthentication=no",
        "-p 2222",
        "-l backup",
    ] {
        assert!(rsh.contains(option), "{option}: {rsh}");
    }
    let calls = fs::read_to_string(dir.path().join("rsync.log")).unwrap();
    let upload = calls.lines().find(|l| l.contains("--info=progress2")).unwrap();
    assert!(upload.ends_with("[fe80::2]:/srv/backups/photos/current/"), "{upload}");
    assert!(upload.contains("--backup-dir=../removed/2026-11-11_030000Z"), "{upload}");
}

#[tokio::test]
async fn host_keys_that_changed_and_servers_that_fail_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let rsync = dir.path().join("rsync");
    fs::write(
        &rsync,
        "#!/bin/sh\necho '@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@' >&2\n\
         echo '@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @' >&2\n\
         echo 'Host key verification failed.' >&2\n\
         echo 'rsync: connection unexpectedly closed (0 bytes received so far) [sender]' >&2\nexit 255\n",
    )
    .unwrap();
    fs::set_permissions(&rsync, fs::Permissions::from_mode(0o755)).unwrap();
    let list = job(Work::List { at: ssh(""), inside: SharePath::default() }, rsync.clone());
    let (said, _) = unit(&list, "").await;
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::HostKey);
    assert!(message.contains("Host key verification failed"), "{message}");

    fs::write(&rsync, "#!/bin/sh\necho 'rsync: failed to connect to nas2.lan (192.0.2.1): Connection refused (111)' >&2\nexit 10\n").unwrap();
    let (said, _) =
        unit(&job(Work::List { at: daemon(""), inside: SharePath::default() }, rsync.clone()), "").await;
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::Failed);
    assert!(message.contains("rsync ended with 10") && message.contains("Connection refused"), "{message}");

    fs::write(&rsync, "#!/bin/sh\necho 'rsync: change_dir \"/nas/missing\" (in nas) failed: No such file or directory (2)' >&2\nexit 23\n").unwrap();
    let (said, report) =
        unit(&job(Work::List { at: daemon("missing"), inside: SharePath::default() }, rsync), "").await;
    assert!(report.unwrap_or_else(|| panic!("{said:?}")).missing);
}

#[tokio::test]
async fn files_that_vanish_make_a_run_partial_not_failed() {
    let (dir, source, target) = places();
    let rsync = dir.path().join("rsync");
    fs::write(
        &rsync,
        "#!/bin/sh\ncase \"$*\" in *--list-only*) exit 0 ;; esac\n\
         echo 'file has vanished: \"/proc/self/fd/7/notes.txt\"'\necho 'Number of regular files transferred: 2'\nexit 24\n",
    )
    .unwrap();
    fs::set_permissions(&rsync, fs::Permissions::from_mode(0o755)).unwrap();
    let run =
        job(run(local(&source, ""), local(&target, ""), BackupMode::Versioned, "2026-11-11_030000Z"), rsync);
    let (said, report) = unit(&run, "").await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    assert!(report.partial.unwrap().contains("vanished"));
    assert_eq!(report.files, 2);
    assert!(target.join("photos/2026-11-11_030000Z").is_dir(), "complete but for it");
}

#[test]
fn listings_and_progress_are_read_as_rsync_writes_them() {
    assert_eq!(
        listed("drwxr-xr-x           4096 2026/11/10 03:00:00 2026-11-10_030000Z"),
        Some(Listed { name: "2026-11-10_030000Z".into(), folder: true })
    );
    assert_eq!(
        listed("-rw-r--r--          1,024 2026/11/10 03:00:00 holiday photos.zip"),
        Some(Listed { name: "holiday photos.zip".into(), folder: false })
    );
    assert_eq!(
        listed("lrwxrwxrwx              5 2026/11/10 03:00:00 a -> b"),
        Some(Listed { name: "a".into(), folder: false })
    );
    assert_eq!(listed("sending incremental file list"), None);
    assert_eq!(progress("     32,768  45%   10.00MB/s    0:00:01 (xfr#1, to-chk=1/3)"), Some(45));
    assert_eq!(progress(">f+++++++++ a.jpg"), None);
    assert!(itemized(">f+++++++++ a.jpg") && itemized("*deleting   old.txt") && itemized("cd+++++++++ dir/"));
    assert!(!itemized("Number of files: 3"));
    assert_eq!(
        number_after("Total transferred file size: 1,234 bytes", "Total transferred file size:"),
        Some(1234)
    );
}

#[tokio::test]
async fn jobs_and_secrets_that_cannot_be_read_are_refused() {
    let mut out = Vec::new();
    assert!(!run_backup_unit("zz", &mut std::io::Cursor::new(Vec::new()), &mut out).await);
    let dir = tempfile::tempdir().unwrap();
    let list = job(
        Work::List { at: local(dir.path(), ""), inside: SharePath::default() },
        PathBuf::from("/bin/true"),
    );
    let mut out = Vec::new();
    assert!(
        !run_backup_unit(&encode(&list), &mut std::io::Cursor::new(b"not json\n".to_vec()), &mut out).await
    );
    assert!(String::from_utf8_lossy(&out).contains("secrets cannot be read"));
}
