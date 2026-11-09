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
fn something_inside_a_folder_deleted_twice_goes_back_into_that_folder() {
    let v = Volumes::new();
    let photos = v.share("photos");
    v.write("photos", "album/a.jpg", "first");
    photos.remove(&path("album"), Some("anna")).unwrap();
    v.write("photos", "album/2024/b.jpg", "second");
    photos.remove(&path("album"), Some("anna")).unwrap();
    assert!(v.exists("photos", ".recycle/anna/Copy #1 of album/2024/b.jpg"));

    // Deep inside the second one: back into `album`, made again.
    let back = photos.restore(&path(".recycle/anna/Copy #1 of album/2024/b.jpg"), "anna").unwrap();
    assert_eq!(back, path("album/2024/b.jpg"));
    assert_eq!(v.read("photos", "album/2024/b.jpg"), "second");
    let first = photos.restore(&path(".recycle/anna/album/a.jpg"), "anna").unwrap();
    assert_eq!(first, path("album/a.jpg"));
    // Deleting inside the bin is for good, at any depth.
    v.write("photos", "gone/deep/c.jpg", "third");
    photos.remove(&path("gone"), Some("anna")).unwrap();
    photos.remove(&path(".recycle/anna/gone/deep/c.jpg"), Some("anna")).unwrap();
    assert!(!v.exists("photos", ".recycle/anna/gone/deep/c.jpg"));
    assert!(v.exists("photos", ".recycle/anna/gone/deep"));
}

fn far_away() -> SearchBounds<'static> {
    static NEVER: AtomicBool = AtomicBool::new(false);
    SearchBounds { until: Instant::now() + std::time::Duration::from_secs(60), stop: &NEVER }
}

fn text(value: &str) -> SearchText {
    SearchText::parse(value).unwrap()
}

fn found_paths(found: &Found) -> Vec<&str> {
    let mut paths: Vec<&str> = found.entries.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths
}

#[test]
fn a_search_finds_names_below_a_folder_whatever_their_case() {
    let v = Volumes::new();
    v.write("photos", "Beach.jpg", "sand");
    v.write("photos", "album/2024/beach party.png", "music");
    v.write("photos", "album/BEACHES/notes.txt", "list");
    v.write("photos", "album/2023/mountain.jpg", "rock");
    v.write("photos", ".hidden/beach.jpg", "secret");
    v.write("photos", "album/.beach.txt", "dot");
    v.write("photos", ".zendiloos-upload-0123456789abcdef", "beach under way");
    let photos = v.share("photos");
    photos.remove(&path("album/2023/mountain.jpg"), Some("anna")).unwrap();
    v.write("photos", "album/2023/beachball.jpg", "ball");
    photos.remove(&path("album/2023/beachball.jpg"), Some("anna")).unwrap();
    symlink(v.share_path("music"), v.share_path("photos").join("beach-link")).unwrap();
    v.write("music", "beach boys.mp3", "surf");

    let found = photos.search(&SharePath::root(), &text("bEaCh"), false, far_away()).unwrap();
    assert_eq!(found.stopped, None);
    assert_eq!(
        found_paths(&found),
        ["Beach.jpg", "album/2024/beach party.png", "album/BEACHES", "beach-link"],
        "no hidden names, no bins, no uploads, no links followed"
    );
    let folder = found.entries.iter().find(|f| f.path.as_str() == "album/BEACHES").unwrap();
    assert_eq!(folder.entry.kind, EntryKind::Directory);
    let link = found.entries.iter().find(|f| f.path.as_str() == "beach-link").unwrap();
    assert_eq!(link.entry.kind, EntryKind::Symlink);

    // Hidden names when asked; below a folder only what is below it.
    let hidden = photos.search(&SharePath::root(), &text("beach"), true, far_away()).unwrap();
    assert!(found_paths(&hidden).contains(&".hidden/beach.jpg"));
    assert!(found_paths(&hidden).contains(&"album/.beach.txt"));
    assert!(!found_paths(&hidden).iter().any(|p| p.starts_with(".recycle")), "the bins only from inside");
    let below = photos.search(&path("album"), &text("beach"), false, far_away()).unwrap();
    assert_eq!(found_paths(&below), ["album/2024/beach party.png", "album/BEACHES"]);
    // From inside a bin, its entries.
    let bin = photos.search(&path(".recycle/anna"), &text("beach"), false, far_away()).unwrap();
    assert_eq!(found_paths(&bin), [".recycle/anna/album/2023/beachball.jpg"]);

    let missing = photos.search(&path("nothing"), &text("beach"), false, far_away());
    assert!(matches!(missing, Err(FileError::NotFound)), "{missing:?}");
}

#[test]
fn a_search_never_takes_a_name_that_is_not_utf8_for_another() {
    use std::os::unix::ffi::OsStrExt;
    let v = Volumes::new();
    let odd = std::ffi::OsStr::from_bytes(b"beach-\xFF");
    fs::create_dir(v.share_path("photos").join(odd)).unwrap();
    fs::write(v.share_path("photos").join(odd).join("beach.jpg"), "inside").unwrap();
    v.write("photos", "beach-\u{FFFD}.txt", "real");
    let found = v.share("photos").search(&SharePath::root(), &text("beach"), false, far_away()).unwrap();
    assert_eq!(found_paths(&found), ["beach-\u{FFFD}.txt"]);
}

