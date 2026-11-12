//! Virtual machines, simulated: libvirt's domains as a map, their states
//! as `virsh list --all` prints them, and a console that speaks just
//! enough of VNC (RFB 3.8, no authentication, raw pixels) for noVNC to show
//! a picture. Nothing runs.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use zendiloos_agent_proto::{
    ArchiveDigest, ErrorCode, HostIommu, HostNeeds, HostPciDevice, HostUsbDevice, PciAddress, Request,
    Response, VM_BACKUP_MANIFEST, VM_IMAGE_UPLOAD_MAX, VMS_DATASET, VmAction, VmBackupFile, VmBackupPart,
    VmConsistency, VmDiskSlot, VmId, VmImageFile, VmImageId, VmNetwork, VmSpec, VmStorage, VolumeNumber,
    WrittenFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Power {
    Off,
    Running,
}

/// An upload being read: the bytes that came once the pipe closed.
pub(crate) type Receiving = std::thread::JoinHandle<u64>;

#[derive(Debug, Default)]
pub(crate) struct SimVms {
    /// libvirt and QEMU installed (the plugin's run of apt).
    pub(crate) installed: bool,
    /// The locations of the VMs' storage.
    locations: Vec<VmStorage>,
    /// The VMs' zvols, `<pool>/zendiloos.vms/<id>-disk<N>`, and their sizes.
    zvols: BTreeMap<String, u64>,
    domains: BTreeMap<VmId, Power>,
    /// Domain ids since libvirt started, as `virsh list` numbers them.
    started: u32,
    /// The library's images, the volume each is on and its size.
    images: BTreeMap<VmImageId, (VolumeNumber, u64)>,
    /// Where each upload under way goes.
    upload_volumes: BTreeMap<VmImageId, VolumeNumber>,
    /// Uploads under way: what was announced, and the thread reading.
    pub(crate) uploads: BTreeMap<VmImageId, (u64, Receiving)>,
    /// Looks at the counters, for CPU time that grows.
    looks: u64,
    /// Snapshots of the VMs' disks, in the order taken: the snapshot's
    /// name, its schedules, its note and how consistent it is.
    snapshots: Vec<(String, String, String, VmConsistency)>,
    /// Backups written: their folder and files.
    backups: BTreeMap<String, Vec<VmBackupFile>>,
    /// The USB devices given to each VM.
    usb: BTreeMap<VmId, Vec<zendiloos_agent_proto::VmUsbDevice>>,
}

/// What a backup of the simulated host's files hashes to: the file's name.
fn sim_digest(name: &str) -> ArchiveDigest {
    let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    ArchiveDigest::parse(&format!("{hex:0<64}")[..64]).expect("64 hex digits are a digest")
}

/// The simulated machine's USB devices: a stick and a keyboard.
fn sim_usb() -> Vec<HostUsbDevice> {
    vec![
        HostUsbDevice {
            vendor: 0x0781,
            product: 0x5583,
            port: "1-1.2".into(),
            manufacturer: Some("SanDisk".into()),
            name: Some("Ultra Fit".into()),
        },
        HostUsbDevice {
            vendor: 0x046d,
            product: 0xc31c,
            port: "1-1.3".into(),
            manufacturer: Some("Logitech".into()),
            name: Some("USB Keyboard".into()),
        },
    ]
}

