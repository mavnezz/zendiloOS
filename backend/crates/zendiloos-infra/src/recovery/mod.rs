//! The configuration backup's adapters (docs/architecture.md, 4.15): the
//! archive made of the database, the host's files and settings, and put
//! back from one; the folder scheduled backups go into; the restart into a
//! restored database.

mod archive;
mod folders;

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ConfigurationFile, ConfigurationPath, Request, Response, RestoredGroup,
    RestoredUser,
};
use zendiloos_kernel::{AuditEntry, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_recovery::application::{Archive, Restart, RestoreError, Restored, Restorer, Snapshots};
use zendiloos_recovery::domain::{FORMAT, Manifest, Passphrase, SchemaFit, archive_name, fit};
use zendiloos_system::application::ports::{HostNameControl, SshControl, TimeControl, WebTlsControl};
use zendiloos_system::domain::{RootLogin, SshSettings, TimeSettings, TlsMinimum};

pub use folders::AgentBackupFolders;

use crate::agent::{AgentHostName, AgentSsh, AgentTime, AgentWebTls};
use crate::sqlite::Database;

use self::archive::{Contents, ManifestFile, ReadError};

const WHAT: &str = "configuration backup";
/// Below the state directory: what a restore staged for the next start.
const RESTORE: &str = "restore";
/// The database a restore staged; its being there is what the next start
/// acts on.
const STAGED: &str = "zendiloos.db";
/// A restore's request carries at most this much of the files at once: a
/// request may have 2 MiB, base64 takes a third more.
const BATCH: usize = 1024 * 1024;

fn port(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> PortError {
    PortError::new(WHAT, error)
}

/// The host's settings no file of the backup holds as zendiloOS reads it
/// back: the clock's (timedated's), SSH's, the oldest TLS version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct HostSettings {
    #[serde(default)]
    time: Option<TimeFile>,
    #[serde(default)]
    ssh: Option<SshFile>,
    #[serde(default)]
    tls_minimum: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TimeFile {
    zone: String,
    network_time: bool,
    servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SshFile {
    enabled: bool,
    port: u16,
    root_login: Option<String>,
    password_login: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct AccountsFile {
    users: Vec<RestoredUser>,
    groups: Vec<RestoredGroup>,
}

fn root_login(value: Option<RootLogin>) -> Option<String> {
    value.map(|root| match root {
        RootLogin::No => "no".to_owned(),
        RootLogin::KeyOnly => "key_only".to_owned(),
        RootLogin::Yes => "yes".to_owned(),
    })
}

fn read_root_login(value: Option<&str>) -> Option<RootLogin> {
    match value? {
        "no" => Some(RootLogin::No),
        "key_only" => Some(RootLogin::KeyOnly),
        "yes" => Some(RootLogin::Yes),
        _ => None,
    }
}

/// A private folder below `state`, made if missing.
fn folder(state: &Path, name: &str) -> Result<PathBuf, PortError> {
    let path = state.join(name);
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&path).map_err(port)?;
    Ok(path)
}

fn unique(stem: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{stem}-{}-{nanos}.db", std::process::id())
}

/// Makes backups: [`Snapshots`] on the database, the agent and the
/// system's settings as their adapters read them.
pub struct ConfigurationSnapshots {
    database: Database,
    transport: Arc<dyn AgentTransport>,
    state: PathBuf,
    version: &'static str,
    clock: Arc<dyn Clock>,
}

impl ConfigurationSnapshots {
    pub fn new(
        database: Database,
        transport: Arc<dyn AgentTransport>,
        state: PathBuf,
        version: &'static str,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { database, transport, state, version, clock }
    }

    async fn settings(&self, context: &RequestContext) -> HostSettings {
        let time = match AgentTime::new(self.transport.clone()).status(context).await {
            Ok(status) => Some(TimeFile {
                zone: status.zone,
                network_time: status.network_time,
                servers: status.servers,
            }),
            Err(error) => {
                tracing::warn!(%error, "configuration backup: the clock's settings are left out");
                None
            }
        };
        let ssh = match AgentSsh::new(self.transport.clone()).status(context).await {
            Ok(status) if status.installed => Some(SshFile {
                enabled: status.enabled,
                port: status.port,
                root_login: root_login(status.root_login),
                password_login: status.password_login,
            }),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(%error, "configuration backup: SSH's settings are left out");
                None
            }
        };
        let tls_minimum = AgentWebTls::new(self.transport.clone()).minimum(context).await.ok().map(
            |minimum| match minimum {
                TlsMinimum::Tls12 => "tls1_2".to_owned(),
                TlsMinimum::Tls13 => "tls1_3".to_owned(),
            },
        );
        HostSettings { time, ssh, tls_minimum }
    }
}

#[async_trait]
impl Snapshots for ConfigurationSnapshots {
    async fn create(&self, passphrase: &Passphrase, context: &RequestContext) -> Result<Archive, PortError> {
        let (files, users, groups) =
            match crate::agent::call_agent(self.transport.as_ref(), Request::ReadConfiguration {}, context)
                .await?
            {
                Response::Configuration { files, users, groups } => (files, users, groups),
                other => return Err(crate::agent::unexpected_answer(other)),
            };
        let host_name = AgentHostName::new(self.transport.clone()).name(context).await.unwrap_or_default();
        let settings = serde_json::to_vec(&self.settings(context).await).map_err(port)?;
        let accounts = serde_json::to_vec(&AccountsFile { users, groups }).map_err(port)?;
        let work = folder(&self.state, "configuration")?;
        let copy = work.join(unique("snapshot"));
        let schema = self.database.copy_to(&copy).await.map_err(port)?;
        let now = self.clock.now();
        let manifest = ManifestFile {
            format: FORMAT,
            version: self.version.to_owned(),
            schema,
            host_name: host_name.clone(),
            created_at: now.unix_seconds(),
        };
        let files: Vec<(ConfigurationPath, Vec<u8>)> =
            files.into_iter().map(|file| (file.path, file.content.into_bytes())).collect();
        let phrase = passphrase.expose().to_owned();
        let made = {
            let (manifest, copy) = (manifest.clone(), copy.clone());
            tokio::task::spawn_blocking(move || {
                let contents = Contents {
                    manifest: &manifest,
                    settings: &settings,
                    accounts: &accounts,
                    database: &copy,
                    files: &files,
                };
                archive::write(&contents, &phrase)
            })
            .await
        };
        let _ = fs::remove_file(&copy);
        let bytes = made.map_err(port)?.map_err(port)?;
        Ok(Archive { name: archive_name(&host_name, now), manifest: domain_manifest(&manifest), bytes })
    }
}

fn domain_manifest(manifest: &ManifestFile) -> Manifest {
    Manifest {
        format: manifest.format,
        version: manifest.version.clone(),
        schema: manifest.schema,
        host_name: manifest.host_name.clone(),
        created_at: Timestamp::from_unix_seconds(manifest.created_at),
    }
}

/// Restores backups: [`Restorer`] on the agent and the system's settings'
/// adapters, the database staged in the state directory for the next
/// start ([`stage_restored_database`]).
pub struct ConfigurationRestorer {
    transport: Arc<dyn AgentTransport>,
    state: PathBuf,
}

impl ConfigurationRestorer {
    pub fn new(transport: Arc<dyn AgentTransport>, state: PathBuf) -> Self {
        Self { transport, state }
    }

    /// Puts the host's part back: the accounts first (the passwords and
    /// files name them), the machine's name, the files, then the other
    /// settings; a setting that fails is only said.
    async fn put_back(
        &self,
        opened: &archive::Opened,
        context: &RequestContext,
    ) -> Result<Vec<String>, PortError> {
        let mut warnings = Vec::new();
        let accounts: AccountsFile = serde_json::from_slice(&opened.accounts).map_err(port)?;
        if !accounts.users.is_empty() || !accounts.groups.is_empty() {
            let request = Request::RestoreAccounts { users: accounts.users, groups: accounts.groups };
            crate::agent::expect_agent_done(self.transport.as_ref(), request, context).await?;
        }
        // The name first: renaming makes the own certificate anew, which the
        // backup's then replaces.
        let name = &opened.manifest.host_name;
        match proto::HostName::parse(name) {
            Ok(host) => {
                let request = Request::SetHostName { name: host };
                if let Err(error) = crate::agent::call_agent(self.transport.as_ref(), request, context).await
                {
                    warnings
                        .push(format!("the machine is not called {name} ({})", crate::agent::reason(&error)));
                }
            }
            Err(_) => warnings.push(format!("the machine's name {name} is not taken here")),
        }
        let files: Vec<ConfigurationFile> = opened
            .files
            .iter()
            .filter(|(path, _)| path.restorable())
            .filter_map(|(path, bytes)| ConfigurationFile::new(path.clone(), bytes.clone()).ok())
            .collect();
        let mut batch: Vec<ConfigurationFile> = Vec::new();
        let mut size = 0;
        let mut batches = Vec::new();
        for file in files {
            let length = file.content.as_bytes().len();
            if length > BATCH {
                warnings.push(format!("{} is too large to put back", file.path.as_path()));
                continue;
            }
            if size + length > BATCH && !batch.is_empty() {
                batches.push(std::mem::take(&mut batch));
                size = 0;
            }
            size += length;
            batch.push(file);
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        for files in batches {
            let names: Vec<String> = files.iter().map(|file| file.path.as_path()).collect();
            let request = Request::RestoreConfiguration { files };
            if let Err(error) =
                crate::agent::expect_agent_done(self.transport.as_ref(), request, context).await
            {
                tracing::warn!(%error, "restore: files not put back");
                warnings.push(format!(
                    "not put back: {} ({})",
                    names.join(", "),
                    crate::agent::reason(&error)
                ));
            }
        }
        let settings: HostSettings = serde_json::from_slice(&opened.settings).unwrap_or_default();
        if let Some(time) = settings.time {
            let applied = AgentTime::new(self.transport.clone())
                .apply(
                    &TimeSettings {
                        zone: time.zone.clone(),
                        network_time: time.network_time,
                        servers: time.servers,
                        clock: None,
                    },
                    context,
                )
                .await;
            if let Err(error) = applied {
                warnings.push(format!(
                    "the time zone {} is not set ({})",
                    time.zone,
                    crate::agent::reason(&error)
                ));
            }
        }
        if let Some(ssh) = settings.ssh {
            let ssh_settings = SshSettings {
                enabled: ssh.enabled,
                port: ssh.port,
                root_login: read_root_login(ssh.root_login.as_deref()),
                password_login: ssh.password_login,
                off_after_minutes: None,
            };
            if let Err(error) = AgentSsh::new(self.transport.clone()).apply(&ssh_settings, context).await {
                warnings.push(format!("SSH's settings are not put back ({})", crate::agent::reason(&error)));
            }
        }
        if let Some(minimum) = settings.tls_minimum {
            let minimum = if minimum == "tls1_3" { TlsMinimum::Tls13 } else { TlsMinimum::Tls12 };
            if let Err(error) = AgentWebTls::new(self.transport.clone()).apply(minimum, context).await {
                warnings.push(format!(
                    "the oldest TLS version is not put back ({})",
                    crate::agent::reason(&error)
                ));
            }
        }
        Ok(warnings)
    }
}

/// The staged database's schema, and the web interface's HTTPS port it
/// keeps; `audit` written into it. Not of zendiloOS: none.
async fn inspect(database: &Path, audit: &AuditEntry) -> Result<Option<(i64, Option<u16>)>, PortError> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let options = SqliteConnectOptions::new().filename(database).foreign_keys(false);
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options).await.map_err(port)?;
    let inspected = async {
        let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&pool)
            .await
            .map_err(port)?;
        if !tables.iter().any(|name| name == "_sqlx_migrations")
            || !tables.iter().any(|name| name == "audit_log")
        {
            return Ok(None);
        }
        let schema: Option<i64> =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(&pool)
                .await
                .map_err(port)?;
        let https: Option<i64> = if tables.iter().any(|name| name == "web_server") {
            sqlx::query_scalar("SELECT https_port FROM web_server WHERE id = 1")
                .fetch_optional(&pool)
                .await
                .map_err(port)?
        } else {
            None
        };
        let outcome = match audit.outcome {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
            Outcome::Denied => "denied",
        };
        // The account that restored is of the other database: by name only.
        sqlx::query(
            "INSERT INTO audit_log (at, account_id, actor, client_ip, action, target, outcome, request_id)
             VALUES (?, NULL, ?, ?, ?, ?, ?, ?)",
        )
        .bind(audit.at.unix_seconds())
        .bind(audit.actor.as_deref())
        .bind(audit.client_ip.map(|ip| ip.to_string()))
        .bind(audit.action)
        .bind(audit.target.as_deref())
        .bind(outcome)
        .bind(audit.request_id.as_deref())
        .execute(&pool)
        .await
        .map_err(port)?;
        Ok(Some((schema.unwrap_or(0), https.and_then(|port| u16::try_from(port).ok()))))
    }
    .await;
    pool.close().await;
    inspected
}

