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