#[test]
fn a_folder_is_counted_as_far_as_it_may_be_seen() {
    let v = Volumes::new();
    v.write("photos", "Beach.jpg", "sand");
    v.write("photos", "album/2024/party.png", "music!");
    v.write("photos", "album/2023/mountain.jpg", "rock");
    v.write("photos", ".hidden/secret.jpg", "secret");
    v.write("photos", "album/.thumbs", "dot");
    v.write("photos", ".zendiloos-upload-0123456789abcdef", "under way");
    fs::create_dir_all(v.share_path("photos").join("album/empty")).unwrap();
    symlink(v.share_path("music"), v.share_path("photos").join("music-link")).unwrap();
    v.write("music", "song.mp3", "never counted through a link");
    let photos = v.share("photos");

    let all = photos.facts(&SharePath::root(), far_away()).unwrap();
    assert!(all.folder);
    assert_eq!(
        (all.files, all.folders, all.hidden, all.stopped),
        (4, 4, 2, None),
        "Beach.jpg, party.png, mountain.jpg and the link; album, 2024, 2023, empty; .hidden and .thumbs"
    );
    assert_eq!(all.bytes, 4 + 6 + 4, "only regular files hold bytes");
    assert!(all.modified.is_some());
    assert_eq!(all.uid, rustix::process::getuid().as_raw());

    let album = photos.facts(&path("album"), far_away()).unwrap();
    assert_eq!((album.files, album.folders, album.hidden), (2, 3, 1));

    let file = photos.facts(&path("album/2024/party.png"), far_away()).unwrap();
    assert!(!file.folder);
    assert_eq!((file.bytes, file.files, file.folders), (6, 0, 0));
    assert!(photos.facts(&path("music-link/song.mp3"), far_away()).is_err(), "never through a link");

    let past = SearchBounds { until: Instant::now(), stop: far_away().stop };
    for n in 0..(SEARCH_CHECK_EVERY * 2) {
        v.write("photos", &format!("many/file-{n}.txt"), "x");
    }
    let late = photos.facts(&SharePath::root(), past).unwrap();
    assert_eq!(late.stopped, Some(CountStop::Time), "counted so far");
}

#[test]
fn a_search_stops_at_its_limits() {
    let v = Volumes::new();
    for n in 0..(MAX_SEARCH_RESULTS + 20) {
        v.write("photos", &format!("many/file-{n}.txt"), "x");
    }
    let photos = v.share("photos");
    let found = photos.search(&SharePath::root(), &text("file"), false, far_away()).unwrap();
    assert_eq!((found.entries.len(), found.stopped), (MAX_SEARCH_RESULTS, Some(SearchStop::Results)));

    let past = SearchBounds { until: Instant::now(), stop: far_away().stop };
    let late = photos.search(&SharePath::root(), &text("file"), false, past).unwrap();
    assert_eq!(late.stopped, Some(SearchStop::Time));
    assert!(late.entries.len() < SEARCH_CHECK_EVERY);
    let gone = AtomicBool::new(true);
    let stopped = SearchBounds { until: far_away().until, stop: &gone };
    let given_up = photos.search(&SharePath::root(), &text("file"), false, stopped).unwrap();
    assert_eq!(given_up.stopped, Some(SearchStop::Time));
}

