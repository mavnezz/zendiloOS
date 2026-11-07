use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::*;

struct Volumes {
    directory: tempfile::TempDir,
}

impl Volumes {
    fn new() -> Self {
        let volumes = Self { directory: tempfile::tempdir().unwrap() };
        fs::create_dir_all(volumes.share_path("photos")).unwrap();
        fs::create_dir_all(volumes.share_path("music")).unwrap();
        volumes
    }

    fn share_path(&self, share: &str) -> PathBuf {
        self.directory.path().join("volume1").join(share)
    }

    fn share(&self, share: &str) -> ShareRoot {
        let name = ShareName::parse(share).unwrap();
        ShareRoot::open(self.directory.path(), VolumeNumber::new(1).unwrap(), &name, false).unwrap()
    }

    fn write(&self, share: &str, path: &str, contents: &str) {
        let path = self.share_path(share).join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(&self, share: &str, path: &str) -> String {
        fs::read_to_string(self.share_path(share).join(path)).unwrap()
    }

    fn exists(&self, share: &str, path: &str) -> bool {
        self.share_path(share).join(path).symlink_metadata().is_ok()
    }
}

fn path(value: &str) -> SharePath {
    SharePath::parse(value).unwrap()
}

fn names(listing: &Listing) -> Vec<&str> {
    let mut names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    names
}

#[test]
fn lists_directories_without_following_links() {
    let v = Volumes::new();
    v.write("photos", "beach.jpg", "sand");
    fs::create_dir(v.share_path("photos").join("2024")).unwrap();
    symlink("beach.jpg", v.share_path("photos").join("link")).unwrap();

    let listing = v.share("photos").list(&SharePath::root()).unwrap();
    assert_eq!(names(&listing), ["2024", "beach.jpg", "link"]);
    let beach = listing.entries.iter().find(|e| e.name == "beach.jpg").unwrap();
    assert_eq!((beach.kind, beach.size), (EntryKind::File, 4));
    assert!(beach.modified > 0);
    let link = listing.entries.iter().find(|e| e.name == "link").unwrap();
    assert_eq!(link.kind, EntryKind::Symlink);
    assert!(listing.writable && !listing.truncated);

    assert!(matches!(v.share("photos").list(&path("missing")), Err(FileError::NotFound)));
    assert!(matches!(v.share("photos").list(&path("beach.jpg")), Err(FileError::Invalid(_))));
}

#[test]
fn nothing_leads_out_of_the_share() {
    let v = Volumes::new();
    v.write("music", "secret.txt", "not for photos");
    fs::write(v.directory.path().join("volume1").join("outside.txt"), "volume").unwrap();
    let root = v.share_path("photos");
    symlink("../music/secret.txt", root.join("relative")).unwrap();
    symlink(v.share_path("music").join("secret.txt"), root.join("absolute")).unwrap();
    symlink("..", root.join("up")).unwrap();
    symlink("/proc/self/root/etc/passwd", root.join("magic")).unwrap();
    v.write("photos", "inside.txt", "fine");
    symlink("inside.txt", root.join("sideways")).unwrap();

    let photos = v.share("photos");
    for escape in ["relative", "absolute", "up/outside.txt", "up/music/secret.txt", "magic"] {
        assert!(photos.open_file(&path(escape)).is_err(), "{escape} must not open");
    }
    assert!(photos.list(&path("up")).is_err());
    let (file, size) = photos.open_file(&path("sideways")).unwrap();
    assert_eq!(size, 4, "links within the share work");
    let mut text = String::new();
    File::from(file).read_to_string(&mut text).unwrap();
    assert_eq!(text, "fine");

    // Removing, copying and moving a link handle the link, not its target.
    photos.remove(&path("relative"), None).unwrap();
    assert_eq!(v.read("music", "secret.txt"), "not for photos");
    let copied = photos.copy_entry(&path("absolute"), &photos, &path("copy")).unwrap();
    assert_eq!(copied, path("copy"));
    assert!(fs::symlink_metadata(root.join("copy")).unwrap().file_type().is_symlink());
}

#[test]
fn uploads_land_under_a_hidden_name_and_are_renamed() {
    let v = Volumes::new();
    let photos = v.share("photos");
    let (file, name) = photos.create_upload(&SharePath::root()).unwrap();
    assert!(name.starts_with(UPLOAD_PREFIX), "{name}");
    File::from(file).write_all(b"new photo").unwrap();

    photos.move_entry(&path(&name), &photos, &path("new.jpg"), false).unwrap();
    assert_eq!(v.read("photos", "new.jpg"), "new photo");

    let (file, name) = photos.create_upload(&SharePath::root()).unwrap();
    File::from(file).write_all(b"newer").unwrap();
    assert!(matches!(
        photos.move_entry(&path(&name), &photos, &path("new.jpg"), false),
        Err(FileError::Exists)
    ));
    photos.move_entry(&path(&name), &photos, &path("new.jpg"), true).unwrap();
    assert_eq!(v.read("photos", "new.jpg"), "newer");

    photos.make_directory(&path("album")).unwrap();
    v.write("photos", "other.jpg", "x");
    assert!(
        matches!(
            photos.move_entry(&path("other.jpg"), &photos, &path("album"), true),
            Err(FileError::Exists)
        ),
        "a file never replaces a folder"
    );
    assert!(matches!(photos.make_directory(&path("album")), Err(FileError::Exists)));
    assert!(matches!(photos.make_directory(&SharePath::root()), Err(FileError::Invalid(_))));
}

#[test]
fn copies_trees_next_to_existing_names() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    v.write("photos", "album/deep/b.jpg", "b");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    File::options()
        .write(true)
        .open(v.share_path("photos").join("album/a.jpg"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let (photos, music) = (v.share("photos"), v.share("music"));

    assert_eq!(photos.copy_entry(&path("album"), &photos, &path("album")).unwrap(), path("album (2)"));
    assert_eq!(photos.copy_entry(&path("album"), &photos, &path("album")).unwrap(), path("album (3)"));
    assert_eq!(v.read("photos", "album (2)/deep/b.jpg"), "b");
    let modified = fs::metadata(v.share_path("photos").join("album (2)/a.jpg")).unwrap().modified().unwrap();
    assert_eq!(modified, old, "modification times are kept");

    assert_eq!(
        photos.copy_entry(&path("album/a.jpg"), &photos, &path("album/a.jpg")).unwrap(),
        path("album/a (2).jpg")
    );
    assert_eq!(photos.copy_entry(&path("album"), &music, &path("album")).unwrap(), path("album"));
    assert_eq!(v.read("music", "album/deep/b.jpg"), "b");
    assert!(matches!(
        photos.copy_entry(&path("album"), &photos, &path("album/deep/album")),
        Err(FileError::Invalid(_))
    ));
    assert!(matches!(
        photos.move_entry(&path("album"), &photos, &path("album/deep/album"), false),
        Err(FileError::Invalid(_))
    ));
    assert_eq!(numbered(".hidden", 2), ".hidden (2)");
    assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
}

#[test]
fn removing_goes_to_the_users_recycle_bin() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "first");
    let photos = v.share("photos");

