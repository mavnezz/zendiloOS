//! In-memory adapters for tests, here and in the API crate.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, PortError, RequestContext};

use crate::application::{
    BackupShare, BackupShares, BackupWritten, ConsoleSession, DatasetUsage, HostError, HostFacts, IsoError,
    IsoFiles, IsoSource, Steps, VmBackupRepository, VmHost, VmIds, VmImageRepository, VmLocationRepository,
    VmPool, VmRepository, VmScheduleRepository, Vms, VmsPorts, ZvolUsage,
};
use crate::domain::{
    BackupFile, BackupPart, BackupPlace, Consistency, Frequency, HostDevices, HostLimits, HostNeeds, HostPci,
    HostUsb, Iommu, IsoChoice, Location, NewVmBackup, PowerState, SnapshotSchedule, UsbDevice, Vm, VmAction,
    VmBackup, VmCounters, VmImage, VmSnapshot,
};

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

/// The locations, in the order added.
#[derive(Default)]
pub struct MemoryLocations(Mutex<Vec<Location>>);

#[async_trait]
impl VmLocationRepository for MemoryLocations {
    async fn list(&self) -> Result<Vec<Location>, PortError> {
        Ok(lock(&self.0).clone())
    }

    async fn add(&self, location: &Location) -> Result<(), PortError> {
        let mut locations = lock(&self.0);
        if !locations.iter().any(|other| other.pool == location.pool) {
            locations.push(location.clone());
        }
        Ok(())
    }

    async fn remove(&self, pool: &str) -> Result<bool, PortError> {
        let mut locations = lock(&self.0);
        let before = locations.len();
        locations.retain(|location| location.pool != pool);
        Ok(locations.len() < before)
    }
}

/// The VMs' snapshot schedules.
#[derive(Default)]
pub struct MemorySchedules(Mutex<BTreeMap<String, Vec<SnapshotSchedule>>>);

#[async_trait]
impl VmScheduleRepository for MemorySchedules {
    async fn list(&self, vm: &str) -> Result<Vec<SnapshotSchedule>, PortError> {
        Ok(lock(&self.0).get(vm).cloned().unwrap_or_default())
    }

    async fn all(&self) -> Result<BTreeMap<String, Vec<SnapshotSchedule>>, PortError> {
        Ok(lock(&self.0)
            .iter()
            .filter(|(_, list)| !list.is_empty())
            .map(|(vm, list)| (vm.clone(), list.clone()))
            .collect())
    }

    async fn set(&self, vm: &str, schedules: &[SnapshotSchedule]) -> Result<(), PortError> {
        lock(&self.0).insert(vm.to_owned(), schedules.to_vec());
        Ok(())
    }
}

/// The VMs' backups, numbered from 1.
#[derive(Default)]
pub struct MemoryBackups(Mutex<Vec<VmBackup>>);

#[async_trait]
impl VmBackupRepository for MemoryBackups {
    async fn list(&self) -> Result<Vec<VmBackup>, PortError> {
        let mut all = lock(&self.0).clone();
        all.sort_by_key(|backup| std::cmp::Reverse((backup.taken_at, backup.id)));
        Ok(all)
    }

    async fn find(&self, id: i64) -> Result<Option<VmBackup>, PortError> {
        Ok(lock(&self.0).iter().find(|backup| backup.id == id).cloned())
    }

    async fn create(&self, backup: &NewVmBackup) -> Result<i64, PortError> {
        let mut all = lock(&self.0);
        let id = all.iter().map(|backup| backup.id).max().unwrap_or(0) + 1;
        let NewVmBackup { vm, vm_name, place, folder, taken_at, consistency, files, settings } =
            backup.clone();
        all.push(VmBackup { id, vm, vm_name, place, folder, taken_at, consistency, files, settings });
        Ok(id)
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let mut all = lock(&self.0);
        let before = all.len();
        all.retain(|backup| backup.id != id);
        Ok(all.len() < before)
    }
}

/// Share 1 is `backups` on volume 1, unless renamed or gone.
pub struct FakeBackupShares(pub Mutex<Option<BackupShare>>);

