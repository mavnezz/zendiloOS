//! Files replaced whole, and still there after a power cut.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::operations::OperationError;

/// Replaces `path` with `content`: written beside it (`<name>.zendiloos-new`,
/// which matches no pattern of the programs that read such folders), on
/// disk, then renamed into place, and the rename on disk too. After a power
/// cut the file is the old one or the new one, never empty or half written.
pub(crate) fn replace(path: &Path, content: &[u8]) -> Result<(), OperationError> {
    let temporary = beside(path);
    let written = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(content)?;
        file.sync_all()
    })();
    written.map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))?;
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
    fn a_file_is_replaced_whole_and_nothing_stays_beside_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("smb.conf");
        fs::write(&path, "old").unwrap();
        replace(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let names: Vec<_> = fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["smb.conf"]);
        assert_eq!(beside(&path), folder.path().join("smb.conf.zendiloos-new"));
    }
}