#[test]
fn a_search_stays_on_its_file_system_and_within_its_depth() {
    let v = Volumes::new();
    let mut deep = String::from("d");
    for _ in 1..(MAX_DEPTH + 4) {
        deep.push_str("/d");
    }
    fs::create_dir_all(v.share_path("photos").join(&deep)).unwrap();
    fs::write(v.share_path("photos").join(&deep).join("target.txt"), "far").unwrap();
    v.write("photos", "near/target.txt", "close");
    let found = v.share("photos").search(&SharePath::root(), &text("target"), false, far_away()).unwrap();
    assert_eq!(found_paths(&found), ["near/target.txt"], "nothing deeper than 256 folders");
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

    let copied = copy_new(
        from.as_fd(),
        c"album",
        to.as_fd(),
        c"album",
        source_fs,
        0,
        &mut Copied::off(),
        NewEntries::Maker,
    );

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
    // Swapped after its identity was taken, as while a failed copy is cleaned up.
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
fn a_personal_folder_and_a_snapshot_are_roots_only_when_mounted() {
    let v = Volumes::new();
    let volume = VolumeNumber::new(1).unwrap();
    let anna = zendiloos_agent_proto::UserName::parse("anna").unwrap();
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
fn a_name_that_is_not_utf8_is_flagged_not_taken_for_another() {
    use std::os::unix::ffi::OsStrExt;
    let v = Volumes::new();
    let root = v.share_path("photos");
    let raw = root.join(std::ffi::OsStr::from_bytes(b"file-\xFF.txt"));
    fs::write(&raw, "bytes").unwrap();
    fs::write(root.join("file-\u{FFFD}.txt"), "text").unwrap();

    let listing = v.share("photos").list(&SharePath::root()).unwrap();
    assert_eq!(names(&listing), ["file-\u{FFFD}.txt", "file-\u{FFFD}.txt"], "both read alike");
    let flagged: Vec<bool> = listing.entries.iter().map(|e| e.unreadable).collect();
    assert_eq!(flagged.iter().filter(|u| **u).count(), 1, "only the one that is not UTF-8: {flagged:?}");
    assert!(!listing.entries.iter().any(|e| e.unreadable && e.kind != EntryKind::File));

    // Removing by the name shown takes the file that has it, never the other.
    v.share("photos").remove(&path("file-\u{FFFD}.txt"), None).unwrap();
    assert_eq!(fs::read_to_string(&raw).unwrap(), "bytes");
    assert!(!v.exists("photos", "file-\u{FFFD}.txt"));
}

#[test]
fn a_removable_drive_is_a_root_only_while_mounted() {
    let places = tempfile::tempdir().unwrap();
    let id = RemovableId::parse("1A2B-3C4D").unwrap();
    assert!(matches!(ShareRoot::open_removable(places.path(), &id, true), Err(FileError::NotFound)));
    fs::create_dir(places.path().join("1A2B-3C4D")).unwrap();
    // The empty folder of a drive that is not mounted: nothing goes in.
    let unmounted = ShareRoot::open_removable(places.path(), &id, true);
    assert!(
        matches!(unmounted, Err(FileError::Invalid(why)) if why.contains("not mounted")),
        "{unmounted:?}"
    );
    fs::write(places.path().join("1A2B-3C4D/song.mp3"), "tune").unwrap();
    let drive = ShareRoot::open_removable(places.path(), &id, false).unwrap();
    assert_eq!(names(&drive.list(&SharePath::root()).unwrap()), ["song.mp3"]);
    // A link in its place leads nowhere.
    fs::remove_dir_all(places.path().join("1A2B-3C4D")).unwrap();
    symlink("/", places.path().join("1A2B-3C4D")).unwrap();
    assert!(ShareRoot::open_removable(places.path(), &id, false).is_err(), "followed a link");
}

#[test]
fn trees_go_across_piece_by_piece_and_keep_their_times() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    v.write("photos", "album/deep/b.jpg", "b");
    symlink("a.jpg", v.share_path("photos").join("album/link")).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    let a = v.share_path("photos").join("album/a.jpg");
    File::options().write(true).open(a).unwrap().set_modified(old).unwrap();
    v.write("music", "album/kept.txt", "kept");
    let (photos, music) = (v.share("photos"), v.share("music"));

    let (copied, _) =
        copy_through(&photos, &path("album"), &music, &path("album"), Taken::Beside, false).unwrap();
    assert_eq!(copied, path("album (2)"), "beside the name that is taken");
    assert_eq!(v.read("music", "album (2)/deep/b.jpg"), "b");
    assert_eq!(v.read("music", "album/kept.txt"), "kept");
    let link = fs::read_link(v.share_path("music").join("album (2)/link")).unwrap();
    assert_eq!(link, Path::new("a.jpg"));
    let modified = fs::metadata(v.share_path("music").join("album (2)/a.jpg")).unwrap().modified().unwrap();
    assert_eq!(modified, old, "modification times are kept");
    let (file, _) =
        copy_through(&photos, &path("album/a.jpg"), &music, &path("a.jpg"), Taken::Beside, false).unwrap();
    assert_eq!((file, v.read("music", "a.jpg")), (path("a.jpg"), "a".to_owned()));

    // A move: refused where the name is taken, then the source goes.
    let refused = copy_through(&photos, &path("album"), &music, &path("album"), Taken::Refuse, true);
    assert!(matches!(refused, Err(FileError::Exists)), "{refused:?}");
    let (moved, copied) =
        copy_through(&photos, &path("album"), &music, &path("moved"), Taken::Refuse, true).unwrap();
    assert_eq!((moved, copied.count()), (path("moved"), 5), "album, a.jpg, deep, b.jpg and the link");
    photos.remove_copied(&path("album"), &copied).unwrap();
    assert!(!v.exists("photos", "album"));
    assert_eq!(v.read("music", "moved/deep/b.jpg"), "b");
    assert!(fs::symlink_metadata(v.share_path("music").join("moved/link")).unwrap().is_symlink());
}

#[test]
fn a_move_across_removes_only_what_was_sent() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (_, copied) =
        copy_through(&photos, &path("album"), &music, &path("album"), Taken::Refuse, true).unwrap();
    // Swapped for another folder meanwhile: that one stays.
    fs::rename(v.share_path("photos").join("album"), v.share_path("photos").join("old")).unwrap();
    v.write("photos", "album/other.jpg", "other");
    let removed = photos.remove_copied(&path("album"), &copied);
    let kept = Kept { count: 1, example: Some(path("album")), too_many: false };
    assert!(matches!(&removed, Err(FileError::PartlyMoved(left)) if *left == kept), "{removed:?}");
    assert_eq!(v.read("photos", "album/other.jpg"), "other");
    assert_eq!(v.read("photos", "old/a.jpg"), "a", "nothing is removed under another name");
    // A plain copy records nothing, and nothing is removed after it.
    let (_, plain) =
        copy_through(&photos, &path("album"), &music, &path("plain"), Taken::Beside, false).unwrap();
    assert!(matches!(photos.remove_copied(&path("album"), &plain), Err(FileError::Invalid(_))));
    assert!(v.exists("photos", "album/other.jpg"));
}

