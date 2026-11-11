//! In-memory adapters for tests, here and in the API crate.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, PortError, RequestContext};

use crate::application::{
    ConsoleSession, HostError, HostFacts, IsoError, IsoFiles, IsoSource, Steps, StoragePlace, VmHost, VmIds,
    VmImageRepository, VmRepository, Vms, VmsPorts,
};
use crate::domain::{HostLimits, IsoChoice, PowerState, Vm, VmAction, VmCounters, VmImage};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct MemoryVms {
    vms: Mutex<Vec<Vm>>,
}

#[async_trait]
impl VmRepository for MemoryVms {
    async fn list(&self) -> Result<Vec<Vm>, PortError> {
        Ok(lock(&self.vms).clone())
    }

    async fn find(&self, id: &str) -> Result<Option<Vm>, PortError> {
        Ok(lock(&self.vms).iter().find(|vm| vm.id == id).cloned())
    }

    async fn create(&self, vm: &Vm) -> Result<bool, PortError> {
        let mut vms = lock(&self.vms);
        let name = vm.settings.name.to_lowercase();
        if vms.iter().any(|other| other.id == vm.id || other.settings.name.to_lowercase() == name) {
            return Ok(false);
        }
        vms.push(vm.clone());
        Ok(true)
    }

    async fn set_ready(&self, id: &str) -> Result<(), PortError> {
        if let Some(vm) = lock(&self.vms).iter_mut().find(|vm| vm.id == id) {
            vm.ready = true;
        }
        Ok(())
    }

    async fn set_autostart(&self, id: &str, autostart: bool) -> Result<bool, PortError> {
        Ok(lock(&self.vms)
            .iter_mut()
            .find(|vm| vm.id == id)
            .map(|vm| vm.settings.autostart = autostart)
            .is_some())
    }