#[async_trait]
impl Restorer for ConfigurationRestorer {
    async fn restore(
        &self,
        archive: Vec<u8>,
        passphrase: &Passphrase,
        audit: AuditEntry,
        context: &RequestContext,
    ) -> Result<Restored, RestoreError> {
        let staging = folder(&self.state, RESTORE)?;
        let incoming = staging.join(unique("incoming"));
        let phrase = passphrase.expose().to_owned();
        let target = incoming.clone();
        let read = tokio::task::spawn_blocking(move || archive::read(&archive, &phrase, &target))
            .await
            .map_err(port)?;
        let discard = || {
            let _ = fs::remove_file(&incoming);
        };
        let opened = match read {
            Ok(opened) => opened,
            Err(error) => {
                discard();
                return Err(match error {
                    ReadError::WrongPassphrase => RestoreError::WrongPassphrase,
                    ReadError::NotABackup(why) => RestoreError::NotABackup(why),
                    ReadError::Io(error) => RestoreError::Port(port(error)),
                });
            }
        };
        if opened.manifest.format != FORMAT {
            discard();
            return Err(RestoreError::UnsupportedFormat(opened.manifest.format));
        }
        let audit = audit.target(opened.manifest.host_name.clone());
        let (schema, https_port) = match inspect(&incoming, &audit).await {
            Ok(Some(found)) => found,
            Ok(None) => {
                discard();
                return Err(RestoreError::NotABackup("its database is not zendiloOS's".into()));
            }
            Err(error) => {
                discard();
                return Err(RestoreError::NotABackup(format!(
                    "its database cannot be read: {}",
                    crate::agent::reason(&error)
                )));
            }
        };
        let supported = Database::supported_schema();
        if fit(schema, supported) == SchemaFit::Newer {
            discard();
            return Err(RestoreError::TooNew { found: schema, supported });
        }
        let warnings = match self.put_back(&opened, context).await {
            Ok(warnings) => warnings,
            Err(error) => {
                discard();
                return Err(RestoreError::Refused(crate::agent::reason(&error)));
            }
        };
        fs::rename(&incoming, staging.join(STAGED)).map_err(port)?;
        let network = opened
            .files
            .iter()
            .filter_map(|(path, _)| match path {
                ConfigurationPath::Network(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        let mut manifest = domain_manifest(&opened.manifest);
        manifest.schema = schema;
        Ok(Restored { manifest, warnings, https_port, network })
    }
}

/// At startup, before the database is opened: a database a restore staged
/// takes the place of the one there, which goes to `backup_dir` with its
/// write-ahead log (`zendiloos-<time>-before-restore.db`). Where the one
/// before went, if a restore was staged.
pub fn stage_restored_database(
    state: &Path,
    database: &Path,
    backup_dir: &Path,
) -> io::Result<Option<PathBuf>> {
    let staged = state.join(RESTORE).join(STAGED);
    if !staged.exists() {
        return Ok(None);
    }
    fs::DirBuilder::new().recursive(true).mode(0o700).create(backup_dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let before = backup_dir.join(format!("zendiloos-{stamp}-before-restore.db"));
    for suffix in ["-wal", "-shm", ""] {
        let from = PathBuf::from(format!("{}{suffix}", database.display()));
        let to = PathBuf::from(format!("{}{suffix}", before.display()));
        match fs::rename(&from, &to) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::rename(&staged, database)?;
    Ok(Some(before))
}

/// [`Restart`] for zendiloosd: asked, it ends its serving a moment later,
/// so the answer is out, and exits for systemd to start it again
/// ([`ProcessRestart::requested`], [`ProcessRestart::was_requested`]).
#[derive(Clone, Default)]
pub struct ProcessRestart {
    asked: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ProcessRestart {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves once a restart was asked for and its moment came.
    pub async fn requested(&self) {
        loop {
            let notified = self.notify.notified();
            if self.asked.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }
            notified.await;
        }
    }

    pub fn was_requested(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Restart for ProcessRestart {
    fn restart(&self) {
        self.asked.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