#[test]
fn a_copy_that_stops_halfway_leaves_nothing_behind() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let music = v.share("music");
    // The source's side sends the folder and a file, then gives up.
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    let a = v.share_path("photos").join("album/a.jpg");
    let file = rustix::fs::open(a, OFlags::RDONLY, Mode::empty()).unwrap();
    let times = timestamps(&rustix::fs::fstat(&file).unwrap());
    out.send(Piece::Folder(c"album".to_owned())).unwrap();
    out.send(Piece::File { name: c"a.jpg".to_owned(), file, times }).unwrap();
    drop(out);
    let written = music.receive_entry(&path("album"), &pieces, Taken::Beside);
    assert!(matches!(written, Err(FileError::Invalid(why)) if why.contains("source stopped")), "{written:?}");
    assert!(!v.exists("music", "album"), "the half copy stayed");

    // Out of order: a folder's end before anything began.
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    out.send(Piece::Leave).unwrap();
    let written = music.receive_entry(&path("x"), &pieces, Taken::Beside);
    assert!(matches!(written, Err(FileError::Invalid(why)) if why.contains("out of order")), "{written:?}");
    // Nothing that is copied (a pipe, say): nothing is made.
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    out.send(Piece::Done).unwrap();
    assert_eq!(music.receive_entry(&path("y"), &pieces, Taken::Beside).unwrap(), path("y"));
    assert!(!v.exists("music", "y"));
}

#[test]
fn the_side_that_failed_first_says_why() {
    let stopped = || FileError::Invalid("the copy's target stopped");
    assert!(matches!(settle(Err(stopped()), Err(FileError::Denied)), Err(FileError::Denied)));
    let both = settle(Err(FileError::NotFound), Err(FileError::Invalid("the copy's source stopped")));
    assert!(matches!(both, Err(FileError::NotFound)));
    assert!(matches!(settle(Ok(Copied::off()), Err(FileError::Exists)), Err(FileError::Exists)));
    assert!(matches!(settle(Err(FileError::Denied), Ok(path("a"))), Err(FileError::Denied)));
    assert_eq!(settle(Ok(Copied::off()), Ok(path("a"))).unwrap().0, path("a"));
}

#[test]
fn a_target_that_gives_up_stops_the_source() {
    let v = Volumes::new();
    for n in 0..(PIECES_IN_FLIGHT * 2) {
        v.write("photos", &format!("many/{n}.txt"), "x");
    }
    let photos = v.share("photos");
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    drop(pieces);
    let read = photos.send_entry(&path("many"), &out, true);
    assert!(matches!(read, Err(FileError::Invalid(why)) if why.contains("target stopped")), "{read:?}");
}

/// A share `name` in a fresh volumes folder under `base`, and its path.
fn share_in(base: &Path, name: &str) -> (tempfile::TempDir, ShareRoot, PathBuf) {
    let volumes = tempfile::tempdir_in(base).unwrap();
    let directory = volumes.path().join("volume1").join(name);
    fs::create_dir_all(&directory).unwrap();
    let share = ShareName::parse(name).unwrap();
    let root = ShareRoot::open(volumes.path(), VolumeNumber::new(1).unwrap(), &share, false).unwrap();
    (volumes, root, directory)
}

/// Two shares on two file systems, as two datasets or a drive and a
/// volume are: `/tmp` and `/dev/shm`. None where there are not two.
fn on_two_file_systems() -> Option<[(tempfile::TempDir, ShareRoot, PathBuf); 2]> {
    let shm = Path::new("/dev/shm");
    tempfile::tempdir_in(shm).ok()?;
    let photos = share_in(&std::env::temp_dir(), "photos");
    let music = share_in(shm, "music");
    (photos.1.file_system != music.1.file_system).then_some([photos, music])
}

/// What `copy_new` copies of `from` in `source` to `to` in `target`,
/// recorded as for a move.
fn copied_for_move(source: &ShareRoot, from: &str, target: &ShareRoot, to: &str) -> Copied {
    let (from, to) = (path(from), path(to));
    let (from_parent, from_name) = source.parent(&from).unwrap();
    let (to_parent, to_name) = target.parent(&to).unwrap();
    let (from_name, to_name) = (c_name(from_name).unwrap(), c_name(to_name).unwrap());
    let mut copied = Copied::recording();
    copy_new(
        from_parent.as_fd(),
        &from_name,
        to_parent.as_fd(),
        &to_name,
        source.file_system,
        0,
        &mut copied,
        NewEntries::Maker,
    )
    .unwrap();
    copied
}

/// Waits until `path` is there, as a test watches another thread.
fn appears(path: &Path) {
    let start = Instant::now();
    while path.symlink_metadata().is_err() {
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "{} never came", path.display());
        std::thread::yield_now();
    }
}

#[test]
fn a_move_across_keeps_what_appeared_after_it_was_sent() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (moved, copied) =
        copy_through(&photos, &path("album"), &music, &path("moved"), Taken::Refuse, true).unwrap();
    assert_eq!(moved, path("moved"));
    // An SMB client (or an app) saves a file there before the source goes.
    v.write("photos", "album/new.txt", "never copied");

    let removed = photos.remove_copied(&path("album"), &copied);

    assert_eq!(v.read("photos", "album/new.txt"), "never copied");
    assert!(!v.exists("photos", "album/a.jpg"), "what was copied went");
    let kept = Kept { count: 1, example: Some(path("album")), too_many: false };
    assert!(matches!(&removed, Err(FileError::PartlyMoved(left)) if *left == kept), "{removed:?}");
}