    async fn update(&self, vm: &Vm) -> Result<bool, PortError> {
        let mut vms = lock(&self.vms);
        let name = vm.settings.name.to_lowercase();
        if vms.iter().any(|other| other.id != vm.id && other.settings.name.to_lowercase() == name) {
            return Ok(false);
        }
        match vms.iter_mut().find(|other| other.id == vm.id) {
            Some(other) => {
                *other = vm.clone();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let mut vms = lock(&self.vms);
        let before = vms.len();
        vms.retain(|vm| vm.id != id);
        Ok(vms.len() < before)
    }
}

#[derive(Default)]
pub struct MemoryImages {
    images: Mutex<Vec<VmImage>>,
}

impl MemoryImages {
    /// The library with `images` in it.
    pub fn with(images: Vec<VmImage>) -> Self {
        Self { images: Mutex::new(images) }
    }
}

#[async_trait]
impl VmImageRepository for MemoryImages {
    async fn list(&self) -> Result<Vec<VmImage>, PortError> {
        Ok(lock(&self.images).clone())
    }

    async fn find(&self, id: &str) -> Result<Option<VmImage>, PortError> {
        Ok(lock(&self.images).iter().find(|image| image.id == id).cloned())
    }

    async fn create(&self, image: &VmImage) -> Result<bool, PortError> {
        let mut images = lock(&self.images);
        let name = image.name.to_lowercase();
        if images.iter().any(|other| other.id == image.id || other.name.to_lowercase() == name) {
            return Ok(false);
        }
        images.push(image.clone());
        Ok(true)
    }

    async fn set_ready(&self, id: &str, bytes: u64) -> Result<(), PortError> {
        if let Some(image) = lock(&self.images).iter_mut().find(|image| image.id == id) {
            image.ready = true;
            image.bytes = Some(bytes);
        }
        Ok(())
    }

    async fn set_bytes(&self, id: &str, bytes: u64) -> Result<(), PortError> {
        if let Some(image) = lock(&self.images).iter_mut().find(|image| image.id == id) {
            image.bytes = Some(bytes);
        }
        Ok(())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<bool, PortError> {
        let mut images = lock(&self.images);
        if images.iter().any(|other| other.id != id && other.name.to_lowercase() == name.to_lowercase()) {
            return Ok(false);
        }
        Ok(images.iter_mut().find(|image| image.id == id).map(|image| image.name = name.to_owned()).is_some())
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let mut images = lock(&self.images);
        let before = images.len();
        images.retain(|image| image.id != id);
        Ok(images.len() < before)
    }
}

/// libvirt and the storage pretended: KVM there, nothing installed at
/// first, the pools `tank` (volume 1) and `cold` (2), a bridge `br0`, two
/// threads and 2 GiB.
pub struct FakeVmHost {
    pub facts: Mutex<HostFacts>,
    pub domains: Mutex<BTreeMap<String, PowerState>>,
    /// The library's files and their sizes.
    pub files: Mutex<BTreeMap<String, u64>>,
    /// What each running VM's counters say: CPU time grows by a second of
    /// one vCPU at each look.
    pub looks: AtomicU64,
    /// What was asked, one word each: `install`, `define <id>`, `start <id>`, …
    pub calls: Mutex<Vec<String>>,
    /// The next definition fails with this.
    pub fail_define: Mutex<Option<String>>,
    /// Uploads under way: what is written, and the bytes announced.
    uploads: Mutex<BTreeMap<String, (std::path::PathBuf, u64)>>,
}

impl Default for FakeVmHost {
    fn default() -> Self {
        Self {
            facts: Mutex::new(HostFacts {
                kvm: true,
                cpu_virtualization: Some("vmx".into()),
                iommu: false,
                installed: false,
                limits: HostLimits { cores: 2, memory_bytes: 2 << 30, bridges: vec!["br0".into()] },
                storage: None,
            }),
            domains: Mutex::default(),
            files: Mutex::default(),
            looks: AtomicU64::new(0),
            calls: Mutex::default(),
            fail_define: Mutex::default(),
            uploads: Mutex::default(),
        }
    }
}

impl FakeVmHost {
    /// Installed and set up on `tank`.
    pub fn ready() -> Self {
        let host = Self::default();
        {
            let mut facts = lock(&host.facts);
            facts.installed = true;
            facts.storage = Some(StoragePlace { pool: "tank".into(), volume: 1 });
        }
        host
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    fn call(&self, what: String) {
        lock(&self.calls).push(what);
    }

    pub fn set_state(&self, id: &str, state: PowerState) {
        lock(&self.domains).insert(id.to_owned(), state);
    }
}

#[async_trait]
impl VmHost for FakeVmHost {
    async fn facts(&self, _: &RequestContext) -> Result<HostFacts, PortError> {
        Ok(lock(&self.facts).clone())
    }

    async fn pools(&self, _: &RequestContext) -> Result<Vec<StoragePlace>, PortError> {
        Ok(vec![
            StoragePlace { pool: "tank".into(), volume: 1 },
            StoragePlace { pool: "cold".into(), volume: 2 },
        ])
    }

    async fn install(&self, _: &RequestContext) -> Result<(), HostError> {
        self.call("install".into());
        lock(&self.facts).installed = true;
        Ok(())
    }

    async fn set_up(&self, place: &StoragePlace, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("set_up {}", place.pool));
        lock(&self.facts).storage = Some(place.clone());
        Ok(())
    }

    /// A path containing `broken` fails halfway.
    async fn copy_image(
        &self,
        image: &str,
        _: &StoragePlace,
        iso: &IsoSource,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<u64, HostError> {
        self.call(format!("copy_image {image} {}/{}", iso.share, iso.path));
        steps.step("copying_iso", Some(50));
        if iso.path.contains("broken") {
            return Err(HostError::Failed("the ISO became shorter while it was copied".into()));
        }
        steps.step("copying_iso", Some(100));
        lock(&self.files).insert(image.to_owned(), 4096);
        Ok(4096)
    }

    async fn image_files(
        &self,
        _: &StoragePlace,
        _: &RequestContext,
    ) -> Result<BTreeMap<String, u64>, PortError> {
        Ok(lock(&self.files).clone())
    }

    async fn delete_image(&self, image: &str, _: u16, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("delete_image {image}"));
        lock(&self.files).remove(image);
        Ok(())
    }

    async fn open_upload(
        &self,
        image: &str,
        _: u16,
        bytes: u64,
        _: &RequestContext,
    ) -> Result<tokio::fs::File, HostError> {
        self.call(format!("open_upload {image} {bytes}"));
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("zendiloos-vm-upload-{}-{number}", std::process::id()));
        let file = tokio::fs::File::create(&path).await.map_err(|e| HostError::Failed(e.to_string()))?;
        lock(&self.uploads).insert(image.to_owned(), (path, bytes));
        Ok(file)
    }

    /// Kept if exactly the bytes announced came.
    async fn finish_upload(&self, image: &str, _: &RequestContext) -> Result<u64, HostError> {
        self.call(format!("finish_upload {image}"));
        let (path, bytes) = lock(&self.uploads)
            .remove(image)
            .ok_or_else(|| HostError::Refused("no upload is under way".into()))?;
        let came = tokio::fs::metadata(&path).await.map(|meta| meta.len()).unwrap_or(0);
        let _ = tokio::fs::remove_file(&path).await;
        if came != bytes {
            return Err(HostError::Refused(format!("the upload ended after {came} of {bytes} bytes")));
        }
        lock(&self.files).insert(image.to_owned(), bytes);
        Ok(bytes)
    }

    async fn define(&self, vm: &Vm, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("define {}", vm.id));
        if let Some(reason) = lock(&self.fail_define).take() {
            return Err(HostError::Failed(reason));
        }
        lock(&self.domains).insert(vm.id.clone(), PowerState::ShutOff);
        Ok(())
    }

    async fn update(&self, vm: &Vm, removed: &[u8], _: &RequestContext) -> Result<(), HostError> {
        let removed: Vec<String> = removed.iter().map(u8::to_string).collect();
        self.call(format!("update {} [{}]", vm.id, removed.join(",")).trim_end().to_owned());
        match lock(&self.domains).get(&vm.id) {
            Some(PowerState::ShutOff) => Ok(()),
            Some(_) => Err(HostError::Refused("the virtual machine is not shut off".into())),
            None => Err(HostError::Refused("no such domain".into())),
        }
    }

    async fn change_media(&self, vm: &Vm, drive: u8, _: &RequestContext) -> Result<(), HostError> {
        let image = vm.settings.drives.iter().find(|d| d.slot == drive).and_then(|d| d.image.clone());
        self.call(format!("media {} {drive} {}", vm.id, image.as_deref().unwrap_or("eject")));
        Ok(())
    }

    async fn set_autostart(&self, id: &str, autostart: bool, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("autostart {id} {autostart}"));
        Ok(())
    }

