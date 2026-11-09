//! What this context needs from the outside world.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use zendiloos_kernel::{Actor, PortError, RequestContext, Timestamp};

use crate::domain::{
    AppVolume, CatalogApp, Containers, DockerHostState, HostFacts, InstalledApp, Refresh, SecretAlphabet,
    Source,
};

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
    /// A program on the host failed, in its words (apt's last lines).
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Docker on the host, through the agent.
#[async_trait]
pub trait DockerHost: Send + Sync {
    async fn state(&self, context: &RequestContext) -> Result<DockerHostState, PortError>;

    /// Installs Docker from Debian (`docker.io`, `docker-compose`) and
    /// returns once apt is done; it may take minutes.
    async fn install(&self, context: &RequestContext) -> Result<(), DockerError>;

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

/// Apps installed the earlier way, on the host through the agent: running
/// and removing one.
#[async_trait]
pub trait AppHost: Send + Sync {
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

/// What apps installed the earlier way may do on shares, in the Sharing
/// context.
#[async_trait]
pub trait AppShares: Send + Sync {
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
    /// `length` characters of `alphabet`.
    fn generate(&self, length: u8, alphabet: SecretAlphabet) -> String;
}

/// A share as the administrator who acts may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsableShare {
    pub name: String,
    pub volume: u16,
    pub writable: bool,
}

/// The shares an administrator may use, with their own permissions, as
/// the file manager has them: an app made from a template runs as them.
#[async_trait]
pub trait UsableShares: Send + Sync {
    async fn usable(&self, actor: &Actor) -> Result<Vec<UsableShare>, PortError>;
}

/// What the machine says for filling in a template, through the agent.
#[async_trait]
pub trait TemplateHost: Send + Sync {
    /// The IDs of `user`, the time zone, and which of `devices` are there.
    async fn facts(
        &self,
        user: &str,
        devices: &[String],
        context: &RequestContext,
    ) -> Result<HostFacts, AppHostError>;
}

/// Compose files filled in.
pub trait ComposeFiller: Send + Sync {
    /// `compose` with every value interpolated with `variables` (as
    /// [`crate::domain::interpolate`] does) and written so that Compose
    /// reads it back as that value; the devices in `absent` left out. The
    /// rest stays as written, comments too. Why it cannot, otherwise.
    fn fill(
        &self,
        compose: &str,
        variables: &BTreeMap<String, String>,
        absent: &[String],
    ) -> Result<String, String>;
}

/// The project a template is filled in for: its name and its folder in a
/// share, where Compose reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplatePlace {
    pub project: String,
    pub share_id: i64,
    /// In the share: `docker/jellyfin`.
    pub folder: String,
}

/// The catalog's policy, on what Compose makes of an app's template: the
/// checks the catalog's own validator runs, by the catalog's own code.
#[async_trait]
pub trait TemplatePolicy: Send + Sync {
    /// What in `app`'s template its catalog's policy forbids, one line
    /// each; none if it keeps to it. The template is filled in with the
    /// catalog's stand-in values, which show where each value ends up, and
    /// read by Compose as a project at `place` of `actor`'s would be.
    async fn problems(
        &self,
        app: &CatalogApp,
        place: &TemplatePlace,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<String>, PolicyError>;
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// The place is no folder of a share the administrator may write, or
    /// the project's name no name Compose allows.
    #[error("{0}")]
    Place(String),
    /// Compose cannot read the template, in its words.
    #[error("{0}")]
    Unreadable(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// An administrator's project made from an app of a catalog, as Docker
/// has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateProject {
    /// The Compose project.
    pub name: String,
    pub source: i64,
    pub app: String,
    pub containers: Containers,
    /// What its containers publish: (container port, host port).
    pub published: Vec<(u16, u16)>,
    pub created_at: Timestamp,
}

/// The projects made from apps of a catalog, from the Containers context.
#[async_trait]
pub trait AppProjects: Send + Sync {
    async fn projects(&self, context: &RequestContext) -> Result<Vec<TemplateProject>, PortError>;
}