#[test]
fn a_move_across_keeps_a_file_changed_after_it_was_sent() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "first version");
    v.write("photos", "album/b.jpg", "b");
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (_, copied) =
        copy_through(&photos, &path("album"), &music, &path("moved"), Taken::Refuse, true).unwrap();
    v.write("photos", "album/a.jpg", "second version, saved during the move");

    let removed = photos.remove_copied(&path("album"), &copied);

    assert_eq!(v.read("photos", "album/a.jpg"), "second version, saved during the move");
    assert_eq!(v.read("music", "moved/a.jpg"), "first version");
    assert!(!v.exists("photos", "album/b.jpg"));
    // The file, and the folder that still holds it.
    let kept = Kept { count: 2, example: Some(path("album/a.jpg")), too_many: false };
    assert!(matches!(&removed, Err(FileError::PartlyMoved(left)) if *left == kept), "{removed:?}");
    // The same size and time, written again in place: its change time tells.
    v.write("photos", "solo.txt", "1234");
    let (_, copied) =
        copy_through(&photos, &path("solo.txt"), &music, &path("solo.txt"), Taken::Refuse, true).unwrap();
    let solo = v.share_path("photos").join("solo.txt");
    let modified = fs::metadata(&solo).unwrap().modified().unwrap();
    fs::write(&solo, "abcd").unwrap();
    File::options().write(true).open(&solo).unwrap().set_modified(modified).unwrap();
    assert!(matches!(photos.remove_copied(&path("solo.txt"), &copied), Err(FileError::PartlyMoved(_))));
    assert_eq!(v.read("photos", "solo.txt"), "abcd");
}

#[test]
fn a_move_across_keeps_what_it_does_not_copy() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let album = rustix::fs::open(v.share_path("photos").join("album"), OFlags::PATH, Mode::empty()).unwrap();
    rustix::fs::mknodat(&album, "pipe", FileType::Fifo, Mode::from_raw_mode(0o600), 0).unwrap();
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (_, copied) =
        copy_through(&photos, &path("album"), &music, &path("moved"), Taken::Refuse, true).unwrap();
    assert!(!v.exists("music", "moved/pipe"), "pipes are not copied");

    let removed = photos.remove_copied(&path("album"), &copied);

    assert!(v.exists("photos", "album/pipe"), "the pipe went, never copied; the move said {removed:?}");
    assert!(matches!(removed, Err(FileError::PartlyMoved(_))), "{removed:?}");
    // Nothing of it copied at all: it stays, and says so.
    let (_, nothing) =
        copy_through(&photos, &path("album/pipe"), &music, &path("pipe"), Taken::Refuse, true).unwrap();
    let removed = photos.remove_copied(&path("album/pipe"), &nothing);
    assert!(matches!(removed, Err(FileError::PartlyMoved(Kept { count: 1, .. }))), "{removed:?}");
    assert!(v.exists("photos", "album/pipe"));
}

#[test]
fn a_move_between_file_systems_removes_only_what_it_copied() {
    let Some([(_source, photos, photos_dir), (_target, music, music_dir)]) = on_two_file_systems() else {
        return;
    };
    let album = photos_dir.join("album");
    fs::create_dir_all(album.join("deep")).unwrap();
    for name in ["a.jpg", "gone.jpg", "deep/b.jpg"] {
        fs::write(album.join(name), name).unwrap();
    }
    symlink("a.jpg", album.join("link")).unwrap();
    let copied = copied_for_move(&photos, "album", &music, "album");
    // Meanwhile: a file new, one deleted, another link in one's place.
    fs::write(album.join("new.txt"), "never copied").unwrap();
    fs::remove_file(album.join("gone.jpg")).unwrap();
    fs::remove_file(album.join("link")).unwrap();
    symlink("elsewhere", album.join("link")).unwrap();

    let removed = photos.remove_copied(&path("album"), &copied);

    assert_eq!(fs::read_to_string(album.join("new.txt")).unwrap(), "never copied");
    assert_eq!(fs::read_link(album.join("link")).unwrap(), Path::new("elsewhere"));
    assert!(!album.join("a.jpg").exists() && !album.join("deep").exists(), "what was copied went");
    assert_eq!(fs::read_to_string(music_dir.join("album/deep/b.jpg")).unwrap(), "deep/b.jpg");
    // The other link, and the folder that holds it and new.txt; what was
    // deleted meanwhile is passed by.
    let kept = Kept { count: 2, example: Some(path("album/link")), too_many: false };
    assert!(matches!(&removed, Err(FileError::PartlyMoved(left)) if *left == kept), "{removed:?}");
}