impl Default for FakeBackupShares {
    fn default() -> Self {
        Self(Mutex::new(Some(BackupShare { name: "backups".into(), volume: 1 })))
    }
}

#[async_trait]
impl BackupShares for FakeBackupShares {
    async fn locate(&self, share_id: i64) -> Result<Option<BackupShare>, PortError> {
        Ok(if share_id == 1 { lock(&self.0).clone() } else { None })
    }
}

/// The pool `tank`, volume 1.
pub fn tank() -> Location {
    Location { pool: "tank".into(), volume: 1 }
}

/// The pool `cold`, volume 2.
pub fn cold() -> Location {
    Location { pool: "cold".into(), volume: 2 }
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
/// first, the pools `tank` (volume 1) and `cold` (2), a bridge `br0`, an
/// internal network `vmnet0`, the cards `enp1s0` and `bond0`, two threads
/// and 2 GiB. Each zvol holds a quarter of its size.
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
    /// The next definition waits for this to be notified first.
    pub hold_define: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// The VMs' zvols, `<pool>/<id>-disk<N>`, and their sizes.
    pub zvols: Mutex<BTreeMap<String, u64>>,
    /// Uploads under way: what is written, and the bytes announced.
    uploads: Mutex<BTreeMap<String, (std::path::PathBuf, u64)>>,
    /// Snapshots taken, in order: VM, label, name, schedules, consistency.
    pub snapshots: Mutex<Vec<VmSnapshot>>,
    /// The backups written, by folder.
    pub written: Mutex<BTreeMap<String, Vec<BackupFile>>>,
    /// The host's devices.
    pub devices: Mutex<HostDevices>,
    /// The next backup or restore waits for this to be notified first.
    pub hold_work: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// The next restore fails with this.
    pub fail_restore: Mutex<Option<String>>,
}

/// A keyboard plugged in; with an IOMMU, a graphics card with its sound in
/// a group, the SATA controller the host needs.
pub fn devices() -> HostDevices {
    let pci = |address: &str, class: u32, group: u32, needed: Option<HostNeeds>| HostPci {
        address: address.into(),
        vendor: 0x10de,
        device: 0x1c82,
        class,
        vendor_name: Some("NVIDIA Corporation".into()),
        device_name: None,
        iommu_group: Some(group),
        driver: None,
        needed,
    };
    HostDevices {
        iommu: Iommu {
            on: true,
            cmdline: vec!["intel_iommu=on".into()],
            cpu_vendor: Some("GenuineIntel".into()),
        },
        usb: vec![HostUsb {
            device: UsbDevice { vendor: 0x046d, product: 0xc31c },
            port: "1-2".into(),
            manufacturer: Some("Logitech".into()),
            name: Some("USB Keyboard".into()),
        }],
        pci: vec![
            pci("0000:01:00.0", 0x030000, 1, None),
            pci("0000:01:00.1", 0x040300, 1, None),
            pci("0000:00:17.0", 0x010601, 2, Some(HostNeeds::Disk("sda".into()))),
        ],
    }
}

impl Default for FakeVmHost {
    fn default() -> Self {
        Self {
            facts: Mutex::new(HostFacts {
                architecture: "amd64".into(),
                kvm: true,
                cpu_virtualization: Some("vmx".into()),
                iommu: false,
                installed: false,
                swtpm: true,
                secure_boot: true,
                limits: HostLimits {
                    cores: 2,
                    memory_bytes: 2 << 30,
                    bridges: vec!["br0".into()],
                    internal: vec!["vmnet0".into()],
                    cards: vec!["enp1s0".into(), "bond0".into()],
                    locations: Vec::new(),
                    swtpm: false,
                    secure_boot: false,
                },
                locations: Vec::new(),
            }),
            domains: Mutex::default(),
            files: Mutex::default(),
            looks: AtomicU64::new(0),
            calls: Mutex::default(),
            fail_define: Mutex::default(),
            hold_define: Mutex::default(),
            zvols: Mutex::default(),
            uploads: Mutex::default(),
            snapshots: Mutex::default(),
            written: Mutex::default(),
            devices: Mutex::new(devices()),
            hold_work: Mutex::default(),
            fail_restore: Mutex::default(),
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
            facts.locations = vec![tank()];
        }
        host
    }