/// The simulated machine's PCI devices, with an IOMMU: the host bridge, the
/// SATA controller with the pools' disks, the NAS's network card, a second
/// card nothing uses and a graphics card with its audio function.
fn sim_pci() -> Vec<HostPciDevice> {
    let device =
        |address: &str, vendor, device, class, group, name: &str, needed: Option<HostNeeds>| HostPciDevice {
            address: PciAddress::parse(address).expect("a PCI address"),
            vendor,
            device,
            class,
            vendor_name: Some(
                if vendor == 0x8086 { "Intel Corporation" } else { "NVIDIA Corporation" }.into(),
            ),
            device_name: Some(name.into()),
            iommu_group: Some(group),
            driver: Some("sim".into()),
            needed,
        };
    vec![
        device("0000:00:00.0", 0x8086, 0x3e0f, 0x060000, 0, "Host Bridge", Some(HostNeeds::Bridge)),
        device(
            "0000:00:17.0",
            0x8086,
            0xa352,
            0x010601,
            1,
            "SATA AHCI Controller",
            Some(HostNeeds::Disk { disk: "sda".into() }),
        ),
        device(
            "0000:00:1f.6",
            0x8086,
            0x15bc,
            0x020000,
            2,
            "Ethernet Connection",
            Some(HostNeeds::Network { interface: Some("enp1s0".into()) }),
        ),
        device("0000:02:00.0", 0x8086, 0x1533, 0x020000, 3, "I210 Gigabit Network Connection", None),
        device("0000:01:00.0", 0x10de, 0x1c82, 0x030000, 4, "GP107 [GeForce GTX 1050 Ti]", None),
        device("0000:01:00.1", 0x10de, 0x0fb9, 0x040300, 4, "GP107GL High Definition Audio Controller", None),
    ]
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

impl SimVms {
    /// The answer to a request about VMs; `None` for other requests.
    pub(crate) fn handle(&mut self, request: &Request) -> Option<Response> {
        let not_installed = || error(ErrorCode::Forbidden, "refused: libvirt and QEMU are not installed");
        Some(match request {
            Request::VmHost {} => Response::VmHost {
                kvm: true,
                cpu_virtualization: Some("vmx".into()),
                iommu: true,
                installed: self.installed,
                swtpm: self.installed,
                secure_boot: self.installed,
                cores: 8,
                memory_bytes: 16 << 30,
                locations: self.locations.clone(),
            },
            Request::EnableVms { .. } if !self.installed => not_installed(),
            Request::EnableVms { pool, volume } => {
                let location = VmStorage { pool: pool.clone(), volume: *volume };
                if !self.locations.contains(&location) {
                    if self.locations.iter().any(|other| other.pool == *pool || other.volume == *volume) {
                        return Some(error(ErrorCode::Forbidden, "refused: the pool is not volume it says"));
                    }
                    self.locations.push(location);
                    self.locations.sort_by_key(|location| location.volume.get());
                }
                Response::Done
            }
            Request::DisableVms { pool, volume } => {
                self.disable(&VmStorage { pool: pool.clone(), volume: *volume })
            }
            Request::VmStorageUsage {} => Response::VmStorageUsage { zfs_list: self.usage() },
            Request::DeleteVm { .. } if self.locations.is_empty() => {
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up")
            }
            Request::CopyVmImage { volume, .. }
            | Request::VmImages { volume }
            | Request::DeleteVmImage { volume, .. }
                if !self.on(*volume) =>
            {
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up there")
            }
            // A small ISO, copied at once.
            Request::CopyVmImage { image, volume, .. } => {
                self.images.insert(image.clone(), (*volume, 4096));
                Response::VmImageStored { bytes: 4096 }
            }
            Request::VmImages { volume } => Response::VmImages {
                files: self
                    .images
                    .iter()
                    .filter(|(_, (on, _))| on == volume)
                    .map(|(image, (_, bytes))| VmImageFile { image: image.clone(), bytes: *bytes })
                    .collect(),
            },
            Request::DeleteVmImage { image, .. } => {
                self.images.remove(image);
                Response::Done
            }
            Request::OpenVmImageUpload { .. } | Request::FinishVmImageUpload { .. } => {
                error(ErrorCode::InvalidRequest, "an upload goes with its pipe")
            }
            Request::DefineVm { .. } | Request::UpdateVm { .. } | Request::ChangeVmMedia { .. }
                if !self.installed =>
            {
                not_installed()
            }
            Request::DefineVm { spec } => {
                if self.domains.contains_key(&spec.id) {
                    error(ErrorCode::Conflict, "the name is already taken")
                } else if let Some(refusal) = self.refuse_devices(spec) {
                    refusal
                } else {
                    self.domains.insert(spec.id.clone(), Power::Off);
                    self.make_disks(spec);
                    Response::Done
                }
            }
            Request::UpdateVm { spec, remove } => match self.domains.get(&spec.id) {
                None => missing(&spec.id),
                Some(Power::Running) => error(
                    ErrorCode::Forbidden,
                    "refused: the virtual machine is not shut off: its hardware changes only while it is off",
                ),
                Some(Power::Off) => match self.refuse_devices(spec) {
                    Some(refusal) => refusal,
                    None => {
                        self.make_disks(spec);
                        for slot in remove {
                            self.remove_disks(&spec.id, Some(*slot));
                        }
                        Response::Done
                    }
                },
            },
            Request::ChangeVmMedia { spec, drive } => {
                if !self.domains.contains_key(&spec.id) {
                    missing(&spec.id)
                } else if spec.drive(*drive).is_none() {
                    error(ErrorCode::Forbidden, &format!("refused: there is no drive {}", drive.get()))
                } else {
                    self.refuse_devices(spec).unwrap_or(Response::Done)
                }
            }
            Request::VmStats {} => Response::VmStats { domstats: self.domstats() },
            Request::SetVmAutostart { id, .. } => {
                if self.domains.contains_key(id) {
                    Response::Done
                } else {
                    missing(id)
                }
            }
            Request::ControlVm { id, action } => {
                let Some(power) = self.domains.get_mut(id) else { return Some(missing(id)) };
                match (action, *power) {
                    (VmAction::Start, Power::Running) => error(
                        ErrorCode::Forbidden,
                        "refused: Requested operation is not valid: domain is already active",
                    ),
                    (VmAction::Start, Power::Off) => {
                        *power = Power::Running;
                        self.started += 1;
                        Response::Done
                    }
                    (_, Power::Off) => error(
                        ErrorCode::Forbidden,
                        "refused: Requested operation is not valid: domain is not running",
                    ),
                    // A guest asked to shut down does at once here.
                    (VmAction::Shutdown | VmAction::ForceOff, Power::Running) => {
                        *power = Power::Off;
                        Response::Done
                    }
                    (VmAction::Reboot, Power::Running) => Response::Done,
                }
            }
            Request::DeleteVm { id } => {
                self.domains.remove(id);
                self.remove_disks(id, None);
                let prefix = format!("{}-disk", id.as_str());
                self.snapshots.retain(|(name, ..)| !name.contains(&prefix));
                Response::Done
            }
            Request::VmStates {} => {
                let mut virsh_list = String::from(" Id   Name                     State\n");
                virsh_list.push_str("-------------------------------------------\n");
                for (number, (id, power)) in self.domains.iter().enumerate() {
                    let (shown, state) = match power {
                        Power::Running => ((number + 1).to_string(), "running"),
                        Power::Off => ("-".to_owned(), "shut off"),
                    };
                    virsh_list.push_str(&format!(" {shown:<4} {:<24} {state}\n", id.domain()));
                }
                // Someone else's domain, which zendiloOS never shows.
                virsh_list.push_str(" -    win10                    shut off\n");
                Response::VmStates { virsh_list }
            }
            Request::OpenVmConsole { .. } => {
                error(ErrorCode::InvalidRequest, "a console is opened with its descriptor")
            }
            Request::VmHostDevices {} => Response::VmHostDevices {
                iommu: HostIommu {
                    on: true,
                    cmdline: vec!["intel_iommu=on".into()],
                    cpu_vendor: Some("GenuineIntel".into()),
                },
                usb: sim_usb(),
                pci: sim_pci(),
            },
            Request::ChangeVmUsb { spec, .. } => {
                if !self.domains.contains_key(&spec.id) {
                    missing(&spec.id)
                } else {
                    self.usb.insert(spec.id.clone(), spec.usb().to_vec());
                    Response::Done
                }
            }
            Request::SnapshotVm { id, label, schedules, note } => {
                let Some(power) = self.domains.get(id).copied() else { return Some(missing(id)) };
                let zvols = self.zvols_of(id);
                if zvols.iter().any(|zvol| {
                    self.snapshots.iter().any(|(name, ..)| *name == format!("{zvol}@{}", label.as_str()))
                }) {
                    return Some(error(ErrorCode::Conflict, "the snapshot exists"));
                }
                let consistency =
                    if power == Power::Running { VmConsistency::Crash } else { VmConsistency::Off };
                let schedules: Vec<&str> = schedules.iter().map(|frequency| frequency.as_str()).collect();
                let schedules = if schedules.is_empty() { "-".to_owned() } else { schedules.join(",") };
                let note = note.as_ref().map_or("-".to_owned(), |note| note.as_str().to_owned());
                for zvol in zvols {
                    self.snapshots.push((
                        format!("{zvol}@{}", label.as_str()),
                        schedules.clone(),
                        note.clone(),
                        consistency,
                    ));
                }
                Response::VmSnapshotTaken { consistency }
            }
            Request::VmSnapshots {} => Response::VmSnapshots {
                zfs_list: self
                    .snapshots
                    .iter()
                    .enumerate()
                    .map(|(number, (name, schedules, note, consistency))| {
                        format!(
                            "{name}\t{}\t{}\t{}\t{schedules}\t{note}\t{}\n",
                            1_793_000_000 + number as u64 * 3600,
                            64 << 20,
                            512 << 20,
                            consistency.as_str()
                        )
                    })
                    .collect(),
            },
            Request::DestroyVmSnapshot { id, label } => {
                let before = self.snapshots.len();
                let wanted = format!("@{}", label.as_str());
                let prefix = format!("{}-disk", id.as_str());
                self.snapshots.retain(|(name, ..)| !(name.ends_with(&wanted) && name.contains(&prefix)));
                if self.snapshots.len() < before {
                    Response::Done
                } else {
                    error(ErrorCode::Forbidden, &format!("refused: there is no snapshot {}", label.as_str()))
                }
            }
            Request::RollbackVmSnapshot { id, label, newer } => match self.domains.get(id) {
                None => missing(id),
                Some(Power::Running) => error(
                    ErrorCode::Forbidden,
                    "refused: the virtual machine is not shut off: shut it down first",
                ),
                Some(Power::Off) => {
                    let prefix = format!("{}-disk", id.as_str());
                    let wanted = format!("@{}", label.as_str());
                    let Some(at) = self
                        .snapshots
                        .iter()
                        .position(|(name, ..)| name.contains(&prefix) && name.ends_with(&wanted))
                    else {
                        return Some(error(
                            ErrorCode::Forbidden,
                            &format!("refused: there is no snapshot {}", label.as_str()),
                        ));
                    };
                    let later: Vec<usize> = (at..self.snapshots.len())
                        .filter(|index| {
                            self.snapshots[*index].0.contains(&prefix)
                                && !self.snapshots[*index].0.ends_with(&wanted)
                        })
                        .collect();
                    if !later.is_empty() && !newer {
                        return Some(error(
                            ErrorCode::Forbidden,
                            "refused: there are snapshots after it: they go with the rollback only if that is confirmed",
                        ));
                    }
                    for index in later.into_iter().rev() {
                        self.snapshots.remove(index);
                    }
                    Response::Done
                }
            },
            Request::BackUpVm { id, folder, stem, .. } => {
                if !self.domains.contains_key(id) {
                    return Some(missing(id));
                }
                let Ok(folder) = folder.join(id.as_str()).and_then(|folder| folder.join(stem.as_str()))
                else {
                    return Some(error(ErrorCode::Forbidden, "refused: the folder's path is too long"));
                };
                if self.backups.contains_key(folder.as_str()) {
                    return Some(error(ErrorCode::Forbidden, "refused: manifest.json exists already"));
                }
                let mut files: Vec<VmBackupFile> = self
                    .zvols
                    .iter()
                    .filter_map(|(zvol, size)| {
                        let slot = zvol.rsplit_once(&format!("{}-disk", id.as_str()))?.1.parse().ok()?;
                        let part = VmBackupPart::Disk { slot: VmDiskSlot::new(slot).ok()?, volsize: *size };
                        Some(VmBackupFile { part, bytes: size / 8, sha256: sim_digest(&part.file_name()) })
                    })
                    .collect();
                files.push(VmBackupFile {
                    part: VmBackupPart::Nvram,
                    bytes: 540_672,
                    sha256: sim_digest("nvram"),
                });
                let manifest = WrittenFile {
                    path: folder.join(VM_BACKUP_MANIFEST).unwrap_or_else(|_| folder.clone()),
                    bytes: 2048,
                    device: 1,
                    inode: 1000 + self.backups.len() as u64,
                    modified: 1_793_000_000_000_000_000,
                };
                self.backups.insert(folder.as_str().to_owned(), files.clone());
                let consistency = if self.domains.get(id) == Some(&Power::Running) {
                    VmConsistency::Crash
                } else {
                    VmConsistency::Off
                };
                Response::VmBackedUp { folder, files, manifest, consistency }
            }
            Request::RestoreVm { spec, folder, files, over, remove, .. } => {
                let Some(written) = self.backups.get(folder.as_str()) else {
                    return Some(error(
                        ErrorCode::Forbidden,
                        &format!("refused: {} is not there any more", folder.as_str()),
                    ));
                };
                if files.iter().any(|file| !written.contains(file)) {
                    return Some(error(
                        ErrorCode::Forbidden,
                        "refused: a file is not the one the backup wrote: its SHA-256 differs",
                    ));
                }
                match (self.domains.get(&spec.id), over) {
                    (Some(Power::Running), true) => {
                        return Some(error(
                            ErrorCode::Forbidden,
                            "refused: the virtual machine is not shut off: shut it down first",
                        ));
                    }
                    (None, true) => return Some(missing(&spec.id)),
                    (Some(_), false) => return Some(error(ErrorCode::Conflict, "the name is already taken")),
                    _ => {}
                }
                if let Some(refusal) = self.refuse_devices(spec) {
                    return Some(refusal);
                }
                self.domains.entry(spec.id.clone()).or_insert(Power::Off);
                self.make_disks(spec);
                for slot in remove {
                    self.remove_disks(&spec.id, Some(*slot));
                }
                Response::Done
            }
            Request::EndVmWork { .. } => Response::VmWorkEnded { stopped: false },
            Request::RemoveVmBackup { folder, .. } => {
                self.backups.remove(folder.as_str());
                Response::Done
            }
            _ => return None,
        })
    }

    /// The VM's zvols, by name.
    fn zvols_of(&self, id: &VmId) -> Vec<String> {
        let prefix = format!("{}-disk", id.as_str());
        self.zvols
            .keys()
            .filter(|zvol| zvol.rsplit('/').next().is_some_and(|disk| disk.starts_with(&prefix)))
            .cloned()
            .collect()
    }

    /// Whether a location is on `volume`.
    fn on(&self, volume: VolumeNumber) -> bool {
        self.locations.iter().any(|location| location.volume == volume)
    }

    /// A location removed, once nothing is on it.
    fn disable(&mut self, location: &VmStorage) -> Response {
        if !self.locations.contains(location) {
            return error(
                ErrorCode::Forbidden,
                "refused: there is no location of the virtual machines' storage there",
            );
        }
        let prefix = format!("{}/{VMS_DATASET}/", location.pool.as_str());
        if let Some(zvol) = self.zvols.keys().find(|zvol| zvol.starts_with(&prefix)) {
            return error(
                ErrorCode::Forbidden,
                &format!("refused: it holds the disk {zvol} of a virtual machine"),
            );
        }
        if self.images.values().any(|(volume, _)| *volume == location.volume) {
            return error(ErrorCode::Forbidden, "refused: its image library holds images");
        }
        self.locations.retain(|other| other != location);
        Response::Done
    }

    /// The disks `spec` has and the host not yet, made; those smaller grown.
    fn make_disks(&mut self, spec: &VmSpec) {
        for disk in spec.disks() {
            let zvol = format!("{}/{VMS_DATASET}/{}", disk.location.pool.as_str(), spec.id.disk(disk.slot));
            let size = self.zvols.entry(zvol).or_default();
            *size = (*size).max(disk.size.bytes());
        }
    }

    /// The VM's disks in `slot` on every location, or all of them.
    fn remove_disks(&mut self, id: &VmId, slot: Option<VmDiskSlot>) {
        let name = slot.map(|slot| id.disk(slot));
        self.zvols.retain(|zvol, _| {
            let disk = zvol.rsplit('/').next().unwrap_or_default();
            !(disk.starts_with(&format!("{}-disk", id.as_str()))
                && name.as_deref().is_none_or(|name| name == disk))
        });
    }

    /// `zfs list` of the locations and their zvols: a sparse zvol holds a
    /// quarter of its size, the library what its images have.
    fn usage(&self) -> String {
        let mut text = String::new();
        for location in &self.locations {
            let dataset = format!("{}/{VMS_DATASET}", location.pool.as_str());
            let images: u64 = self
                .images
                .values()
                .filter(|(volume, _)| *volume == location.volume)
                .map(|(_, bytes)| bytes)
                .sum();
            let zvols: Vec<(&String, &u64)> =
                self.zvols.iter().filter(|(zvol, _)| zvol.starts_with(&format!("{dataset}/"))).collect();
            let used = images + zvols.iter().map(|(_, size)| *size / 4).sum::<u64>();
            text.push_str(&format!(
                "{dataset}\tfilesystem\t{used}\t{images}\t{images}\t{}\t-\n",
                400_u64 << 30
            ));
            for (zvol, size) in zvols {
                let held = size / 4;
                text.push_str(&format!(
                    "{zvol}\tvolume\t{held}\t{held}\t{held}\t{}\t{size}\n",
                    400_u64 << 30
                ));
            }
        }
        text
    }

    /// Why the devices could not be defined: a location the storage does
    /// not have, an image not in the library, a bridge the simulated host
    /// does not have (it has `br0`), a card it does not have for a VM to
    /// go on directly (`enp1s0`, `enp2s0`, `bond0`), or an internal network
    /// it does not have (`vmnet0`).
    fn refuse_devices(&self, spec: &VmSpec) -> Option<Response> {
        if let Some(disk) = spec.disks().iter().find(|disk| !self.locations.contains(&disk.location)) {
            return Some(error(
                ErrorCode::Forbidden,
                &format!(
                    "refused: no location of the virtual machines' storage is on pool {}",
                    disk.location.pool.as_str()
                ),
            ));
        }
        if let Some(image) = spec.images().find(|image| !self.images.contains_key(*image)) {
            return Some(error(
                ErrorCode::Forbidden,
                &format!("refused: the image {} is not in the library", image.as_str()),
            ));
        }
        spec.nics().iter().find_map(|nic| match &nic.network {
            VmNetwork::Bridge { bridge } if bridge.as_str() != "br0" => Some(error(
                ErrorCode::Forbidden,
                &format!("refused: {} is no bridge of this machine", bridge.as_str()),
            )),
            VmNetwork::Direct { card } if !["enp1s0", "enp2s0", "bond0"].contains(&card.as_str()) => {
                Some(error(
                    ErrorCode::Forbidden,
                    &format!("refused: {} is no wired card or bond of this machine", card.as_str()),
                ))
            }
            VmNetwork::Internal { bridge } if bridge.as_str() != "vmnet0" => Some(error(
                ErrorCode::Forbidden,
                &format!("refused: {} is no internal network the NAS keeps itself off", bridge.as_str()),
            )),
            _ => None,
        })
    }

    /// `virsh domstats` of the running domains: CPU time that grows by a
    /// second's worth of one vCPU at each look, a guest using half of 1 GiB,
    /// and network counters that grow too.
    fn domstats(&mut self) -> String {
        self.looks += 1;
        let mut text = String::new();
        for (id, power) in &self.domains {
            if *power != Power::Running {
                continue;
            }
            let n = self.looks;
            text.push_str(&format!(
                "Domain: '{}'
  state.state=1
  cpu.time={}
  balloon.current=1048576
                   balloon.maximum=1048576
  balloon.available=1000000
  balloon.usable=500000
                   balloon.rss=600000
  net.count=1
  net.0.name=vnet0
  net.0.rx.bytes={}
                   net.0.tx.bytes={}

",
                id.domain(),
                n * 1_000_000_000,
                n * 1_000_000,
                n * 250_000
            ));
        }
        text
    }

    /// An upload as the agent takes it: a pipe whose reading end a thread
    /// drains, counting.
    pub(crate) fn open_upload(
        &mut self,
        image: &VmImageId,
        volume: VolumeNumber,
        bytes: u64,
    ) -> (Response, Option<OwnedFd>) {
        if !self.on(volume) {
            return (
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up"),
                None,
            );
        }
        if bytes == 0 || bytes > VM_IMAGE_UPLOAD_MAX {
            return (error(ErrorCode::Forbidden, "refused: an uploaded image has 1 byte to 32 GiB"), None);
        }
        if self.images.contains_key(image) || self.uploads.contains_key(image) {
            return (error(ErrorCode::Conflict, "the name is already taken"), None);
        }
        let (mut reader, writer) = match std::io::pipe() {
            Ok(pipe) => pipe,
            Err(e) => return (error(ErrorCode::Failed, &format!("no pipe: {e}")), None),
        };
        self.upload_volumes.insert(image.clone(), volume);
        let thread = std::thread::spawn(move || {
            let mut sink = [0u8; 64 * 1024];
            let mut received = 0u64;
            while let Ok(read) = reader.read(&mut sink) {
                if read == 0 {
                    break;
                }
                received += read as u64;
            }
            received
        });
        self.uploads.insert(image.clone(), (bytes, thread));
        (Response::VmImageUpload {}, Some(OwnedFd::from(writer)))
    }

    /// What an upload whose thread ended stored.
    pub(crate) fn finish_upload(&mut self, image: &VmImageId, announced: u64, received: u64) -> Response {
        let volume = self.upload_volumes.remove(image);
        if let (Some(volume), true) = (volume, received == announced) {
            self.images.insert(image.clone(), (volume, received));
            Response::VmImageStored { bytes: received }
        } else {
            error(
                ErrorCode::Forbidden,
                &format!("refused: the upload ended after {received} of {announced} bytes"),
            )
        }
    }

    /// The console of a running VM, as the agent answers: one side of a
    /// socket pair, whose other side a thread serves as a VNC server.
    pub(crate) fn console(&self, id: &VmId) -> (Response, Option<OwnedFd>) {
        if self.domains.get(id) != Some(&Power::Running) {
            let refused = "refused: the virtual machine is not running, or has no console";
            return (error(ErrorCode::Forbidden, refused), None);
        }
        let (ours, theirs) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(e) => return (error(ErrorCode::Failed, &format!("no console: {e}")), None),
        };
        let name = id.domain();
        std::thread::spawn(move || {
            let _ = serve_vnc(theirs, &name);
        });
        (Response::VmConsole {}, Some(OwnedFd::from(ours)))
    }
}

