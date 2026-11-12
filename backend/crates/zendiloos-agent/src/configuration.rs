//! The configuration backup's files (see
//! [`zendiloos_agent_proto::ConfigurationPath`]): read whole into a backup,
//! and the restorable ones written back from one. Each is a place of the
//! agent's own; a file is read only if it is a plain file there, never
//! through a link, and no larger than its place allows.

use std::fs;
use std::io::{self, Read as _};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use zendiloos_agent_proto::{ConfigurationFile, ConfigurationPath, Response, network_file_name};
use zendiloos_exec::{Limits, Program};

use crate::operations::OperationError;
use crate::web_server::WebServer;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 64 * 1024 };
const SAMBA: &str = "smbd.service";

/// Where each file of the configuration is; other than the real places in
/// tests. Most are the places of the operations that write them.
#[derive(Debug, Clone)]
pub struct ConfigurationPaths {
    pub own_certificate: PathBuf,
    pub own_key: PathBuf,
    /// `/etc/zendiloos/web/certificates`: `<n>/fullchain.pem`, `<n>/key.pem`.
    pub certificates: PathBuf,
    pub tls_settings: PathBuf,
    pub ssh_settings: PathBuf,
    pub time_servers: PathBuf,
    /// `/var/lib/samba/private/passdb.tdb`.
    pub samba_passwords: PathBuf,
    /// `/etc/hostid`.
    pub host_id: PathBuf,
    pub id_high_water: PathBuf,
    /// `/etc/systemd/network`.
    pub network: PathBuf,
}

/// The places of the configuration no operation of its own writes.
#[derive(Debug, Clone)]
pub struct ConfigurationPlaces {
    pub samba_passwords: PathBuf,
    pub host_id: PathBuf,
}

impl Default for ConfigurationPlaces {
    fn default() -> Self {
        Self {
            samba_passwords: PathBuf::from("/var/lib/samba/private/passdb.tdb"),
            host_id: PathBuf::from("/etc/hostid"),
        }
    }
}

