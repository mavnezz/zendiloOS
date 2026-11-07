//! Files inside shares, as the files agent sees them: always a share plus
//! a path relative to its root, never an absolute path.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ShareName, VolumeNumber};

/// Most entries [`crate::Response::Directory`] lists.
pub const MAX_DIRECTORY_ENTRIES: usize = 20_000;

/// A path inside a share, relative to its root: names separated by `/`.
/// No name is empty, `.` or `..`, longer than 255 bytes or contains NUL;
/// the whole path has at most 4096 bytes. Empty means the share's root.
///
/// The agent resolves it beneath the share's root only, so even a path
/// through a symbolic link cannot leave the share.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SharePath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid path inside a share")]
pub struct InvalidSharePath;

/// Whether `name` can be one element of a [`SharePath`].
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name != "." && name != ".." && !name.contains(['/', '\0'])
}

impl SharePath {
    pub const MAX_BYTES: usize = 4096;

    pub fn parse(value: &str) -> Result<Self, InvalidSharePath> {
        let valid =
            value.is_empty() || (value.len() <= Self::MAX_BYTES && value.split('/').all(is_file_name));
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidSharePath) }
    }

    /// The share's root.
    pub fn root() -> Self {
        Self::default()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|name| !name.is_empty())
    }

    /// The last name; none for the root.
    pub fn file_name(&self) -> Option<&str> {
        self.names().last()
    }

    /// The directory containing this path; none for the root.
    pub fn parent(&self) -> Option<SharePath> {
        if self.is_root() {
            return None;
        }
        Some(Self(self.0.rsplit_once('/').map_or("", |(parent, _)| parent).to_owned()))
    }

    pub fn join(&self, name: &str) -> Result<SharePath, InvalidSharePath> {
        if !is_file_name(name) {
            return Err(InvalidSharePath);
        }
        Self::parse(&if self.is_root() { name.to_owned() } else { format!("{}/{name}", self.0) })
    }

    /// Whether `self` is `other` or inside it.
    pub fn starts_with(&self, other: &SharePath) -> bool {
        other.is_root()
            || self.0 == other.0
            || self.0.strip_prefix(&other.0).is_some_and(|rest| rest.starts_with('/'))
    }
}

impl fmt::Display for SharePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.0)
    }
}

impl TryFrom<String> for SharePath {
    type Error = InvalidSharePath;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SharePath> for String {
    fn from(path: SharePath) -> Self {
        path.0
    }
}

/// Where a location's path starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRoot {
    /// The share's directory, `/volume<volume>/<share>`.
    Share(ShareName),
    /// The personal folder of the user the request is for,
    /// `/volume<volume>/@homes/<user>`; never anybody else's.
    Home,
}

/// The directory below a volume where the personal folders are.
pub const HOMES_DIRECTORY: &str = "@homes";

/// A file or directory: `<root>/<path>`, or the same in one of the
/// share's snapshots, read only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileLocation {
    pub volume: VolumeNumber,
    pub root: FileRoot,
    pub path: SharePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<crate::SnapshotLabel>,
}

impl FileLocation {
    /// Whether both lie below the same root, or the same snapshot of it.
    pub fn same_share(&self, other: &FileLocation) -> bool {
        self.volume == other.volume && self.root == other.root && self.snapshot == other.snapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    /// Devices, sockets, pipes: listed, but not opened.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryEntry {
    /// Names that are not UTF-8 arrive with replacement characters and
    /// cannot be addressed: see `unreadable`.
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Last modification, Unix seconds.
    pub modified: i64,
    /// The name is not UTF-8, so `name` is only what it looks like: another
    /// entry may have exactly that name, and a request with it reaches that
    /// one. Shown, never acted on by its name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_paths_stay_relative_and_plain() {
        for valid in ["", "a", "Fotos 2024/Urlaub/IMG_0001.JPG", ".recycle/anna", "ü€/…", "a..b"] {
            assert!(SharePath::parse(valid).is_ok(), "{valid:?}");
        }
        for invalid in ["/etc", "a/", "a//b", ".", "..", "a/../b", "a/./b", "../x", "a\0b", "/"] {
            assert!(SharePath::parse(invalid).is_err(), "{invalid:?}");
        }
        assert!(SharePath::parse(&"a".repeat(256)).is_err(), "names up to 255 bytes");
        let long = vec!["a".repeat(255); 17].join("/");
        assert!(SharePath::parse(&long).is_err(), "paths up to 4096 bytes");
        assert!(serde_json::from_str::<SharePath>("\"../etc\"").is_err(), "checked while reading");
    }

    #[test]
    fn share_paths_have_parents_and_names() {
        let path = SharePath::parse("photos/2024/beach.jpg").unwrap();
        assert_eq!(path.file_name(), Some("beach.jpg"));
        assert_eq!(path.parent().unwrap().as_str(), "photos/2024");
        assert_eq!(SharePath::parse("top").unwrap().parent(), Some(SharePath::root()));
        assert_eq!(SharePath::root().parent(), None);
        assert_eq!(SharePath::root().file_name(), None);
        assert_eq!(SharePath::root().join("a").unwrap().join("b").unwrap().as_str(), "a/b");
        assert!(SharePath::root().join("a/b").is_err());
        assert!(path.starts_with(&SharePath::parse("photos").unwrap()));
        assert!(path.starts_with(&SharePath::root()));
        assert!(!path.starts_with(&SharePath::parse("photo").unwrap()));
        assert_eq!(path.to_string(), "/photos/2024/beach.jpg");
    }

    #[test]
    fn only_an_unreadable_name_says_so() {
        let entry = DirectoryEntry {
            name: "a".into(),
            kind: EntryKind::File,
            size: 1,
            modified: 2,
            unreadable: false,
        };
        let text = serde_json::to_string(&entry).unwrap();
        assert!(!text.contains("unreadable"), "{text}");
        assert_eq!(serde_json::from_str::<DirectoryEntry>(&text).unwrap(), entry);
        let odd = DirectoryEntry { unreadable: true, ..entry };
        let text = serde_json::to_string(&odd).unwrap();
        assert_eq!(serde_json::from_str::<DirectoryEntry>(&text).unwrap(), odd);
    }
}