    photos.remove(&path("album/a.jpg"), Some("anna")).unwrap();
    assert_eq!(v.read("photos", ".recycle/anna/album/a.jpg"), "first");
    v.write("photos", "album/a.jpg", "second");
    photos.remove(&path("album/a.jpg"), Some("anna")).unwrap();
    assert_eq!(v.read("photos", ".recycle/anna/album/Copy #1 of a.jpg"), "second");
    let mode = fs::metadata(v.share_path("photos").join(".recycle/anna")).unwrap();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o077, 0, "private");

    photos.remove(&path("album"), Some("anna")).unwrap();
    assert!(v.exists("photos", ".recycle/anna/album/a.jpg"));
    assert!(v.exists("photos", ".recycle/anna/Copy #1 of album"), "the folder joins the bin's tree");

    // In the bin, removing is for good.
    photos.remove(&path(".recycle/anna/album"), Some("anna")).unwrap();
    assert!(!v.exists("photos", ".recycle/anna/album"));
    v.write("photos", "gone/x/y.txt", "y");
    photos.remove(&path("gone"), None).unwrap();
    assert!(!v.exists("photos", "gone"));
    assert!(matches!(photos.remove(&path("gone"), None), Err(FileError::NotFound)));
    assert!(matches!(photos.remove(&SharePath::root(), None), Err(FileError::Invalid(_))));
}

