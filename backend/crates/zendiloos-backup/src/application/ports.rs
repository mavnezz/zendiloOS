//! What the Backup context needs from outside: its records, the host
//! (rsync and ssh through the system agent), and the places other
//! contexts own (shares, removable drives), each in its own terms.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Secret, Timestamp};

use crate::domain::{
    CloudAccount, Entry, Existing, Filters, Outcome, Provider, Run, Server, Settings, Task, TaskSettings,
    Transfer, Trigger,
};

/// A task to be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub settings: TaskSettings,
    pub folder: String,
    pub created_at: Timestamp,
    pub last_plug: Option<String>,
}

/// What becomes of a task's stored password when it changes.
#[derive(Debug, Clone)]
pub enum PasswordChange {
    Keep,
    Set(Secret),
    Clear,
}

#[async_trait]
pub trait TaskRepository: Send + Sync {
    /// Oldest first.
    async fn list(&self) -> Result<Vec<Task>, PortError>;
    async fn find(&self, id: i64) -> Result<Option<Task>, PortError>;
    /// The new task's id; `None` if another has its name.
    async fn create(&self, task: &NewTask, password: Option<&Secret>) -> Result<Option<i64>, PortError>;
    /// Replaces its settings, `since` and `last_plug`; `false` if another
    /// has its name.
    async fn update(&self, task: &Task, password: PasswordChange) -> Result<bool, PortError>;
    async fn delete(&self, id: i64) -> Result<bool, PortError>;
    /// The rsync daemon's password, never shown.
    async fn password(&self, id: i64) -> Result<Option<Secret>, PortError>;
    /// Its runs on schedule are due from `since`.
    async fn noted(&self, id: i64, since: Timestamp) -> Result<(), PortError>;
    /// Its target drive's plugging in, backed up for.
    async fn plugged(&self, id: i64, plug: Option<&str>) -> Result<(), PortError>;
    /// The password (and salt) its target is encrypted with, never shown.
    async fn crypt(&self, id: i64) -> Result<Option<Crypt>, PortError>;
    async fn set_crypt(&self, id: i64, crypt: Option<&Crypt>) -> Result<(), PortError>;
}

/// rclone's crypt: its password, and its salt (rclone's `password2`).
#[derive(Debug, Clone)]
pub struct Crypt {
    pub password: Secret,
    pub salt: Option<Secret>,
}

/// A cloud account's sign-in, as a run hands it to rclone.
#[derive(Debug, Clone)]
pub struct CloudCredentials {
    pub provider: Provider,
    /// The OAuth token as rclone keeps it (JSON).
    pub token: Secret,
    /// The administrator's own client; none: rclone's public one.
    pub client_id: Option<String>,
    pub client_secret: Option<Secret>,
    pub drive_id: Option<String>,
    pub drive_type: Option<String>,
}

/// What a run needs to reach its places, none of it ever shown.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    /// An rsync daemon's.
    pub password: Option<Secret>,
    pub crypt: Option<Crypt>,
    pub cloud: Option<CloudCredentials>,
}

/// A cloud account to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub name: String,
    pub provider: Provider,
    pub client_id: Option<String>,
    pub drive_id: Option<String>,
    pub drive_type: Option<String>,
    pub created_at: Timestamp,
}

#[async_trait]
pub trait CloudAccountRepository: Send + Sync {
    async fn list(&self) -> Result<Vec<CloudAccount>, PortError>;
    async fn find(&self, id: i64) -> Result<Option<CloudAccount>, PortError>;
    /// The new account's id; `None` if another has its name.
    async fn create(
        &self,
        account: &NewAccount,
        token: &Secret,
        client_secret: Option<&Secret>,
    ) -> Result<Option<i64>, PortError>;
    /// Its token and own client's secret.
    async fn secrets(&self, id: i64) -> Result<Option<(Secret, Option<Secret>)>, PortError>;
    /// A token rclone refreshed.
    async fn set_token(&self, id: i64, token: &Secret) -> Result<(), PortError>;
    async fn delete(&self, id: i64) -> Result<bool, PortError>;
}

/// Signing in to a cloud with OAuth: the provider's consent page, the code
/// a browser brought back exchanged for a token.
#[async_trait]
pub trait CloudSignIn: Send + Sync {
    /// A random state for one sign-in.
    fn new_state(&self) -> String;
    /// The page where the administrator consents; it sends the browser to
    /// `http://127.0.0.1:53682/` with the code.
    fn consent_url(&self, provider: Provider, client_id: Option<&str>, state: &str) -> String;
    /// The token for `code`, as rclone keeps one (JSON).
    async fn exchange(
        &self,
        provider: Provider,
        client_id: Option<&str>,
        client_secret: Option<&Secret>,
        code: &str,
    ) -> Result<Secret, HostError>;
    /// OneDrive's drive (its id and type) the token reaches; none for others.
    async fn drive(&self, provider: Provider, token: &Secret) -> Result<Option<(String, String)>, HostError>;
}

