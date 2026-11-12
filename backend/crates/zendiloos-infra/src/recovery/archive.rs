//! The archive of a configuration backup: tar, compressed with zstd,
//! encrypted with age's passphrase (scrypt). Debian's tools open it by
//! hand: `age -d backup.tar.zst.age | zstd -d | tar -t`.
//!
//! Inside, in this order: `manifest.json` (what it is, read before
//! anything else), `settings.json` (the host's settings the agent keeps in
//! no file of the backup's), `accounts.json` (the people's Linux users and
//! groups), `zendiloos.db` (the database, a consistent copy) and the
//! host's files below `files/`, at their paths.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use age::secrecy::SecretString;
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::ConfigurationPath;

pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const SETTINGS: &str = "settings.json";
pub(crate) const ACCOUNTS: &str = "accounts.json";
pub(crate) const DATABASE: &str = "zendiloos.db";
pub(crate) const FILES: &str = "files/";
/// More than a manifest, the settings or the accounts of any NAS take.
const MAX_JSON: u64 = 4 * 1024 * 1024;
/// More than a database of zendiloOS reaches.
const MAX_DATABASE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
/// age's default for decrypting is lower than what a slow machine
/// encrypting with its own default may have made; 2^22 is about four
/// GiB of work and still bounded.
const MAX_WORK_FACTOR: u8 = 22;

/// What a backup says of itself (`manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManifestFile {
    pub format: u32,
    pub version: String,
    pub schema: i64,
    pub host_name: String,
    pub created_at: i64,
}

/// What goes into an archive.
pub(crate) struct Contents<'a> {
    pub manifest: &'a ManifestFile,
    pub settings: &'a [u8],
    pub accounts: &'a [u8],
    pub database: &'a Path,
    pub files: &'a [(ConfigurationPath, Vec<u8>)],
}

fn entry<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    mode: u32,
    size: u64,
    data: impl Read,
) -> io::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_entry_type(tar::EntryType::Regular);
    builder.append_data(&mut header, name, data)
}

/// The archive of `contents`, encrypted with `passphrase`.
pub(crate) fn write(contents: &Contents<'_>, passphrase: &str) -> io::Result<Vec<u8>> {
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_owned()));
    let mut output = Vec::new();
    let encrypted = encryptor.wrap_output(&mut output)?;
    let compressed = zstd::Encoder::new(encrypted, 9)?;
    let mut builder = tar::Builder::new(compressed);
    let manifest = serde_json::to_vec_pretty(contents.manifest).map_err(io::Error::other)?;
    entry(&mut builder, MANIFEST, 0o600, manifest.len() as u64, manifest.as_slice())?;
    entry(&mut builder, SETTINGS, 0o600, contents.settings.len() as u64, contents.settings)?;
    entry(&mut builder, ACCOUNTS, 0o600, contents.accounts.len() as u64, contents.accounts)?;
    let database = fs::File::open(contents.database)?;
    let size = database.metadata()?.len();
    entry(&mut builder, DATABASE, 0o600, size, database)?;
    for (path, bytes) in contents.files {
        let name = format!("{FILES}{}", path.as_path().trim_start_matches('/'));
        entry(&mut builder, &name, 0o600, bytes.len() as u64, bytes.as_slice())?;
    }
    let compressed = builder.into_inner()?;
    let encrypted = compressed.finish()?;
    encrypted.finish()?;
    Ok(output)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("the passphrase does not open it")]
    WrongPassphrase,
    #[error("{0}")]
    NotABackup(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// An archive, read; its database written to the file [`read`] was given.
pub(crate) struct Opened {
    pub manifest: ManifestFile,
    pub settings: Vec<u8>,
    pub accounts: Vec<u8>,
    pub files: Vec<(ConfigurationPath, Vec<u8>)>,
}

fn not_a_backup(why: impl Into<String>) -> ReadError {
    ReadError::NotABackup(why.into())
}

fn read_json(entry: &mut impl Read) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    entry.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(not_a_backup("an entry is too large"));
    }
    Ok(bytes)
}