fn missing(id: &VmId) -> Response {
    error(ErrorCode::Forbidden, &format!("refused: there is no virtual machine {}", id.as_str()))
}

const WIDTH: u16 = 640;
const HEIGHT: u16 = 400;

/// How the client wants its pixels: 32 bits, the shifts of red, green and
/// blue, and their byte order.
#[derive(Clone, Copy)]
struct Format {
    big_endian: bool,
    shifts: [u8; 3],
}

/// RFB 3.8 without authentication; a gradient for the picture, sent whole
/// when asked for and never again, as nothing changes.
fn serve_vnc(mut stream: UnixStream, name: &str) -> std::io::Result<()> {
    let mut version = [0u8; 12];
    stream.write_all(b"RFB 003.008\n")?;
    stream.read_exact(&mut version)?;
    stream.write_all(&[1, 1])?; // one type of security: none
    let mut chosen = [0u8; 1];
    stream.read_exact(&mut chosen)?;
    stream.write_all(&0u32.to_be_bytes())?;
    let mut shared = [0u8; 1];
    stream.read_exact(&mut shared)?;
    let mut init = Vec::new();
    init.extend(WIDTH.to_be_bytes());
    init.extend(HEIGHT.to_be_bytes());
    // 32 bits per pixel, depth 24, little endian, true colour, 255 each,
    // red at 16, green at 8, blue at 0.
    init.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    let title = format!("{name} (simulated)");
    init.extend(u32::try_from(title.len()).unwrap_or(0).to_be_bytes());
    init.extend(title.as_bytes());
    stream.write_all(&init)?;
    let mut format = Format { big_endian: false, shifts: [16, 8, 0] };
    loop {
        let mut kind = [0u8; 1];
        stream.read_exact(&mut kind)?;
        match kind[0] {
            0 => {
                let mut rest = [0u8; 19];
                stream.read_exact(&mut rest)?;
                format = Format { big_endian: rest[5] != 0, shifts: [rest[13], rest[14], rest[15]] };
            }
            2 => {
                let mut head = [0u8; 3];
                stream.read_exact(&mut head)?;
                let count = u16::from_be_bytes([head[1], head[2]]);
                let mut encodings = vec![0u8; usize::from(count) * 4];
                stream.read_exact(&mut encodings)?;
            }
            3 => {
                let mut request = [0u8; 9];
                stream.read_exact(&mut request)?;
                if request[0] == 0 {
                    stream.write_all(&picture(format))?;
                }
            }
            4 => stream.read_exact(&mut [0u8; 7])?,
            5 => stream.read_exact(&mut [0u8; 5])?,
            6 => {
                let mut head = [0u8; 7];
                stream.read_exact(&mut head)?;
                let length = u32::from_be_bytes([head[3], head[4], head[5], head[6]]);
                std::io::copy(&mut (&mut stream).take(u64::from(length)), &mut std::io::sink())?;
            }
            _ => return Ok(()),
        }
    }
}

