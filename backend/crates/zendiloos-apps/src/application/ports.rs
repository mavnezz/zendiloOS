//! What this context needs from the outside world.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::domain::{AppVolume, CatalogApp, Containers, DockerHostState, InstalledApp, Refresh, Source};

/// An app an index offered that failed the format or the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub app: String,
    pub reason: String,
}

/// An index, read and checked: what may be shown, and what was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadIndex {
    pub apps: Vec<CatalogApp>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// The address did not answer as it should.
    #[error("{0}")]
    Unreachable(String),
    /// What came back is no index this version can read.
    #[error("{0}")]
    NotAnIndex(String),
}

/// Catalogs on the network, read with the catalog's own checks.
#[async_trait]
pub trait CatalogReader: Send + Sync {
    /// The text of the index at `address`.
    async fn fetch(&self, address: &str) -> Result<String, CatalogError>;

    /// The apps in an index's text that pass the format and the policy.
    fn read(&self, text: &str) -> Result<ReadIndex, CatalogError>;

    /// The icon `icon` (relative to the index at `address`), checked to be
    /// a PNG of the allowed size.
    async fn fetch_icon(&self, address: &str, icon: &str) -> Result<Vec<u8>, CatalogError>;
}

#[async_trait]
pub trait SourceRepository: Send + Sync {
    /// The community catalog first, then by when they were added.
    async fn list(&self) -> Result<Vec<Source>, PortError>;

    /// `None` if a source with this index exists.
    async fn add(&self, index: &str, at: Timestamp) -> Result<Option<Source>, PortError>;

    /// `false` if there is no such source.
    async fn remove(&self, id: i64) -> Result<bool, PortError>;

    /// The index text last read from the source.
    async fn index(&self, id: i64) -> Result<Option<String>, PortError>;

    /// Records a fetch; `index` replaces the kept text when given, so a
    /// failed fetch keeps showing the apps read before.
    async fn refreshed(&self, id: i64, refresh: &Refresh, index: Option<&str>) -> Result<(), PortError>;

    /// A cached icon, for the app's digest.
    async fn icon(&self, id: i64, app: &str, digest: &str) -> Result<Option<Vec<u8>>, PortError>;

    async fn store_icon(&self, id: i64, app: &str, digest: &str, png: &[u8]) -> Result<(), PortError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    /// The agent's checks refused, with its reason.
    #[error("refused: {0}")]
    Refused(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Docker on the host, through the agent.
#[async_trait]
pub trait DockerHost: Send + Sync {
    async fn state(&self, context: &RequestContext) -> Result<DockerHostState, PortError>;

    /// Puts Docker's data on the volume; Docker restarts.
    async fn enable(&self, volume: &AppVolume, context: &RequestContext) -> Result<(), DockerError>;

    /// The pools zendiloos created.
    async fn volumes(&self, context: &RequestContext) -> Result<Vec<AppVolume>, PortError>;
}

/// The apps installed, as this context records them.
#[async_trait]
pub trait InstalledRepository: Send + Sync {
    /// By id.
    async fn list(&self) -> Result<Vec<InstalledApp>, PortError>;

    async fn find(&self, id: &str) -> Result<Option<InstalledApp>, PortError>;

    /// `false` if an app with this id is installed already.
    async fn create(&self, app: &InstalledApp) -> Result<bool, PortError>;

    async fn update(&self, app: &InstalledApp) -> Result<(), PortError>;

    async fn delete(&self, id: &str) -> Result<(), PortError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AppHostError {
    /// The agent's checks refused, with its reason.
    #[error("refused: {0}")]
    Refused(String),
    /// Docker or another program failed, in its words.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// A share picked for one of the app's shares, as the host finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShare {
    pub volume: u16,
    pub name: String,
}

/// The values for the agent: shares with where they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostValues {
    pub shares: BTreeMap<String, HostShare>,
    pub ports: BTreeMap<String, u16>,
    pub settings: BTreeMap<String, String>,
}

/// Apps on the host, through the agents: each step of installing,
/// running and removing one. The agent checks the app against the
/// catalog's policy again in [`AppHost::configure`].
#[async_trait]
pub trait AppHost: Send + Sync {
    /// The app's dataset on `volume` and its account.
    async fn prepare(
        &self,
        app: &str,
        volume: &AppVolume,
        context: &RequestContext,
    ) -> Result<(), AppHostError>;

    /// Checks the app with `values` and keeps what Docker will run; the
    /// folders below its data folder the containers mount.
    async fn configure(
        &self,
        app: &str,
        volume: u16,
        manifest: &str,
        compose: &str,
        values: &HostValues,
        context: &RequestContext,
    ) -> Result<Vec<String>, AppHostError>;

    async fn make_folders(
        &self,
        app: &str,
        volume: u16,
        folders: &[String],
        context: &RequestContext,
    ) -> Result<(), AppHostError>;

    async fn pull(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError>;

    async fn start(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError>;

    async fn stop(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError>;

    /// Containers, images, what Docker ran, and the dataset with the data.
    async fn remove(
        &self,
        app: &str,
        volume: &AppVolume,
        context: &RequestContext,
    ) -> Result<(), AppHostError>;

    async fn delete_account(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError>;

    /// The installed apps' containers, by app id.
    async fn status(&self, context: &RequestContext) -> Result<BTreeMap<String, Containers>, AppHostError>;

    async fn logs(&self, app: &str, lines: u32, context: &RequestContext) -> Result<String, AppHostError>;
}

/// A share apps can be given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareChoice {
    pub name: String,
    pub volume: u16,
}

/// The shares, from the Sharing context.
#[async_trait]
pub trait AppShares: Send + Sync {
    async fn shares(&self) -> Result<Vec<ShareChoice>, PortError>;

    /// Gives the app's `account` exactly `grants` (share name, write), on
    /// what is in the shares too; shares not named lose its access.
    async fn grant(
        &self,
        account: &str,
        grants: &[(String, bool)],
        context: &RequestContext,
    ) -> Result<(), AppHostError>;
}

/// Ports something on the NAS listens on.
#[async_trait]
pub trait ListeningPorts: Send + Sync {
    async fn listening(&self) -> Result<BTreeSet<u16>, PortError>;
}

/// New secrets for settings zendiloOS generates.
pub trait Secrets: Send + Sync {
    /// 32 letters and digits.
    fn generate(&self) -> String;
}