    /// Ready, with a second location on `cold`.
    pub fn two_locations() -> Self {
        let host = Self::ready();
        lock(&host.facts).locations.push(cold());
        host
    }

    pub fn zvols(&self) -> Vec<String> {
        lock(&self.zvols).keys().cloned().collect()
    }

    fn make_disks(&self, vm: &Vm) {
        let mut zvols = lock(&self.zvols);
        for disk in &vm.settings.disks {
            let size =
                zvols.entry(format!("{}/{}-disk{}", disk.location.pool, vm.id, disk.slot)).or_default();
            *size = (*size).max(u64::from(disk.size_gib) << 30);
        }
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

    async fn pools(&self, _: &RequestContext) -> Result<Vec<VmPool>, PortError> {
        let pool = |location: Location| VmPool {
            location,
            health: "online".into(),
            used_bytes: 10 << 30,
            available_bytes: 90 << 30,
        };
        Ok(vec![pool(tank()), pool(cold())])
    }

    async fn install(&self, _: &RequestContext) -> Result<(), HostError> {
        self.call("install".into());
        lock(&self.facts).installed = true;
        Ok(())
    }

    async fn add_location(&self, place: &Location, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("add_location {}", place.pool));
        let mut facts = lock(&self.facts);
        if !facts.locations.contains(place) {
            facts.locations.push(place.clone());
            facts.locations.sort_by_key(|location| location.volume);
        }
        Ok(())
    }

