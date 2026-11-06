//! The catalogs: which sources there are, and the apps they offer.

use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use super::ports::{CatalogError, CatalogReader, SourceRepository};
use crate::domain::{CatalogApp, InvalidSource, Refresh, Source, index_address};

/// An app with the source that offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedApp {
    pub source: Source,
    pub app: CatalogApp,
}

#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("there is no such source")]
    SourceNotFound,
    #[error("there is no such app")]
    AppNotFound,
    #[error(transparent)]
    InvalidSource(#[from] InvalidSource),
    #[error("this catalog is there already")]
    Duplicate,
    #[error("the community catalog is built in")]
    Official,
    /// The address answered with something that is no catalog.
    #[error("not a catalog: {0}")]
    NotACatalog(String),
    #[error("cannot reach the catalog: {0}")]
    Unreachable(String),
    #[error("there is no such pool")]
    PoolNotFound,
    #[error("the app is installed already")]
    AlreadyInstalled,
    #[error("the app is not installed")]
    NotInstalled,
    /// Its account name, `app-<id>`, would be too long.
    #[error("the app's id is too long to install it")]
    IdTooLong,
    #[error("Docker is not set up and running on a pool")]
    DockerNotReady,
    #[error("Docker is installed already")]
    DockerInstalled,
    #[error("the app's permissions were not confirmed")]
    PermissionsNotConfirmed,
    #[error(transparent)]
    InvalidValue(#[from] crate::domain::InvalidValue),
    /// A job is working on the app, or it is not in a state for this.
    #[error("the app is busy or not ready for this")]
    Busy,
    #[error("the confirmation does not match the app's id")]
    ConfirmationMismatch,
    #[error("the app is up to date")]
    UpToDate,
    /// Docker or another program on the host failed, in its words.
    #[error("{0}")]
    HostFailed(String),
    /// The agent's checks refused, with its reason.
    #[error("refused by the host: {0}")]
    Refused(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<CatalogError> for AppsError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::Unreachable(reason) => AppsError::Unreachable(reason),
            CatalogError::NotAnIndex(reason) => AppsError::NotACatalog(reason),
        }
    }
}

