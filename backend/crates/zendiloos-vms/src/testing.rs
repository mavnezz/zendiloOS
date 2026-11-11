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
    VmRepository, Vms, VmsPorts,
};
use crate::domain::{HostLimits, IsoChoice, PowerState, Vm, VmAction};

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

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let mut vms = lock(&self.vms);
        let before = vms.len();
        vms.retain(|vm| vm.id != id);
        Ok(vms.len() < before)
    }
}

/// libvirt and the storage pretended: KVM there, nothing installed at
/// first, the pools `tank` (volume 1) and `cold` (2), a bridge `br0`, two
/// threads and 2 GiB.
pub struct FakeVmHost {
    pub facts: Mutex<HostFacts>,
    pub domains: Mutex<BTreeMap<String, PowerState>>,
    /// What was asked, one word each: `install`, `define <id>`, `start <id>`, …
    pub calls: Mutex<Vec<String>>,
    /// The next definition fails with this.
    pub fail_define: Mutex<Option<String>>,
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
            calls: Mutex::default(),
            fail_define: Mutex::default(),
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

    async fn copy_iso(
        &self,
        id: &str,
        _: &StoragePlace,
        iso: &IsoSource,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.call(format!("copy_iso {id} {}/{}", iso.share, iso.path));
        steps.step("copying_iso", Some(50));
        steps.step("copying_iso", Some(100));
        Ok(())
    }

    async fn define(&self, vm: &Vm, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("define {}", vm.id));
        if let Some(reason) = lock(&self.fail_define).take() {
            return Err(HostError::Failed(reason));
        }
        lock(&self.domains).insert(vm.id.clone(), PowerState::ShutOff);
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

/// `000000000001`, `000000000002`, …
#[derive(Default)]
pub struct CountingIds(AtomicU64);

impl VmIds for CountingIds {
    fn new_id(&self) -> String {
        format!("{:012x}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
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
    pub jobs: Jobs,
    pub audit: Arc<MemoryAudit>,
}

impl TestVms {
    pub fn new(host: FakeVmHost) -> Self {
        let host = Arc::new(host);
        let repository = Arc::new(MemoryVms::default());
        let audit = Arc::new(MemoryAudit::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let vms = Vms::new(VmsPorts {
            repository: repository.clone(),
            host: host.clone(),
            isos: Arc::new(FakeIsos),
            ids: Arc::new(CountingIds::default()),
            jobs: jobs.clone(),
            audit: audit.clone(),
            clock: Arc::new(FixedClock),
        });
        Self { vms, host, repository, jobs, audit }
    }
}
