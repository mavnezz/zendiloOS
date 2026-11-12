//! Files replaced whole, and still there after a power cut.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::operations::OperationError;

/// Replaces `path` with `content`: written beside it (`<name>.zendiloos-new`,
/// which matches no pattern of the programs that read such folders), on
/// disk, then renamed into place, and the rename on disk too. After a power
/// cut the file is the old one or the new one, never empty or half written.
pub(crate) fn replace(path: &Path, content: &[u8]) -> Result<(), OperationError> {
    write(path, content, None)
}

/// As [`replace`], but only when `path` holds something else: whether it
/// was written. A unit file written again unchanged would only make systemd
/// reload for nothing.
pub(crate) fn replace_if_changed(path: &Path, content: &[u8]) -> Result<bool, OperationError> {
    if fs::read(path).is_ok_and(|before| before == content) {
        return Ok(false);
    }
    replace(path, content)?;
    Ok(true)
}

/// As [`replace`], the file with the permission bits `mode`, whatever the
/// umask says.
pub(crate) fn replace_mode(path: &Path, content: &[u8], mode: u32) -> Result<(), OperationError> {
    write(path, content, Some(mode))
}

fn write(path: &Path, content: &[u8], mode: Option<u32>) -> Result<(), OperationError> {
    let temporary = beside(path);
    let written = (|| {
        let mut file = match mode {
            None => File::create(&temporary)?,
            Some(mode) => {
                let _ = fs::remove_file(&temporary);
                let file = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&temporary)?;
                file.set_permissions(fs::Permissions::from_mode(mode))?;
                file
            }
        };
        file.write_all(content)?;
        file.sync_all()
    })();
    written.map_err(|e| OperationError::Write(temporary.clone(), e))?;
    renamed(&temporary, path)
}

/// Replaces `path` with a symbolic link to `target`, made beside it and
/// renamed into place: there is always a file or link at `path`.
pub(crate) fn link(path: &Path, target: &str) -> Result<(), OperationError> {
    let temporary = beside(path);
    let _ = fs::remove_file(&temporary);
    std::os::unix::fs::symlink(target, &temporary)
        .map_err(|e| OperationError::Write(temporary.clone(), e))?;
    renamed(&temporary, path)
}

fn renamed(temporary: &Path, path: &Path) -> Result<(), OperationError> {
    fs::rename(temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))?;
    if let Some(folder) = path.parent()
        && let Ok(folder) = File::open(folder)
    {
        let _ = folder.sync_all();
    }
    Ok(())
}

fn beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".zendiloos-new");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_the_same_content_is_left_alone() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("volume1.mount");
        assert!(replace_if_changed(&path, b"[Mount]\n").unwrap(), "a new file");
        let written = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!replace_if_changed(&path, b"[Mount]\n").unwrap());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written, "not written again");
        assert!(replace_if_changed(&path, b"[Mount]\nWhat=tank\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[Mount]\nWhat=tank\n");
    }

    #[test]
    fn a_file_is_replaced_whole_and_nothing_stays_beside_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("smb.conf");
        fs::write(&path, "old").unwrap();
        replace(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let names: Vec<_> = fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["smb.conf"]);
        assert_eq!(beside(&path), folder.path().join("smb.conf.zendiloos-new"));

        replace_mode(&path, b"secret", 0o600).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        link(&path, "../run/elsewhere").unwrap();
        assert_eq!(fs::read_link(&path).unwrap(), Path::new("../run/elsewhere"));
        let names: Vec<_> = fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["smb.conf"]);
    }
}
