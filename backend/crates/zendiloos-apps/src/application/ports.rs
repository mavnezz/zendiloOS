//! What this context needs from the outside world.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};

use crate::domain::{CatalogApp, Refresh, Source};

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