/// End to end through `move_entry`, as a job runs it: the source on one file
/// system, the target on another, and a file saved into a folder of the
/// source that the copy has already gone through, while it copies the rest.
#[test]
fn a_file_saved_during_a_move_between_shares_stays() {
    let Some([(_source, photos, photos_dir), (_target, music, music_dir)]) = on_two_file_systems() else {
        return;
    };
    let album = photos_dir.join("album");
    fs::create_dir_all(album.join("one")).unwrap();
    fs::create_dir_all(album.join("two")).unwrap();
    // The copy reads `album` in directory order: the first folder is read to
    // its end before the second one is begun.
    let order: Vec<String> =
        fs::read_dir(&album).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    let (done, busy) = (order[0].clone(), order[1].clone());
    fs::write(album.join(&done).join("old.txt"), "copied").unwrap();
    for n in 0..20_000 {
        fs::write(album.join(&busy).join(format!("{n:05}")), "").unwrap();
    }
    let watched = music_dir.join("album").join(&busy);
    let late = album.join(&done).join("new.txt");
    let writer = std::thread::spawn(move || {
        appears(&watched);
        fs::write(&late, "saved during the move").is_ok()
    });

    let moved = photos.move_entry(&path("album"), &music, &path("album"), false);

    assert!(writer.join().unwrap(), "the file could not be saved while the copy ran");
    assert_eq!(fs::read_to_string(album.join(&done).join("new.txt")).unwrap(), "saved during the move");
    assert!(!album.join(&done).join("old.txt").exists() && !album.join(&busy).exists());
    assert_eq!(fs::read_dir(music_dir.join("album").join(&busy)).unwrap().count(), 20_000);
    assert!(matches!(&moved, Err(FileError::PartlyMoved(Kept { count: 2, .. }))), "{moved:?}");
}

/// Ext4 and ZFS give a new entry the number of one just removed. What took
/// a copied entry's name and number meanwhile stays all the same, whichever
/// way it was copied: the record knows more of an entry than its number.
/// The number is given by hand here (`renumber`), so the test does not
/// depend on the file system it runs on.
#[test]
fn what_took_the_name_and_number_of_a_copied_entry_stays() {
    for across in [false, true] {
        let v = Volumes::new();
        let album = v.share_path("photos").join("album");
        v.write("photos", "album/a.jpg", "a");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        File::options().write(true).open(album.join("a.jpg")).unwrap().set_modified(old).unwrap();
        symlink("a.jpg", album.join("link")).unwrap();
        fs::create_dir(album.join("empty")).unwrap();
        let (photos, music) = (v.share("photos"), v.share("music"));
        let mut copied = if across {
            copy_through(&photos, &path("album"), &music, &path("album"), Taken::Refuse, true).unwrap().1
        } else {
            copied_for_move(&photos, "album", &music, "album")
        };
        // Another link, to another target; another file of the same size,
        // written now.
        fs::remove_file(album.join("link")).unwrap();
        symlink("elsewhere", album.join("link")).unwrap();
        fs::remove_file(album.join("a.jpg")).unwrap();
        fs::write(album.join("a.jpg"), "b").unwrap();
        // Another empty folder, made a clock tick later at least: only its
        // change time tells it apart.
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::remove_dir(album.join("empty")).unwrap();
        fs::create_dir(album.join("empty")).unwrap();
        let folder = rustix::fs::open(&album, OFlags::PATH | OFlags::DIRECTORY, Mode::empty()).unwrap();
        for name in [c"link", c"a.jpg", c"empty"] {
            copied.renumber(name, &statat(&folder, name, AtFlags::SYMLINK_NOFOLLOW).unwrap());
        }

        let removed = photos.remove_copied(&path("album"), &copied);

        assert_eq!(fs::read_link(album.join("link")).unwrap(), Path::new("elsewhere"), "across: {across}");
        assert_eq!(v.read("photos", "album/a.jpg"), "b", "across: {across}");
        assert!(album.join("empty").is_dir(), "across: {across}");
        // The three, and the folder that holds them.
        let left = matches!(&removed, Err(FileError::PartlyMoved(Kept { count: 4, too_many: false, .. })));
        assert!(left, "across: {across}, {removed:?}");
        assert_eq!(fs::read_link(v.share_path("music").join("album/link")).unwrap(), Path::new("a.jpg"));
    }
}

#[test]
fn a_move_whose_copy_cannot_be_written_out_keeps_its_source() {
    if rustix::process::geteuid().is_root() {
        return; // root opens the folder anyway
    }
    let Some([(_source, photos, photos_dir), (_target, music, music_dir)]) = on_two_file_systems() else {
        return;
    };
    fs::write(photos_dir.join("a.jpg"), "a").unwrap();
    music.sync().unwrap();
    // The target takes the copy, but cannot be opened to be written out.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&music_dir, fs::Permissions::from_mode(0o300)).unwrap();

    let moved = photos.move_entry(&path("a.jpg"), &music, &path("a.jpg"), false);

    fs::set_permissions(&music_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(moved, Err(FileError::Denied)), "{moved:?}");
    assert_eq!(fs::read_to_string(photos_dir.join("a.jpg")).unwrap(), "a", "the source stayed");
    assert_eq!(fs::read_to_string(music_dir.join("a.jpg")).unwrap(), "a");
    let error = FileError::NotDurable(std::io::Error::from_raw_os_error(5));
    assert!(error.to_string().contains("nothing was removed"), "{error}");
}