/// A run as it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    pub task_id: i64,
    pub trigger: Trigger,
    pub by: Option<String>,
    pub started_at: Timestamp,
    pub job_id: Option<i64>,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEnd {
    pub finished_at: Timestamp,
    pub outcome: Outcome,
    pub files: u64,
    pub bytes: u64,
    pub version: Option<String>,
    pub error: Option<String>,
    /// The first entries of its transfer list ([`crate::domain::TRANSFERS_KEPT`]).
    pub transfers: Vec<Transfer>,
    /// All of them.
    pub transfers_total: u64,
}

#[async_trait]
pub trait RunRepository: Send + Sync {
    async fn start(&self, run: &NewRun) -> Result<i64, PortError>;
    async fn finish(&self, id: i64, end: &RunEnd) -> Result<(), PortError>;
    async fn find(&self, id: i64) -> Result<Option<Run>, PortError>;
    /// Newest first, of one task or all.
    async fn list(&self, task: Option<i64>, limit: u32) -> Result<Vec<Run>, PortError>;
    /// A run's transfer list as kept, in order.
    async fn transfers(&self, run: i64, offset: u32, limit: u32) -> Result<Vec<Transfer>, PortError>;
    /// Runs that were running become interrupted; how many.
    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError>;
    /// Forgets runs that started before `at`, with their transfer lists.
    async fn forget_before(&self, at: Timestamp) -> Result<u64, PortError>;
}

#[async_trait]
pub trait SettingsRepository: Send + Sync {
    async fn get(&self) -> Result<Settings, PortError>;
    async fn set(&self, settings: Settings) -> Result<(), PortError>;
}

/// A share, where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareSpot {
    pub id: i64,
    pub name: String,
    pub volume: u16,
}

/// Who may read a share: whoever reads the backups put in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readers {
    pub accounts: Vec<String>,
    pub groups: Vec<String>,
    /// Computers it is exported to over NFS.
    pub nfs: Vec<String>,
}

#[async_trait]
pub trait Shares: Send + Sync {
    async fn locate(&self, share_id: i64) -> Result<Option<ShareSpot>, PortError>;
    async fn readers(&self, share_id: i64) -> Result<Option<Readers>, PortError>;
}

/// A removable drive as the disks show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// Its file system's UUID.
    pub id: String,
    pub label: Option<String>,
    /// `vfat`, `exfat`, `ntfs` or `ext4`.
    pub filesystem: String,
    pub mounted: bool,
    pub read_only: bool,
    /// Which plugging in of its disk this is.
    pub plugged_in: Option<String>,
}

impl Drive {
    /// Whether its file system has hard links, which versions need.
    pub fn links(&self) -> bool {
        self.filesystem == "ext4"
    }
}

#[async_trait]
pub trait Drives: Send + Sync {
    /// The drives plugged in; mounting those that can be.
    async fn drives(&self, context: &RequestContext) -> Result<Vec<Drive>, PortError>;
}

/// A place as the host reaches it: a folder beneath a share's root or a
/// drive's, or of an rsync server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Share {
        volume: u16,
        share: String,
        path: String,
    },
    Drive {
        drive: String,
        path: String,
    },
    Remote {
        server: Server,
        path: String,
    },
    /// A folder of a cloud account.
    Cloud {
        provider: Provider,
        account: i64,
        path: String,
    },
}

impl Location {
    pub fn path(&self) -> &str {
        match self {
            Self::Share { path, .. }
            | Self::Drive { path, .. }
            | Self::Remote { path, .. }
            | Self::Cloud { path, .. } => path,
        }
    }

    /// The folder `name` (a path of names) in this one.
    pub fn join(&self, name: &str) -> Self {
        let join = |path: &str| match (path, name) {
            (path, "") => path.to_owned(),
            ("", name) => name.to_owned(),
            ("/", name) => format!("/{name}"),
            (path, name) => format!("{path}/{name}"),
        };
        match self {
            Self::Share { volume, share, path } => {
                Self::Share { volume: *volume, share: share.clone(), path: join(path) }
            }
            Self::Drive { drive, path } => Self::Drive { drive: drive.clone(), path: join(path) },
            Self::Remote { server, path } => Self::Remote { server: server.clone(), path: join(path) },
            Self::Cloud { provider, account, path } => {
                Self::Cloud { provider: *provider, account: *account, path: join(path) }
            }
        }
    }

