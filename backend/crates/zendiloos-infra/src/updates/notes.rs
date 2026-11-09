//! What changed in the version of zendiloOS that runs: its package brings
//! it along (`/usr/share/zendiloos/changes.md`, written by
//! `scripts/release-notes.sh`), so the NAS knows it without asking GitHub.

use std::io::{self, Read};
use std::path::Path;

/// More is no longer release notes.
const MAX_NOTES: u64 = 64 * 1024;

/// The notes in `path`, in Markdown. None when there are none: no file (a
/// build by hand), an empty one, or one that cannot be read, which the log
/// says.
pub fn installed_notes(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    match std::fs::File::open(path).and_then(|file| file.take(MAX_NOTES).read_to_end(&mut bytes)) {
        Ok(_) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot read what changed in this version");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_notes_the_package_brought() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.md");
        std::fs::write(&path, "\n- A file explorer\n- Two-factor sign-in\n\n").unwrap();
        assert_eq!(installed_notes(&path).as_deref(), Some("- A file explorer\n- Two-factor sign-in"));
    }

    #[test]
    fn none_without_a_file_an_empty_one_or_one_that_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(installed_notes(&dir.path().join("changes.md")), None);
        std::fs::write(dir.path().join("empty.md"), " \n\n").unwrap();
        assert_eq!(installed_notes(&dir.path().join("empty.md")), None);
        assert_eq!(installed_notes(dir.path()), None, "a folder is no file");
    }

    #[test]
    fn takes_no_more_than_notes_need() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.md");
        std::fs::write(&path, "- x\n".repeat(100_000)).unwrap();
        let notes = installed_notes(&path).unwrap();
        assert_eq!(notes.len() as u64, MAX_NOTES - 1, "the cut ends on a newline, which is trimmed");
    }
}