#[test]
fn a_move_of_more_than_it_records_keeps_its_source_whole() {
    let v = Volumes::new();
    for n in 0..5 {
        v.write("photos", &format!("album/{n}.jpg"), "x");
    }
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (from_parent, _) = photos.parent(&path("album")).unwrap();
    let (to_parent, _) = music.parent(&path("album")).unwrap();
    let copy = |name: &CStr, copied: &mut Copied| {
        let owners = NewEntries::Maker;
        copy_new(
            from_parent.as_fd(),
            c"album",
            to_parent.as_fd(),
            name,
            photos.file_system,
            0,
            copied,
            owners,
        )
        .unwrap();
    };
    let mut copied = Copied::within(3, 1024);
    copy(c"album", &mut copied);

    let removed = photos.remove_copied(&path("album"), &copied);

    let kept = Kept { count: 6, example: Some(path("album")), too_many: true };
    assert!(matches!(&removed, Err(FileError::PartlyMoved(left)) if *left == kept), "{removed:?}");
    assert_eq!(
        fs::read_dir(v.share_path("photos").join("album")).unwrap().count(),
        5,
        "the source stayed whole"
    );
    assert_eq!(fs::read_dir(v.share_path("music").join("album")).unwrap().count(), 5, "the copy is complete");
    // Names count against it too.
    let mut names = Copied::within(100, 8);
    copy(c"names", &mut names);
    let removed = photos.remove_copied(&path("album"), &names);
    assert!(matches!(removed, Err(FileError::PartlyMoved(Kept { too_many: true, .. }))), "{removed:?}");
    assert!(removed.unwrap_err().to_string().contains("too many"));
}

#[test]
fn a_move_removes_every_name_of_a_file_with_several() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let album = v.share_path("photos").join("album");
    fs::hard_link(album.join("a.jpg"), album.join("b.jpg")).unwrap();
    let (photos, music) = (v.share("photos"), v.share("music"));
    let copied = copied_for_move(&photos, "album", &music, "album");

    photos.remove_copied(&path("album"), &copied).unwrap();

    assert!(!v.exists("photos", "album"));
    assert_eq!((v.read("music", "album/a.jpg"), v.read("music", "album/b.jpg")), ("a".into(), "a".into()));
}

#[test]
fn a_copy_leaves_uploads_under_way_behind() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "a");
    let (photos, music) = (v.share("photos"), v.share("music"));
    let (file, name) = photos.create_upload(&path("album")).unwrap();
    drop(file);

    let copied = photos.copy_entry(&path("album"), &music, &path("album")).unwrap();
    let (across, _) =
        copy_through(&photos, &path("album"), &music, &path("across"), Taken::Beside, false).unwrap();

    for copy in [copied, across] {
        assert!(v.exists("music", &format!("{}/a.jpg", copy.as_str())));
        assert!(
            !v.exists("music", &format!("{}/{name}", copy.as_str())),
            "the half-written {name} came along"
        );
    }
    // A move leaves it where it began, and its folder with it.
    let (_, moved) =
        copy_through(&photos, &path("album"), &music, &path("moved"), Taken::Refuse, true).unwrap();
    let removed = photos.remove_copied(&path("album"), &moved);
    assert!(matches!(removed, Err(FileError::PartlyMoved(Kept { count: 1, .. }))), "{removed:?}");
    assert!(v.exists("photos", &format!("album/{name}")) && !v.exists("photos", "album/a.jpg"));
    // Asked for by its name, it is refused.
    let upload = path(&format!("album/{name}"));
    assert!(matches!(photos.copy_entry(&upload, &music, &path("x")), Err(FileError::Invalid(_))));
    let across = copy_through(&photos, &upload, &music, &path("x"), Taken::Beside, false);
    assert!(matches!(across, Err(FileError::Invalid(_))), "{across:?}");
}