#[test]
fn a_bin_someone_else_could_read_is_refused() {
    let v = Volumes::new();
    v.write("photos", "a.jpg", "mine");
    let bin = v.share_path("photos").join(".recycle/anna");
    fs::create_dir_all(&bin).unwrap();
    fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();

    let refused = v.share("photos").remove(&path("a.jpg"), Some("anna"));

    assert!(matches!(refused, Err(FileError::Invalid(_))), "{refused:?}");
    assert!(v.exists("photos", "a.jpg"), "nothing moved");
}

#[test]
fn restoring_puts_an_entry_back_where_it_was() {
    let v = Volumes::new();
    v.write("photos", "album/2024/a.jpg", "first");
    let photos = v.share("photos");
    photos.remove(&path("album/2024/a.jpg"), Some("anna")).unwrap();
    v.write("photos", "album/2024/a.jpg", "second");
    photos.remove(&path("album/2024/a.jpg"), Some("anna")).unwrap();
    photos.remove(&path("album"), Some("anna")).unwrap();
    assert!(!v.exists("photos", "album"));

    // The folders on the way are made again.
    let back = photos.restore(&path(".recycle/anna/album/2024/a.jpg"), "anna").unwrap();
    assert_eq!(back, path("album/2024/a.jpg"));
    assert_eq!(v.read("photos", "album/2024/a.jpg"), "first");
    // The second one gets its own name back, or a free one beside it.
    let second = photos.restore(&path(".recycle/anna/album/2024/Copy #1 of a.jpg"), "anna").unwrap();
    assert_eq!(second, path("album/2024/a (2).jpg"));
    assert_eq!(v.read("photos", "album/2024/a (2).jpg"), "second");
    // A whole folder from the bin's root.
    let folder = photos.restore(&path(".recycle/anna/Copy #1 of album"), "anna").unwrap();
    assert_eq!(folder, path("album (2)"));

    for (bin_path, user) in [
        (".recycle/ben/x.jpg", "anna"),
        ("album/2024/a.jpg", "anna"),
        (".recycle/anna", "anna"),
        (".recycle", "anna"),
    ] {
        let refused = photos.restore(&path(bin_path), user);
        assert!(matches!(refused, Err(FileError::Invalid(_))), "{bin_path}: {refused:?}");
    }
    let gone = photos.restore(&path(".recycle/anna/nothing.jpg"), "anna");
    assert!(matches!(gone, Err(FileError::NotFound)), "{gone:?}");
}

#[test]
fn bin_names_lose_only_a_real_copy_prefix() {
    assert_eq!(original_name("Copy #12 of a.jpg"), "a.jpg");
    for kept in ["a.jpg", "Copy # of a.jpg", "Copy #x of a.jpg", "Copy #1 of ", "Copy #1a.jpg"] {
        assert_eq!(original_name(kept), kept);
    }
}

#[test]
fn an_unmounted_share_is_refused() {
    let v = Volumes::new();
    let name = ShareName::parse("photos").unwrap();
    let opened = ShareRoot::open(v.directory.path(), VolumeNumber::new(1).unwrap(), &name, true);
    assert!(matches!(opened, Err(FileError::Invalid(_))));
    let missing = ShareName::parse("missing").unwrap();
    let opened = ShareRoot::open(v.directory.path(), VolumeNumber::new(1).unwrap(), &missing, false);
    assert!(matches!(opened, Err(FileError::NotFound)));
    assert!(Path::new(v.directory.path()).exists());
}

/// What `tar` reads back: path, kind, content or link.
fn unpacked(archive: &[u8]) -> Vec<(String, char, String)> {
    let mut reader = tar::Archive::new(archive);
    reader
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let kind = char::from(entry.header().entry_type().as_byte());
            let link = entry.link_name().unwrap().map(|l| l.to_string_lossy().into_owned());
            let mut content = String::new();
            entry.read_to_string(&mut content).unwrap();
            (path, kind, link.unwrap_or(content))
        })
        .collect()
}

