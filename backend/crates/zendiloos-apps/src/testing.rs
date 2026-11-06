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
        version: Some("1.0".into()),
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
        manifest: format!(r#"{{"id":"{id}"}}"#),
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

/// Docker installed (unless [`FakeDocker::missing`]), its data nowhere
/// yet; pool `tank` as volume 1.
pub struct FakeDocker {
    data_root: Mutex<Option<String>>,
    installed: std::sync::atomic::AtomicBool,
}

impl Default for FakeDocker {
    fn default() -> Self {
        Self { data_root: Mutex::default(), installed: std::sync::atomic::AtomicBool::new(true) }
    }
}

#[async_trait]
impl crate::application::ports::DockerHost for FakeDocker {
    async fn state(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<crate::domain::DockerHostState, PortError> {
        let data_root = self.data_root.lock().unwrap().clone();
        let installed = self.installed.load(Ordering::Relaxed);
        let running = installed && data_root.is_some();
        let version = running.then(|| "26.1.5".to_owned());
        Ok(crate::domain::DockerHostState { installed, running, data_root, version })
    }

    async fn install(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::DockerError> {
        self.installed.store(true, Ordering::Relaxed);
        Ok(())
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
        Ok(vec![
            crate::domain::AppVolume { pool: "tank".into(), number: 1 },
            crate::domain::AppVolume { pool: "backup".into(), number: 2 },
        ])
    }
}

impl FakeDocker {
    /// Docker not installed yet.
    pub fn missing() -> Self {
        Self { data_root: Mutex::default(), installed: std::sync::atomic::AtomicBool::new(false) }
    }

    /// Docker runs with its data on `tank`.
    pub fn enabled() -> Self {
        Self { data_root: Mutex::new(Some("/volume1/@docker".into())), ..Self::default() }
    }
}

/// Installed apps in memory.
#[derive(Default)]
pub struct MemoryInstalled(Mutex<BTreeMap<String, crate::domain::InstalledApp>>);

#[async_trait]
impl crate::application::ports::InstalledRepository for MemoryInstalled {
    async fn list(&self) -> Result<Vec<crate::domain::InstalledApp>, PortError> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }

    async fn find(&self, id: &str) -> Result<Option<crate::domain::InstalledApp>, PortError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }

    async fn create(&self, app: &crate::domain::InstalledApp) -> Result<bool, PortError> {
        let mut apps = self.0.lock().unwrap();
        if apps.contains_key(&app.id) {
            return Ok(false);
        }
        apps.insert(app.id.clone(), app.clone());
        Ok(true)
    }

    async fn update(&self, app: &crate::domain::InstalledApp) -> Result<(), PortError> {
        self.0.lock().unwrap().insert(app.id.clone(), app.clone());
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), PortError> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

/// Records each step on the host; [`FakeAppHost::fail_at`] makes one fail.
#[derive(Default)]
pub struct FakeAppHost {
    calls: Mutex<Vec<String>>,
    failing: Mutex<Option<&'static str>>,
    running: Mutex<BTreeMap<String, bool>>,
}

impl FakeAppHost {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The step named `step` (`prepare`, `pull`, `start`, …) fails from now on;
    /// `None` lets every step work again.
    pub fn fail_at(&self, step: Option<&'static str>) {
        *self.failing.lock().unwrap() = step;
    }

    fn step(
        &self,
        step: &'static str,
        detail: String,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.calls.lock().unwrap().push(format!("{step} {detail}").trim_end().to_owned());
        if *self.failing.lock().unwrap() == Some(step) {
            return Err(crate::application::ports::AppHostError::Failed(format!("{step} went wrong")));
        }
        Ok(())
    }
}

#[async_trait]
impl crate::application::ports::AppHost for FakeAppHost {
    async fn prepare(
        &self,
        app: &str,
        volume: &crate::domain::AppVolume,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("prepare", format!("{app} {}/{}", volume.pool, volume.number))
    }

    async fn configure(
        &self,
        app: &str,
        volume: u16,
        _manifest: &str,
        _compose: &str,
        values: &crate::application::ports::HostValues,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<Vec<String>, crate::application::ports::AppHostError> {
        let shares: Vec<String> = values
            .shares
            .iter()
            .map(|(id, s)| {
                let folder = if s.folder.is_empty() { String::new() } else { format!(" in {}", s.folder) };
                format!("{id}=/volume{}/{}{folder}", s.volume, s.name)
            })
            .collect();
        let ports: Vec<String> = values.ports.iter().map(|(id, p)| format!("{id}={p}")).collect();
        self.step("configure", format!("{app} {volume} [{}] [{}]", shares.join(","), ports.join(",")))?;
        Ok(vec!["config".into()])
    }

    async fn make_folders(
        &self,
        app: &str,
        _: u16,
        folders: &[String],
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("folders", format!("{app} {}", folders.join(",")))
    }

    async fn pull(
        &self,
        app: &str,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("pull", app.to_owned())
    }

    async fn start(
        &self,
        app: &str,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("start", app.to_owned())?;
        self.running.lock().unwrap().insert(app.to_owned(), true);
        Ok(())
    }

    async fn stop(
        &self,
        app: &str,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("stop", app.to_owned())?;
        self.running.lock().unwrap().insert(app.to_owned(), false);
        Ok(())
    }

    async fn remove(
        &self,
        app: &str,
        _: &crate::domain::AppVolume,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("remove", app.to_owned())?;
        self.running.lock().unwrap().remove(app);
        Ok(())
    }

    async fn delete_account(
        &self,
        app: &str,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        self.step("delete_account", app.to_owned())
    }

    async fn status(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<BTreeMap<String, crate::domain::Containers>, crate::application::ports::AppHostError> {
        Ok(self
            .running
            .lock()
            .unwrap()
            .iter()
            .map(|(app, running)| {
                let containers = if *running {
                    crate::domain::Containers { running: 1, ..Default::default() }
                } else {
                    crate::domain::Containers { other: 1, ..Default::default() }
                };
                (app.clone(), containers)
            })
            .collect())
    }

    async fn logs(
        &self,
        app: &str,
        lines: u32,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<String, crate::application::ports::AppHostError> {
        Ok(format!("{app}: {lines} lines\n"))
    }
}

/// Shares `family` and `media` on volume 1; records what apps were given.
#[derive(Default)]
pub struct FakeShares {
    grants: Mutex<BTreeMap<String, Vec<(String, bool)>>>,
}

impl FakeShares {
    /// What `account` may use now.
    pub fn granted(&self, account: &str) -> Vec<(String, bool)> {
        self.grants.lock().unwrap().get(account).cloned().unwrap_or_default()
    }
}

#[async_trait]
impl crate::application::ports::AppShares for FakeShares {
    async fn shares(&self) -> Result<Vec<crate::application::ports::ShareChoice>, PortError> {
        Ok(["family", "media"]
            .map(|name| crate::application::ports::ShareChoice { name: name.into(), volume: 1 })
            .to_vec())
    }

    async fn grant(
        &self,
        account: &str,
        grants: &[(String, bool)],
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ports::AppHostError> {
        let mut all = self.grants.lock().unwrap();
        if grants.is_empty() {
            all.remove(account);
        } else {
            all.insert(account.to_owned(), grants.to_vec());
        }
        Ok(())
    }
}

/// Port 9000 is in use.
pub struct FakeListening;

#[async_trait]
impl crate::application::ports::ListeningPorts for FakeListening {
    async fn listening(&self) -> Result<std::collections::BTreeSet<u16>, PortError> {
        Ok(std::collections::BTreeSet::from([9000]))
    }
}

/// Counts up: `secret-1`, `secret-2`, …
#[derive(Default)]
pub struct CountingSecrets(AtomicUsize);

impl crate::application::ports::Secrets for CountingSecrets {
    fn generate(&self, _: u8, _: crate::domain::SecretAlphabet) -> String {
        format!("secret-{}", self.0.fetch_add(1, Ordering::Relaxed) + 1)
    }
}
