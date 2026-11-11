//! The rclone engine against a fake rclone that records its calls, its
//! variables and its config file, and answers as rclone does.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::volume_archive::encode;

/// The fake: `obscure` turns its input round (no space); other calls are
/// logged with the `RCLONE_` variables and the config file. What it
/// answers depends on files in `dir`: `missing`, `host-key`, `foreign`.
fn fake_rclone(dir: &Path) -> PathBuf {
    let path = dir.join("rclone");
    let script = format!(
        r#"#!/bin/sh
d={dir}
if [ "$1" = obscure ]; then read -r secret; printf 'obscured-%s\n' "$(printf %s "$secret" | tr 'A-Za-z ' 'N-ZA-Mn-za-m_')"; exit 0; fi
echo "$@" >> $d/rclone.log
env | grep '^RCLONE_' | sort | tr '\n' ' ' >> $d/rclone.env; echo >> $d/rclone.env
conf=
for a in "$@"; do case "$a" in --config=*) conf="${{a#--config=}}" ;; --filter-from=*) cat "${{a#--filter-from=}}" >> $d/rclone.filters ;; esac; done
[ -n "$conf" ] && cat "$conf" >> $d/rclone.conf
case "$1" in
  lsjson)
    if [ -f $d/host-key ]; then echo '{{"level":"critical","msg":"Failed to create file system for \"dest:\": NewFs: couldn'"'"'t connect SSH: ssh: handshake failed: knownhosts: key mismatch","time":"t"}}'; exit 1; fi
    if [ -f $d/missing ]; then echo '{{"level":"error","msg":"error listing: directory not found","source":"x","time":"t"}}'; exit 3; fi
    if [ -f $d/foreign ]; then
      if [ "$3" = dest: ]; then printf '[\n]\n'; else printf '[\n{{"Path":"q8f3","Name":"q8f3","Size":3,"ModTime":"2026-11-10T03:00:00Z","IsDir":false}}\n]\n'; fi
      exit 0
    fi
    printf '[\n{{"Path":".zendiloos-backup","Name":".zendiloos-backup","Size":70,"ModTime":"2026-11-10T03:00:00Z","IsDir":false}},\n{{"Path":"2026-11-10_030000Z","Name":"2026-11-10_030000Z","Size":-1,"ModTime":"2026-11-10T04:00:00.5+01:00","IsDir":true}}\n]\n' ;;
  sync|copy)
    echo '{{"level":"info","msg":"Copied (new)","object":"a.jpg","objectType":"*local.Object","source":"operations/copy.go:1","time":"t"}}'
    echo '{{"level":"info","msg":"b.txt: Copied (replaced existing)","source":"operations/copy.go:1","time":"t"}}'
    echo '{{"level":"info","msg":"Moved (server-side) to: removed/x/gone.txt","object":"gone.txt","time":"t"}}'
    echo '{{"level":"info","msg":"Transferred: 50 / 100","stats":{{"bytes":50,"totalBytes":100,"transfers":1}},"time":"t"}}'
    echo '{{"level":"info","msg":"done","stats":{{"bytes":100,"totalBytes":100,"transfers":2}},"time":"t"}}'
    [ -d "$3" ] && echo restored > "$3/a.jpg"
    [ -n "$conf" ] && [ -s "$conf" ] && sed -i 's/^token = .*/token = {{"access_token":"fresh"}}/' "$conf"
    ;;
esac
exit 0
"#,
        dir = dir.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap_or_default()
}

fn job(work: Work, rclone: PathBuf) -> UnitJob {
    UnitJob {
        work,
        rsync: PathBuf::from("/nonexistent/rsync"),
        rclone,
        ssh: PathBuf::from("/usr/bin/ssh"),
        key: PathBuf::from("/run/credentials/u/backup-key"),
    }
}

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

fn local(root: &Path, path: &str) -> End {
    End::Local { root: root.to_owned(), path: SharePath::parse(path).unwrap() }
}

