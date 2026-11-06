//! Sources in memory and a catalog reader that serves fixed apps, for
//! tests here and in the API crate (feature `testing`).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};

use crate::application::ports::{CatalogError, CatalogReader, ReadIndex, SourceRepository};
use crate::domain::{CatalogApp, OFFICIAL_INDEX, Permissions, Port, Refresh, Source};

/// A small app with one web port.
pub fn app(id: &str, name: &str) -> CatalogApp {
    CatalogApp {
        id: id.into(),
        name: name.into(),
        version: "1.0".into(),
        tagline: format!("{name} on the NAS"),
        description: format!("{name} does what it does."),
        categories: vec!["media".into()],
        developer: format!("{name} people"),
        website: format!("https://{id}.example"),
        source_code: format!("https://github.com/example/{id}"),
        license: "MIT".into(),
        architectures: vec!["amd64".into()],
        notes: None,
        web: None,
        ports: vec![Port { id: "web".into(), label: "Web interface".into(), default: 8096, fixed: false }],
        shares: vec![],
        settings: vec![],
        permissions: Permissions::default(),
        translations: BTreeMap::new(),
        compose: format!("services:\n  {id}:\n    image: example/{id}:1@sha256:00\n"),
        icon: format!("icons/{id}.png"),
        digest: format!("sha256:{id}"),
    }
}

/// The community catalog as source 1; others count up from 2.
pub struct MemorySources {
    sources: Mutex<Vec<Source>>,
    indexes: Mutex<BTreeMap<i64, String>>,
    icons: Mutex<BTreeMap<(i64, String, String), Vec<u8>>>,
}

impl Default for MemorySources {
    fn default() -> Self {
        let official = Source {
            id: 1,
            index: OFFICIAL_INDEX.into(),
            official: true,
            added_at: Timestamp::from_unix_seconds(0),
            refreshed: None,
        };
        Self { sources: Mutex::new(vec![official]), indexes: Mutex::default(), icons: Mutex::default() }
    }
}

#[async_trait]
impl SourceRepository for MemorySources {
    async fn list(&self) -> Result<Vec<Source>, PortError> {
        Ok(self.sources.lock().unwrap().clone())
    }

    async fn add(&self, index: &str, at: Timestamp) -> Result<Option<Source>, PortError> {
        let mut sources = self.sources.lock().unwrap();
        if sources.iter().any(|s| s.index == index) {
            return Ok(None);
        }
        let id = sources.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        let source = Source { id, index: index.into(), official: false, added_at: at, refreshed: None };
        sources.push(source.clone());
        Ok(Some(source))
    }

    async fn remove(&self, id: i64) -> Result<bool, PortError> {
        let mut sources = self.sources.lock().unwrap();
        let before = sources.len();
        sources.retain(|s| s.id != id);
        self.indexes.lock().unwrap().remove(&id);
        Ok(sources.len() < before)
    }

    async fn index(&self, id: i64) -> Result<Option<String>, PortError> {
        Ok(self.indexes.lock().unwrap().get(&id).cloned())
    }

    async fn refreshed(&self, id: i64, refresh: &Refresh, index: Option<&str>) -> Result<(), PortError> {
        if let Some(source) = self.sources.lock().unwrap().iter_mut().find(|s| s.id == id) {
            source.refreshed = Some(refresh.clone());
        }
        if let Some(index) = index {
            self.indexes.lock().unwrap().insert(id, index.to_owned());
        }
        Ok(())
    }

    async fn icon(&self, id: i64, app: &str, digest: &str) -> Result<Option<Vec<u8>>, PortError> {
        Ok(self.icons.lock().unwrap().get(&(id, app.to_owned(), digest.to_owned())).cloned())
    }

    async fn store_icon(&self, id: i64, app: &str, digest: &str, png: &[u8]) -> Result<(), PortError> {
        self.icons.lock().unwrap().insert((id, app.to_owned(), digest.to_owned()), png.to_vec());
        Ok(())
    }
}

/// Serves fixed apps per address; the index text is the address itself.
#[derive(Default)]
pub struct FakeReader {
    served: Mutex<BTreeMap<String, Vec<CatalogApp>>>,
    stopped: Mutex<Vec<String>>,
    icons: AtomicUsize,
}

impl FakeReader {
    pub fn serve(&self, address: &str, apps: Vec<CatalogApp>) {
        self.served.lock().unwrap().insert(address.to_owned(), apps);
        self.stopped.lock().unwrap().retain(|a| a != address);
    }

    /// The address stops answering.
    pub fn stop(&self, address: &str) {
        self.stopped.lock().unwrap().push(address.to_owned());
    }

    pub fn icons_fetched(&self) -> usize {
        self.icons.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl CatalogReader for FakeReader {
    async fn fetch(&self, address: &str) -> Result<String, CatalogError> {
        if self.stopped.lock().unwrap().iter().any(|a| a == address)
            || !self.served.lock().unwrap().contains_key(address)
        {
            return Err(CatalogError::Unreachable(format!("{address} does not answer")));
        }
        Ok(address.to_owned())
    }

    fn read(&self, text: &str) -> Result<ReadIndex, CatalogError> {
        let apps = self
            .served
            .lock()
            .unwrap()
            .get(text)
            .cloned()
            .ok_or_else(|| CatalogError::NotAnIndex("no index".into()))?;
        Ok(ReadIndex { apps, skipped: vec![] })
    }

    async fn fetch_icon(&self, _: &str, icon: &str) -> Result<Vec<u8>, CatalogError> {
        self.icons.fetch_add(1, Ordering::Relaxed);
        Ok([b"\x89PNG\r\n\x1a\n".as_slice(), icon.as_bytes()].concat())
    }
}

/// Docker installed, its data nowhere yet; pool `tank` as volume 1.
#[derive(Default)]
pub struct FakeDocker {
    data_root: Mutex<Option<String>>,
}

#[async_trait]
impl crate::application::ports::DockerHost for FakeDocker {
    async fn state(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<crate::domain::DockerHostState, PortError> {
        let data_root = self.data_root.lock().unwrap().clone();
        Ok(crate::domain::DockerHostState { installed: true, running: data_root.is_some(), data_root })
    }

    async fn enable(
        &self,
        volume: &crate::domain::AppVolume,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::DockerError> {
        *self.data_root.lock().unwrap() = Some(format!("/volume{}/@docker", volume.number));
        Ok(())
    }

    async fn volumes(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<Vec<crate::domain::AppVolume>, PortError> {
        Ok(vec![crate::domain::AppVolume { pool: "tank".into(), number: 1 }])
    }
}
