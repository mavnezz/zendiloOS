use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use zendiloos_identity::application::ports::SetupCodes;
use zendiloos_identity::domain::SetupCode;
use zendiloos_kernel::PortError;

use super::tokens::random_bytes;
use crate::blocking;

/// Keeps the setup code in a file readable only by the service account, so
/// a local administrator can look it up (`/var/lib/zendiloos/setup-code`).
/// The setup is open while a marker file exists, which the ISO's installer
/// leaves (`/var/lib/zendiloos/setup-open`); closing removes it.
#[derive(Debug, Clone)]
pub struct FileSetupCodes {
    path: PathBuf,
    open_marker: Option<PathBuf>,
}

impl FileSetupCodes {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), open_marker: None }
    }

    /// The setup is open while `marker` exists.
    #[must_use]
    pub fn with_open_marker(mut self, marker: impl Into<PathBuf>) -> Self {
        self.open_marker = Some(marker.into());
        self
    }
}

/// Removes `path`; a no-op if it is gone already.
fn remove(path: &Path) -> Result<(), PortError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(PortError::new(WHAT, error)),
        _ => Ok(()),
    }
}

const WHAT: &str = "setup code";

fn read(path: &Path) -> Result<Option<SetupCode>, PortError> {
    match fs::read_to_string(path) {
        // An unreadable or damaged code is treated as none; issuing replaces it.
        Ok(content) => Ok(SetupCode::parse(content.trim())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(PortError::new(WHAT, error)),
    }
}

/// Writes `content` to a private temporary file and renames it over `path`,
/// so readers never see a partial code.
fn write_atomically(path: &Path, content: &str) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[async_trait]
impl SetupCodes for FileSetupCodes {
    async fn current(&self) -> Result<Option<SetupCode>, PortError> {
        let path = self.path.clone();
        blocking::run(WHAT, move || read(&path)).await
    }

    async fn issue(&self) -> Result<SetupCode, PortError> {
        let path = self.path.clone();
        blocking::run(WHAT, move || {
            if let Some(code) = read(&path)? {
                return Ok(code);
            }
            let code = SetupCode::from_random_bytes(random_bytes());
            write_atomically(&path, &format!("{}\n", code.reveal())).map_err(|e| PortError::new(WHAT, e))?;
            Ok(code)
        })
        .await
    }

    async fn consume(&self) -> Result<(), PortError> {
        let path = self.path.clone();
        blocking::run(WHAT, move || remove(&path)).await
    }

    async fn is_open(&self) -> Result<bool, PortError> {
        let Some(marker) = self.open_marker.clone() else { return Ok(false) };
        // Whatever stands there counts, a link too: only root could have put
        // it in the service's private state directory.
        blocking::run(WHAT, move || match fs::symlink_metadata(&marker) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(PortError::new(WHAT, error)),
        })
        .await
    }

    async fn close(&self) -> Result<(), PortError> {
        let Some(marker) = self.open_marker.clone() else { return Ok(()) };
        blocking::run(WHAT, move || remove(&marker)).await
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn issues_a_private_code_once_and_consumes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup-code");
        let codes = FileSetupCodes::new(&path);
        assert!(codes.current().await.unwrap().is_none());

        let code = codes.issue().await.unwrap();
        let again = codes.issue().await.unwrap();

        assert_eq!(code.reveal(), again.reveal(), "an existing code is kept");
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{}\n", code.reveal()));
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(codes.current().await.unwrap().unwrap().matches(&code.reveal()));

        codes.consume().await.unwrap();
        assert!(!path.exists());
        codes.consume().await.unwrap();
    }

    #[tokio::test]
    async fn the_setup_is_open_while_its_marker_exists() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("setup-open");
        let codes = FileSetupCodes::new(dir.path().join("setup-code")).with_open_marker(&marker);
        assert!(!codes.is_open().await.unwrap());
        codes.close().await.unwrap();

        fs::write(&marker, "").unwrap();
        assert!(codes.is_open().await.unwrap());
        codes.close().await.unwrap();
        assert!(!marker.exists());
        assert!(!codes.is_open().await.unwrap());

        let without = FileSetupCodes::new(dir.path().join("setup-code"));
        fs::write(&marker, "").unwrap();
        assert!(!without.is_open().await.unwrap(), "no marker configured, never open");
        without.close().await.unwrap();
        assert!(marker.exists());
    }

    #[tokio::test]
    async fn replaces_a_damaged_code() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup-code");
        fs::write(&path, "garbage").unwrap();
        let codes = FileSetupCodes::new(&path);

        assert!(codes.current().await.unwrap().is_none());
        let code = codes.issue().await.unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().trim(), code.reveal());
    }
}