    async fn control(&self, id: &str, action: VmAction, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("{action:?} {id}").to_lowercase());
        let mut domains = lock(&self.domains);
        let state = domains.get_mut(id).ok_or_else(|| HostError::Refused("no such domain".into()))?;
        match (action, *state) {
            (VmAction::Start, PowerState::Running) => {
                Err(HostError::Refused("domain is already active".into()))
            }
            (VmAction::Start, _) => {
                *state = PowerState::Running;
                Ok(())
            }
            (_, PowerState::ShutOff) => Err(HostError::Refused("domain is not running".into())),
            (VmAction::Shutdown | VmAction::ForceOff, _) => {
                *state = PowerState::ShutOff;
                Ok(())
            }
            (VmAction::Restart, _) => Ok(()),
        }
    }

    async fn remove(&self, vm: &Vm, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("remove {}", vm.id));
        lock(&self.domains).remove(&vm.id);
        Ok(())
    }

    async fn states(&self, _: &RequestContext) -> Result<BTreeMap<String, PowerState>, PortError> {
        Ok(lock(&self.domains).clone())
    }

    async fn counters(&self, _: &RequestContext) -> Result<BTreeMap<String, VmCounters>, PortError> {
        let look = self.looks.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(lock(&self.domains)
            .iter()
            .filter(|(_, state)| state.is_on())
            .map(|(id, _)| {
                let counters = VmCounters {
                    cpu_time_ns: Some(look * 3_000_000_000),
                    memory_current_kib: Some(256 * 1024),
                    memory_rss_kib: Some(128 * 1024),
                    received_bytes: Some(look * 3000),
                    sent_bytes: Some(look * 1500),
                    ..VmCounters::default()
                };
                (id.clone(), counters)
            })
            .collect())
    }

    /// A console that greets as VNC does and echoes what it gets.
    async fn open_console(&self, id: &str, _: &RequestContext) -> Result<ConsoleSession, HostError> {
        self.call(format!("console {id}"));
        let (to_browser, output) = tokio::sync::mpsc::channel(16);
        let (input, mut from_browser) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        tokio::spawn(async move {
            if to_browser.send(b"RFB 003.008\n".to_vec()).await.is_err() {
                return;
            }
            while let Some(data) = from_browser.recv().await {
                if to_browser.send(data).await.is_err() {
                    return;
                }
            }
        });
        Ok(ConsoleSession { output, input })
    }
}