impl ConfigurationPaths {
    fn place(&self, path: &ConfigurationPath) -> PathBuf {
        match path {
            ConfigurationPath::OwnCertificate => self.own_certificate.clone(),
            ConfigurationPath::OwnKey => self.own_key.clone(),
            ConfigurationPath::IssuedCertificate(n) => {
                self.certificates.join(n.to_string()).join("fullchain.pem")
            }
            ConfigurationPath::IssuedKey(n) => self.certificates.join(n.to_string()).join("key.pem"),
            ConfigurationPath::TlsSettings => self.tls_settings.clone(),
            ConfigurationPath::SshSettings => self.ssh_settings.clone(),
            ConfigurationPath::TimeServers => self.time_servers.clone(),
            ConfigurationPath::SambaPasswords => self.samba_passwords.clone(),
            ConfigurationPath::HostId => self.host_id.clone(),
            ConfigurationPath::IdHighWater => self.id_high_water.clone(),
            ConfigurationPath::Network(name) => self.network.join(name),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Configuration {
    systemctl: Program,
    paths: ConfigurationPaths,
}

impl Configuration {
    pub(crate) fn new(systemctl: Program, paths: ConfigurationPaths) -> Self {
        Self { systemctl, paths }
    }

    /// Every file of the configuration that is there, and the accounts.
    pub(crate) async fn read(
        &self,
        accounts: &crate::accounts::Accounts,
    ) -> Result<Response, OperationError> {
        let (users, groups) = accounts.managed().await?;
        let mut paths = vec![
            ConfigurationPath::OwnCertificate,
            ConfigurationPath::OwnKey,
            ConfigurationPath::TlsSettings,
            ConfigurationPath::SshSettings,
            ConfigurationPath::TimeServers,
            ConfigurationPath::SambaPasswords,
            ConfigurationPath::HostId,
            ConfigurationPath::IdHighWater,
        ];
        for number in numbered_folders(&self.paths.certificates)? {
            paths
                .extend([ConfigurationPath::IssuedCertificate(number), ConfigurationPath::IssuedKey(number)]);
        }
        for name in names(&self.paths.network)? {
            if network_file_name(&name) {
                paths.push(ConfigurationPath::Network(name));
            }
        }
        let mut files = Vec::new();
        for path in paths {
            let place = self.paths.place(&path);
            if let Some(content) = read_plain(&place, path.max_bytes())? {
                files.push(
                    ConfigurationFile::new(path, content)
                        .map_err(|_| OperationError::Refused(format!("{} is too large", place.display())))?,
                );
            }
        }
        Ok(Response::Configuration { files, users, groups })
    }

    /// See [`zendiloos_agent_proto::Request::RestoreConfiguration`]. All is
    /// checked before anything is written.
    pub(crate) async fn restore(
        &self,
        files: &[ConfigurationFile],
        web: &WebServer,
    ) -> Result<Response, OperationError> {
        for file in files {
            check(file)?;
        }
        let mut pairs = Vec::new();
        for file in files {
            let (place, bytes) = (self.paths.place(&file.path), file.content.as_bytes());
            match file.path {
                ConfigurationPath::OwnCertificate | ConfigurationPath::IssuedCertificate(_) => {
                    pairs.push((place, bytes.to_vec(), 0o644));
                }
                ConfigurationPath::OwnKey | ConfigurationPath::IssuedKey(_) => {
                    pairs.push((place, bytes.to_vec(), 0o600));
                }
                _ => {}
            }
        }
        if !pairs.is_empty() {
            web.put_restored(&pairs).await?;
        }
        for file in files {
            let (place, bytes) = (self.paths.place(&file.path), file.content.as_bytes());
            match file.path {
                ConfigurationPath::SambaPasswords => self.put_samba_passwords(&place, bytes).await?,
                ConfigurationPath::HostId => crate::durable::replace_mode(&place, bytes, 0o644)?,
                ConfigurationPath::IdHighWater => {
                    let now = read_plain(&place, file.path.max_bytes())?.unwrap_or_default();
                    let merged =
                        merge_high_water(&String::from_utf8_lossy(&now), &String::from_utf8_lossy(bytes));
                    crate::durable::replace_mode(&place, merged.as_bytes(), 0o644)?;
                }
                _ => {}
            }
        }
        tracing::info!(files = files.len(), "the configuration's files were restored");
        Ok(Response::Done)
    }

    /// Samba's passwords, with Samba stopped meanwhile (it keeps the
    /// database open) and started again if it ran.
    async fn put_samba_passwords(&self, place: &Path, bytes: &[u8]) -> Result<(), OperationError> {
        let active = zendiloos_exec::run(&self.systemctl, ["is-active", "--quiet", SAMBA], LIMITS)
            .await?
            .status
            .success();
        if active {
            self.systemctl(&["stop", SAMBA]).await?;
        }
        let written = (|| {
            if let Some(parent) = place.parent() {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|e| OperationError::Write(parent.to_owned(), e))?;
            }
            crate::durable::replace_mode(place, bytes, 0o600)
        })();
        if active {
            self.systemctl(&["start", SAMBA]).await?;
        }
        written
    }

    async fn systemctl(&self, args: &[&str]) -> Result<(), OperationError> {
        zendiloos_exec::run(&self.systemctl, args, LIMITS).await?.success(&self.systemctl)?;
        Ok(())
    }
}

/// What a restorable file must look like: certificates and keys are PEM,
/// the host id four bytes, the high-water file `uid=`/`gid=` lines.
fn check(file: &ConfigurationFile) -> Result<(), OperationError> {
    let bytes = file.content.as_bytes();
    let refused = |why: &str| Err(OperationError::Refused(format!("{}: {why}", file.path.as_path())));
    match &file.path {
        path if !path.restorable() => refused("not restored from a backup"),
        ConfigurationPath::OwnCertificate
        | ConfigurationPath::IssuedCertificate(_)
        | ConfigurationPath::OwnKey
        | ConfigurationPath::IssuedKey(_) => {
            let pem =
                std::str::from_utf8(bytes).is_ok_and(|text| text.trim_start().starts_with("-----BEGIN "));
            if pem { Ok(()) } else { refused("not PEM") }
        }
        ConfigurationPath::HostId if bytes.len() != 4 => refused("not four bytes"),
        ConfigurationPath::IdHighWater => {
            let lines = std::str::from_utf8(bytes).is_ok_and(|text| {
                text.lines().all(|line| {
                    line.starts_with('#')
                        || line.trim().is_empty()
                        || ["uid=", "gid="].iter().any(|key| {
                            line.strip_prefix(key).is_some_and(|id| id.trim().parse::<u32>().is_ok())
                        })
                })
            });
            if lines { Ok(()) } else { refused("not the IDs handed out") }
        }
        _ => Ok(()),
    }
}

/// The higher of each ID in `now` and `restored`.
fn merge_high_water(now: &str, restored: &str) -> String {
    let id = |text: &str, key: &str| {
        text.lines().find_map(|line| line.strip_prefix(key)?.trim().parse::<u32>().ok())
    };
    let mut text = String::from("# The highest IDs zendiloos handed out; they are never reused.\n");
    for key in ["gid=", "uid="] {
        if let Some(highest) = id(now, key).max(id(restored, key)) {
            text.push_str(&format!("{key}{highest}\n"));
        }
    }
    text
}

/// `path` if it is a plain file (not a link), at most `max` bytes.
fn read_plain(path: &Path, max: usize) -> Result<Option<Vec<u8>>, OperationError> {
    let file = match fs::OpenOptions::new().read(true).custom_flags(libc_nofollow()).open(path) {
        Ok(file) => file,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
            return Ok(None);
        }
        // A link in its place (ELOOP) is not ours to read.
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            return Ok(None);
        }
        Err(error) => return Err(OperationError::Read(path.to_owned(), error)),
    };
    let meta = file.metadata().map_err(|e| OperationError::Read(path.to_owned(), e))?;
    if !meta.is_file() || meta.nlink() == 0 {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| OperationError::Read(path.to_owned(), e))?;
    if bytes.len() > max {
        return Err(OperationError::Refused(format!("{} is larger than {max} bytes", path.display())));
    }
    Ok(Some(bytes))
}

