//! Backing up and restoring on folders of the test's own: a volume's data,
//! a share and Docker's folder of the volume, all plain folders. The
//! malicious archives are made by hand, as nothing zendiloOS writes could
//! hold such entries, with the SHA-256 they really have: the extraction
//! itself must refuse them.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use tempfile::TempDir;

use super::*;

/// A volume's data, a share and the volume's folder in Docker's data root.
struct Place {
    _dir: TempDir,
    root: PathBuf,
}

impl Place {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for folder in ["source", "share", "docker/volumes/db/_data"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        Self { _dir: dir, root }
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn data(&self) -> PathBuf {
        self.path("docker/volumes/db/_data")
    }

    fn job(&self) -> BackupJob {
        BackupJob {
            source: self.path("source"),
            share: self.path("share"),
            folder: SharePath::parse("backups/docker").unwrap(),
            volume: VolumeName::parse("db").unwrap(),
            name: BackupStem::parse("db-20261108-030000").unwrap(),
            driver: "local".into(),
            labels: BTreeMap::from([("com.docker.compose.project".to_owned(), "tesla".to_owned())]),
            created: "2026-11-01T10:00:00Z".into(),
        }
    }

    /// A restore of `archive`, recorded with `sha256` and as much data as a
    /// gibibyte.
    fn restore_job(&self, archive: &str, sha256: &str) -> RestoreJob {
        RestoreJob {
            volume_dir: self.path("docker/volumes/db"),
            share: self.path("share"),
            archive: SharePath::parse(archive).unwrap(),
            sha256: ArchiveDigest::parse(sha256).unwrap(),
            data_bytes: 1 << 30,
            entries: None,
        }
    }

    /// A restore of the backup `done`, as it was recorded.
    fn restore_of(&self, done: &BackupDone) -> RestoreJob {
        RestoreJob {
            data_bytes: done.data_bytes,
            entries: Some(done.entries),
            ..self.restore_job(done.archive.path.as_str(), done.sha256.as_str())
        }
    }

    /// The fresh folders a restore left beside the data.
    fn fresh_folders(&self) -> Vec<PathBuf> {
        fs::read_dir(self.path("docker/volumes/db"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(FRESH_PREFIX))
            .collect()
    }

    /// What is in Docker's folder of the volume besides `_data`: nothing
    /// after a restore, successful or not.
    fn beside_the_data(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path("docker/volumes/db"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "_data")
            .collect();
        names.sort();
        names
    }

    /// Puts `bytes` into the share as `name`, writable by its owner only,
    /// and its SHA-256.
    fn archive(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.path("share").join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        sha256(bytes)
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn quiet(_: u8) {}

fn quietly(_: &'static str, _: u8) {}

/// A user extended attribute on `path`, if the file system takes them.
fn mark(path: &Path) -> bool {
    rustix::fs::setxattr(path, "user.zendiloos", b"kept", XattrFlags::empty()).is_ok()
}

fn marked(path: &Path) -> Option<Vec<u8>> {
    let mut value = vec![0u8; 64];
    let length = rustix::fs::getxattr(path, "user.zendiloos", &mut value[..]).ok()?;
    value.truncate(length);
    Some(value)
}

/// A volume's data with something of every kind.
fn fill(source: &Path) -> bool {
    fs::create_dir_all(source.join("pgdata/base")).unwrap();
    fs::write(source.join("pgdata/base/1"), vec![7u8; 300_000]).unwrap();
    fs::write(source.join("config.yml"), "port: 5432\n").unwrap();
    fs::set_permissions(source.join("config.yml"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(source.join("pgdata"), fs::Permissions::from_mode(0o2750)).unwrap();
    fs::hard_link(source.join("config.yml"), source.join("config.again")).unwrap();
    symlink("../etc/passwd", source.join("pgdata/outside")).unwrap();
    symlink("config.yml", source.join("current")).unwrap();
    let long = "n".repeat(120);
    fs::create_dir_all(source.join(&long)).unwrap();
    fs::write(source.join(&long).join(&long), "deep\n").unwrap();
    rustix::fs::mknodat(rustix::fs::CWD, source.join("pipe"), FileType::Fifo, Mode::from_raw_mode(0o600), 0)
        .unwrap();
    let old = Timestamps {
        last_access: Timespec { tv_sec: 1_700_000_000, tv_nsec: 0 },
        last_modification: Timespec { tv_sec: 1_700_000_000, tv_nsec: 0 },
    };
    utimensat(rustix::fs::CWD, source.join("config.yml"), &old, AtFlags::empty()).unwrap();
    mark(&source.join("config.yml"))
}

/// The paths an archive holds, in its order.
fn listed(archive: &Path) -> Vec<String> {
    let decoded = zstd::stream::decode_all(File::open(archive).unwrap()).unwrap();
    let mut archive = tar::Archive::new(decoded.as_slice());
    archive.entries().unwrap().map(|e| shown(&e.unwrap().path_bytes())).collect()
}

#[test]
fn a_volume_is_backed_up_and_restored_as_it_was() {
    let place = Place::new();
    let attributes = fill(&place.path("source"));
    let mut told = Vec::new();

    let done = back_up(&place.job(), 1_793_847_600, &mut |percent| told.push(percent)).unwrap();

    let folder = place.path("share/backups/docker/db");
    let archive = folder.join("db-20261108-030000.tar.zst");
    let bytes = fs::read(&archive).unwrap();
    assert_eq!(done.sha256.as_str(), sha256(&bytes));
    assert_eq!(done.archive.path.as_str(), "backups/docker/db/db-20261108-030000.tar.zst");
    assert_eq!(done.archive.bytes, bytes.len() as u64);
    let meta = fs::metadata(&archive).unwrap();
    assert_eq!((done.archive.device, done.archive.inode), (meta.dev(), meta.ino()));
    // Made as any new file: the folder's inherited ACL applies (here the
    // umask, as the folder has none); then only its owner may write it.
    assert_eq!(meta.mode() & 0o7777, 0o666 & !current_umask() & 0o755);
    let manifest_mode = fs::metadata(folder.join("db-20261108-030000.json")).unwrap().mode();
    assert_eq!(manifest_mode & 0o022, 0);
    assert_eq!(told.last(), Some(&100));
    assert_eq!(done.skipped, 1, "the pipe");
    assert_eq!(done.data_bytes, 300_000 + 11 + 5);
    // ., pgdata, pgdata/base, pgdata/base/1, config.yml, config.again,
    // pgdata/outside, current, the long folder and the file in it.
    assert_eq!(done.entries, 10);

    let manifest: Manifest =
        serde_json::from_str(&fs::read_to_string(folder.join("db-20261108-030000.json")).unwrap()).unwrap();
    assert_eq!(manifest.volume, "db");
    assert_eq!(manifest.driver, "local");
    assert_eq!(manifest.labels["com.docker.compose.project"], "tesla");
    assert_eq!((manifest.size_bytes, manifest.sha256.as_str()), (done.data_bytes, done.sha256.as_str()));
    assert_eq!(manifest.archive, "db-20261108-030000.tar.zst");
    assert_eq!(manifest.backed_up_at, 1_793_847_600);
    assert_eq!(manifest.zendiloos_version, crate::VERSION);
    assert_eq!(serde_json::from_str::<Manifest>(&done.manifest_json).unwrap(), manifest);
    let entries = listed(&archive);
    assert_eq!(entries.first().map(String::as_str), Some("./"));
    assert!(entries.contains(&"pgdata/base/1".to_owned()), "{entries:?}");
    assert!(!entries.iter().any(|e| e.contains("pipe")), "{entries:?}");
    assert!(entries.iter().any(|e| e.len() > 200), "long names whole: {entries:?}");
    let hidden: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(hidden.is_empty(), "no hidden file left");

    // The volume holds other things now; restoring brings back the backup.
    fs::write(place.data().join("newer"), "changed since").unwrap();
    let job = place.restore_of(&done);
    let mut steps = Vec::new();
    let restored = restore(&job, &mut |step, percent| steps.push((step, percent))).unwrap();
    assert_eq!(restored.entries, done.entries, "{restored:?}");
    assert_eq!(restored.left_over, None);
    assert!(steps.contains(&("verifying", 100)) && steps.contains(&("restoring", 100)), "{steps:?}");

    let data = place.data();
    assert!(!data.join("newer").exists(), "replaced, not merged");
    assert_eq!(fs::read_to_string(data.join("config.yml")).unwrap(), "port: 5432\n");
    assert_eq!(fs::read(data.join("pgdata/base/1")).unwrap(), vec![7u8; 300_000]);
    let config = fs::metadata(data.join("config.yml")).unwrap();
    assert_eq!(config.mode() & 0o7777, 0o640);
    assert_eq!(config.mtime(), 1_700_000_000);
    assert_eq!(config.ino(), fs::metadata(data.join("config.again")).unwrap().ino(), "a hard link again");
    assert_eq!(fs::metadata(data.join("pgdata")).unwrap().mode() & 0o7777, 0o2750);
    assert_eq!(
        fs::read_link(data.join("pgdata/outside")).unwrap(),
        Path::new("../etc/passwd"),
        "kept, not followed"
    );
    assert_eq!(fs::read_link(data.join("current")).unwrap(), Path::new("config.yml"));
    let long = "n".repeat(120);
    assert_eq!(fs::read_to_string(data.join(&long).join(&long)).unwrap(), "deep\n");
    assert!(!data.join("pipe").exists());
    if attributes {
        assert_eq!(marked(&data.join("config.yml")), Some(b"kept".to_vec()));
    }
    assert!(place.beside_the_data().is_empty(), "the old contents are gone");
}

fn current_umask() -> u32 {
    let probe = tempfile::tempdir().unwrap();
    let path = probe.path().join("probe");
    fs::write(&path, "").unwrap();
    0o666 & !(fs::metadata(&path).unwrap().mode() & 0o777)
}

#[test]
fn a_backup_never_replaces_a_file_and_never_follows_a_link_on_its_way() {
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    back_up(&place.job(), 0, &mut quiet).unwrap();
    let again = back_up(&place.job(), 0, &mut quiet);
    assert!(matches!(&again, Err(ArchiveError::Refused(why)) if why.contains("exists already")), "{again:?}");

    // A folder on the way swapped for a link, by whoever may write the share.
    let elsewhere = place.path("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::create_dir_all(place.path("share/links")).unwrap();
    symlink(&elsewhere, place.path("share/links/docker")).unwrap();
    let job = BackupJob { folder: SharePath::parse("links/docker").unwrap(), ..place.job() };
    let refused = back_up(&job, 0, &mut quiet);
    assert!(matches!(&refused, Err(ArchiveError::Refused(why)) if why.contains("link")), "{refused:?}");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing written through the link");
}

#[test]
fn a_mismatching_or_missing_archive_restores_nothing() {
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    let done = back_up(&place.job(), 0, &mut quiet).unwrap();
    fs::write(place.data().join("kept"), "old").unwrap();

    let wrong = place.restore_job(done.archive.path.as_str(), &"0".repeat(64));
    let refused = restore(&wrong, &mut quietly);
    assert!(matches!(&refused, Err(ArchiveError::Refused(why)) if why.contains("SHA-256")), "{refused:?}");
    let gone = place.restore_job("backups/none.tar.zst", done.sha256.as_str());
    assert!(matches!(restore(&gone, &mut quietly), Err(ArchiveError::Refused(_))));
    // An archive that is a link to the real one, or to anything else.
    symlink(place.path("share").join(done.archive.path.as_str()), place.path("share/linked.tar.zst"))
        .unwrap();
    let linked = place.restore_job("linked.tar.zst", done.sha256.as_str());
    let refused = restore(&linked, &mut quietly);
    assert!(matches!(&refused, Err(ArchiveError::Refused(why)) if why.contains("link")), "{refused:?}");
    fs::create_dir(place.path("share/folder.tar.zst")).unwrap();
    let folder = place.restore_job("folder.tar.zst", done.sha256.as_str());
    assert!(matches!(restore(&folder, &mut quietly), Err(ArchiveError::Refused(_))));

    assert_eq!(fs::read_to_string(place.data().join("kept")).unwrap(), "old");
    assert!(place.beside_the_data().is_empty());
}

/// An entry made by hand: its name, kind, link target and content.
type Crafted<'a> = (&'a [u8], tar::EntryType, &'a [u8], &'a [u8]);

/// An archive of `entries`, each a header made by hand (any name, any kind)
/// and its content, compressed as a backup is.
fn crafted(entries: &[Crafted<'_>]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
    for (name, kind, link, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.as_old_mut().linkname[..link.len()].copy_from_slice(link);
        header.set_entry_type(*kind);
        header.set_mode(if *kind == tar::EntryType::Directory { 0o755 } else { 0o644 });
        header.set_uid(u64::from(uid));
        header.set_gid(u64::from(gid));
        header.set_size(content.len() as u64);
        header.set_cksum();
        builder.append(&header, *content).unwrap();
    }
    zstd::stream::encode_all(builder.into_inner().unwrap().as_slice(), 3).unwrap()
}

#[test]
fn malicious_archives_are_refused_and_change_nothing() {
    use tar::EntryType::{Block, Char, Directory, Link, Regular, Symlink};
    let cases: Vec<(&str, Vec<Crafted<'_>>, &str)> = vec![
        ("dotdot", vec![(b"./", Directory, b"", b""), (b"../../../evil", Regular, b"", b"x")], "leads up"),
        ("deep-dotdot", vec![(b"a/../../evil", Regular, b"", b"x")], "leads up"),
        ("absolute", vec![(b"/tmp/evil", Regular, b"", b"x")], "absolute"),
        (
            "symlink-escape",
            vec![(b"out", Symlink, b"../../..", b""), (b"out/evil", Regular, b"", b"x")],
            "link",
        ),
        (
            "symlink-to-root",
            vec![(b"root", Symlink, b"/", b""), (b"root/tmp/evil", Regular, b"", b"x")],
            "link",
        ),
        ("hard-link-escape", vec![(b"h", Link, b"../../../../etc/passwd", b"")], "leads up"),
        (
            "hard-link-through-symlink",
            vec![(b"out", Symlink, b"/etc", b""), (b"h", Link, b"out/passwd", b"")],
            "link",
        ),
        ("char-device", vec![(b"null", Char, b"", b"")], "device"),
        ("block-device", vec![(b"disk", Block, b"", b"")], "device"),
        ("twice", vec![(b"a", Regular, b"", b"1"), (b"a", Regular, b"", b"2")], "twice"),
    ];
    for (name, entries, why) in cases {
        let place = Place::new();
        fs::write(place.data().join("kept"), "old").unwrap();
        let file = format!("{name}.tar.zst");
        let digest = place.archive(&file, &crafted(&entries));

        let refused = restore(&place.restore_job(&file, &digest), &mut quietly);

        assert!(
            matches!(&refused, Err(ArchiveError::Refused(reason)) if reason.contains(why)),
            "{name}: {refused:?}"
        );
        assert_eq!(fs::read_to_string(place.data().join("kept")).unwrap(), "old", "{name}");
        assert_eq!(fs::read_dir(place.data()).unwrap().count(), 1, "{name}: nothing added");
        assert!(place.beside_the_data().is_empty(), "{name}: the fresh folder is gone");
        for outside in ["evil", "docker/evil", "docker/volumes/evil", "share/evil"] {
            assert!(!place.path(outside).exists(), "{name}: {outside}");
        }
    }
}

#[test]
fn an_archive_changed_while_it_is_read_is_not_swapped_in() {
    let place = Place::new();
    let digest = place.archive("x.tar.zst", &crafted(&[(b"./", tar::EntryType::Directory, b"", b"")]));
    let root = open_folder(&place.data(), OFlags::PATH).unwrap();
    let (mut file, size) = open_archive(
        open_folder(&place.path("share"), OFlags::PATH).unwrap().as_fd(),
        &SharePath::parse("x.tar.zst").unwrap(),
    )
    .unwrap();
    let bounds = Bounds::of(1 << 20, None);
    assert!(extract(&mut file, root.as_fd(), size, &digest, bounds, &mut quiet).is_ok());
    // As if the file's bytes were others by the time they were extracted.
    let other = extract(&mut file, root.as_fd(), size, &"0".repeat(64), bounds, &mut quiet);
    assert!(matches!(&other, Err(ArchiveError::Refused(why)) if why.contains("changed")), "{other:?}");
    let garbage = place.archive("garbage.tar.zst", b"not an archive at all");
    let refused = restore(&place.restore_job("garbage.tar.zst", &garbage), &mut quietly);
    assert!(matches!(refused, Err(ArchiveError::Refused(_))), "{refused:?}");
    assert!(place.beside_the_data().is_empty());
}

/// Bytes zstd cannot shrink.
fn noise(length: usize, mut seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(length);
    while out.len() < length {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.extend_from_slice(&seed.to_le_bytes());
    }
    out.truncate(length);
    out
}

/// What someone who may write the archive puts into its inode once it was
/// checked: `zeros` bytes of zeros in a few KiB, a file `deep` folders
/// deep, and `noise_after` bytes nobody can compress.
fn replacement(zeros: u64, deep: usize, noise_after: usize) -> Vec<u8> {
    let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
    let encoder = zstd::stream::write::Encoder::new(Vec::new(), 19).unwrap();
    let mut builder = tar::Builder::new(encoder);
    let header = |kind: tar::EntryType, size: u64| {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.set_uid(u64::from(uid));
        header.set_gid(u64::from(gid));
        header.set_size(size);
        header
    };
    if zeros > 0 {
        let mut h = header(tar::EntryType::Regular, zeros);
        builder.append_data(&mut h, "bomb", io::repeat(0).take(zeros)).unwrap();
    }
    if deep > 0 {
        let path = vec!["d"; deep].join("/");
        let mut h = header(tar::EntryType::Regular, 1);
        builder.append_data(&mut h, &path, &b"x"[..]).unwrap();
    }
    if noise_after > 0 {
        let bytes = noise(noise_after, 0x9e37_79b9_7f4a_7c15);
        let mut h = header(tar::EntryType::Regular, bytes.len() as u64);
        builder.append_data(&mut h, "padding", bytes.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Overwrites `path` in place: the same inode.
fn rewrite_in_place(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new().write(true).truncate(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

/// What the files below `path` hold.
fn size_below(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path).map(|it| it.flatten().map(|e| size_below(&e.path())).sum()).unwrap_or(0)
}

/// A backup of a volume holding 4 MiB that nobody can compress.
fn recorded(place: &Place) -> BackupDone {
    fs::write(place.path("source/table"), noise(4 << 20, 7)).unwrap();
    back_up(&place.job(), 0, &mut |_| {}).unwrap()
}

/// Root (or here, its owner) could still change a recorded archive after it
/// was checked: what is extracted stops at what the backup held, long
/// before the second hash refuses it.
#[test]
fn bytes_changed_after_the_check_are_extracted_no_further_than_the_backup_held() {
    let place = Place::new();
    let done = recorded(&place);
    let archive = place.path("share").join(done.archive.path.as_str());
    // 256 MiB of zeros in a few KiB, then padding nobody can compress.
    let swapped = replacement(256 << 20, 0, 1 << 20);
    assert!(swapped.len() < (2 << 20), "the replacement is small: {}", swapped.len());

    let mut rewritten = false;
    let mut largest = 0u64;
    let result = restore(&place.restore_of(&done), &mut |step, percent| {
        if step == "verifying" && percent == 100 && !rewritten {
            rewrite_in_place(&archive, &swapped);
            rewritten = true;
        }
        if step == "restoring" {
            for fresh in place.fresh_folders() {
                largest = largest.max(size_below(&fresh));
            }
        }
    });

    assert!(rewritten);
    assert!(
        matches!(&result, Err(ArchiveError::Refused(why)) if why.contains("changed")),
        "refused: {result:?}"
    );
    assert!(largest <= 2 * done.data_bytes, "{largest} bytes extracted of a backup of {}", done.data_bytes);
    assert!(place.fresh_folders().is_empty(), "and removed again");
}

/// A deep tree in an archive changed after the check is refused, and
/// nothing of it stays in Docker's folder, even with a unit's soft limit of
/// descriptors.
#[test]
fn a_tree_changed_after_the_check_leaves_nothing_in_dockers_folder() {
    let limit = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    rustix::process::setrlimit(
        rustix::process::Resource::Nofile,
        rustix::process::Rlimit { current: Some(1024), maximum: limit.maximum },
    )
    .unwrap();
    let place = Place::new();
    let done = recorded(&place);
    let archive = place.path("share").join(done.archive.path.as_str());
    // A file 1024 folders deep, and some data beside.
    let swapped = replacement(64 << 20, MAX_DEPTH, 1 << 20);

    let mut rewritten = false;
    let result = restore(&place.restore_of(&done), &mut |step, percent| {
        if step == "verifying" && percent == 100 && !rewritten {
            rewrite_in_place(&archive, &swapped);
            rewritten = true;
        }
    });
    rustix::process::setrlimit(rustix::process::Resource::Nofile, limit).unwrap();

    assert!(matches!(&result, Err(ArchiveError::Refused(_))), "{result:?}");
    let left: Vec<(PathBuf, u64)> =
        place.fresh_folders().into_iter().map(|p| (p.clone(), size_below(&p))).collect();
    assert!(left.is_empty(), "left in Docker's folder after the refusal: {left:?}");
}

/// A share as zendiloOS sets it lets its writers write what is made in it:
/// the archive and its manifest are made writable by root only all the
/// same, readable as the share says.
#[test]
fn a_writer_of_the_share_may_not_write_the_archive() {
    use crate::acl::{self, Acl, Who};
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    // A writer (UID 65534 here) rwx, also by default for what is made in it.
    let mut rules = Acl::from_mode(0o770);
    rules.set(Who::User(65534), 7);
    rules.fix_mask();
    let share = place.path("share");
    if rustix::fs::setxattr(&share, acl::DEFAULT, &rules.to_bytes(), XattrFlags::empty()).is_err() {
        eprintln!("no POSIX ACLs here; skipped");
        return;
    }
    let done = back_up(&place.job(), 0, &mut |_| {}).unwrap();
    for file in [&done.archive, &done.manifest] {
        let path = share.join(file.path.as_str());
        let mut value = vec![0u8; 4096];
        let length = rustix::fs::getxattr(&path, acl::ACCESS, &mut value[..]).unwrap();
        let access = Acl::parse(&value[..length]).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        let writer = access.named(Who::User(65534)).unwrap_or(0);
        // The mode's group bits are the ACL's mask: no write left in it.
        assert!(writer & 4 != 0, "the writer may still read it: {writer:o}");
        assert!(
            !(writer & 2 != 0 && mode & 0o020 != 0),
            "a writer of the share may write {} (mode {:o}, entry {writer:o})",
            file.path.as_str(),
            mode & 0o7777
        );
    }
}

/// An archive others than root may write could have been changed between
/// its check and its extraction: it is not restored at all.
#[test]
fn an_archive_others_may_write_is_not_restored() {
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    let done = back_up(&place.job(), 0, &mut quiet).unwrap();
    fs::write(place.data().join("kept"), "old").unwrap();
    let archive = place.path("share").join(done.archive.path.as_str());
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o664)).unwrap();

    let refused = restore(&place.restore_of(&done), &mut quietly);

    assert!(
        matches!(&refused, Err(ArchiveError::Refused(why)) if why.contains("written by others")),
        "{refused:?}"
    );
    assert_eq!(fs::read_to_string(place.data().join("kept")).unwrap(), "old");
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(restore(&place.restore_of(&done), &mut quietly).is_ok());
}

/// An archive and its manifest are renamed into place together: should the
/// manifest not follow, the archive goes again, and nothing hidden stays.
#[test]
fn an_archive_is_never_left_without_its_manifest() {
    let place = Place::new();
    let folder = open_folder(&place.path("share"), OFlags::PATH).unwrap();
    let archive = write_partial(folder.as_fd(), "db-1.tar.zst", |mut file| {
        file.write_all(b"archive").map_err(|e| failed("archive", e))
    })
    .unwrap();
    let manifest = write_partial(folder.as_fd(), "db-1.json", |mut file| {
        file.write_all(b"{}").map_err(|e| failed("manifest", e))
    })
    .unwrap();
    // Someone took the manifest's name meanwhile.
    fs::write(place.path("share/db-1.json"), "someone else's").unwrap();

    assert!(put_in_place(folder.as_fd(), &archive, &manifest).is_err());

    let mut names: Vec<String> = fs::read_dir(place.path("share"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["db-1.json"], "only the other file stays");
    assert_eq!(fs::read_to_string(place.path("share/db-1.json")).unwrap(), "someone else's");
}

/// A file with two names whose first name was left out (replaced while it
/// was read) is written whole under its second, never as a link to a name
/// the archive does not hold.
#[test]
fn a_file_whose_first_name_was_left_out_is_written_whole_under_its_second() {
    let place = Place::new();
    let source = place.path("source");
    fs::write(source.join("a"), "data").unwrap();
    fs::hard_link(source.join("a"), source.join("b")).unwrap();
    let folder = open_folder(&source, OFlags::RDONLY).unwrap();
    let mut told = |_: u8| {};
    let mut writer = Writer {
        builder: tar::Builder::new(Vec::new()),
        device: identity(&fstat(&folder).unwrap()).0,
        links: HashMap::new(),
        progress: Progress::new(4, &mut told),
        bytes: 0,
        entries: 0,
        skipped: 0,
    };
    // `a` as it was looked at before it was replaced: another file now.
    let mut stale = statat(&folder, "a", AtFlags::SYMLINK_NOFOLLOW).unwrap();
    stale.st_ino += 1;
    writer.regular(folder.as_fd(), c"a", &stale, PathBuf::from("a")).unwrap();
    let b = statat(&folder, "b", AtFlags::SYMLINK_NOFOLLOW).unwrap();
    writer.regular(folder.as_fd(), c"b", &b, PathBuf::from("b")).unwrap();
    assert_eq!((writer.skipped, writer.entries), (1, 1));

    let bytes = writer.builder.into_inner().unwrap();
    let mut archive = tar::Archive::new(bytes.as_slice());
    let entries: Vec<(String, tar::EntryType, u64)> = archive
        .entries()
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (shown(&e.path_bytes()), e.header().entry_type(), e.header().size().unwrap())
        })
        .collect();
    assert_eq!(entries, [("b".to_owned(), tar::EntryType::Regular, 4)]);
}

#[test]
fn entries_are_named_beneath_the_target_only() {
    assert_eq!(names(b"./").unwrap(), Vec::<Vec<u8>>::new());
    assert_eq!(names(b"a//b/./c").unwrap(), vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    assert!(names(b"/a").is_err() && names(b"a/../b").is_err() && names(b"..").is_err());
    assert!(names(&[b'x'; 256]).is_err());
    let deep = vec!["d"; MAX_DEPTH + 1].join("/");
    assert!(names(deep.as_bytes()).is_err());
}

#[test]
fn only_the_file_a_backup_wrote_is_removed() {
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    let done = back_up(&place.job(), 0, &mut quiet).unwrap();
    let remove = |file: &WrittenFile| RemoveJob {
        share: place.path("share"),
        path: file.path.clone(),
        device: file.device,
        inode: file.inode,
    };
    // Swapped for another file of that name: it stays.
    let archive = place.path("share").join(done.archive.path.as_str());
    fs::rename(&archive, place.path("share/moved")).unwrap();
    fs::write(&archive, "someone else's").unwrap();
    assert!(!remove_backup(&remove(&done.archive)).unwrap());
    assert!(archive.exists());
    fs::rename(place.path("share/moved"), &archive).unwrap();
    assert!(remove_backup(&remove(&done.archive)).unwrap());
    assert!(!archive.exists());
    assert!(!remove_backup(&remove(&done.archive)).unwrap(), "gone already");
    assert!(remove_backup(&remove(&done.manifest)).unwrap());
    let elsewhere = RemoveJob { path: SharePath::parse("nowhere/x").unwrap(), ..remove(&done.manifest) };
    assert!(!remove_backup(&elsewhere).unwrap());
    // Never through a link to a folder.
    symlink(place.path("share/backups"), place.path("share/link")).unwrap();
    let through = RemoveJob { path: SharePath::parse("link/docker/x").unwrap(), ..remove(&done.manifest) };
    assert!(matches!(remove_backup(&through), Err(ArchiveError::Refused(_))));
}

#[test]
fn counting_stays_on_the_file_system_and_within_its_bounds() {
    let place = Place::new();
    fill(&place.path("source"));
    let source = open_folder(&place.path("source"), OFlags::RDONLY).unwrap();
    let all = count(&source, u64::MAX, None);
    assert!(all.complete);
    assert_eq!(all.bytes, 300_000 + 11 + 5, "a file with two names counts once");
    let some = count(&source, 3, None);
    assert!(!some.complete && some.entries == 4);
    let late = count(&source, u64::MAX, Some(Instant::now() - std::time::Duration::from_secs(1)));
    assert!(!late.complete);
}

#[test]
fn units_take_their_jobs_as_hex_and_say_what_came_of_them() {
    let place = Place::new();
    fs::write(place.path("source/a"), "a").unwrap();
    let job = place.job();
    assert_eq!(decode::<BackupJob>(&encode(&job)).unwrap(), job);
    assert!(decode::<BackupJob>("xyz").is_err() && decode::<BackupJob>("abc").is_err());
    assert!(decode::<BackupJob>(&encode(&"not a job")).is_err());

    let mut out = Vec::new();
    assert!(run_volume_job(VolumeJob::BackUp, &encode(&job), 0, &mut out));
    let lines: Vec<UnitLine> =
        String::from_utf8(out).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(matches!(&lines[0], UnitLine::Progress { step, .. } if step == "backing_up"), "{lines:?}");
    let Some(UnitLine::Done(value)) = lines.last() else { panic!("{lines:?}") };
    let done: BackupDone = serde_json::from_value(value.clone()).unwrap();

    let mut out = Vec::new();
    let restore = place.restore_of(&done);
    assert!(run_volume_job(VolumeJob::Restore, &encode(&restore), 0, &mut out));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(r#""step":"verifying""#) && text.contains(r#"{"done":"#), "{text}");

    let mut out = Vec::new();
    let remove =
        RemoveJob { share: place.path("share"), path: done.manifest.path.clone(), device: 0, inode: 0 };
    assert!(run_volume_job(VolumeJob::RemoveBackup, &encode(&remove), 0, &mut out));
    assert_eq!(String::from_utf8(out).unwrap().trim(), r#"{"done":false}"#);

    let mut out = Vec::new();
    assert!(!run_volume_job(VolumeJob::Restore, "00", 0, &mut out));
    let failed: UnitLine = serde_json::from_slice(out.trim_ascii_end()).unwrap();
    assert!(matches!(failed, UnitLine::Failed { refused: true, .. }), "{failed:?}");
    assert_eq!(
        [VolumeJob::BackUp, VolumeJob::Restore, VolumeJob::RemoveBackup].map(VolumeJob::command),
        ["backup-volume", "restore-volume", "remove-volume-backup"]
    );
}