    async fn remove_location(&self, place: &Location, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("remove_location {}", place.pool));
        if lock(&self.zvols).keys().any(|zvol| zvol.starts_with(&format!("{}/", place.pool))) {
            return Err(HostError::Refused(format!("{} holds a disk of a virtual machine", place.pool)));
        }
        lock(&self.facts).locations.retain(|location| location != place);
        Ok(())
    }

    async fn storage_usage(&self, _: &RequestContext) -> Result<Vec<DatasetUsage>, PortError> {
        let zvols = lock(&self.zvols).clone();
        Ok(lock(&self.facts)
            .locations
            .iter()
            .map(|location| {
                let zvols: Vec<ZvolUsage> = zvols
                    .iter()
                    .filter_map(|(zvol, size)| {
                        let name = zvol.strip_prefix(&format!("{}/", location.pool))?;
                        Some(ZvolUsage {
                            name: name.to_owned(),
                            used_bytes: size / 4,
                            referenced_bytes: size / 8,
                            volsize_bytes: *size,
                        })
                    })
                    .collect();
                DatasetUsage {
                    pool: location.pool.clone(),
                    used_bytes: 4096 + zvols.iter().map(|zvol| zvol.used_bytes).sum::<u64>(),
                    files_bytes: 4096,
                    available_bytes: 90 << 30,
                    zvols,
                }
            })
            .collect())
    }

    /// A path containing `broken` fails halfway.
    async fn copy_image(
        &self,
        image: &str,
        _: &Location,
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
        _: &Location,
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

    async fn define(&self, vm: &Vm, library: u16, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("define {} {library}", vm.id));
        let held = lock(&self.hold_define).take();
        if let Some(held) = held {
            held.notified().await;
        }
        if let Some(reason) = lock(&self.fail_define).take() {
            return Err(HostError::Failed(reason));
        }
        self.make_disks(vm);
        lock(&self.domains).insert(vm.id.clone(), PowerState::ShutOff);
        Ok(())
    }

    async fn update(
        &self,
        vm: &Vm,
        library: u16,
        removed: &[u8],
        _: &RequestContext,
    ) -> Result<(), HostError> {
        let removed_text: Vec<String> = removed.iter().map(u8::to_string).collect();
        self.call(format!("update {} {library} [{}]", vm.id, removed_text.join(",")).trim_end().to_owned());
        match lock(&self.domains).get(&vm.id) {
            Some(PowerState::ShutOff) => {}
            Some(_) => return Err(HostError::Refused("the virtual machine is not shut off".into())),
            None => return Err(HostError::Refused("no such domain".into())),
        }
        self.make_disks(vm);
        lock(&self.zvols)
            .retain(|zvol, _| !removed.iter().any(|slot| zvol.ends_with(&format!("/{}-disk{slot}", vm.id))));
        Ok(())
    }

    async fn change_media(&self, vm: &Vm, _: u16, drive: u8, _: &RequestContext) -> Result<(), HostError> {
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
        lock(&self.zvols).retain(|zvol, _| !zvol.contains(&format!("/{}-disk", vm.id)));
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

    async fn host_devices(&self, _: &RequestContext) -> Result<HostDevices, PortError> {
        Ok(lock(&self.devices).clone())
    }

    async fn change_usb(
        &self,
        vm: &Vm,
        _: u16,
        attach: &[UsbDevice],
        detach: &[UsbDevice],
        _: &RequestContext,
    ) -> Result<(), HostError> {
        let ids = |devices: &[UsbDevice]| devices.iter().map(|d| d.id()).collect::<Vec<_>>().join(",");
        self.call(format!("usb {} +[{}] -[{}]", vm.id, ids(attach), ids(detach)));
        Ok(())
    }

    async fn snapshot(
        &self,
        id: &str,
        label: &str,
        schedules: &[Frequency],
        name: Option<&str>,
        _: &RequestContext,
    ) -> Result<Consistency, HostError> {
        self.call(format!("snapshot {id} {label}"));
        let state = lock(&self.domains).get(id).copied();
        let consistency = match state {
            None => return Err(HostError::Refused("no such domain".into())),
            Some(PowerState::ShutOff) => Consistency::Off,
            Some(_) => Consistency::Crash,
        };
        let mut snapshots = lock(&self.snapshots);
        if snapshots.iter().any(|snapshot| snapshot.vm == id && snapshot.label == label) {
            return Err(HostError::Refused("the snapshot exists".into()));
        }
        snapshots.push(VmSnapshot {
            vm: id.to_owned(),
            label: label.to_owned(),
            taken_at: crate::domain::label_time(label)
                .unwrap_or_else(|| zendiloos_kernel::Timestamp::from_unix_seconds(0)),
            name: name.map(str::to_owned),
            schedules: schedules.to_vec(),
            consistency,
            disks: vec![0],
            used_bytes: 4096,
            referenced_bytes: 8192,
        });
        Ok(consistency)
    }

    async fn snapshots(&self, _: &RequestContext) -> Result<Vec<VmSnapshot>, PortError> {
        Ok(lock(&self.snapshots).clone())
    }

    async fn destroy_snapshot(&self, id: &str, label: &str, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("destroy_snapshot {id} {label}"));
        let mut snapshots = lock(&self.snapshots);
        let before = snapshots.len();
        snapshots.retain(|snapshot| !(snapshot.vm == id && snapshot.label == label));
        if snapshots.len() < before {
            Ok(())
        } else {
            Err(HostError::Refused("there is no such snapshot".into()))
        }
    }

    async fn rollback_snapshot(
        &self,
        id: &str,
        label: &str,
        newer: bool,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        self.call(format!("rollback {id} {label} {newer}"));
        let mut snapshots = lock(&self.snapshots);
        let Some(at) = snapshots.iter().find(|s| s.vm == id && s.label == label).map(|s| s.taken_at) else {
            return Err(HostError::Refused("there is no such snapshot".into()));
        };
        snapshots.retain(|snapshot| snapshot.vm != id || snapshot.taken_at <= at);
        Ok(())
    }

    async fn back_up(
        &self,
        vm: &Vm,
        place: &BackupPlace,
        folder: &str,
        stem: &str,
        label: &str,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<BackupWritten, HostError> {
        self.call(format!("back_up {} {} {folder} {stem} {label}", vm.id, place.shown()));
        let held = lock(&self.hold_work).take();
        if let Some(held) = held {
            held.notified().await;
        }
        steps.step("backing_up", Some(50));
        let folder = if folder.is_empty() {
            format!("{}/{stem}", vm.id)
        } else {
            format!("{folder}/{}/{stem}", vm.id)
        };
        let mut files: Vec<BackupFile> = vm
            .settings
            .disks
            .iter()
            .map(|disk| BackupFile {
                part: BackupPart::Disk { slot: disk.slot, volsize: u64::from(disk.size_gib) << 30 },
                bytes: 1 << 20,
                sha256: "ab".repeat(32),
            })
            .collect();
        files.push(BackupFile { part: BackupPart::Nvram, bytes: 4096, sha256: "cd".repeat(32) });
        lock(&self.written).insert(folder.clone(), files.clone());
        let off = lock(&self.domains).get(&vm.id) == Some(&PowerState::ShutOff);
        Ok(BackupWritten {
            folder,
            files,
            consistency: if off { Consistency::Off } else { Consistency::Crash },
        })
    }

    async fn restore(
        &self,
        vm: &Vm,
        library: u16,
        backup: &VmBackup,
        over: bool,
        remove: &[u8],
        label: &str,
        steps: &dyn Steps,
        _: &RequestContext,
    ) -> Result<(), HostError> {
        let removed: Vec<String> = remove.iter().map(u8::to_string).collect();
        self.call(format!(
            "restore {} {library} {} over={over} [{}] {label}",
            vm.id,
            backup.folder,
            removed.join(",")
        ));
        let held = lock(&self.hold_work).take();
        if let Some(held) = held {
            held.notified().await;
        }
        if !lock(&self.written).contains_key(&backup.folder) {
            return Err(HostError::Refused(format!("{} is not there any more", backup.folder)));
        }
        if let Some(reason) = lock(&self.fail_restore).take() {
            return Err(HostError::Failed(reason));
        }
        steps.step("restoring", Some(50));
        if over {
            match lock(&self.domains).get(&vm.id) {
                Some(PowerState::ShutOff) => {}
                _ => return Err(HostError::Refused("the virtual machine is not shut off".into())),
            }
        } else {
            lock(&self.domains).insert(vm.id.clone(), PowerState::ShutOff);
        }
        self.make_disks(vm);
        lock(&self.zvols)
            .retain(|zvol, _| !remove.iter().any(|slot| zvol.ends_with(&format!("/{}-disk{slot}", vm.id))));
        Ok(())
    }

    async fn end_work(&self, id: &str, _: &RequestContext) -> Result<bool, HostError> {
        self.call(format!("end_work {id}"));
        Ok(false)
    }

    async fn remove_backup(&self, backup: &VmBackup, _: &RequestContext) -> Result<(), HostError> {
        self.call(format!("remove_backup {}", backup.folder));
        lock(&self.written).remove(&backup.folder);
        Ok(())
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
    pub locations: Arc<MemoryLocations>,
    pub schedules: Arc<MemorySchedules>,
    pub backups: Arc<MemoryBackups>,
    pub shares: Arc<FakeBackupShares>,
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
        let locations = Arc::new(MemoryLocations::default());
        let schedules = Arc::new(MemorySchedules::default());
        let backups = Arc::new(MemoryBackups::default());
        let shares = Arc::new(FakeBackupShares::default());
        let audit = Arc::new(MemoryAudit::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
        let vms = Vms::new(VmsPorts {
            repository: repository.clone(),
            images: images.clone(),
            locations: locations.clone(),
            schedules: schedules.clone(),
            backups: backups.clone(),
            shares: shares.clone(),
            host: host.clone(),
            isos: Arc::new(FakeIsos),
            ids: Arc::new(CountingIds::default()),
            jobs: jobs.clone(),
            audit: audit.clone(),
            clock: Arc::new(FixedClock),
        });
        Self { vms, host, repository, images, locations, schedules, backups, shares, jobs, audit }
    }
}