#[derive(Clone)]
pub struct Catalogs {
    sources: Arc<dyn SourceRepository>,
    reader: Arc<dyn CatalogReader>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl Catalogs {
    pub fn new(
        sources: Arc<dyn SourceRepository>,
        reader: Arc<dyn CatalogReader>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { sources, reader, audit, clock }
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    pub async fn sources(&self) -> Result<Vec<Source>, AppsError> {
        Ok(self.sources.list().await?)
    }

    async fn source(&self, id: i64) -> Result<Source, AppsError> {
        self.sources.list().await?.into_iter().find(|s| s.id == id).ok_or(AppsError::SourceNotFound)
    }

    /// Adds a catalog: a GitHub repository or an index address. It must
    /// answer with an index this version can read.
    pub async fn add_source(
        &self,
        typed: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Source, AppsError> {
        let address = index_address(typed)?;
        if self.sources.list().await?.iter().any(|s| s.index == address) {
            return Err(AppsError::Duplicate);
        }
        let text = self.reader.fetch(&address).await?;
        let read = self.reader.read(&text)?;
        let now = self.clock.now();
        let source = self.sources.add(&address, now).await?.ok_or(AppsError::Duplicate)?;
        let refresh = Refresh { at: now, error: None, skipped: read.skipped.len() };
        self.sources.refreshed(source.id, &refresh, Some(&text)).await?;
        self.audit("apps.source_add", Outcome::Success, &address, actor, request).await;
        Ok(Source { refreshed: Some(refresh), ..source })
    }

    pub async fn remove_source(
        &self,
        id: i64,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), AppsError> {
        let source = self.source(id).await?;
        if source.official {
            return Err(AppsError::Official);
        }
        if !self.sources.remove(id).await? {
            return Err(AppsError::SourceNotFound);
        }
        self.audit("apps.source_remove", Outcome::Success, &source.index, actor, request).await;
        Ok(())
    }

    /// Reads every source's index again. A source that fails keeps its
    /// apps from before; the failure is recorded with it.
    pub async fn refresh(&self) -> Result<Vec<Source>, AppsError> {
        for source in self.sources.list().await? {
            let fetched = match self.reader.fetch(&source.index).await {
                Ok(text) => self.reader.read(&text).map(|read| (text, read)),
                Err(error) => Err(error),
            };
            let at = self.clock.now();
            let recorded = match &fetched {
                Ok((text, read)) => {
                    for skipped in &read.skipped {
                        tracing::warn!(
                            source = source.index,
                            app = skipped.app,
                            reason = skipped.reason,
                            "apps: left out"
                        );
                    }
                    let refresh = Refresh { at, error: None, skipped: read.skipped.len() };
                    self.sources.refreshed(source.id, &refresh, Some(text)).await
                }
                Err(error) => {
                    tracing::warn!(source = source.index, %error, "apps: cannot read the catalog");
                    let refresh = Refresh { at, error: Some(error.to_string()), skipped: 0 };
                    self.sources.refreshed(source.id, &refresh, None).await
                }
            };
            if let Err(error) = recorded {
                tracing::warn!(%error, "apps: cannot record a refresh");
            }
        }
        self.sources().await
    }

    /// Every app of every source, by name; an app in two catalogs is
    /// listed twice, with its source.
    pub async fn apps(&self) -> Result<Vec<ListedApp>, AppsError> {
        let mut listed = Vec::new();
        for source in self.sources.list().await? {
            let Some(text) = self.sources.index(source.id).await? else { continue };
            match self.reader.read(&text) {
                Ok(read) => {
                    listed.extend(read.apps.into_iter().map(|app| ListedApp { source: source.clone(), app }))
                }
                Err(error) => {
                    tracing::warn!(source = source.index, %error, "apps: the kept catalog no longer reads")
                }
            }
        }
        listed.sort_by(|a, b| {
            (a.app.name.to_lowercase(), a.source.id).cmp(&(b.app.name.to_lowercase(), b.source.id))
        });
        Ok(listed)
    }

    pub async fn app(&self, source: i64, id: &str) -> Result<ListedApp, AppsError> {
        let source = self.source(source).await?;
        let text = self.sources.index(source.id).await?.ok_or(AppsError::AppNotFound)?;
        let app =
            self.reader.read(&text)?.apps.into_iter().find(|a| a.id == id).ok_or(AppsError::AppNotFound)?;
        Ok(ListedApp { source, app })
    }

    /// The app's icon: kept for its digest, fetched once.
    pub async fn icon(&self, source: i64, id: &str) -> Result<Vec<u8>, AppsError> {
        let ListedApp { source, app } = self.app(source, id).await?;
        if let Some(png) = self.sources.icon(source.id, &app.id, &app.digest).await? {
            return Ok(png);
        }
        let png = self.reader.fetch_icon(&source.index, &app.icon).await?;
        self.sources.store_icon(source.id, &app.id, &app.digest, &png).await?;
        Ok(png)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::domain::OFFICIAL_INDEX;
    use crate::testing::{FakeReader, MemorySources, app};

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_793_836_800)
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<&'static str>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(entry.action);
            Ok(())
        }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    fn setup() -> (Catalogs, Arc<FakeReader>, Arc<Audit>) {
        let reader = Arc::new(FakeReader::default());
        reader.serve(OFFICIAL_INDEX, vec![app("jellyfin", "Jellyfin"), app("immich", "Immich")]);
        let audit = Arc::new(Audit::default());
        let catalogs =
            Catalogs::new(Arc::new(MemorySources::default()), reader.clone(), audit.clone(), Arc::new(Fixed));
        (catalogs, reader, audit)
    }

    #[tokio::test]
    async fn the_community_catalog_is_there_and_stays() {
        let (catalogs, _, _) = setup();
        catalogs.refresh().await.unwrap();

        let names: Vec<String> = catalogs.apps().await.unwrap().into_iter().map(|l| l.app.name).collect();
        assert_eq!(names, ["Immich", "Jellyfin"]);
        let official = catalogs.sources().await.unwrap()[0].clone();
        assert!(official.official);
        let removed = catalogs.remove_source(official.id, &alice(), &RequestContext::default()).await;
        assert!(matches!(removed, Err(AppsError::Official)), "{removed:?}");
    }

    #[tokio::test]
    async fn sources_are_added_like_hacs_and_must_be_catalogs() {
        let (catalogs, reader, audit) = setup();
        let request = RequestContext::default();
        let pages = "https://friend.github.io/nas-apps/v1/index.json";
        reader.serve(pages, vec![app("jellyfin", "Jellyfin"), app("paperless", "Paperless")]);

        let added = catalogs.add_source("github.com/Friend/nas-apps", &alice(), &request).await.unwrap();
        assert_eq!((added.index.as_str(), added.official), (pages, false));
        let again =
            catalogs.add_source("https://friend.github.io/nas-apps/v1/index.json", &alice(), &request).await;
        assert!(matches!(again, Err(AppsError::Duplicate)), "{again:?}");
        let nowhere = catalogs.add_source("https://nowhere.example/index.json", &alice(), &request).await;
        assert!(matches!(nowhere, Err(AppsError::Unreachable(_))), "{nowhere:?}");

        catalogs.refresh().await.unwrap();
        let listed = catalogs.apps().await.unwrap();
        let jellyfins: Vec<bool> =
            listed.iter().filter(|l| l.app.id == "jellyfin").map(|l| l.source.official).collect();
        assert_eq!(jellyfins, [true, false], "listed once per catalog");

        catalogs.remove_source(added.id, &alice(), &request).await.unwrap();
        assert!(catalogs.apps().await.unwrap().iter().all(|l| l.source.official));
        assert_eq!(*audit.0.lock().unwrap(), ["apps.source_add", "apps.source_remove"]);
    }

    #[tokio::test]
    async fn a_catalog_that_fails_keeps_its_apps_and_says_why() {
        let (catalogs, reader, _) = setup();
        catalogs.refresh().await.unwrap();
        reader.stop(OFFICIAL_INDEX);

        let sources = catalogs.refresh().await.unwrap();

        assert!(sources[0].refreshed.as_ref().unwrap().error.is_some());
        assert_eq!(catalogs.apps().await.unwrap().len(), 2, "the apps from before");
    }

    #[tokio::test]
    async fn icons_are_fetched_once_per_digest() {
        let (catalogs, reader, _) = setup();
        catalogs.refresh().await.unwrap();
        let source = catalogs.sources().await.unwrap()[0].id;

        let first = catalogs.icon(source, "jellyfin").await.unwrap();
        let second = catalogs.icon(source, "jellyfin").await.unwrap();

        assert_eq!(first, second);
        assert_eq!(reader.icons_fetched(), 1);
        assert!(matches!(catalogs.icon(source, "nothing").await, Err(AppsError::AppNotFound)));
    }
}