fn libc_nofollow() -> i32 {
    rustix::fs::OFlags::NOFOLLOW.bits() as i32
}

/// The names in `folder`; none if it is missing.
fn names(folder: &Path) -> Result<Vec<String>, OperationError> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(OperationError::Read(folder.to_owned(), error)),
    };
    let mut names: Vec<String> =
        entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    Ok(names)
}

/// The folders of issued certificates: numbers.
fn numbered_folders(folder: &Path) -> Result<Vec<u32>, OperationError> {
    Ok(names(folder)?
        .iter()
        .filter(|name| !name.starts_with('0') && name.len() <= 9)
        .filter_map(|name| name.parse::<u32>().ok())
        .filter(|number| folder.join(number.to_string()).is_dir())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_handed_out_only_rise() {
        assert_eq!(
            merge_high_water("uid=2003\n", "# kept\nuid=2010\ngid=2001\n"),
            "# The highest IDs zendiloos handed out; they are never reused.\ngid=2001\nuid=2010\n"
        );
        assert!(merge_high_water("uid=2020\ngid=2030\n", "uid=2010\n").contains("uid=2020\n"));
    }

    #[test]
    fn restorable_files_look_as_they_should() {
        let file = |path, content: &[u8]| ConfigurationFile::new(path, content.to_vec()).unwrap();
        assert!(check(&file(ConfigurationPath::OwnKey, b"-----BEGIN PRIVATE KEY-----\n")).is_ok());
        assert!(check(&file(ConfigurationPath::OwnKey, b"#!/bin/sh\n")).is_err());
        assert!(check(&file(ConfigurationPath::HostId, b"abc")).is_err());
        assert!(check(&file(ConfigurationPath::IdHighWater, b"uid=2001\nrm -rf\n")).is_err());
        assert!(check(&file(ConfigurationPath::SshSettings, b"Port 22\n")).is_err(), "through SetSsh");
    }
}