#[test]
fn a_name_that_differs_only_in_case_is_named_when_it_clashes() {
    let v = Volumes::new();
    v.write("music", "album/photo.jpg", "x");
    let folder = rustix::fs::open(
        v.share_path("music").join("album"),
        OFlags::RDONLY | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .unwrap();
    // On FAT, exFAT and NTFS `Photo.jpg` is taken by `photo.jpg`.
    let clashed = clash(folder.as_fd(), c"Photo.jpg");
    assert!(
        matches!(&clashed, FileError::CaseClash(a, b) if a == "photo.jpg" && b == "Photo.jpg"),
        "{clashed:?}"
    );
    assert!(clashed.to_string().contains("“photo.jpg” and “Photo.jpg” differ only in case"), "{clashed}");
    // The very name, or none alike: somebody else made it meanwhile.
    for name in [c"photo.jpg", c"other.jpg"] {
        let taken = clash(folder.as_fd(), name);
        assert!(taken.to_string().contains("taken meanwhile"), "{taken}");
    }
    // Twice the same name in a copy: the folder is looked through, and the
    // copy goes again.
    let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
    v.write("photos", "a.jpg", "a");
    for piece in [c"album", c"a.jpg", c"a.jpg"] {
        if piece == c"album" {
            out.send(Piece::Folder(piece.to_owned())).unwrap();
            continue;
        }
        let file =
            rustix::fs::open(v.share_path("photos").join("a.jpg"), OFlags::RDONLY, Mode::empty()).unwrap();
        let times = timestamps(&fstat(&file).unwrap());
        out.send(Piece::File { name: piece.to_owned(), file, times }).unwrap();
    }
    let written = v.share("music").receive_entry(&path("twice"), &pieces, Taken::Refuse);
    assert!(
        matches!(&written, Err(FileError::Io(why)) if why.to_string().contains("taken meanwhile")),
        "{written:?}"
    );
    assert!(!v.exists("music", "twice"));
}

#[test]
fn a_copy_across_that_fails_removes_only_what_it_made() {
    let v = Volumes::new();
    v.write("music", "Private/diary.txt", "mine");
    let music = &v.share("music");
    let at = |name: &str| v.share_path("music").join(name);

    // Its folder made, then swapped for the user's own before the copy fails.
    // Each piece is handed over as it is taken: once the folder inside is,
    // the copy's folder was made and opened, and only then is it swapped
    // (swapped between the two, the copy would refuse the user's folder as
    // taken instead).
    let (out, pieces) = std::sync::mpsc::sync_channel(0);
    let written = std::thread::scope(|scope| {
        let receiving = scope.spawn(move || music.receive_entry(&path("album"), &pieces, Taken::Refuse));
        out.send(Piece::Folder(c"album".to_owned())).unwrap();
        out.send(Piece::Folder(c"inner".to_owned())).unwrap();
        fs::rename(at("album"), at("made")).unwrap();
        fs::rename(at("Private"), at("album")).unwrap();
        drop(out);
        receiving.join().unwrap()
    });
    assert!(matches!(written, Err(FileError::Invalid(why)) if why.contains("source stopped")), "{written:?}");
    assert_eq!(v.read("music", "album/diary.txt"), "mine", "the folder swapped in stayed");

    // A link made, then a folder in its place: only a link is removed.
    for swapped in [false, true] {
        let (out, pieces) = std::sync::mpsc::sync_channel(PIECES_IN_FLIGHT);
        out.send(Piece::Link { name: c"link".to_owned(), target: c"a.jpg".to_owned() }).unwrap();
        let written = std::thread::scope(|scope| {
            let receiving = scope.spawn(move || music.receive_entry(&path("link"), &pieces, Taken::Refuse));
            appears(&at("link"));
            if swapped {
                fs::remove_file(at("link")).unwrap();
                v.write("music", "link/kept.txt", "kept");
            }
            out.send(Piece::Leave).unwrap();
            receiving.join().unwrap()
        });
        assert!(
            matches!(written, Err(FileError::Invalid(why)) if why.contains("out of order")),
            "{written:?}"
        );
        assert_eq!(v.exists("music", "link"), swapped);
    }
    assert_eq!(v.read("music", "link/kept.txt"), "kept");
}

#[test]
fn a_share_is_written_out() {
    let v = Volumes::new();
    v.share("photos").sync().unwrap();
}

/// The test's own user and group, as a share's root would be given what is
/// made in it: the owner of the folder, and a group.
fn inheriting(root: ShareRoot) -> (ShareRoot, u32, u32) {
    let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
    (root.with_new_entries(NewEntries::Inherit { group: gid }), uid, gid)
}

fn owners_of(path: &Path) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).unwrap();
    (meta.uid(), meta.gid())
}

#[test]
fn what_is_made_in_a_share_takes_the_folders_owner_and_the_group() {
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "x");
    symlink("a.jpg", v.share_path("photos").join("album/link")).unwrap();
    let (photos, uid, gid) = inheriting(v.share("photos"));
    let (music, _, _) = inheriting(v.share("music"));

    photos.make_directory(&path("new")).unwrap();
    let (_, upload) = photos.create_upload(&path("new")).unwrap();
    photos.copy_entry(&path("album"), &photos, &path("copy")).unwrap();
    photos.copy_entry(&path("album"), &music, &path("album")).unwrap();
    photos.remove(&path("copy/a.jpg"), Some("anna")).unwrap();
    photos.restore(&path(".recycle/anna/copy/a.jpg"), "anna").unwrap();

    // As root, each would be the folder's owner's with the group; the
    // test's own user can give it only its own, which it is checked to do.
    for (share, made) in [
        ("photos", "new"),
        ("photos", &format!("new/{upload}")),
        ("photos", "copy"),
        ("photos", "copy/link"),
        ("photos", "copy/a.jpg"),
        ("music", "album/a.jpg"),
        ("photos", ".recycle/anna"),
    ] {
        assert_eq!(owners_of(&v.share_path(share).join(made)), (uid, gid), "{share}/{made}");
    }
}

#[test]
fn a_shares_recycle_bin_is_everyones_who_may_use_it() {
    use std::os::unix::fs::PermissionsExt;
    let v = Volumes::new();
    v.write("photos", "album/a.jpg", "x");
    // Someone else's bin, made before by another account.
    fs::create_dir_all(v.share_path("photos").join(".recycle/anna")).unwrap();
    fs::set_permissions(v.share_path("photos").join(".recycle/anna"), fs::Permissions::from_mode(0o770))
        .unwrap();
    let (photos, _, _) = inheriting(v.share("photos"));

    photos.remove(&path("album/a.jpg"), Some("anna")).unwrap();

    let bin = v.share_path("photos").join(".recycle/anna/album");
    assert_eq!(fs::metadata(&bin).unwrap().permissions().mode() & 0o070, 0o070, "the group's too");
    assert_eq!(fs::read_to_string(bin.join("a.jpg")).unwrap(), "x");
    // In a personal folder a bin that is not the user's alone refuses.
    let home = v.share("photos");
    v.write("photos", "b.txt", "y");
    assert!(matches!(home.remove(&path("b.txt"), Some("anna")), Err(FileError::Invalid(_))));
}