    pub fn server(&self) -> Option<&Server> {
        match self {
            Self::Remote { server, .. } => Some(server),
            _ => None,
        }
    }
}

/// How a run writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Into `current/`; with `keep_changed`, what it deletes or replaces
    /// goes to `removed/<stamp>/`.
    Mirror { keep_changed: bool },
    /// Into `<stamp>.incomplete/`, unchanged files linked to the newest
    /// complete version, renamed `<stamp>` once complete.
    Versioned,
}

/// One run, as the host carries it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    pub task: i64,
    pub source: Location,
    /// The task's folder in its target.
    pub target: Location,
    pub mode: RunMode,
    /// The run's time as a version's name.
    pub stamp: String,
    pub filters: Filters,
    /// KiB/s, 0 for none.
    pub bwlimit_kib: u32,
    /// Through rclone's crypt (its secrets come apart).
    pub encrypted: bool,
}

/// What a run did.
#[derive(Debug, Clone)]
pub struct RunReport {
    pub files: u64,
    pub bytes: u64,
    /// What could not be read, if anything (the run is complete but for it).
    pub partial: Option<String>,
    pub transfers: Vec<Transfer>,
    pub transfers_total: u64,
    /// The task's folder after the run, for retention.
    pub entries: Vec<Entry>,
    /// A cloud's token rclone refreshed meanwhile, to keep.
    pub token: Option<Secret>,
}

/// A folder's entries, and a cloud's token refreshed meanwhile.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub token: Option<Secret>,
}

/// A restore, as the host carries it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub task: i64,
    /// The version (or a mirror's copy) restored from: a folder in the
    /// task's folder.
    pub from: Location,
    /// What of it, as paths in it; none for all.
    pub paths: Vec<String>,
    /// A folder on the NAS.
    pub to: Location,
    pub existing: Existing,
    pub encrypted: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub files: u64,
    pub bytes: u64,
    pub partial: Option<String>,
    pub token: Option<Secret>,
}

/// What the host offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub rsync: bool,
    pub ssh: bool,
    /// rclone, for clouds and encryption.
    pub rclone: bool,
    /// The NAS's own public key for SSH servers, once made.
    pub public_key: Option<String>,
}

/// A host key an SSH server offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// `ssh-ed25519 AAAA…`
    pub line: String,
    /// `SHA256:…`, as `ssh` shows it.
    pub fingerprint: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host would not, in its words.
    #[error("{0}")]
    Refused(String),
    /// It tried and failed, in its words.
    #[error("{0}")]
    Failed(String),
    /// The SSH server's host key is not the one pinned.
    #[error("the server's host key changed: {0}")]
    HostKeyChanged(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Where a run's steps go.
pub trait Steps: Send + Sync {
    fn step(&self, step: &str, percent: Option<u8>);
}

/// rsync, the SSH client and the NAS's key, through the system agent.
#[async_trait]
pub trait BackupHost: Send + Sync {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError>;
    /// Installs rsync and the SSH client from Debian, waiting for apt.
    async fn install(&self, context: &RequestContext) -> Result<(), HostError>;
    /// Makes the NAS's key pair if it has none; its public key.
    async fn make_key(&self, context: &RequestContext) -> Result<String, HostError>;
    async fn host_keys(
        &self,
        host: &str,
        port: u16,
        context: &RequestContext,
    ) -> Result<Vec<HostKey>, HostError>;
    /// The entries of the folder `inside` (names separated by `/`) of `at`;
    /// none if it is not there. With a crypt among `secrets`, `at` is the
    /// task's folder, where the encryption begins: what is inside is read
    /// decrypted.
    async fn list(
        &self,
        at: &Location,
        inside: &str,
        secrets: &Secrets,
        context: &RequestContext,
    ) -> Result<Listing, HostError>;
    async fn run(
        &self,
        plan: &RunPlan,
        secrets: &Secrets,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<RunReport, HostError>;
    /// Stops a task's run; none running is no failure.
    async fn stop(&self, task: i64, context: &RequestContext) -> Result<(), HostError>;
    /// Removes versions (and kept changes) of task `task`'s folder `at` by
    /// their names in it.
    async fn prune(
        &self,
        task: i64,
        at: &Location,
        names: &[String],
        secrets: &Secrets,
        context: &RequestContext,
    ) -> Result<Option<Secret>, HostError>;
    async fn restore(
        &self,
        plan: &RestorePlan,
        secrets: &Secrets,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<RestoreReport, HostError>;
}