#[test]
fn folders_become_tar_archives_of_the_planned_length() {
    let v = Volumes::new();
    v.write("photos", "album/b.txt", "bee");
    v.write("photos", "album/a/inner.txt", "inner");
    let long = format!("album/{}/{}.txt", "x".repeat(80), "y".repeat(60));
    v.write("photos", &long, "deep");
    v.write("photos", "album/.zendiloos-upload-1", "half");
    symlink("b.txt", v.share_path("photos").join("album/link")).unwrap();
    let photos = v.share("photos");

    let plan = photos.plan_archive(&path("album"), "album").unwrap();
    let mut archive = Vec::new();
    photos.write_archive(&path("album"), &plan, &mut archive).unwrap();

    assert_eq!(archive.len() as u64, plan.length(), "exactly as announced");
    assert_eq!(
        unpacked(&archive),
        [
            ("album/".to_owned(), '5', String::new()),
            ("album/a/".to_owned(), '5', String::new()),
            ("album/a/inner.txt".to_owned(), '0', "inner".to_owned()),
            ("album/b.txt".to_owned(), '0', "bee".to_owned()),
            ("album/link".to_owned(), '2', "b.txt".to_owned()),
            (format!("album/{}/", "x".repeat(80)), '5', String::new()),
            (long.clone(), '0', "deep".to_owned()),
        ],
        "sorted, links kept as links, long names whole, uploads in progress left out"
    );
}

#[test]
fn an_archive_follows_its_plan_to_the_byte() {
    let v = Volumes::new();
    v.write("photos", "grows.txt", "1234");
    v.write("photos", "shrinks.txt", "12345678");
    v.write("photos", ".recycle/anna/old.txt", "old");
    let photos = v.share("photos");

    let plan = photos.plan_archive(&SharePath::root(), "photos").unwrap();
    v.write("photos", "grows.txt", "123456789");
    v.write("photos", "shrinks.txt", "12");
    let mut archive = Vec::new();
    photos.write_archive(&SharePath::root(), &plan, &mut archive).unwrap();

    assert_eq!(archive.len() as u64, plan.length());
    let entries = unpacked(&archive);
    let names: Vec<&str> = entries.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(names, ["photos/", "photos/grows.txt", "photos/shrinks.txt"], "no recycle bins");
    assert_eq!(entries[1].2, "1234", "cut to the planned size");
    assert_eq!(entries[2].2, "12\0\0\0\0\0\0", "padded to the planned size");

    // A file gone since the plan: the archive ends short.
    fs::remove_file(v.share_path("photos").join("grows.txt")).unwrap();
    let mut short = Vec::new();
    assert!(photos.write_archive(&SharePath::root(), &plan, &mut short).is_err());
    assert!((short.len() as u64) < plan.length());
}

#[test]
fn what_cannot_be_read_refuses_the_archive() {
    if rustix::process::geteuid().is_root() {
        return; // root reads everything
    }
    let v = Volumes::new();
    v.write("photos", "album/secret/x.txt", "x");
    let secret = v.share_path("photos").join("album/secret");
    fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();

    let refused = v.share("photos").plan_archive(&path("album"), "album");

    fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert!(matches!(refused, Err(FileError::Denied)), "{refused:?}");
}

#[test]
fn deep_trees_archive_with_one_name_per_entry() {
    let v = Volumes::new();
    let deep: String = (0..200).map(|_| "d/").collect();
    v.write("photos", &format!("top/{deep}bottom.txt"), "deep");
    v.write("photos", "top/z.txt", "after the deep folder");
    let photos = v.share("photos");

    let plan = photos.plan_archive(&path("top"), "top").unwrap();
    let mut archive = Vec::new();
    photos.write_archive(&path("top"), &plan, &mut archive).unwrap();

    assert_eq!(archive.len() as u64, plan.length());
    let entries = unpacked(&archive);
    assert_eq!(entries.len(), 1 + 200 + 2);
    let bottom = entries.iter().find(|(name, _, _)| name.ends_with("bottom.txt")).unwrap();
    assert_eq!((bottom.0.len(), bottom.2.as_str()), ("top/".len() + deep.len() + "bottom.txt".len(), "deep"));
    assert_eq!(entries.last().unwrap(), &("top/z.txt".to_owned(), '0', "after the deep folder".to_owned()));
}