/// One update with the whole screen in raw pixels.
fn picture(format: Format) -> Vec<u8> {
    let (width, height) = (usize::from(WIDTH), usize::from(HEIGHT));
    let mut update = Vec::with_capacity(16 + width * height * 4);
    update.extend([0, 0]);
    update.extend(1u16.to_be_bytes());
    update.extend([0, 0, 0, 0]);
    update.extend(WIDTH.to_be_bytes());
    update.extend(HEIGHT.to_be_bytes());
    update.extend(0i32.to_be_bytes()); // raw
    for y in 0..height {
        for x in 0..width {
            let (red, green, blue) = ((x * 255 / width) as u32, (y * 255 / height) as u32, 160u32);
            let [r, g, b] = format.shifts.map(u32::from);
            let pixel = (red << r) | (green << g) | (blue << b);
            update.extend(if format.big_endian { pixel.to_be_bytes() } else { pixel.to_le_bytes() });
        }
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> VmSpec {
        serde_json::from_value(serde_json::json!({
            "id": id, "os": "windows", "cpus": 1, "memory": 512, "firmware": "uefi",
            "disks": [{"slot": 0, "size": 1, "bus": "sata", "location": {"pool": "tank", "volume": 1}}],
            "nics": [], "drives": [], "boot": [{"disk": 0}],
            "usb": [{"vendor": 1133, "product": 49948}], "tpm": true, "secure_boot": true,
            "autostart": false, "library": 1
        }))
        .unwrap()
    }

    fn answer(vms: &mut SimVms, request: serde_json::Value) -> Response {
        let request: Request = serde_json::from_value(request).unwrap();
        vms.handle(&request).unwrap()
    }

    fn refused(response: Response, says: &str) {
        match response {
            Response::Error { message, .. } => {
                assert!(message.contains(says), "{message} does not say {says}")
            }
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn devices_snapshots_backups_and_restores_are_simulated() {
        let mut vms = SimVms { installed: true, ..SimVms::default() };
        vms.locations.push(VmStorage {
            pool: zendiloos_agent_proto::PoolName::parse("tank").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
        });
        let id = "0a1b2c3d4e5f";
        assert_eq!(vms.handle(&Request::DefineVm { spec: spec(id) }), Some(Response::Done));
        let Response::VmHostDevices { iommu, usb, pci } =
            answer(&mut vms, serde_json::json!({"op": "vm_host_devices"}))
        else {
            panic!("no devices")
        };
        assert!(iommu.on && !usb.is_empty() && pci.iter().any(|device| device.needed.is_some()));
        let usb_change =
            serde_json::json!({"op": "change_vm_usb", "spec": spec(id), "attach": [], "detach": []});
        assert_eq!(answer(&mut vms, usb_change), Response::Done);
        refused(
            answer(
                &mut vms,
                serde_json::json!({"op": "change_vm_usb", "spec": spec("0a1b2c3d4e60"), "attach": [], "detach": []}),
            ),
            "no virtual machine",
        );

        let snapshot = |label: &str| serde_json::json!({"op": "snapshot_vm", "id": id, "label": label, "schedules": ["daily"], "note": "first"});
        assert_eq!(
            answer(&mut vms, snapshot("zdl-2026-11-12_03.00.00")),
            Response::VmSnapshotTaken { consistency: VmConsistency::Off }
        );
        assert!(matches!(
            answer(&mut vms, snapshot("zdl-2026-11-12_03.00.00")),
            Response::Error { code: ErrorCode::Conflict, .. }
        ));
        vms.domains.insert(VmId::parse(id).unwrap(), Power::Running);
        assert_eq!(
            answer(&mut vms, snapshot("zdl-2026-11-12_04.00.00")),
            Response::VmSnapshotTaken { consistency: VmConsistency::Crash }
        );
        let Response::VmSnapshots { zfs_list } = answer(&mut vms, serde_json::json!({"op": "vm_snapshots"}))
        else {
            panic!("no list")
        };
        assert!(
            zfs_list.contains(&format!("tank/zendiloos.vms/{id}-disk0@zdl-2026-11-12_03.00.00\t"))
                && zfs_list.contains("\tdaily\tfirst\toff"),
            "{zfs_list}"
        );
        let rollback = |label: &str, newer: bool| serde_json::json!({"op": "rollback_vm_snapshot", "id": id, "label": label, "newer": newer});
        refused(answer(&mut vms, rollback("zdl-2026-11-12_03.00.00", true)), "not shut off");
        vms.domains.insert(VmId::parse(id).unwrap(), Power::Off);
        refused(answer(&mut vms, rollback("zdl-2026-11-12_03.00.00", false)), "snapshots after it");
        refused(answer(&mut vms, rollback("zdl-2026-11-12_09.00.00", false)), "no snapshot");
        assert_eq!(answer(&mut vms, rollback("zdl-2026-11-12_03.00.00", true)), Response::Done);
        let destroy =
            serde_json::json!({"op": "destroy_vm_snapshot", "id": id, "label": "zdl-2026-11-12_03.00.00"});
        assert_eq!(answer(&mut vms, destroy.clone()), Response::Done);
        refused(answer(&mut vms, destroy), "no snapshot");
        refused(answer(&mut vms, rollback("zdl-2026-11-12_03.00.00", false)), "no snapshot");
        refused(
            answer(
                &mut vms,
                serde_json::json!({"op": "rollback_vm_snapshot", "id": "0a1b2c3d4e60", "label": "zdl-2026-11-12_03.00.00", "newer": false}),
            ),
            "no virtual machine",
        );

        let root = serde_json::json!({"kind": "share", "location": {"volume": 1, "share": "backups"}});
        let backup = serde_json::json!({"op": "back_up_vm", "id": id, "root": root, "folder": "vm-backups",
            "stem": "20261112-030000", "label": "zdl-2026-11-12_05.00.00", "settings": "{\"name\":\"win\"}"});
        let Response::VmBackedUp { folder, files, manifest, consistency } = answer(&mut vms, backup.clone())
        else {
            panic!("not backed up")
        };
        assert_eq!(folder.as_str(), format!("vm-backups/{id}/20261112-030000"));
        assert_eq!((files.len(), consistency), (2, VmConsistency::Off));
        assert_eq!(manifest.path.as_str(), format!("{}/manifest.json", folder.as_str()));
        refused(answer(&mut vms, backup), "exists already");
        let restore = |id: &str,
                       over: bool,
                       files: &Vec<zendiloos_agent_proto::VmBackupFile>,
                       folder: &str| {
            serde_json::json!({"op": "restore_vm",
            "spec": spec(id), "root": root, "folder": folder, "files": files, "over": over, "remove": [], "label": "zdl-2026-11-12_06.00.00"})
        };
        assert_eq!(answer(&mut vms, restore("0a1b2c3d4e60", false, &files, folder.as_str())), Response::Done);
        assert!(vms.zvols.keys().any(|zvol| zvol.contains("0a1b2c3d4e60-disk0")));
        assert!(matches!(
            answer(&mut vms, restore("0a1b2c3d4e60", false, &files, folder.as_str())),
            Response::Error { code: ErrorCode::Conflict, .. }
        ));
        assert_eq!(answer(&mut vms, restore(id, true, &files, folder.as_str())), Response::Done);
        vms.domains.insert(VmId::parse(id).unwrap(), Power::Running);
        refused(answer(&mut vms, restore(id, true, &files, folder.as_str())), "not shut off");
        refused(
            answer(&mut vms, restore("0a1b2c3d4e61", true, &files, folder.as_str())),
            "no virtual machine",
        );
        let mut other = files.clone();
        other[0].bytes += 1;
        refused(answer(&mut vms, restore(id, true, &other, folder.as_str())), "SHA-256 differs");
        refused(answer(&mut vms, restore(id, true, &files, "elsewhere")), "not there any more");
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "end_vm_work", "id": id})),
            Response::VmWorkEnded { stopped: false }
        );
        let remove = serde_json::json!({"op": "remove_vm_backup", "root": root, "folder": folder.as_str(), "files": [{"kind": "nvram"}]});
        assert_eq!(answer(&mut vms, remove), Response::Done);
        assert!(vms.backups.is_empty());
        vms.domains.insert(VmId::parse(id).unwrap(), Power::Off);
        refused(
            answer(
                &mut vms,
                serde_json::json!({"op": "back_up_vm", "id": "0a1b2c3d4e61", "root": root, "folder": "", "stem": "20261112-030000",
            "label": "zdl-2026-11-12_05.00.00", "settings": "{}"}),
            ),
            "no virtual machine",
        );
        refused(
            answer(
                &mut vms,
                serde_json::json!({"op": "snapshot_vm", "id": "0a1b2c3d4e61", "label": "zdl-2026-11-12_05.00.00", "schedules": [], "note": null}),
            ),
            "no virtual machine",
        );
        assert_eq!(answer(&mut vms, serde_json::json!({"op": "delete_vm", "id": id})), Response::Done);
        let Response::VmSnapshots { zfs_list } = answer(&mut vms, serde_json::json!({"op": "vm_snapshots"}))
        else {
            panic!("no list")
        };
        assert!(!zfs_list.contains(id), "its snapshots went with it");
        assert_eq!(sim_digest("nvram").as_str().len(), 64);
    }

    #[test]
    fn what_the_simulated_host_refuses_and_how_its_vms_run() {
        let mut vms = SimVms::default();
        let id = "0a1b2c3d4e5f";
        refused(
            answer(&mut vms, serde_json::json!({"op": "enable_vms", "pool": "tank", "volume": 1})),
            "not installed",
        );
        refused(answer(&mut vms, serde_json::json!({"op": "delete_vm", "id": id})), "not set up");
        vms.installed = true;
        refused(answer(&mut vms, serde_json::json!({"op": "vm_images", "volume": 1})), "not set up there");
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "enable_vms", "pool": "tank", "volume": 1})),
            Response::Done
        );
        refused(
            answer(&mut vms, serde_json::json!({"op": "enable_vms", "pool": "tank", "volume": 2})),
            "not volume",
        );
        refused(
            answer(&mut vms, serde_json::json!({"op": "finish_vm_image_upload", "image": "00000000000a"})),
            "pipe",
        );
        refused(answer(&mut vms, serde_json::json!({"op": "open_vm_console", "id": id})), "descriptor");
        // Devices the simulated host has not.
        let with = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut spec = serde_json::to_value(spec(id)).unwrap();
            change(&mut spec);
            serde_json::json!({"op": "define_vm", "spec": spec})
        };
        let mac = "52:54:00:00:00:01";
        for (network, says) in [
            (serde_json::json!({"kind": "bridge", "bridge": "br9"}), "no bridge"),
            (serde_json::json!({"kind": "direct", "card": "eth9"}), "no wired card"),
            (serde_json::json!({"kind": "internal", "bridge": "lab9"}), "no internal network"),
        ] {
            let request = with(&|spec| {
                spec["nics"] = serde_json::json!([{"slot": 0, "model": "virtio", "network": network.clone(), "mac": mac}])
            });
            refused(answer(&mut vms, request), says);
        }
        refused(
            answer(
                &mut vms,
                with(&|spec| spec["drives"] = serde_json::json!([{"slot": 0, "image": "00000000000b"}])),
            ),
            "not in the library",
        );
        refused(
            answer(
                &mut vms,
                with(&|spec| spec["disks"][0]["location"] = serde_json::json!({"pool": "cold", "volume": 2})),
            ),
            "pool cold",
        );
        refused(
            answer(&mut vms, serde_json::json!({"op": "update_vm", "spec": spec(id), "remove": []})),
            "no virtual machine",
        );
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "define_vm", "spec": spec(id)})),
            Response::Done
        );
        let mut two = serde_json::to_value(spec(id)).unwrap();
        two["disks"] = serde_json::json!([{"slot": 0, "size": 1, "bus": "sata", "location": {"pool": "tank", "volume": 1}},
                                          {"slot": 1, "size": 2, "bus": "sata", "location": {"pool": "tank", "volume": 1}}]);
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "update_vm", "spec": two, "remove": []})),
            Response::Done
        );
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "update_vm", "spec": spec(id), "remove": [1]})),
            Response::Done
        );
        assert!(!vms.zvols.keys().any(|zvol| zvol.ends_with("-disk1")));
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "set_vm_autostart", "id": id, "autostart": true})),
            Response::Done
        );
        refused(
            answer(
                &mut vms,
                serde_json::json!({"op": "set_vm_autostart", "id": "0a1b2c3d4e60", "autostart": true}),
            ),
            "no virtual machine",
        );
        refused(
            answer(&mut vms, serde_json::json!({"op": "control_vm", "id": id, "action": "shutdown"})),
            "not running",
        );
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "control_vm", "id": id, "action": "start"})),
            Response::Done
        );
        assert_eq!(
            answer(&mut vms, serde_json::json!({"op": "control_vm", "id": id, "action": "reboot"})),
            Response::Done
        );
        let Response::VmStates { virsh_list } = answer(&mut vms, serde_json::json!({"op": "vm_states"}))
        else {
            panic!("no states")
        };
        assert!(
            virsh_list.contains(&format!("zendiloos-{id}")) && virsh_list.contains("running"),
            "{virsh_list}"
        );
        refused(
            answer(&mut vms, serde_json::json!({"op": "update_vm", "spec": spec(id), "remove": []})),
            "not shut off",
        );
        let root = serde_json::json!({"kind": "drive", "drive": "1234-ABCD"});
        let Response::VmBackedUp { consistency, .. } = answer(
            &mut vms,
            serde_json::json!({"op": "back_up_vm", "id": id, "root": root,
            "folder": "", "stem": "20261112-030000", "label": "zdl-2026-11-12_05.00.00", "settings": "{}"}),
        ) else {
            panic!("not backed up")
        };
        assert_eq!(consistency, VmConsistency::Crash, "while it runs");
        refused(
            answer(&mut vms, serde_json::json!({"op": "disable_vms", "pool": "tank", "volume": 1})),
            "holds the disk",
        );
    }

    #[test]
    fn the_console_speaks_rfb_without_authentication() {
        let mut vms = SimVms { installed: true, ..SimVms::default() };
        let id = VmId::parse("0a1b2c3d4e5f").unwrap();
        vms.domains.insert(id.clone(), Power::Off);
        assert!(vms.console(&id).1.is_none(), "not while it is off");
        vms.domains.insert(id.clone(), Power::Running);
        let mut console = UnixStream::from(vms.console(&id).1.unwrap());
        let mut version = [0u8; 12];
        console.read_exact(&mut version).unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        console.write_all(b"RFB 003.008\n").unwrap();
        let mut security = [0u8; 2];
        console.read_exact(&mut security).unwrap();
        assert_eq!(security, [1, 1]);
        console.write_all(&[1]).unwrap();
        let mut result = [0u8; 4];
        console.read_exact(&mut result).unwrap();
        assert_eq!(result, [0; 4]);
        console.write_all(&[1]).unwrap();
        let mut size = [0u8; 4];
        console.read_exact(&mut size).unwrap();
        assert_eq!(
            (u16::from_be_bytes([size[0], size[1]]), u16::from_be_bytes([size[2], size[3]])),
            (640, 400)
        );
    }
}
