//! Files inside shares, as the file manager sees them.

use zendiloos_kernel::Timestamp;

use super::ShareName;

/// The recycle bin at a share's root; `<bin>/<user>` holds a user's files.
pub const RECYCLE_DIRECTORY: &str = ".recycle";

/// Whether `name` can name a file or folder: not empty, `.` or `..`, at
/// most 255 bytes, without `/` and NUL.
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// Whether `name` may be given to a new file or folder: a [file name]
/// (is_file_name) without control characters, without spaces at either end
/// and not ending in a dot. Windows cannot reach names like those, and
/// they are easy to get wrong. Existing files keep their names, whatever
/// wrote them, and stay reachable.
pub fn is_new_file_name(name: &str) -> bool {
    is_file_name(name) && !name.chars().any(char::is_control) && name.trim() == name && !name.ends_with('.')
}

/// A path inside a share, relative to its root: names separated by `/`, at
/// most 4096 bytes. Empty means the root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct FilePath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid path inside a share")]
pub struct InvalidFilePath;

impl FilePath {
    pub const MAX_BYTES: usize = 4096;

    /// Leading and trailing `/` are ignored: `/photos/` is `photos`.
    pub fn parse(value: &str) -> Result<Self, InvalidFilePath> {
        let value = value.trim_matches('/');
        let valid =
            value.is_empty() || (value.len() <= Self::MAX_BYTES && value.split('/').all(is_file_name));
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidFilePath) }
    }

    pub fn root() -> Self {
        Self::default()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last name; none for the root.
    pub fn name(&self) -> Option<&str> {
        self.0.rsplit('/').next().filter(|name| !name.is_empty())
    }

    /// The folder containing this path; none for the root.
    pub fn parent(&self) -> Option<FilePath> {
        (!self.is_root()).then(|| Self(self.0.rsplit_once('/').map_or("", |(parent, _)| parent).to_owned()))
    }

    pub fn join(&self, name: &str) -> Result<FilePath, InvalidFilePath> {
        if !is_file_name(name) {
            return Err(InvalidFilePath);
        }
        Self::parse(&if self.is_root() { name.to_owned() } else { format!("{}/{name}", self.0) })
    }

    /// Whether `self` is `other` or inside it.
    pub fn starts_with(&self, other: &FilePath) -> bool {
        other.is_root()
            || self.0 == other.0
            || self.0.strip_prefix(&other.0).is_some_and(|rest| rest.starts_with('/'))
    }
}

/// Where a file location starts: a share's folder, or the personal folder
/// of the user acting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRoot {
    Share(ShareName),
    Home,
}

impl std::fmt::Display for FileRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileRoot::Share(share) => f.write_str(share.as_str()),
            FileRoot::Home => f.write_str("home"),
        }
    }
}

/// A file or folder: `<root>/<path>` on volume `volume`, or the same in
/// one of a share's snapshots (read only), by its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLocation {
    pub volume: u16,
    pub root: FileRoot,
    pub path: FilePath,
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    /// Devices, sockets, pipes.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Too many entries; the rest is left out.
    pub truncated: bool,
    /// The user may create and delete entries.
    pub writable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_names_are_stricter_than_existing_ones() {
        for good in ["photo.jpg", ".hidden", "Notes: 2024", "Ünïcödé 📷", "a.b.c"] {
            assert!(is_new_file_name(good), "{good:?}");
        }
        for bad in
            ["", ".", "..", "a/b", "line\nbreak", "tab\t", "del\u{7f}", " lead", "trail ", "dot.", "x\0"]
        {
            assert!(!is_new_file_name(bad), "{bad:?}");
        }
        // What SMB or NFS wrote can still be opened.
        assert!(FilePath::parse("odd\tname/trail ").is_ok());
    }

    #[test]
    fn paths_are_relative_and_plain() {
        assert_eq!(FilePath::parse("/photos/2024/").unwrap().as_str(), "photos/2024");
        assert!(FilePath::parse("/").unwrap().is_root());
        for invalid in ["a//b", "../etc", "a/../b", "a/./b", "a\0b"] {
            assert!(FilePath::parse(invalid).is_err(), "{invalid:?}");
        }
        let path = FilePath::parse("photos/2024/beach.jpg").unwrap();
        assert_eq!(path.name(), Some("beach.jpg"));
        assert_eq!(path.parent().unwrap().as_str(), "photos/2024");
        assert_eq!(FilePath::root().name(), None);
        assert_eq!(FilePath::parse("a").unwrap().parent(), Some(FilePath::root()));
        assert!(path.starts_with(&FilePath::parse("photos").unwrap()));
        assert!(!path.starts_with(&FilePath::parse("photo").unwrap()));
        assert!(FilePath::root().join("a/b").is_err());
    }
}

/// The most favourites one account keeps.
pub const MAX_FAVOURITES: usize = 50;

/// A folder someone wants at hand in the file manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favourite {
    pub share: super::ShareId,
    pub path: FilePath,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidFavourites {
    #[error("at most {MAX_FAVOURITES} favourites")]
    TooMany,
    #[error("a folder is a favourite only once")]
    Duplicate,
}

/// An account's favourites, checked, in the order given.
pub fn favourites(list: Vec<Favourite>) -> Result<Vec<Favourite>, InvalidFavourites> {
    if list.len() > MAX_FAVOURITES {
        return Err(InvalidFavourites::TooMany);
    }
    if list.iter().enumerate().any(|(index, favourite)| list[..index].contains(favourite)) {
        return Err(InvalidFavourites::Duplicate);
    }
    Ok(list)
}

#[cfg(test)]
mod favourite_tests {
    use super::*;
    use crate::domain::ShareId;

    fn at(share: i64, path: &str) -> Favourite {
        Favourite { share: ShareId::new(share), path: FilePath::parse(path).unwrap() }
    }

    #[test]
    fn favourites_are_few_and_each_once() {
        let list = vec![at(1, "album"), at(2, "album"), at(1, "")];
        assert_eq!(favourites(list.clone()), Ok(list), "the order stays");
        assert_eq!(favourites(vec![at(1, "a"), at(1, "/a/")]), Err(InvalidFavourites::Duplicate));
        let many = (0..=MAX_FAVOURITES).map(|n| at(1, &format!("f{n}"))).collect();
        assert_eq!(favourites(many), Err(InvalidFavourites::TooMany));
    }
}