#[test]
fn app_folders_are_made_name_by_name_and_never_through_a_link() {
    let v = Volumes::new();
    let root = v.share("photos");
    root.make_folders(&AppFolder::parse("config/cache").unwrap()).unwrap();
    root.make_folders(&AppFolder::parse("config").unwrap()).unwrap();
    assert!(v.share_path("photos").join("config/cache").is_dir());

    symlink("/etc", v.share_path("photos").join("evil")).unwrap();
    let refused = root.make_folders(&AppFolder::parse("evil/inside").unwrap());
    assert!(matches!(refused, Err(FileError::Invalid(_))), "{refused:?}");
    assert!(!Path::new("/etc/inside").exists());
}

#[test]
fn app_data_needs_its_own_dataset() {
    let v = Volumes::new();
    fs::create_dir_all(v.directory.path().join("volume1/@apps/demo")).unwrap();
    let app = AppName::parse("demo").unwrap();
    let refused = ShareRoot::open_app_data(v.directory.path(), VolumeNumber::new(1).unwrap(), &app);
    assert!(matches!(refused, Err(FileError::Invalid(_))), "{refused:?}");
}

#[test]
fn a_failed_copy_into_another_file_system_leaves_nothing_behind() {
    // As a copy into another share: the target on another file system.
    let Ok(target) = tempfile::tempdir_in("/dev/shm") else { return };
    let source = tempfile::tempdir().unwrap();
    if rustix::process::geteuid().is_root() {
        return; // root reads the unreadable file
    }
    let album = source.path().join("album");
    fs::create_dir_all(&album).unwrap();
    fs::write(album.join("a.jpg"), "a").unwrap();
    fs::write(album.join("b.jpg"), "b").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(album.join("b.jpg"), fs::Permissions::from_mode(0o000)).unwrap();
    let directory =
        |path: &Path| rustix::fs::open(path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty()).unwrap();
    let (from, to) = (directory(source.path()), directory(target.path()));
    let source_fs = device(&rustix::fs::fstat(&from).unwrap());
    if source_fs == device(&rustix::fs::fstat(&to).unwrap()) {
        return; // one file system here: nothing to show
    }

    let copied = copy_new(from.as_fd(), c"album", to.as_fd(), c"album", source_fs, 0);

    assert!(copied.is_err(), "an unreadable file was copied");
    assert!(!target.path().join("album").exists(), "the half copy stayed");
}

#[test]
fn a_given_up_upload_is_discarded_only_as_a_file() {
    let v = Volumes::new();
    let share = v.share("photos");
    let (file, name) = share.create_upload(&path("")).unwrap();
    drop(file);
    share.discard_upload(&path(&name)).unwrap();
    assert!(!v.exists("photos", &name), "the upload went");

    // Somebody who may write the folder put the user's folder in its place.
    v.write("photos", "Private/diary.txt", "mine");
    let (file, name) = share.create_upload(&path("")).unwrap();
    drop(file);
    fs::remove_file(v.share_path("photos").join(&name)).unwrap();
    fs::rename(v.share_path("photos").join("Private"), v.share_path("photos").join(&name)).unwrap();
    let refused = share.discard_upload(&path(&name));
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(v.read("photos", &format!("{name}/diary.txt")), "mine", "the folder stays");

    v.write("photos", "notes.txt", "x");
    assert!(matches!(share.discard_upload(&path("notes.txt")), Err(FileError::Invalid(_))), "not an upload");
    assert!(v.exists("photos", "notes.txt"));
}

#[test]
fn what_replaced_the_folder_meant_is_not_removed() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    v.write("photos", "Private/diary.txt", "mine");
    let directory =
        rustix::fs::open(v.share_path("photos"), OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty()).unwrap();
    let album = identity(&statat(&directory, c"album", AtFlags::SYMLINK_NOFOLLOW).unwrap());
    let file_system = device(&fstat(&directory).unwrap());
    // Swapped after its identity was taken, as during a move between shares.
    let at = |name: &str| v.share_path("photos").join(name);
    fs::rename(at("album"), at("elsewhere")).unwrap();
    fs::rename(at("Private"), at("album")).unwrap();

    let refused = remove_entry(directory.as_fd(), c"album", file_system, 0, Some(album));

    assert!(matches!(refused, Err(FileError::Invalid(_))), "{refused:?}");
    assert_eq!(v.read("photos", "album/diary.txt"), "mine");
    fs::rename(at("album"), at("Private")).unwrap();
    fs::rename(at("elsewhere"), at("album")).unwrap();
    remove_entry(directory.as_fd(), c"album", file_system, 0, Some(album)).unwrap();
    assert!(!v.exists("photos", "album"));
}