/// Opens `archive` with `passphrase` and reads it, the database into
/// `database` (a new file, private). Anything not of the layout refuses
/// it: other entries, links, a manifest not first, an entry twice.
pub(crate) fn read(archive: &[u8], passphrase: &str, database: &Path) -> Result<Opened, ReadError> {
    let decryptor = age::Decryptor::new(archive).map_err(|_| not_a_backup("not encrypted with age"))?;
    if !decryptor.is_scrypt() {
        return Err(not_a_backup("not encrypted with a passphrase"));
    }
    let mut identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    identity.set_max_work_factor(MAX_WORK_FACTOR);
    let decrypted = match decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)) {
        Ok(reader) => reader,
        Err(age::DecryptError::ExcessiveWork { .. }) => return Err(not_a_backup("too much work to open")),
        Err(_) => return Err(ReadError::WrongPassphrase),
    };
    let decompressed = zstd::Decoder::new(decrypted)?;
    let mut tar = tar::Archive::new(decompressed);
    let (mut manifest, mut settings, mut accounts, mut stored) = (None, None, None, false);
    let mut files: Vec<(ConfigurationPath, Vec<u8>)> = Vec::new();
    for (count, entry) in tar.entries().map_err(|e| not_a_backup(e.to_string()))?.enumerate() {
        let mut entry = entry.map_err(|e| not_a_backup(e.to_string()))?;
        if count >= MAX_ENTRIES {
            return Err(not_a_backup("too many entries"));
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(not_a_backup("an entry is no plain file"));
        }
        let name = entry.path().map_err(|e| not_a_backup(e.to_string()))?.to_string_lossy().into_owned();
        if manifest.is_none() && name != MANIFEST {
            return Err(not_a_backup("no manifest first"));
        }
        match name.as_str() {
            MANIFEST if manifest.is_none() => {
                let bytes = read_json(&mut entry)?;
                let parsed: ManifestFile =
                    serde_json::from_slice(&bytes).map_err(|e| not_a_backup(format!("its manifest: {e}")))?;
                manifest = Some(parsed);
            }
            SETTINGS if settings.is_none() => settings = Some(read_json(&mut entry)?),
            ACCOUNTS if accounts.is_none() => accounts = Some(read_json(&mut entry)?),
            DATABASE if !stored => {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file =
                    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(database)?;
                let copied = io::copy(&mut (&mut entry).take(MAX_DATABASE + 1), &mut file)?;
                if copied > MAX_DATABASE {
                    return Err(not_a_backup("the database is too large"));
                }
                file.sync_all()?;
                stored = true;
            }
            other => {
                let Some(path) = other
                    .strip_prefix(FILES)
                    .and_then(|path| ConfigurationPath::parse(&format!("/{path}")).ok())
                else {
                    return Err(not_a_backup(format!("an entry {other} of another layout")));
                };
                if files.iter().any(|(seen, _)| *seen == path) {
                    return Err(not_a_backup(format!("{other} twice")));
                }
                let mut bytes = Vec::new();
                (&mut entry).take(path.max_bytes() as u64 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > path.max_bytes() {
                    return Err(not_a_backup(format!("{other} is too large")));
                }
                files.push((path, bytes));
            }
        }
    }
    let (Some(manifest), Some(settings), Some(accounts), true) = (manifest, settings, accounts, stored)
    else {
        return Err(not_a_backup("something of it is missing"));
    };
    Ok(Opened { manifest, settings, accounts, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ManifestFile {
        ManifestFile {
            format: 1,
            version: "0.1.0".into(),
            schema: 55,
            host_name: "nas".into(),
            created_at: 1_800_000_000,
        }
    }

    fn archive(dir: &Path, passphrase: &str) -> Vec<u8> {
        let database = dir.join("source.db");
        fs::write(&database, b"SQLite format 3\0...").unwrap();
        let files = [(ConfigurationPath::HostId, vec![1, 2, 3, 4])];
        let contents = Contents {
            manifest: &manifest(),
            settings: b"{}",
            accounts: br#"{"users":[],"groups":[]}"#,
            database: &database,
            files: &files,
        };
        write(&contents, passphrase).unwrap()
    }

    #[test]
    fn what_is_written_is_read_back_with_its_passphrase_only() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(dir.path(), "correct horse battery");
        assert!(bytes.starts_with(b"age-encryption.org/v1\n"));

        let opened = read(&bytes, "correct horse battery", &dir.path().join("restored.db")).unwrap();

        assert_eq!(opened.manifest, manifest());
        assert_eq!(fs::read(dir.path().join("restored.db")).unwrap(), b"SQLite format 3\0...");
        assert_eq!(opened.files, [(ConfigurationPath::HostId, vec![1, 2, 3, 4])]);
        assert_eq!(opened.settings, b"{}");
        assert!(matches!(
            read(&bytes, "another passphrase", &dir.path().join("other.db")),
            Err(ReadError::WrongPassphrase)
        ));
        assert!(matches!(read(b"PK\x03\x04", "x", &dir.path().join("x.db")), Err(ReadError::NotABackup(_))));
    }

    /// An archive of other entries, encrypted rightly: refused.
    #[test]
    fn another_layout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let encrypted = |build: &dyn Fn(&mut tar::Builder<Vec<u8>>)| {
            let mut builder = tar::Builder::new(Vec::new());
            build(&mut builder);
            let tar = builder.into_inner().unwrap();
            let compressed = zstd::encode_all(tar.as_slice(), 3).unwrap();
            let encryptor =
                age::Encryptor::with_user_passphrase(SecretString::from("pass phrase 12".to_owned()));
            let mut output = Vec::new();
            let mut writer = encryptor.wrap_output(&mut output).unwrap();
            writer.write_all(&compressed).unwrap();
            writer.finish().unwrap();
            output
        };
        let manifest = serde_json::to_vec(&manifest()).unwrap();
        let cases: Vec<Vec<u8>> = vec![
            encrypted(&|b| entry(b, "zendiloos.db", 0o600, 1, b"x".as_slice()).unwrap()),
            encrypted(&|b| {
                entry(b, MANIFEST, 0o600, manifest.len() as u64, manifest.as_slice()).unwrap();
                entry(b, "files/etc/shadow", 0o600, 1, b"x".as_slice()).unwrap();
            }),
            encrypted(&|b| entry(b, MANIFEST, 0o600, manifest.len() as u64, manifest.as_slice()).unwrap()),
        ];
        for (at, bytes) in cases.iter().enumerate() {
            let result = read(bytes, "pass phrase 12", &dir.path().join(format!("{at}.db")));
            assert!(matches!(result, Err(ReadError::NotABackup(_))), "case {at}");
        }
    }
}
