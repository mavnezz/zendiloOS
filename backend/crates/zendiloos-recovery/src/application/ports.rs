//! What this context needs from the outside world.

use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, PortError, RequestContext};

use crate::domain::{LastRun, Manifest, Passphrase, Plan, Target};

/// A backup, made.
#[derive(Debug, Clone)]
pub struct Archive {
    pub name: String,
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
}

/// Makes backups of the configuration: the database (a consistent copy,
/// never the live file), the host's files and settings, as one archive
/// encrypted with the passphrase.
#[async_trait]
pub trait Snapshots: Send + Sync {
    async fn create(&self, passphrase: &Passphrase, context: &RequestContext) -> Result<Archive, PortError>;
}

/// What came of a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    pub manifest: Manifest,
    /// What was not put back, in words; the rest was.
    pub warnings: Vec<String>,
    /// The web interface's HTTPS port as the backup has it, which holds
    /// once zendiloosd started again.
    pub https_port: Option<u16>,
    /// The network's files in the backup, which are not put in force: the
    /// interfaces here may be others, and the network is changed with
    /// confirmation.
    pub network: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("the passphrase does not open this backup")]
    WrongPassphrase,
    #[error("not a configuration backup: {0}")]
    NotABackup(String),
    #[error("a backup of another layout: {0}")]
    UnsupportedFormat(u32),
    #[error("the backup's database is newer than this release knows")]
    TooNew { found: i64, supported: i64 },
    /// The host would not take it back, in its words; nothing of the
    /// database is staged.
    #[error("refused by the host: {0}")]
    Refused(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Restores backups on this installation.
#[async_trait]
pub trait Restorer: Send + Sync {
    /// Opens `archive` with `passphrase` and checks it; then puts the
    /// host's part back (accounts, files, settings) and stages the
    /// database for zendiloosd's next start, `audit` written into it.
    async fn restore(
        &self,
        archive: Vec<u8>,
        passphrase: &Passphrase,
        audit: AuditEntry,
        context: &RequestContext,
    ) -> Result<Restored, RestoreError>;
}

/// zendiloosd starts again, with the restored database, once the answer
/// is out.
pub trait Restart: Send + Sync {
    fn restart(&self);
}

/// The plan, one.
#[async_trait]
pub trait PlanRepository: Send + Sync {
    async fn get(&self) -> Result<Plan, PortError>;
    /// Everything but the last run.
    async fn save(&self, plan: &Plan) -> Result<(), PortError>;
    async fn record(&self, run: &LastRun) -> Result<(), PortError>;
}

/// The folder backups are written into, with `user`'s access to it.
#[async_trait]
pub trait BackupFolders: Send + Sync {
    /// Writes `bytes` as `name` into the folder (made if missing), whole or
    /// not at all.
    async fn write(
        &self,
        target: &Target,
        user: &str,
        name: &str,
        bytes: Vec<u8>,
        context: &RequestContext,
    ) -> Result<(), PortError>;

    /// The names in the folder.
    async fn names(
        &self,
        target: &Target,
        user: &str,
        context: &RequestContext,
    ) -> Result<Vec<String>, PortError>;

    async fn remove(
        &self,
        target: &Target,
        user: &str,
        name: &str,
        context: &RequestContext,
    ) -> Result<(), PortError>;
}

/// Accounts, as far as the backups are concerned.
#[async_trait]
pub trait Operators: Send + Sync {
    async fn name(&self, account_id: i64) -> Result<Option<String>, PortError>;

    /// The account's name if it still may manage the system and is not
    /// disabled.
    async fn administrator(&self, account_id: i64) -> Result<Option<String>, PortError>;
}