fn cloud(cloud: BackupCloud, path: &str) -> End {
    End::Cloud { cloud, path: SharePath::parse(path).unwrap() }
}

fn run(source: End, target: End, mode: BackupMode) -> Work {
    Work::Run(Box::new(RunJob {
        source,
        target,
        folder: BackupFolder::parse("photos").unwrap(),
        mode,
        stamp: BackupStamp::parse("2026-11-11_030000Z").unwrap(),
        excludes: vec![BackupPattern::parse("*.tmp").unwrap(), BackupPattern::parse("cache/").unwrap()],
        hidden: false,
        bwlimit_kib: 512,
    }))
}

/// A share's folder with a file, and an empty one.
fn places() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("volume1/photos");
    let target = dir.path().join("volume2/backups");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("a.jpg"), "a").unwrap();
    (dir, source, target)
}

const SECRETS: &str = r#"{"crypt":{"password":"correct horse","salt":"pepper"},"cloud":{"token":"{\"access_token\":\"old\",\"refresh_token\":\"r\"}","client_id":"own.apps.googleusercontent.com","client_secret":"shh"}}"#;

#[tokio::test]
async fn an_encrypted_cloud_backup_keeps_its_secrets_off_command_lines_and_brings_back_the_token() {
    let (dir, source, _) = places();
    let rclone = fake_rclone(dir.path());
    let versioned =
        job(run(local(&source, ""), cloud(BackupCloud::GoogleDrive, "nas"), BackupMode::Versioned), rclone);
    let (said, report) = unit(&versioned, SECRETS).await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    assert_eq!(
        report.token.as_deref(),
        Some(r#"{"access_token":"fresh"}"#),
        "the refreshed token comes back"
    );
    assert_eq!(report.transfers, [">f+++++++++ a.jpg", ">f.st...... b.txt", "*deleting   gone.txt"]);
    assert_eq!((report.files, report.bytes, report.transfers_total), (2, 100, 3));
    assert!(said.iter().any(|line| matches!(line, UnitSays::Progress { percent: 50, .. })));
    let calls = read(dir.path(), "rclone.log");
    let lines: Vec<&str> = calls.lines().collect();
    // Listed through the crypt; the newest version copied on the server;
    // the run into the incomplete name; named once complete; listed again.
    assert!(lines[0].starts_with("lsjson --no-mimetype dest: --config="), "{calls}");
    assert!(
        lines[1].starts_with("copy dest:2026-11-10_030000Z dest:2026-11-11_030000Z.incomplete"),
        "{calls}"
    );
    assert!(lines[2].starts_with("sync /proc/self/fd/"), "{calls}");
    assert!(lines[2].contains(" dest:2026-11-11_030000Z.incomplete --log-level=INFO"), "{calls}");
    for option in [
        "--exclude=*.tmp --exclude=*.tmp/**",
        "--exclude=cache/**",
        "--exclude=.* --exclude=.*/**",
        "--bwlimit=512K",
        "--delete-excluded",
        "--skip-links",
        "--use-json-log",
    ] {
        assert!(lines[2].contains(option), "{option}: {}", lines[2]);
    }
    assert!(
        lines[3].starts_with("moveto dest:2026-11-11_030000Z.incomplete dest:2026-11-11_030000Z "),
        "{calls}"
    );
    assert!(lines[4].starts_with("lsjson"), "{calls}");
    let variables = read(dir.path(), "rclone.env");
    for secret in ["correct horse", "pepper", "shh", "access_token"] {
        assert!(!calls.contains(secret), "{secret} on a command line: {calls}");
    }
    for plain in ["correct horse", "pepper"] {
        assert!(!variables.contains(plain), "{plain} not obscured: {variables}");
    }
    for expected in [
        "RCLONE_CONFIG_BASE_TYPE=drive",
        "RCLONE_CONFIG_BASE_SCOPE=drive.file",
        "RCLONE_CONFIG_BASE_CLIENT_ID=own.apps.googleusercontent.com",
        // As rclone keeps an OAuth client's secret: as it is.
        "RCLONE_CONFIG_BASE_CLIENT_SECRET=shh",
        "RCLONE_CONFIG_DEST_TYPE=crypt",
        "RCLONE_CONFIG_DEST_REMOTE=base:nas/photos",
        "RCLONE_CONFIG_DEST_PASSWORD=obscured-pbeerpg_ubefr",
        "RCLONE_CONFIG_DEST_PASSWORD2=obscured-crccre",
        "RCLONE_CONFIG_DEST_FILENAME_ENCRYPTION=standard",
        "RCLONE_CONFIG_DEST_DIRECTORY_NAME_ENCRYPTION=true",
    ] {
        assert!(variables.lines().next().unwrap().contains(expected), "{expected}: {variables}");
    }
    let config = read(dir.path(), "rclone.conf");
    assert!(
        config.starts_with("[base]\ntoken = {\"access_token\":\"old\",\"refresh_token\":\"r\"}\n"),
        "{config}"
    );
}

#[tokio::test]
async fn a_mirror_to_onedrive_keeps_changes_and_lists_its_folder_as_rsync_does() {
    let (dir, source, _) = places();
    let rclone = fake_rclone(dir.path());
    let mirror = job(
        run(local(&source, ""), cloud(BackupCloud::OneDrive, ""), BackupMode::Mirror { keep_changed: true }),
        rclone,
    );
    let secrets =
        r#"{"cloud":{"token":"{\"access_token\":\"old\"}","drive_id":"b!drive","drive_type":"personal"}}"#;
    let (said, report) = unit(&mirror, secrets).await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    let calls = read(dir.path(), "rclone.log");
    let sync = calls.lines().find(|line| line.starts_with("sync ")).unwrap();
    assert!(sync.contains(" base:photos/current "), "{sync}");
    assert!(sync.contains("--backup-dir=base:photos/removed/2026-11-11_030000Z"), "{sync}");
    assert!(!calls.contains("copy ") && !calls.contains("moveto "), "{calls}");
    let variables = read(dir.path(), "rclone.env");
    for expected in [
        "RCLONE_CONFIG_BASE_TYPE=onedrive",
        "RCLONE_CONFIG_BASE_DRIVE_ID=b!drive",
        "RCLONE_CONFIG_BASE_DRIVE_TYPE=personal",
    ] {
        assert!(variables.contains(expected), "{expected}: {variables}");
    }
    assert!(!variables.contains("RCLONE_CONFIG_DEST_"), "no crypt: {variables}");
    let lines: Vec<&str> = report.listing.lines().collect();
    assert_eq!(lines[0], "-rw-r--r--             70 2026/11/10 03:00:00 .zendiloos-backup");
    assert_eq!(lines[1], "drwxr-xr-x              0 2026/11/10 03:00:00 2026-11-10_030000Z");
    assert_eq!(listed(lines[1]), Some(Listed { name: "2026-11-10_030000Z".into(), folder: true }));
}

#[tokio::test]
async fn an_encrypted_backup_over_sftp_pins_the_servers_host_key() {
    let (dir, source, _) = places();
    let rclone = fake_rclone(dir.path());
    fs::write(dir.path().join("host-key"), "").unwrap();
    let server = End::Ssh {
        host: BackupHostName::parse("fe80::2").unwrap(),
        port: 2222,
        user: BackupWord::parse("backup").unwrap(),
        path: BackupRemotePath::parse("/srv/backups").unwrap(),
        host_key: BackupHostKey::parse(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        )
        .unwrap(),
    };
    let push = job(run(local(&source, ""), server, BackupMode::Versioned), rclone);
    let (said, _) = unit(&push, r#"{"crypt":{"password":"pw"}}"#).await;
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::HostKey, "{message}");
    assert!(message.contains("key mismatch"), "{message}");
    let variables = read(dir.path(), "rclone.env");
    for expected in [
        "RCLONE_CONFIG_BASE_TYPE=sftp",
        "RCLONE_CONFIG_BASE_HOST=fe80::2",
        "RCLONE_CONFIG_BASE_PORT=2222",
        "RCLONE_CONFIG_BASE_USER=backup",
        "RCLONE_CONFIG_BASE_KEY_FILE=/run/credentials/u/backup-key",
        "RCLONE_CONFIG_BASE_SHELL_TYPE=none",
        "RCLONE_CONFIG_DEST_REMOTE=base:/srv/backups/photos",
    ] {
        assert!(variables.contains(expected), "{expected}: {variables}");
    }
    assert!(!variables.contains("RCLONE_CONFIG_DEST_PASSWORD2"), "no salt given: {variables}");
}

#[tokio::test]
async fn restores_from_an_encrypted_folder_only_what_was_asked_skipping_what_is_there() {
    let (dir, source, target) = places();
    fs::create_dir_all(target.join("photos")).unwrap();
    let rclone = fake_rclone(dir.path());
    let restore = job(
        Work::Restore(Box::new(RestoreJob {
            from: local(&target, "photos"),
            within: SharePath::parse("2026-11-10_030000Z").unwrap(),
            paths: vec![SharePath::parse("2024/summer").unwrap(), SharePath::parse("odd*name").unwrap()],
            to: local(&source, "restored"),
            existing: BackupExisting::Skip,
            into_share: true,
        })),
        rclone,
    );
    let (said, report) = unit(&restore, r#"{"crypt":{"password":"pw"}}"#).await;
    let report = report.unwrap_or_else(|| panic!("{said:?}"));
    assert_eq!(report.files, 2);
    assert!(report.token.is_none(), "no cloud");
    assert_eq!(fs::read_to_string(source.join("restored/a.jpg")).unwrap(), "restored\n");
    let calls = read(dir.path(), "rclone.log");
    let copy = calls.lines().find(|line| line.starts_with("copy ")).unwrap();
    assert!(copy.starts_with("copy dest:2026-11-10_030000Z /proc/self/fd/"), "{copy}");
    assert!(copy.contains("--ignore-existing") && copy.contains("--filter-from="), "{copy}");
    assert_eq!(
        read(dir.path(), "rclone.filters"),
        "+ /2024/summer\n+ /2024/summer/**\n+ /odd\\*name\n+ /odd\\*name/**\n- **\n"
    );
    assert!(read(dir.path(), "rclone.env").contains("RCLONE_CONFIG_DEST_REMOTE=/proc/self/fd/"));
}

#[tokio::test]
async fn retention_removes_for_good_and_a_missing_folder_is_no_failure() {
    let (dir, _, _) = places();
    let rclone = fake_rclone(dir.path());
    let names = vec![
        BackupPruned::parse("2026-11-01_030000Z").unwrap(),
        BackupPruned::parse("removed/2026-11-02_030000Z").unwrap(),
    ];
    let prune = job(Work::Prune { at: cloud(BackupCloud::GoogleDrive, "nas/photos"), names }, rclone.clone());
    let (said, report) = unit(&prune, SECRETS).await;
    assert!(report.is_some(), "{said:?}");
    let calls = read(dir.path(), "rclone.log");
    assert!(calls.contains("purge dest:2026-11-01_030000Z "), "{calls}");
    assert!(calls.contains("purge dest:removed/2026-11-02_030000Z "), "{calls}");
    assert!(read(dir.path(), "rclone.env").contains("RCLONE_CONFIG_BASE_USE_TRASH=false"));

    fs::write(dir.path().join("missing"), "").unwrap();
    let list = job(
        Work::List {
            at: cloud(BackupCloud::GoogleDrive, "nas/photos"),
            inside: SharePath::parse("current").unwrap(),
        },
        rclone,
    );
    let (said, report) = unit(&list, SECRETS).await;
    assert!(report.unwrap_or_else(|| panic!("{said:?}")).missing);
    assert!(
        read(dir.path(), "rclone.log")
            .lines()
            .last()
            .unwrap()
            .starts_with("lsjson --no-mimetype dest:current ")
    );
}

#[tokio::test]
async fn a_daemon_is_never_encrypted_and_a_folder_it_cannot_decrypt_is_not_written() {
    let (dir, source, target) = places();
    let rclone = fake_rclone(dir.path());
    let daemon = End::Daemon {
        host: BackupHostName::parse("nas2.lan").unwrap(),
        port: 873,
        user: BackupWord::parse("zendi").unwrap(),
        module: BackupWord::parse("nas").unwrap(),
        path: BackupRemotePath::parse("").unwrap(),
    };
    let to_daemon = job(run(local(&source, ""), daemon, BackupMode::Versioned), rclone.clone());
    let (said, _) = unit(&to_daemon, r#"{"crypt":{"password":"pw"}}"#).await;
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::Refused);
    assert!(message.contains("rsync daemon"), "{message}");

    fs::write(dir.path().join("foreign"), "").unwrap();
    let to_drive = job(run(local(&source, ""), local(&target, ""), BackupMode::Versioned), rclone);
    let (said, _) = unit(&to_drive, r#"{"crypt":{"password":"another"}}"#).await;
    let (message, kind) = failure(&said);
    assert_eq!(kind, FailKind::Refused);
    assert!(message.contains("another password"), "{message}");
    assert!(!read(dir.path(), "rclone.log").contains("sync "), "nothing written");
}

#[test]
fn times_names_and_changes_are_read_as_rclone_writes_them() {
    assert_eq!(rsync_time("2026-11-10T04:00:00.5+01:00"), "2026/11/10 03:00:00");
    assert_eq!(rsync_time("2024-02-29T23:30:00-01:00"), "2024/03/01 00:30:00");
    assert_eq!(rsync_time("2026-11-11T07:00:00.123456789Z"), "2026/11/11 07:00:00");
    assert_eq!(rsync_time("not a time"), "1970/01/01 00:00:00");
    assert_eq!(escaped("a\\b\nc"), "a\\#134b\\#012c");
    assert_eq!(glob_escaped("a*[b]{c}?\\"), "a\\*\\[b\\]\\{c\\}\\?\\\\");
    assert_eq!(join("dest:", "current"), "dest:current");
    assert_eq!(join("base:nas", ""), "base:nas");
    assert_eq!(join("/proc/self/fd/7", "x/y"), "/proc/self/fd/7/x/y");
    assert_eq!(change("photos/a: b.jpg: Copied (new)", None), Some((NEW, "photos/a: b.jpg".to_owned())));
    assert_eq!(change("Deleted", Some("x.txt")), Some((DELETING, "x.txt".to_owned())));
    assert_eq!(change("Set directory modification time", Some("d")), None);

    let mut heard = RcloneHeard::default();
    heard.hear(r#"{"level":"error","msg":"Failed to copy: can't copy - source file is being updated","object":"live.db","time":"t"}"#);
    heard.hear(r#"{"level":"error","msg":"not deleting files as there were IO errors","time":"t"}"#);
    assert!(heard.partial(), "{:?}", heard.errors);
    assert!(heard.reason().contains("live.db: Failed to copy"), "{}", heard.reason());
    heard.hear(r#"{"level":"error","msg":"Failed to copy: googleapi: Error 403: The user's Drive storage quota has been exceeded.","time":"t"}"#);
    assert!(!heard.partial());
    heard.hear("{ not json, a piece of a long line");
    assert!(heard.complaints.last().unwrap().starts_with("{ not json"));
}