/// Share 1, `media` on volume 1, readable; share 2 not.
#[derive(Default)]
pub struct FakeIsos;

#[async_trait]
impl IsoFiles for FakeIsos {
    async fn locate(&self, _: &Actor, iso: &IsoChoice, _: &RequestContext) -> Result<IsoSource, IsoError> {
        match iso.share_id {
            1 if iso.path.contains("missing") => Err(IsoError::NotFound),
            1 => Ok(IsoSource { share: "media".into(), volume: 1, path: iso.path.clone() }),
            _ => Err(IsoError::Denied),
        }
    }
}

/// `000000000001`, `000000000002`, …, and MAC addresses
/// `52:54:00:00:00:01`, `…:02`, …
#[derive(Default)]
pub struct CountingIds(AtomicU64, AtomicU64);

impl VmIds for CountingIds {
    fn new_id(&self) -> String {
        format!("{:012x}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn new_mac(&self) -> String {
        let n = self.1.fetch_add(1, Ordering::SeqCst) + 1;
        format!("52:54:00:{:02x}:{:02x}:{:02x}", (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff)
    }
}

#[derive(Default)]
pub struct MemoryAudit(pub Mutex<Vec<AuditEntry>>);

#[async_trait]
impl AuditLog for MemoryAudit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        lock(&self.0).push(entry);
        Ok(())
    }
}

/// The use cases over the fakes, and the fakes to look at.
pub struct TestVms {
    pub vms: Vms,
    pub host: Arc<FakeVmHost>,
    pub repository: Arc<MemoryVms>,
    pub images: Arc<MemoryImages>,
    pub jobs: Jobs,
    pub audit: Arc<MemoryAudit>,
}

impl TestVms {
    pub fn new(host: FakeVmHost) -> Self {
        Self::with_images(host, MemoryImages::default())
    }

    pub fn with_images(host: FakeVmHost, images: MemoryImages) -> Self {
        let host = Arc::new(host);
        let repository = Arc::new(MemoryVms::default());
        let images = Arc::new(images);
        let audit = Arc::new(MemoryAudit::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let vms = Vms::new(VmsPorts {
            repository: repository.clone(),
            images: images.clone(),
            host: host.clone(),
            isos: Arc::new(FakeIsos),
            ids: Arc::new(CountingIds::default()),
            jobs: jobs.clone(),
            audit: audit.clone(),
            clock: Arc::new(FixedClock),
        });
        Self { vms, host, repository, images, jobs, audit }
    }
}