#[test]
fn large_folders_are_copied_and_removed_without_reading_them_whole() {
    let v = Volumes::new();
    let big = v.share_path("photos").join("big");
    fs::create_dir_all(&big).unwrap();
    let count = BATCH * 2 + 17;
    for n in 0..count {
        fs::write(big.join(format!("{n:05}.txt")), "").unwrap();
    }
    let share = v.share("photos");
    let copied = share.copy_entry(&path("big"), &v.share("photos"), &path("copy")).unwrap();
    assert_eq!(fs::read_dir(v.share_path("photos").join(copied.as_str())).unwrap().count(), count);
    share.remove(&path("big"), None).unwrap();
    assert!(!v.exists("photos", "big"));
}

#[test]
fn an_archive_plan_stays_within_its_limits_while_it_reads() {
    let v = Volumes::new();
    let links = v.share_path("photos").join("links");
    fs::create_dir_all(&links).unwrap();
    let target = "t".repeat(4000);
    // Enough long link targets to go over the budget.
    for n in 0..(MAX_ARCHIVE_NAME_BYTES / 4000 + 2) {
        symlink(&target, links.join(format!("{n:05}"))).unwrap();
    }
    let refused = v.share("photos").plan_archive(&path("links"), "links");
    assert!(matches!(refused, Err(FileError::Invalid(why)) if why.contains("too long")), "{refused:?}");
}

#[test]
fn an_apps_data_and_a_personal_folder_are_roots_only_when_mounted() {
    let v = Volumes::new();
    let volume = VolumeNumber::new(1).unwrap();
    let app = AppName::parse("kuma").unwrap();
    let anna = zendiloos_agent_proto::UserName::parse("anna").unwrap();
    assert!(matches!(ShareRoot::open_app_data(v.directory.path(), volume, &app), Err(FileError::NotFound)));
    fs::create_dir_all(v.directory.path().join("volume1/@apps/kuma")).unwrap();
    let unmounted = ShareRoot::open_app_data(v.directory.path(), volume, &app);
    assert!(
        matches!(unmounted, Err(FileError::Invalid(why)) if why.contains("not mounted")),
        "{unmounted:?}"
    );

    assert!(matches!(ShareRoot::open_home(v.directory.path(), volume, &anna), Err(FileError::NotFound)));
    fs::create_dir_all(v.directory.path().join("volume1").join(HOMES_DIRECTORY).join("anna")).unwrap();
    let unmounted = ShareRoot::open_home(v.directory.path(), volume, &anna);
    assert!(
        matches!(unmounted, Err(FileError::Invalid(why)) if why.contains("not mounted")),
        "{unmounted:?}"
    );

    let label = zendiloos_agent_proto::SnapshotLabel::parse("zdl-2026-11-07_08.00.00").unwrap();
    let photos = ShareName::parse("photos").unwrap();
    let snapshot = ShareRoot::open_snapshot(v.directory.path(), volume, &photos, &label);
    assert!(matches!(snapshot, Err(FileError::Invalid(why)) if why.contains("not mounted")), "{snapshot:?}");
}

#[test]
fn an_apps_folders_are_made_name_by_name_and_its_root_given_to_it() {
    let v = Volumes::new();
    let share = v.share("photos");
    let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());

    share.set_root_owner(uid, gid, 0o750).unwrap();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(v.share_path("photos")).unwrap().permissions())
            & 0o777,
        0o750
    );
    share.make_folders(&AppFolder::parse("config/cache").unwrap()).unwrap();
    share.make_folders(&AppFolder::parse("config/cache").unwrap()).unwrap();
    assert!(v.share_path("photos").join("config/cache").is_dir(), "made, and again without harm");
    symlink("/etc", v.share_path("photos").join("escape")).unwrap();
    let through = share.make_folders(&AppFolder::parse("escape/x").unwrap());
    assert!(matches!(through, Err(FileError::Invalid(why)) if why.contains("link")), "{through:?}");
    v.write("photos", "file", "");
    assert!(share.make_folders(&AppFolder::parse("file/x").unwrap()).is_err(), "no folder");
}
