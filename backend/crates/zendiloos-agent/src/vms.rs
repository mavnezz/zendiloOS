//! Virtual machines through libvirt (docs/architecture.md, 4.13).
//!
//! libvirt's daemon runs outside this sandbox and does the work: it starts
//! QEMU as `libvirt-qemu`, confined by AppArmor, and passt for user-mode
//! networking. The agent talks to it with `virsh` over its Unix socket and
//! only ever names domains `zendiloos-<id>`; it never lists, changes or
//! removes another. Each domain's XML is built here from the checked
//! [`VmSpec`], never from text a client sent, and kept as
//! `/etc/zendiloos/vms/zendiloos-<id>.xml`; a change defines it anew from
//! the whole spec, so nothing is ever edited in place.
//!
//! The VMs' storage is in one or more locations, each the dataset
//! `<pool>/zendiloos.vms` of a pool zendiloOS made, mounted by its own unit
//! at `/volume<N>/@vms`. A VM's disks are sparse zvols
//! `<pool>/zendiloos.vms/<id>-disk<N>` (`volmode=dev`: no partitions of them
//! show up on the host), each on the location it was made on; they grow
//! and are added or removed only while it is shut off. Its CD drives hold
//! images of the library, `/volume<N>/@vms/isos/<image>.iso` on the first
//! location (zendiloosd says which volume that is), copied there
//! from a share by a unit of its own (`copy-vm-iso`, see [`crate::vm_iso`])
//! or uploaded through a pipe into one (`receive-vm-image`, see
//! [`crate::vm_upload`]): QEMU never sees a share, and libvirt, which hands
//! the files it attaches to QEMU's account, never changes a file in one. A
//! drive's medium changes while the VM runs too.
//!
//! The console is QEMU's VNC server on a Unix socket in
//! `/run/zendiloos-vms` (`libvirt-qemu`'s, made by systemd-tmpfiles). The
//! agent opens it without following a link, connects, and hands the socket
//! to zendiloosd, as it hands a terminal's master.
//!
//! A VM may have a TPM 2.0 (swtpm, whose state libvirt keeps in
//! `/var/lib/libvirt/swtpm/<uuid>`, which a redefinition keeps as it keeps
//! the UUID) and UEFI with Secure Boot and Microsoft's keys enrolled
//! (Debian's `OVMF_CODE_4M.ms.fd` and `OVMF_VARS_4M.ms.fd`, which libvirt
//! picks from the firmware QEMU describes), and be given USB and PCI
//! devices of the host's ([`devices`]). Its disks are snapshotted together
//! ([`snapshots`]) and backed up into a share or onto a removable drive, and
//! restored, by units of their own ([`backups`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use zendiloos_agent_proto::{
    InterfaceName, PoolName, Response, VM_CONSOLES, VM_IMAGE_UPLOAD_MAX, VM_IMAGES_DIRECTORY, VMS_DATASET,
    VMS_DIRECTORY, VmAction, VmBoot, VmDisk, VmDiskBus, VmDiskSlot, VmDriveSlot, VmFirmware, VmId,
    VmImageFile, VmImageId, VmIsoSource, VmNetwork, VmNicModel, VmOs, VmSpec, VmStorage, VmUsbDevice,
    VolumeNumber,
};
use zendiloos_exec::{ExecError, Limits, Program, Surroundings};

use crate::operations::OperationError;
use crate::pools::HostPaths;
use crate::vm_iso::IsoJob;
use crate::vm_upload::UploadJob;
use crate::volume_archive::{UnitLine, encode};
pub use devices::DevicePaths;

/// libvirt's system instance; `virsh` is always told so, whatever its
/// environment would say.
const CONNECT: [&str; 2] = ["--connect", "qemu:///system"];
const QUICK: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 1024 * 1024 };
/// Starting a VM waits for QEMU; destroying a large zvol takes a while.
const SLOW: Limits = Limits { timeout: Duration::from_secs(5 * 60), max_stdout: 1024 * 1024 };
/// What an ISO's copy may take; zendiloosd waits two hours for a long request.
const COPY_FOR: Duration = Duration::from_secs(100 * 60);
/// What an upload may take, from the browser's first byte to its last: a
/// large image over a slow line.
const UPLOAD_FOR: Duration = Duration::from_secs(12 * 3600);
/// How long the end of an upload is waited for once its pipe is closed:
/// the image written out and named. Within a quick request's time.
const FINISH_WITHIN: Duration = Duration::from_secs(90);
/// Uploads under way at once.
const UPLOADS_AT_ONCE: usize = 4;
/// An upload's end nobody asked for is forgotten after this.
const UPLOAD_KEPT: Duration = Duration::from_secs(15 * 60);
/// Most of `/proc/cpuinfo` read: a line of flags per thread.
const MAX_CPUINFO: u64 = 8 * 1024 * 1024;
/// The unit an ISO is copied in reads the share, whoever owns the file.
const COPY_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH";
/// Marks the files zendiloOS writes for libvirt, so that one an
/// administrator wrote is never replaced.
const MARKER: &str = "# Written by zendiloOS";
/// What libvirt calls the states in which QEMU runs.
const ON: [&str; 6] = ["running", "idle", "blocked", "paused", "in shutdown", "pmsuspended"];
/// What a firmware's description says of OVMF with Microsoft's keys
/// enrolled, which Secure Boot needs.
const ENROLLED: [&str; 2] = ["\"enrolled-keys\"", "\"secure-boot\""];
/// A firmware's description read at most.
const MAX_DESCRIPTOR: u64 = 64 * 1024;

pub(crate) mod backups;
pub(crate) mod devices;
mod snapshots;

/// Where the agent looks at the machine and keeps the domains' XML; other
/// than the real places only in tests.
#[derive(Debug, Clone)]
pub struct VmPaths {
    pub kvm: PathBuf,
    pub cpuinfo: PathBuf,
    pub meminfo: PathBuf,
    /// The host's devices a VM can be given, the IOMMU's groups among them.
    pub devices: DevicePaths,
    /// The firmware QEMU describes, `/usr/share/qemu/firmware`: OVMF with
    /// Microsoft's keys enrolled is there for Secure Boot.
    pub firmware: PathBuf,
    /// libvirt's copies of the VMs' UEFI variables, `<domain>_VARS.fd`.
    pub nvram: PathBuf,
    /// The TPMs' states, `<uuid>/tpm2/` each, as swtpm keeps them.
    pub swtpm: PathBuf,
    /// Where systemd keeps transient units' files: those of backups and
    /// restores left running when the agent starts are there.
    pub transient: PathBuf,
    /// The accounts: swtpm's, whose a TPM's restored state becomes.
    pub passwd: PathBuf,
    /// The XML of each domain defined: `zendiloos-<id>.xml`.
    pub domains: PathBuf,
    /// Where the consoles' sockets are ([`VM_CONSOLES`]).
    pub consoles: PathBuf,
    /// Makes [`VmPaths::consoles`] at every boot, `libvirt-qemu`'s.
    pub tmpfiles: PathBuf,
    /// Lets the AppArmor profile libvirt makes for each VM use its console
    /// socket there: a file of zendiloOS's in the abstraction's own folder
    /// (Debian's QEMU driver clears `local/abstractions/libvirt-qemu` as
    /// obsolete).
    pub apparmor: PathBuf,
    /// Where the zvols' devices are: `/dev/zvol`.
    pub zvols: PathBuf,
    /// systemd-networkd's folder with zendiloOS's files, which say which
    /// bridges are internal networks.
    pub network: PathBuf,
    /// The rules that keep the NAS off them, as the network's changes
    /// write them ([`crate::internal_networks`]).
    pub internal: PathBuf,
}

impl Default for VmPaths {
    fn default() -> Self {
        Self {
            kvm: PathBuf::from("/dev/kvm"),
            cpuinfo: PathBuf::from("/proc/cpuinfo"),
            meminfo: PathBuf::from("/proc/meminfo"),
            devices: DevicePaths::default(),
            firmware: PathBuf::from("/usr/share/qemu/firmware"),
            nvram: PathBuf::from("/var/lib/libvirt/qemu/nvram"),
            swtpm: PathBuf::from("/var/lib/libvirt/swtpm"),
            transient: PathBuf::from("/run/systemd/transient"),
            passwd: PathBuf::from("/etc/passwd"),
            domains: PathBuf::from("/etc/zendiloos/vms"),
            consoles: PathBuf::from(VM_CONSOLES),
            tmpfiles: PathBuf::from("/etc/tmpfiles.d/zendiloos-vms.conf"),
            apparmor: PathBuf::from("/etc/apparmor.d/abstractions/libvirt-qemu.d/zendiloos-consoles"),
            zvols: PathBuf::from("/dev/zvol"),
            network: PathBuf::from("/etc/systemd/network"),
            internal: PathBuf::from(crate::network::INTERNAL_NETWORKS),
        }
    }
}

/// The programs the VMs need, from the agent's tools.
#[derive(Debug, Clone)]
pub(crate) struct VmTools {
    pub virsh: Program,
    pub zfs: Program,
    pub systemctl: Program,
    pub systemd_run: Program,
    /// Asks systemd-networkd whether a VM's bridge is one of the host's.
    pub busctl: Program,
    /// Waits for a snapshot's device to show, for a backup.
    pub udevadm: Program,
    /// Only looked for: libvirt's daemon and QEMU, which libvirt runs, and
    /// swtpm, which it starts for a TPM.
    pub libvirtd: PathBuf,
    pub qemu: PathBuf,
    pub swtpm: PathBuf,
    /// Run in transient units.
    pub tmpfiles: PathBuf,
    pub rm: PathBuf,
}

/// How an upload's unit ended.
#[derive(Debug, Clone)]
enum Received {
    Stored(u64),
    Refused(String),
    Failed(String),
}

/// An upload under way: its end, once the unit ended.
#[derive(Debug)]
struct Upload {
    ended: tokio::sync::watch::Receiver<Option<(Instant, Received)>>,
}

#[derive(Debug)]
pub(crate) struct Vms {
    tools: VmTools,
    paths: VmPaths,
    host: HostPaths,
    /// One change of a VM at a time: two deletes of one VM, or a define
    /// beside its delete, must not interleave.
    serial: tokio::sync::Mutex<()>,
    uploads: Mutex<BTreeMap<VmImageId, Upload>>,
}

fn refused(reason: impl Into<String>) -> OperationError {
    OperationError::Refused(reason.into())
}

/// The VMs' mount unit on a volume: `volume<N>-\x40vms.mount`.
pub(crate) fn vms_unit_name(volume: VolumeNumber) -> String {
    format!("volume{}-\\x40vms.mount", volume.get())
}

/// `/volume<N>/@vms`.
pub(crate) fn vms_root(host: &HostPaths, volume: VolumeNumber) -> PathBuf {
    host.volumes.join(format!("volume{}", volume.get())).join(VMS_DIRECTORY)
}

/// The locations of the VMs' storage: each mount unit of a volume's
/// `@vms` that names `<pool>/zendiloos.vms`, by volume. Two that name one
/// pool (a hand's work) count as none of that pool, so nothing goes to the
/// wrong place.
pub(crate) fn vms_locations(host: &HostPaths) -> Vec<VmStorage> {
    let Ok(entries) = fs::read_dir(&host.units) else { return Vec::new() };
    let mut found: Vec<VmStorage> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name.strip_prefix("volume").and_then(|rest| rest.strip_suffix("-\\x40vms.mount"))
        else {
            continue;
        };
        let Ok(volume) =
            number.parse::<u16>().map_err(|_| ()).and_then(|n| VolumeNumber::new(n).map_err(|_| ()))
        else {
            continue;
        };
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        let pool = text
            .lines()
            .find_map(|line| line.strip_prefix("What="))
            .and_then(|what| what.strip_suffix(&format!("/{VMS_DATASET}")))
            .and_then(|pool| PoolName::parse(pool).ok());
        if let Some(pool) = pool {
            found.push(VmStorage { pool, volume });
        }
    }
    let twice: Vec<PoolName> = found
        .iter()
        .filter(|location| found.iter().filter(|other| other.pool == location.pool).count() > 1)
        .map(|location| location.pool.clone())
        .collect();
    found.retain(|location| !twice.contains(&location.pool));
    found.sort_by_key(|location| location.volume.get());
    found
}

/// What a disk of a VM becomes on an update.
enum DiskPlan {
    Keep,
    Make,
    Grow,
}

impl Vms {
    pub(crate) fn new(tools: VmTools, paths: VmPaths, host: HostPaths) -> Self {
        Self { tools, paths, host, serial: tokio::sync::Mutex::new(()), uploads: Mutex::default() }
    }

    fn installed(&self) -> bool {
        self.tools.virsh.path().exists() && self.tools.libvirtd.exists() && self.tools.qemu.exists()
    }

    pub(crate) fn require_installed(&self) -> Result<(), OperationError> {
        if self.installed() {
            Ok(())
        } else {
            Err(refused("libvirt and QEMU are not installed: install the plugin first"))
        }
    }

    /// The bridge a VM goes on is a link of the host's and a bridge. This
    /// sandbox has a network of its own: its sysfs and netlink show only
    /// its loopback, so the host's links are asked of systemd-networkd over
    /// D-Bus, which knows every link of the host's, also those it does not
    /// manage (`GetLinkByName`), and describes each (`DescribeLink`: its
    /// kind and type). A link it does not know, or describes as anything
    /// but a bridge, is refused; so is every bridge where systemd-networkd
    /// does not run, as on a host with ifupdown or NetworkManager. Should
    /// it know the link but not describe it, the link is taken as it is:
    /// libvirt refuses to start a VM on anything but a bridge.
    async fn check_bridge(&self, bridge: &InterfaceName) -> Result<(), OperationError> {
        let no_bridge = || refused(format!("{} is no bridge of this machine", bridge.as_str()));
        let Some((index, _)) = crate::network::networkd_link(&self.tools.busctl, bridge).await else {
            return Err(no_bridge());
        };
        match crate::network::networkd_kind(&self.tools.busctl, index).await {
            Some((kind, kind_of_type)) => {
                let is_bridge =
                    kind.as_deref() == Some("bridge") || kind_of_type.as_deref() == Some("bridge");
                if is_bridge { Ok(()) } else { Err(no_bridge()) }
            }
            None => {
                tracing::warn!(bridge = bridge.as_str(), "vms: systemd-networkd did not describe the bridge");
                Ok(())
            }
        }
    }

    /// A card a VM goes on directly (macvtap) is a wired card or a bond of
    /// the host's, as systemd-networkd describes the link: of the type
    /// `ether` with no kind (a card; a `veth` or a VLAN has one), or of the
    /// kind `bond`. Anything else, or a link it does not know, is refused;
    /// one it knows but does not describe is taken as it is, as libvirt
    /// refuses to put macvtap on what cannot carry it.
    async fn check_card(&self, card: &InterfaceName) -> Result<(), OperationError> {
        let no_card = || refused(format!("{} is no wired card or bond of this machine", card.as_str()));
        let Some((index, _)) = crate::network::networkd_link(&self.tools.busctl, card).await else {
            return Err(no_card());
        };
        match crate::network::networkd_kind(&self.tools.busctl, index).await {
            Some((kind, kind_of_type)) => {
                let wired = kind.is_none() && kind_of_type.as_deref() == Some("ether");
                if wired || kind.as_deref() == Some("bond") { Ok(()) } else { Err(no_card()) }
            }
            None => {
                tracing::warn!(card = card.as_str(), "vms: systemd-networkd did not describe the card");
                Ok(())
            }
        }
    }

    /// An internal network is a bridge of the host's, one zendiloOS made
    /// without members or an address, and the NAS keeps itself off it now
    /// ([`crate::internal_networks`]): otherwise a VM on it could reach
    /// the NAS.
    async fn check_internal(&self, bridge: &InterfaceName) -> Result<(), OperationError> {
        self.check_bridge(bridge).await?;
        let kept_off = crate::internal_networks::keeps_off(
            &self.tools.systemctl,
            &self.paths.network,
            &self.paths.internal,
            bridge,
        )
        .await;
        if kept_off {
            Ok(())
        } else {
            Err(refused(format!(
                "{} is no internal network the NAS keeps itself off: a bridge zendiloOS made without members \
                 or an address",
                bridge.as_str()
            )))
        }
    }

    /// What the machine offers VMs.
    pub(crate) fn host_facts(&self) -> Result<Response, OperationError> {
        let mut cpuinfo = String::new();
        if let Ok(file) = fs::File::open(&self.paths.cpuinfo) {
            let _ = file.take(MAX_CPUINFO).read_to_string(&mut cpuinfo);
        }
        let (cores, cpu_virtualization) = cpu_facts(&cpuinfo);
        let meminfo = fs::read_to_string(&self.paths.meminfo).unwrap_or_default();
        let iommu = devices::iommu(&self.paths.devices, &cpuinfo).on;
        Ok(Response::VmHost {
            kvm: self.paths.kvm.exists(),
            cpu_virtualization,
            iommu,
            installed: self.installed(),
            swtpm: self.tools.swtpm.exists(),
            secure_boot: self.secure_boot_firmware(),
            cores,
            memory_bytes: memory_bytes(&meminfo),
            locations: vms_locations(&self.host),
        })
    }

    /// Whether QEMU describes an OVMF with Microsoft's keys enrolled and
    /// Secure Boot (Debian's `40-edk2-x86_64-secure-enrolled.json`), which
    /// libvirt picks for a VM with Secure Boot.
    fn secure_boot_firmware(&self) -> bool {
        let Ok(entries) = fs::read_dir(&self.paths.firmware) else { return false };
        entries.flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".json")).any(
            |entry| {
                let mut text = String::new();
                fs::File::open(entry.path())
                    .and_then(|file| file.take(MAX_DESCRIPTOR).read_to_string(&mut text))
                    .is_ok()
                    && ENROLLED.iter().all(|feature| text.contains(feature))
            },
        )
    }

    /// The host's USB and PCI devices, and the IOMMU.
    pub(crate) async fn host_devices(&self) -> Result<Response, OperationError> {
        let mut cpuinfo = String::new();
        if let Ok(file) = fs::File::open(&self.paths.cpuinfo) {
            let _ = file.take(MAX_CPUINFO).read_to_string(&mut cpuinfo);
        }
        Ok(Response::VmHostDevices {
            iommu: devices::iommu(&self.paths.devices, &cpuinfo),
            usb: devices::usb_devices(&self.paths.devices),
            pci: devices::pci_devices(&self.paths.devices, &self.host, &self.tools.busctl).await,
        })
    }

    /// After the VMs' dataset is mounted: the consoles' folder at every
    /// boot and now, AppArmor's leave to use it, and libvirt's daemon
    /// enabled and running.
    pub(crate) async fn prepare_libvirt(&self) -> Result<Response, OperationError> {
        self.require_installed()?;
        let consoles = self.paths.consoles.display();
        // `libvirt-qemu`'s, so QEMU makes its sockets there; the group root
        // and the set-group-ID bit give them root's group, so the agent
        // (root without CAP_DAC_OVERRIDE) may connect.
        let tmpfiles = format!(
            "{MARKER}: the consoles of virtual machines, QEMU's VNC sockets.\n\
             d {consoles} 2770 libvirt-qemu root -\n"
        );
        write_own(&self.paths.tmpfiles, &tmpfiles)?;
        let apparmor = format!(
            "{MARKER}: QEMU may use its console's socket (docs/architecture.md, 4.13).\n\
             {consoles}/ r,\n\
             {consoles}/*.vnc rw,\n"
        );
        if let Err(error) = write_own(&self.paths.apparmor, &apparmor) {
            // The socket's own path is in each VM's profile anyway.
            tracing::warn!(%error, "vms: cannot write AppArmor's local rules for the consoles");
        }
        self.make_consoles().await?;
        run(&self.tools.systemctl, &["daemon-reload"], QUICK).await?;
        // QEMU's output goes through virtlogd; without libvirt-daemon-system
        // nothing else enables its socket before the next boot.
        run(&self.tools.systemctl, &["enable", "--now", "virtlogd.socket"], QUICK).await?;
        run(&self.tools.systemctl, &["enable", "--now", "libvirtd.socket"], QUICK).await?;
        run(&self.tools.systemctl, &["enable", "libvirtd.service"], QUICK).await?;
        run(&self.tools.systemctl, &["restart", "libvirtd.service"], SLOW).await?;
        Ok(Response::Done)
    }

    /// The consoles' folder, made by systemd-tmpfiles outside this sandbox
    /// (`/run` is read-only here).
    async fn make_consoles(&self) -> Result<(), OperationError> {
        if self.paths.consoles.is_dir() {
            return Ok(());
        }
        let tmpfiles = self.tools.tmpfiles.to_string_lossy().into_owned();
        let conf = self.paths.tmpfiles.to_string_lossy().into_owned();
        let args = [
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--description=zendiloOS: the consoles' folder of virtual machines",
            tmpfiles.as_str(),
            "--create",
            conf.as_str(),
        ];
        run(&self.tools.systemd_run, &args, QUICK).await
    }

    // The image library.

    /// `/volume<N>/@vms/isos`.
    fn images_folder(&self, volume: VolumeNumber) -> PathBuf {
        vms_root(&self.host, volume).join(VM_IMAGES_DIRECTORY)
    }

    fn image_file(&self, volume: VolumeNumber, image: &VmImageId) -> PathBuf {
        self.images_folder(volume).join(image.file_name())
    }

    /// Copies the ISO from the share into the library as `image`, in a
    /// unit of its own that may write only the VMs' folder, reporting how
    /// far it is.
    pub(crate) async fn copy_image(
        &self,
        image: &VmImageId,
        volume: VolumeNumber,
        source: &VmIsoSource,
    ) -> Result<Response, OperationError> {
        let storage = self.storage_on(volume)?;
        let share = self
            .host
            .volumes
            .join(format!("volume{}", source.share.volume.get()))
            .join(source.share.share.as_str());
        if self.host.share_mounts {
            zendiloos_fileops::ShareRoot::open(
                &self.host.volumes,
                source.share.volume,
                &source.share.share,
                true,
            )
            .map_err(|e| refused(format!("the share {} cannot be used: {e}", source.share.share.as_str())))?;
        }
        let root = vms_root(&self.host, storage.volume);
        let job = IsoJob {
            share,
            path: source.path.clone(),
            folder: self.images_folder(storage.volume),
            name: image.file_name(),
        };
        let unit = format!("zendiloos-vm-iso-{}.service", image.as_str());
        let description =
            format!("zendiloOS: copy an ISO into the library of virtual machines ({})", image.as_str());
        let args = unit_args(
            &unit,
            &description,
            &root,
            COPY_CAPABILITIES,
            COPY_FOR,
            &self.host.agent,
            "copy-vm-iso",
            &encode(&job),
        );
        let bytes = self.copy_unit(&unit, &args).await?;
        Ok(Response::VmImageStored { bytes })
    }

    /// Runs the copy's unit; its progress becomes the request's, its
    /// answer the bytes copied.
    async fn copy_unit(&self, unit: &str, args: &[String]) -> Result<u64, OperationError> {
        let until = Instant::now() + COPY_FOR;
        let mut said: Option<UnitLine> = None;
        let ran = {
            let mut heard = |line: &str| match serde_json::from_str::<UnitLine>(line) {
                Ok(UnitLine::Progress { percent, .. }) => crate::progress::step("copying_iso", Some(percent)),
                Ok(other) => said = Some(other),
                Err(_) => tracing::debug!(line, "an ISO's unit wrote"),
            };
            let limits =
                Limits { timeout: until.saturating_duration_since(Instant::now()), max_stdout: 1024 * 1024 };
            let surroundings = Surroundings::default();
            let running =
                zendiloos_exec::run_lines(&self.tools.systemd_run, args, &surroundings, limits, &mut heard);
            tokio::select! {
                ran = running => Some(ran),
                () = crate::waiting::given_up() => None,
            }
        };
        let output = match ran {
            Some(Ok(output)) => output,
            Some(Err(ExecError::Timeout { .. })) | None => {
                let _ = run(&self.tools.systemctl, &["stop", unit], SLOW).await;
                return Err(OperationError::Failed(format!("{unit} was stopped before the ISO was copied")));
            }
            Some(Err(error)) => return Err(error.into()),
        };
        unit_end(said, output.status.success(), &output.stderr)
    }

    /// The library's images: regular files named `<image>.iso`.
    pub(crate) fn images(&self, volume: VolumeNumber) -> Result<Response, OperationError> {
        let storage = self.storage_on(volume)?;
        let folder = self.images_folder(storage.volume);
        let mut files = Vec::new();
        match fs::read_dir(&folder) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let Some(image) = VmImageId::of_file_name(&entry.file_name().to_string_lossy()) else {
                        continue;
                    };
                    // Not followed: a link is no image.
                    if let Ok(meta) = fs::symlink_metadata(entry.path())
                        && meta.is_file()
                    {
                        files.push(VmImageFile { image, bytes: meta.len() });
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(OperationError::Read(folder, error)),
        }
        files.sort_by(|a, b| a.image.cmp(&b.image));
        Ok(Response::VmImages { files })
    }

    /// Removes an image's file; zendiloosd checked no VM has it.
    pub(crate) async fn delete_image(
        &self,
        image: &VmImageId,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let storage = self.storage_on(volume)?;
        let file = self.image_file(storage.volume, image);
        if fs::symlink_metadata(&file).is_ok() {
            self.remove_file(&file, &vms_root(&self.host, storage.volume)).await?;
        }
        Ok(Response::Done)
    }

    /// `rm` in a transient unit that may write only the VMs' folder; the
    /// path is made of checked parts.
    async fn remove_file(&self, file: &Path, writable: &Path) -> Result<(), OperationError> {
        let rm = self.tools.rm.to_string_lossy().into_owned();
        let file = file.to_string_lossy().into_owned();
        let writable = format!("--property=ReadWritePaths={}", writable.display());
        let mut args = vec![
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--description=zendiloOS: remove an image of virtual machines",
        ];
        args.extend(crate::volumes::SANDBOX);
        args.extend([
            writable.as_str(),
            "--property=CapabilityBoundingSet=",
            rm.as_str(),
            "--force",
            "--",
            &file,
        ]);
        run(&self.tools.systemd_run, &args, QUICK).await
    }

    /// Starts an upload's unit, which reads the pipe whose writing end is
    /// handed over. It may write only the VMs' folder and has no capability.
    pub(crate) async fn open_upload(
        &self,
        image: &VmImageId,
        volume: VolumeNumber,
        bytes: u64,
    ) -> Result<(Response, OwnedFd), OperationError> {
        let storage = self.storage_on(volume)?;
        if bytes == 0 || bytes > VM_IMAGE_UPLOAD_MAX {
            return Err(refused(format!(
                "an uploaded image has 1 byte to {} GiB",
                VM_IMAGE_UPLOAD_MAX >> 30
            )));
        }
        let root = vms_root(&self.host, storage.volume);
        let free = rustix::fs::statvfs(&root).map_err(|e| OperationError::Read(root.clone(), e.into()))?;
        let available = free.f_bavail.saturating_mul(free.f_frsize);
        if bytes > available {
            return Err(refused(format!(
                "the image needs {} MiB, and the pool has {} MiB free",
                bytes.div_ceil(1 << 20),
                available >> 20
            )));
        }
        if self.image_file(storage.volume, image).exists() {
            return Err(OperationError::Conflict);
        }
        let (reader, writer) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|e| OperationError::Failed(format!("no pipe for the upload: {e}")))?;
        let (told, ended) = tokio::sync::watch::channel(None);
        {
            let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
            uploads.retain(|_, upload| match &*upload.ended.borrow() {
                Some((at, _)) => at.elapsed() < UPLOAD_KEPT,
                None => true,
            });
            if uploads.contains_key(image) {
                return Err(OperationError::Conflict);
            }
            if uploads.len() >= UPLOADS_AT_ONCE {
                return Err(refused(format!("{UPLOADS_AT_ONCE} uploads of images are under way already")));
            }
            uploads.insert(image.clone(), Upload { ended });
        }
        let job = UploadJob { folder: self.images_folder(storage.volume), name: image.file_name(), bytes };
        let unit = upload_unit(image);
        let description = format!("zendiloOS: receive an image of virtual machines ({})", image.as_str());
        let args = unit_args(
            &unit,
            &description,
            &root,
            "",
            UPLOAD_FOR,
            &self.host.agent,
            "receive-vm-image",
            &encode(&job),
        );
        let (systemd_run, systemctl) = (self.tools.systemd_run.clone(), self.tools.systemctl.clone());
        tokio::spawn(async move {
            let limits = Limits { timeout: UPLOAD_FOR, max_stdout: 64 * 1024 };
            let ran = zendiloos_exec::run_on(&systemd_run, &args, fs::File::from(reader), limits).await;
            let received = match ran {
                Ok(output) => {
                    let said = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .filter_map(|line| serde_json::from_str::<UnitLine>(line).ok())
                        .next_back();
                    match unit_end(said, output.status.success(), &output.stderr) {
                        Ok(bytes) => Received::Stored(bytes),
                        Err(OperationError::Refused(reason)) => Received::Refused(reason),
                        Err(error) => Received::Failed(error.to_string()),
                    }
                }
                Err(error) => {
                    if matches!(error, ExecError::Timeout { .. }) {
                        let _ = run(&systemctl, &["stop", unit.as_str()], SLOW).await;
                    }
                    Received::Failed(format!("the upload's unit: {error}"))
                }
            };
            told.send_replace(Some((Instant::now(), received)));
        });
        Ok((Response::VmImageUpload {}, writer))
    }

    /// Waits for the end of an upload whose pipe is closed.
    pub(crate) async fn finish_upload(&self, image: &VmImageId) -> Result<Response, OperationError> {
        let upload = self.uploads.lock().unwrap_or_else(PoisonError::into_inner).remove(image);
        let Some(mut upload) = upload else {
            return Err(refused(format!("no upload of image {} is under way", image.as_str())));
        };
        let ended = tokio::time::timeout(FINISH_WITHIN, async {
            let ended = upload.ended.wait_for(Option::is_some).await;
            ended.ok().and_then(|ended| (*ended).as_ref().map(|(_, received)| received.clone()))
        })
        .await;
        let received = match ended {
            Ok(received) => received,
            Err(_) => {
                let _ = run(&self.tools.systemctl, &["stop", upload_unit(image).as_str()], SLOW).await;
                return Err(OperationError::Failed(
                    "the upload's unit did not finish; it was stopped".into(),
                ));
            }
        };
        match received {
            Some(Received::Stored(bytes)) => Ok(Response::VmImageStored { bytes }),
            Some(Received::Refused(reason)) => Err(OperationError::Refused(reason)),
            Some(Received::Failed(reason)) => Err(OperationError::Failed(reason)),
            None => Err(OperationError::Failed("the upload's unit ended without a word".into())),
        }
    }

    // The VMs.

    /// The location of the VMs' storage on `volume`.
    fn storage_on(&self, volume: VolumeNumber) -> Result<VmStorage, OperationError> {
        let locations = vms_locations(&self.host);
        if locations.is_empty() {
            return Err(refused("the virtual machines' storage is not set up"));
        }
        locations.into_iter().find(|location| location.volume == volume).ok_or_else(|| {
            refused(format!("no location of the virtual machines' storage is on volume {}", volume.get()))
        })
    }

    /// `location` is one of the VMs' storage, its pool on its volume.
    fn location(&self, location: &VmStorage) -> Result<(), OperationError> {
        let found = self.storage_on(location.volume)?;
        if found.pool != location.pool {
            return Err(refused(format!(
                "the virtual machines' storage on volume {} is not on pool {}",
                location.volume.get(),
                location.pool.as_str()
            )));
        }
        Ok(())
    }

    /// Every location a VM's disks are on is one of the storage's, with its
    /// dataset there.
    async fn locations_of(&self, spec: &VmSpec) -> Result<(), OperationError> {
        for location in spec.locations() {
            self.location(location)?;
            let dataset = Self::dataset(&location.pool);
            if !self.exists(&dataset).await? {
                return Err(refused(format!("there is no dataset {dataset}")));
            }
        }
        Ok(())
    }

    fn dataset(pool: &PoolName) -> String {
        format!("{}/{VMS_DATASET}", pool.as_str())
    }

    fn zvol(pool: &PoolName, id: &VmId, slot: VmDiskSlot) -> String {
        format!("{}/{}", Self::dataset(pool), id.disk(slot))
    }

    /// A disk's zvol, on its location.
    fn disk_zvol(spec: &VmSpec, disk: &VmDisk) -> String {
        Self::zvol(&disk.location.pool, &spec.id, disk.slot)
    }

    /// The zvols of the VM's disk in `slot` on every location there is,
    /// those that are there.
    async fn zvols_in(&self, id: &VmId, slot: VmDiskSlot) -> Result<Vec<String>, OperationError> {
        let mut found = Vec::new();
        for location in vms_locations(&self.host) {
            let zvol = Self::zvol(&location.pool, id, slot);
            if self.exists(&zvol).await? {
                found.push(zvol);
            }
        }
        Ok(found)
    }

    fn domain_file(&self, id: &VmId) -> PathBuf {
        self.paths.domains.join(format!("{}.xml", id.domain()))
    }

    /// Marks a domain whose UEFI variables start afresh at its next start
    /// (`virsh start --reset-nvram`).
    fn reset_marker(&self, id: &VmId) -> PathBuf {
        self.paths.domains.join(format!("{}.reset-nvram", id.domain()))
    }

    /// What libvirt says the domain does (`running`, `shut off`, …); none
    /// if it is not defined.
    async fn domain_state(&self, id: &VmId) -> Result<Option<String>, OperationError> {
        let domain = id.domain();
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&["domstate", &domain]), QUICK).await?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
    }

    async fn defined(&self, id: &VmId) -> Result<bool, OperationError> {
        Ok(self.domain_state(id).await?.is_some())
    }

    /// The UUID libvirt gave a defined domain. Defining it again must name
    /// it: XML without one is a new domain to libvirt, refused as long as
    /// one of the same name is there.
    async fn domain_uuid(&self, id: &VmId) -> Result<String, OperationError> {
        let domain = id.domain();
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&["domuuid", &domain]), QUICK).await?;
        let uuid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.status.success() || !is_uuid(&uuid) {
            return Err(OperationError::Failed(format!("libvirt gave no UUID for {domain}: {uuid}")));
        }
        Ok(uuid)
    }

    /// The zvol's size in bytes; none if there is no such zvol.
    async fn volsize(&self, dataset: &str) -> Result<Option<u64>, OperationError> {
        let output = zendiloos_exec::run(
            &self.tools.zfs,
            ["get", "-H", "-p", "-o", "value", "volsize", dataset],
            QUICK,
        )
        .await?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse().ok())
    }

    async fn exists(&self, dataset: &str) -> Result<bool, OperationError> {
        let output =
            zendiloos_exec::run(&self.tools.zfs, ["list", "-H", "-o", "name", dataset], QUICK).await?;
        Ok(output.status.success())
    }

    /// The images the drives hold are in the library, the bridges the cards
    /// go on are the host's, the cards they go on directly are its wired
    /// cards or bonds, and the internal networks are kept off the NAS.
    async fn check_devices(&self, spec: &VmSpec) -> Result<(), OperationError> {
        if spec.images().next().is_some() {
            self.storage_on(spec.library)?;
        }
        for image in spec.images() {
            let file = self.image_file(spec.library, image);
            if !fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file()) {
                return Err(refused(format!("the image {} is not in the library", image.as_str())));
            }
        }
        let mut asked = BTreeSet::new();
        for nic in spec.nics() {
            match &nic.network {
                VmNetwork::User => {}
                VmNetwork::Bridge { bridge } if asked.insert(("bridge", bridge.as_str())) => {
                    self.check_bridge(bridge).await?;
                }
                VmNetwork::Direct { card } if asked.insert(("direct", card.as_str())) => {
                    self.check_card(card).await?;
                }
                VmNetwork::Internal { bridge } if asked.insert(("internal", bridge.as_str())) => {
                    self.check_internal(bridge).await?;
                }
                VmNetwork::Bridge { .. } | VmNetwork::Direct { .. } | VmNetwork::Internal { .. } => {}
            }
        }
        if spec.tpm() && !self.tools.swtpm.exists() {
            return Err(refused("swtpm is not installed: install the plugin's packages for a TPM first"));
        }
        if spec.secure_boot() && !self.secure_boot_firmware() {
            return Err(refused(
                "there is no OVMF with Microsoft's keys enrolled for Secure Boot: install Debian's ovmf",
            ));
        }
        if !spec.pci().is_empty() {
            let iommu = devices::iommu(&self.paths.devices, "").on;
            let found = devices::pci_devices(&self.paths.devices, &self.host, &self.tools.busctl).await;
            devices::check_pci(spec.pci(), &found, iommu)?;
        }
        Ok(())
    }

    /// Before a start: the host's devices the domain is given are there,
    /// each USB device once (libvirt could not tell two apart), and the
    /// PCI devices may still go to it.
    async fn check_handed_over(&self, id: &VmId) -> Result<(), OperationError> {
        let Ok(xml) = fs::read_to_string(self.domain_file(id)) else { return Ok(()) };
        let (usb, pci) = devices::handed_over(&xml);
        for device in usb {
            match devices::usb_count(&self.paths.devices, device) {
                1 => {}
                0 => {
                    return Err(refused(format!(
                        "the USB device {} the virtual machine is given is not plugged in",
                        device.id()
                    )));
                }
                _ => {
                    return Err(refused(format!(
                        "more than one USB device {} is plugged in, and they cannot be told apart: unplug \
                         all but one",
                        device.id()
                    )));
                }
            }
        }
        if !pci.is_empty() {
            let iommu = devices::iommu(&self.paths.devices, "").on;
            let found = devices::pci_devices(&self.paths.devices, &self.host, &self.tools.busctl).await;
            devices::check_pci(&pci, &found, iommu)?;
        }
        Ok(())
    }

    async fn make_disk(&self, spec: &VmSpec, disk: &VmDisk) -> Result<(), OperationError> {
        let zvol = Self::disk_zvol(spec, disk);
        let size = format!("{}G", disk.size.gib());
        run(&self.tools.zfs, &["create", "-s", "-V", &size, "-o", "volmode=dev", &zvol], SLOW).await
    }

    async fn destroy_disks(&self, zvols: &[String]) {
        for zvol in zvols {
            let _ = run(&self.tools.zfs, &["destroy", zvol], SLOW).await;
        }
    }

    /// Makes a new VM's disks, each on its location, and defines its domain.
    pub(crate) async fn define(&self, spec: &VmSpec) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        self.locations_of(spec).await?;
        if self.defined(&spec.id).await? {
            return Err(OperationError::Conflict);
        }
        self.check_devices(spec).await?;
        let mut made = Vec::new();
        for disk in spec.disks() {
            let zvol = Self::disk_zvol(spec, disk);
            if self.exists(&zvol).await? {
                continue;
            }
            if let Err(error) = self.make_disk(spec, disk).await {
                self.destroy_disks(&made).await;
                return Err(error);
            }
            made.push(zvol);
        }
        if let Err(error) = self.define_domain(spec, false).await {
            self.destroy_disks(&made).await;
            return Err(error);
        }
        Ok(Response::Done)
    }

    /// Changes a VM that is shut off: its disks made, grown or destroyed as
    /// `spec` and `remove` say, its domain defined anew. Nothing is changed
    /// if a disk would shrink.
    pub(crate) async fn update(
        &self,
        spec: &VmSpec,
        remove: &[VmDiskSlot],
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        self.locations_of(spec).await?;
        self.not_busy(&spec.id).await?;
        match self.domain_state(&spec.id).await? {
            None => return Err(refused(format!("there is no virtual machine {}", spec.id.as_str()))),
            Some(state) if state != "shut off" => {
                return Err(refused(
                    "the virtual machine is not shut off: its hardware changes only while it is off",
                ));
            }
            Some(_) => {}
        }
        if let Some(slot) = remove.iter().find(|slot| spec.disks().iter().any(|disk| disk.slot == **slot)) {
            return Err(refused(format!("disk {} is both kept and removed", slot.get())));
        }
        self.check_devices(spec).await?;
        let mut plan = Vec::new();
        for disk in spec.disks() {
            let zvol = Self::disk_zvol(spec, disk);
            let size = self.volsize(&zvol).await?;
            // A disk stays on the location it was made on.
            if size.is_none()
                && let Some(elsewhere) = self.zvols_in(&spec.id, disk.slot).await?.first()
            {
                return Err(refused(format!(
                    "disk {} lies on {elsewhere}: a disk stays where it was made",
                    disk.slot.get()
                )));
            }
            let step = match size {
                None => DiskPlan::Make,
                Some(size) if size < disk.size.bytes() => DiskPlan::Grow,
                Some(size) if size > disk.size.bytes() => {
                    return Err(refused(format!(
                        "disk {} has {} GiB: a disk only grows",
                        disk.slot.get(),
                        size.div_ceil(1 << 30)
                    )));
                }
                Some(_) => DiskPlan::Keep,
            };
            plan.push((disk, step));
        }
        let mut made = Vec::new();
        for (disk, step) in plan {
            let zvol = Self::disk_zvol(spec, disk);
            let done = match step {
                DiskPlan::Keep => Ok(()),
                DiskPlan::Make => self.make_disk(spec, disk).await.map(|()| made.push(zvol.clone())),
                DiskPlan::Grow => {
                    let size = format!("volsize={}G", disk.size.gib());
                    run(&self.tools.zfs, &["set", &size, &zvol], SLOW).await
                }
            };
            if let Err(error) = done {
                self.destroy_disks(&made).await;
                return Err(error);
            }
        }
        if let Err(error) = self.define_domain(spec, true).await {
            self.destroy_disks(&made).await;
            return Err(error);
        }
        for slot in remove {
            for zvol in self.zvols_in(&spec.id, *slot).await? {
                run(&self.tools.zfs, &["destroy", "-r", &zvol], SLOW).await?;
            }
        }
        Ok(Response::Done)
    }

    /// Puts the image `spec` has in drive `slot` into it, or ejects it: the
    /// domain defined anew, and the medium changed at once while it runs.
    pub(crate) async fn change_media(
        &self,
        spec: &VmSpec,
        slot: VmDriveSlot,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        self.locations_of(spec).await?;
        let Some(state) = self.domain_state(&spec.id).await? else {
            return Err(refused(format!("there is no virtual machine {}", spec.id.as_str())));
        };
        let drive = spec.drive(slot).ok_or_else(|| refused(format!("there is no drive {}", slot.get())))?;
        self.check_devices(spec).await?;
        self.define_domain(spec, true).await?;
        if !ON.contains(&state.as_str()) {
            return Ok(Response::Done);
        }
        let domain = spec.id.domain();
        let target = drive_target(slot);
        let file = drive
            .image
            .as_ref()
            .map(|image| self.image_file(spec.library, image).to_string_lossy().into_owned());
        let mut args = vec!["change-media", domain.as_str(), target.as_str()];
        match &file {
            Some(file) => args.extend([file.as_str(), "--update"]),
            None => args.push("--eject"),
        }
        // A guest that locked its tray holds the medium otherwise.
        args.extend(["--live", "--force"]);
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&args), SLOW).await?;
        if output.status.success() {
            return Ok(Response::Done);
        }
        let complaint = output.stderr.trim().trim_start_matches("error: ").replace('\n', " ");
        Err(OperationError::Failed(format!("the medium did not change while the VM runs: {complaint}")))
    }

    /// Gives the VM the USB devices `spec` names: the domain defined anew
    /// and, while it runs, `detach` pulled out of it and `attach` plugged
    /// in at once, each present exactly once.
    pub(crate) async fn change_usb(
        &self,
        spec: &VmSpec,
        attach: &[VmUsbDevice],
        detach: &[VmUsbDevice],
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        self.locations_of(spec).await?;
        let Some(state) = self.domain_state(&spec.id).await? else {
            return Err(refused(format!("there is no virtual machine {}", spec.id.as_str())));
        };
        if attach.iter().any(|device| !spec.usb().contains(device))
            || detach.iter().any(|device| spec.usb().contains(device))
        {
            return Err(refused("the USB devices plugged in or out are not those the VM is given"));
        }
        let running = ON.contains(&state.as_str());
        if running {
            for device in attach {
                if devices::usb_count(&self.paths.devices, *device) != 1 {
                    return Err(refused(format!(
                        "the USB device {} is not plugged in exactly once",
                        device.id()
                    )));
                }
            }
        }
        self.check_devices(spec).await?;
        self.define_domain(spec, true).await?;
        if !running {
            return Ok(Response::Done);
        }
        let domain = spec.id.domain();
        let file = self.paths.domains.join(format!("{domain}.usb.xml"));
        let path = file.to_string_lossy().into_owned();
        let plugged = async {
            for (device, command) in detach
                .iter()
                .map(|device| (device, "detach-device"))
                .chain(attach.iter().map(|device| (device, "attach-device")))
            {
                crate::durable::replace(&file, usb_hostdev(*device, "").as_bytes())?;
                let output =
                    zendiloos_exec::run(&self.tools.virsh, virsh(&[command, &domain, &path, "--live"]), SLOW)
                        .await?;
                if !output.status.success() {
                    let complaint = output.stderr.trim().trim_start_matches("error: ").replace('\n', " ");
                    return Err(OperationError::Failed(format!(
                        "the USB device {} was not plugged {} while the VM runs: {complaint}",
                        device.id(),
                        if command == "attach-device" { "in" } else { "out" }
                    )));
                }
            }
            Ok(())
        }
        .await;
        let _ = fs::remove_file(&file);
        plugged.map(|()| Response::Done)
    }

    /// Writes the domain's XML and defines it; the file kept is libvirt's
    /// definition, also when libvirt refuses the new one. `existing`: the
    /// domain is defined already, and autostart follows `spec` either way.
    async fn define_domain(&self, spec: &VmSpec, existing: bool) -> Result<(), OperationError> {
        let console = self.paths.consoles.join(format!("{}.vnc", spec.id.as_str()));
        let uuid = if existing { Some(self.domain_uuid(&spec.id).await?) } else { None };
        let places = Places {
            zvols: self.paths.zvols.clone(),
            images: self.images_folder(spec.library),
            console,
            uuid,
        };
        let xml = domain_xml(spec, &places);
        fs::create_dir_all(&self.paths.domains)
            .map_err(|e| OperationError::Write(self.paths.domains.clone(), e))?;
        let file = self.domain_file(&spec.id);
        // Another firmware, or Secure Boot switched: the UEFI variables
        // start afresh from the new firmware's template at the next start.
        let before = fs::read_to_string(&file).ok();
        let fresh_nvram = existing && before.is_some_and(|before| os_section(&before) != os_section(&xml));
        let new = file.with_extension("xml.new");
        crate::durable::replace(&new, xml.as_bytes())?;
        let path = new.to_string_lossy().into_owned();
        if let Err(error) = run(&self.tools.virsh, &virsh(&["define", "--validate", &path]), QUICK).await {
            let _ = fs::remove_file(&new);
            return Err(error);
        }
        fs::rename(&new, &file).map_err(|e| OperationError::Write(file.clone(), e))?;
        if fresh_nvram {
            crate::durable::replace(&self.reset_marker(&spec.id), b"")?;
        }
        let domain = spec.id.domain();
        if spec.autostart {
            run(&self.tools.virsh, &virsh(&["autostart", &domain]), QUICK).await?;
        } else if existing {
            run(&self.tools.virsh, &virsh(&["autostart", "--disable", &domain]), QUICK).await?;
        }
        Ok(())
    }

    pub(crate) async fn set_autostart(&self, id: &VmId, autostart: bool) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_defined(id).await?;
        let domain = id.domain();
        let args: &[&str] =
            if autostart { &["autostart", &domain] } else { &["autostart", "--disable", &domain] };
        run(&self.tools.virsh, &virsh(args), QUICK).await?;
        Ok(Response::Done)
    }

    async fn require_defined(&self, id: &VmId) -> Result<(), OperationError> {
        self.require_installed()?;
        if self.defined(id).await? {
            Ok(())
        } else {
            Err(refused(format!("there is no virtual machine {}", id.as_str())))
        }
    }

    pub(crate) async fn control(&self, id: &VmId, action: VmAction) -> Result<Response, OperationError> {
        self.require_defined(id).await?;
        let domain = id.domain();
        let reset = self.reset_marker(id);
        let args: Vec<&str> = match action {
            VmAction::Start => {
                self.check_handed_over(id).await?;
                // Gone after a restart until systemd-tmpfiles made it again.
                self.make_consoles().await?;
                if reset.exists() { vec!["start", "--reset-nvram", &domain] } else { vec!["start", &domain] }
            }
            VmAction::Shutdown => vec!["shutdown", "--mode", "acpi", &domain],
            VmAction::ForceOff => vec!["destroy", &domain],
            VmAction::Reboot => vec!["reboot", "--mode", "acpi", &domain],
        };
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&args), SLOW).await?;
        if output.status.success() {
            if action == VmAction::Start {
                let _ = fs::remove_file(&reset);
            }
            return Ok(Response::Done);
        }
        // libvirt's own words: "domain is already active", "domain is not running".
        let complaint = output.stderr.trim().trim_start_matches("error: ").replace('\n', " ");
        Err(refused(complaint))
    }

    /// Powers the VM off, undefines it, destroys its disks on every location
    /// and removes its XML; the library's images stay. What is gone already
    /// is no failure, so a VM whose creation failed halfway goes too.
    pub(crate) async fn delete(&self, id: &VmId) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.not_busy(id).await?;
        if vms_locations(&self.host).is_empty() {
            return Err(refused("the virtual machines' storage is not set up"));
        }
        let domain = id.domain();
        if self.installed() && self.defined(id).await? {
            // Not running is fine; anything else shows in undefine.
            let _ = zendiloos_exec::run(&self.tools.virsh, virsh(&["destroy", &domain]), SLOW).await;
            let undefine =
                ["undefine", "--nvram", "--tpm", "--managed-save", "--snapshots-metadata", domain.as_str()];
            run(&self.tools.virsh, &virsh(&undefine), QUICK).await?;
        }
        for slot in (0..).map_while(|slot| VmDiskSlot::new(slot).ok()) {
            for zvol in self.zvols_in(id, slot).await? {
                // Its snapshots with it.
                run(&self.tools.zfs, &["destroy", "-r", &zvol], SLOW).await?;
            }
        }
        for file in
            [self.domain_file(id), self.domain_file(id).with_extension("xml.new"), self.reset_marker(id)]
        {
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(OperationError::Write(file, error)),
            }
        }
        Ok(Response::Done)
    }

    /// `virsh list --all`, for zendiloosd to read.
    pub(crate) async fn states(&self) -> Result<Response, OperationError> {
        if !self.installed() {
            return Ok(Response::VmStates { virsh_list: String::new() });
        }
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&["list", "--all"]), QUICK)
            .await?
            .success(&self.tools.virsh)?;
        let virsh_list = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(Response::VmStates { virsh_list })
    }

    /// `virsh domstats` once for every running domain, without waiting for
    /// a guest that does not answer; only zendiloOS's domains are passed
    /// on.
    pub(crate) async fn stats(&self) -> Result<Response, OperationError> {
        if !self.installed() {
            return Ok(Response::VmStats { domstats: String::new() });
        }
        let args = ["domstats", "--list-active", "--nowait", "--cpu-total", "--balloon", "--interface"];
        let output =
            zendiloos_exec::run(&self.tools.virsh, virsh(&args), QUICK).await?.success(&self.tools.virsh)?;
        let text = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(Response::VmStats { domstats: own_domains(&text) })
    }

    /// What every location's dataset and the zvols on it hold, for
    /// zendiloosd to read: `zfs list` of the locations' datasets alone.
    pub(crate) async fn storage_usage(&self) -> Result<Response, OperationError> {
        let datasets: Vec<String> =
            vms_locations(&self.host).iter().map(|location| Self::dataset(&location.pool)).collect();
        if datasets.is_empty() {
            return Ok(Response::VmStorageUsage { zfs_list: String::new() });
        }
        let mut args = vec![
            "list",
            "-H",
            "-p",
            "-r",
            "-t",
            "filesystem,volume",
            "-o",
            "name,type,used,usedbydataset,referenced,available,volsize",
        ];
        args.extend(datasets.iter().map(String::as_str));
        let output = zendiloos_exec::run(&self.tools.zfs, args, QUICK).await?.success(&self.tools.zfs)?;
        let zfs_list = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        Ok(Response::VmStorageUsage { zfs_list })
    }

    /// Connects to the VM's console. The socket is opened without following
    /// a link and must be one: QEMU's account owns the folder, and a link
    /// it put there must never lead the agent to another socket (Docker's)
    /// that zendiloosd would then hold.
    pub(crate) async fn open_console(&self, id: &VmId) -> Result<(Response, OwnedFd), OperationError> {
        use rustix::fs::{FileType, Mode, OFlags};

        let path = self.paths.consoles.join(format!("{}.vnc", id.as_str()));
        let opened =
            rustix::fs::open(&path, OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty());
        let handle = match opened {
            Ok(handle) => handle,
            Err(rustix::io::Errno::NOENT) => {
                return Err(refused("the virtual machine is not running, or has no console"));
            }
            Err(error) => return Err(OperationError::Read(path, error.into())),
        };
        let stat = rustix::fs::fstat(&handle).map_err(|e| OperationError::Read(path.clone(), e.into()))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Socket {
            return Err(refused(format!("{} is no socket", path.display())));
        }
        // Through the descriptor: the very socket looked at, whatever the
        // name leads to by now.
        let through = format!("/proc/self/fd/{}", std::os::fd::AsRawFd::as_raw_fd(&handle));
        let stream = tokio::time::timeout(Duration::from_secs(10), tokio::net::UnixStream::connect(&through))
            .await
            .map_err(|_| OperationError::Failed("the console did not answer".into()))?
            .map_err(|error| OperationError::Failed(format!("cannot reach the console: {error}")))?;
        let stream = stream.into_std().map_err(|e| OperationError::Failed(format!("the console: {e}")))?;
        stream.set_nonblocking(false).map_err(|e| OperationError::Failed(format!("the console: {e}")))?;
        Ok((Response::VmConsole {}, OwnedFd::from(stream)))
    }
}

/// The unit an upload of `image` is received in.
fn upload_unit(image: &VmImageId) -> String {
    format!("zendiloos-vm-upload-{}.service", image.as_str())
}

/// `systemd-run` for one of this binary's jobs in a unit of its own: the
/// sandbox of [`crate::volumes::SANDBOX`], writable only `writable`, with
/// `capabilities` alone (none if empty), for `runtime` at most.
#[allow(clippy::too_many_arguments)]
fn unit_args(
    unit: &str,
    description: &str,
    writable: &Path,
    capabilities: &str,
    runtime: Duration,
    agent: &Path,
    command: &str,
    hex: &str,
) -> Vec<String> {
    let mut args: Vec<String> = [
        format!("--unit={unit}"),
        "--wait".into(),
        "--pipe".into(),
        "--collect".into(),
        "--quiet".into(),
        format!("--description={description}"),
    ]
    .into();
    args.extend(crate::volumes::SANDBOX.iter().map(|property| (*property).to_owned()));
    args.extend([
        format!("--property=ReadWritePaths={}", writable.display()),
        format!("--property=CapabilityBoundingSet={capabilities}"),
        format!("--property=RuntimeMaxSec={}", runtime.as_secs()),
        "--property=TimeoutStopSec=60".into(),
        agent.to_string_lossy().into_owned(),
        command.into(),
        hex.into(),
    ]);
    args
}

/// What a unit's last line and its end say: the bytes it stored, or why
/// not.
fn unit_end(said: Option<UnitLine>, success: bool, stderr: &str) -> Result<u64, OperationError> {
    match said {
        Some(UnitLine::Done(value)) if success => Ok(value["bytes"].as_u64().unwrap_or(0)),
        Some(UnitLine::Failed { message, refused: true }) => Err(OperationError::Refused(message)),
        Some(UnitLine::Failed { message, .. }) => Err(OperationError::Failed(message)),
        _ => {
            let tail = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
            Err(OperationError::Failed(format!("the unit ended unexpectedly: {tail}")))
        }
    }
}

/// `domstats`' blocks of zendiloOS's domains only: `Domain: 'zendiloos-<id>'`
/// and the lines under it.
fn own_domains(text: &str) -> String {
    let mut kept = String::new();
    let mut keep = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Domain: '") {
            keep = rest.strip_suffix('\'').and_then(VmId::of_domain).is_some();
        }
        if keep {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

/// `virsh --connect qemu:///system <args>`.
fn virsh<'a>(args: &[&'a str]) -> Vec<&'a str> {
    CONNECT.iter().copied().chain(args.iter().copied()).collect()
}

async fn run(program: &Program, args: &[&str], limits: Limits) -> Result<(), OperationError> {
    let output = zendiloos_exec::run(program, args, limits).await?;
    if output.status.success() {
        return Ok(());
    }
    let name = program
        .path()
        .file_name()
        .map_or_else(|| program.path().display().to_string(), |n| n.to_string_lossy().into_owned());
    let complaint = output.stderr.trim().replace('\n', " ");
    Err(OperationError::Failed(format!("{name} failed: {complaint}")))
}

/// Writes one of zendiloOS's files for libvirt, unless an administrator
/// wrote that file.
fn write_own(path: &Path, content: &str) -> Result<(), OperationError> {
    match fs::read_to_string(path) {
        Ok(existing) if !existing.starts_with(MARKER) => {
            return Err(refused(format!("{} was not written by zendiloOS; it stays", path.display())));
        }
        _ => {}
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
    }
    crate::durable::replace(path, content.as_bytes())
}

/// The threads `/proc/cpuinfo` lists, and the CPU's virtualisation flag.
fn cpu_facts(cpuinfo: &str) -> (u32, Option<String>) {
    let mut cores = 0u32;
    let mut flag = None;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim() {
            "processor" => cores += 1,
            "flags" if flag.is_none() => {
                let words: Vec<&str> = value.split_whitespace().collect();
                flag = ["vmx", "svm"].into_iter().find(|wanted| words.contains(wanted)).map(str::to_owned);
            }
            _ => {}
        }
    }
    (cores.max(1), flag)
}

/// `MemTotal` of `/proc/meminfo`, in bytes.
fn memory_bytes(meminfo: &str) -> u64 {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map_or(0, |kib| kib.saturating_mul(1024))
}

/// `&`, `<`, `>` and quotes as XML wants them. The values are checked
/// protocol types that hold none of these; escaped all the same.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// Where a domain's files are: the folder of the zvols' devices
/// (`/dev/zvol`, below which each disk is `<pool>/zendiloos.vms/<id>-disk<N>`
/// on its location), the library's folder, and its console's socket.
pub(crate) struct Places {
    pub zvols: PathBuf,
    pub images: PathBuf,
    pub console: PathBuf,
    /// The defined domain's UUID, when it is defined again; libvirt makes
    /// one up for a new domain.
    pub uuid: Option<String>,
}

/// `8b45cec4-586f-4885-973a-9e24c63b82fe`: what `virsh domuuid` answers.
fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, length)| group.len() == length && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `vda`, `vdb`, … or `sda`, `sdb`, … for the `index`th device of a bus.
fn target(prefix: &str, index: u8) -> String {
    format!("{prefix}{}", char::from(b'a' + index))
}

/// A disk's target: its slot on its bus, so a disk keeps its place
/// whatever else is added. SATA's `sda` to `sdh`.
fn disk_target(bus: VmDiskBus, slot: VmDiskSlot) -> String {
    match bus {
        VmDiskBus::Virtio => target("vd", slot.get()),
        VmDiskBus::Sata => target("sd", slot.get()),
    }
}

/// A drive's target: SATA after the disks' places, `sdi` to `sdk`.
fn drive_target(slot: VmDriveSlot) -> String {
    target("sd", 8 + slot.get())
}

/// A USB device of the host's as a domain is given it, indented by `indent`.
fn usb_hostdev(device: VmUsbDevice, indent: &str) -> String {
    format!(
        "{indent}<hostdev mode='subsystem' type='usb' managed='yes'>\n{indent}  <source>\n{indent}    \
         <vendor id='0x{:04x}'/>\n{indent}    <product id='0x{:04x}'/>\n{indent}  </source>\n{indent}</hostdev>\n",
        device.vendor, device.product
    )
}

/// The `<os>` element of a domain's XML: another one means another firmware.
fn os_section(xml: &str) -> &str {
    let start = xml.find("<os").unwrap_or(0);
    let end = xml.find("</os>").map_or(start, |end| end + 5);
    &xml[start..end.max(start)]
}

/// The domain's XML, from the checked spec alone.
pub(crate) fn domain_xml(spec: &VmSpec, places: &Places) -> String {
    let windows = spec.os == VmOs::Windows;
    let linux = spec.os == VmOs::Linux;
    let name = escape(&spec.id.domain());
    let uuid = places
        .uuid
        .as_deref()
        .filter(|uuid| is_uuid(uuid))
        .map(|uuid| format!("  <uuid>{}</uuid>\n", escape(uuid)))
        .unwrap_or_default();
    let memory = spec.memory.mib();
    let cpus = spec.cpus.get();
    let os = match (spec.firmware, spec.secure_boot()) {
        // OVMF with Microsoft's keys enrolled, as libvirt picks it from the
        // firmware QEMU describes (Debian's OVMF_CODE_4M.ms.fd and
        // OVMF_VARS_4M.ms.fd); Secure Boot needs SMM, below.
        (VmFirmware::Uefi, true) => "  <os firmware='efi'>\n    <type arch='x86_64' machine='q35'>hvm</type>\n    \
             <firmware>\n      <feature enabled='yes' name='enrolled-keys'/>\n      \
             <feature enabled='yes' name='secure-boot'/>\n    </firmware>\n    <loader secure='yes'/>\n  </os>\n"
            .to_owned(),
        (VmFirmware::Uefi, false) => "  <os firmware='efi'>\n    <type arch='x86_64' machine='q35'>hvm</type>\n    \
             <firmware>\n      <feature enabled='no' name='secure-boot'/>\n    </firmware>\n  </os>\n"
            .to_owned(),
        (VmFirmware::Bios, _) => "  <os>\n    <type arch='x86_64' machine='q35'>hvm</type>\n  </os>\n".to_owned(),
    };
    let smm =
        if spec.firmware == VmFirmware::Uefi && spec.secure_boot() { "    <smm state='on'/>\n" } else { "" };
    let hyperv = if windows {
        "    <hyperv mode='custom'>\n      <relaxed state='on'/>\n      <vapic state='on'/>\n      \
         <spinlocks state='on' retries='8191'/>\n    </hyperv>\n"
    } else {
        ""
    };
    let clock = if windows { "localtime" } else { "utc" };
    let boot = |device: VmBoot| {
        spec.boot()
            .iter()
            .position(|first| *first == device)
            .map(|index| format!("      <boot order='{}'/>\n", index + 1))
            .unwrap_or_default()
    };
    let mut devices = String::new();
    let mut disks = spec.disks().to_vec();
    disks.sort_by_key(|disk| disk.slot);
    for disk in disks {
        let source = places.zvols.join(Vms::disk_zvol(spec, &disk));
        let bus = match disk.bus {
            VmDiskBus::Virtio => "virtio",
            VmDiskBus::Sata => "sata",
        };
        devices.push_str(&format!(
            "    <disk type='block' device='disk'>\n      \
             <driver name='qemu' type='raw' cache='none' io='native' discard='unmap'/>\n      \
             <source dev='{source}'/>\n      <target dev='{target}' bus='{bus}'/>\n{boot}    </disk>\n",
            source = escape(&source.to_string_lossy()),
            target = disk_target(disk.bus, disk.slot),
            boot = boot(VmBoot::Disk(disk.slot)),
        ));
    }
    let mut drives = spec.drives().to_vec();
    drives.sort_by_key(|drive| drive.slot);
    for drive in drives {
        let source = drive
            .image
            .as_ref()
            .map(|image| {
                format!(
                    "      <source file='{}'/>\n",
                    escape(&places.images.join(image.file_name()).to_string_lossy())
                )
            })
            .unwrap_or_default();
        devices.push_str(&format!(
            "    <disk type='file' device='cdrom'>\n      <driver name='qemu' type='raw'/>\n{source}      \
             <target dev='{target}' bus='sata'/>\n      <readonly/>\n{boot}    </disk>\n",
            target = drive_target(drive.slot),
            boot = boot(VmBoot::Drive(drive.slot)),
        ));
    }
    devices.push_str("    <controller type='usb' model='qemu-xhci'/>\n");
    let mut nics = spec.nics().to_vec();
    nics.sort_by_key(|nic| nic.slot);
    for nic in nics {
        let model = match nic.model {
            VmNicModel::Virtio => "virtio",
            VmNicModel::E1000e => "e1000e",
        };
        let mac = escape(nic.mac.as_str());
        devices.push_str(&match &nic.network {
            // passt: the VM's traffic leaves from the NAS's own address; no
            // bridge, no NAT rules, nothing reaches the VM.
            VmNetwork::User => format!(
                "    <interface type='user'>\n      <mac address='{mac}'/>\n      <backend type='passt'/>\n      \
                 <model type='{model}'/>\n    </interface>\n"
            ),
            // A bridge of the host's, or an internal network: libvirt puts
            // the VM's tap on it.
            VmNetwork::Bridge { bridge } | VmNetwork::Internal { bridge } => format!(
                "    <interface type='bridge'>\n      <mac address='{mac}'/>\n      <source bridge='{}'/>\n      \
                 <model type='{model}'/>\n    </interface>\n",
                escape(bridge.as_str())
            ),
            // macvtap on the card, in bridge mode: on the card's LAN
            // without a bridge; the NAS and the VM do not reach each other.
            VmNetwork::Direct { card } => format!(
                "    <interface type='direct'>\n      <mac address='{mac}'/>\n      \
                 <source dev='{}' mode='bridge'/>\n      <model type='{model}'/>\n    </interface>\n",
                escape(card.as_str())
            ),
        });
    }
    // The host's USB devices by vendor and product, its PCI devices by
    // address: libvirt binds them to vfio-pci while the VM runs.
    for device in spec.usb() {
        devices.push_str(&usb_hostdev(*device, "    "));
    }
    for address in spec.pci() {
        let (domain, bus, slot, function) = address.parts();
        devices.push_str(&format!(
            "    <hostdev mode='subsystem' type='pci' managed='yes'>\n      <source>\n        \
             <address domain='0x{domain:04x}' bus='0x{bus:02x}' slot='0x{slot:02x}' function='0x{function:x}'/>\n      \
             </source>\n    </hostdev>\n"
        ));
    }
    if spec.tpm() {
        // swtpm, its state in /var/lib/libvirt/swtpm/<uuid>.
        devices.push_str(
            "    <tpm model='tpm-crb'>\n      <backend type='emulator' version='2.0'/>\n    </tpm>\n",
        );
    }
    // The QEMU guest agent's channel: a snapshot freezes the guest's file
    // systems through it, if the guest runs the agent.
    devices.push_str(
        "    <channel type='unix'>\n      <target type='virtio' name='org.qemu.guest_agent.0'/>\n    </channel>\n",
    );
    devices.push_str("    <input type='tablet' bus='usb'/>\n");
    devices.push_str(&format!(
        "    <graphics type='vnc'>\n      <listen type='socket' socket='{}'/>\n    </graphics>\n",
        escape(&places.console.to_string_lossy())
    ));
    devices.push_str(&format!(
        "    <video>\n      <model type='{}'/>\n    </video>\n",
        if linux { "virtio" } else { "vga" }
    ));
    if linux {
        // The guest tells what of its memory it uses, every few seconds.
        devices.push_str("    <memballoon model='virtio'>\n      <stats period='5'/>\n    </memballoon>\n");
        devices.push_str(
            "    <rng model='virtio'>\n      <backend model='random'>/dev/urandom</backend>\n    </rng>\n",
        );
    } else {
        devices.push_str("    <memballoon model='none'/>\n");
    }
    format!(
        "<!-- Written by zendiloOS from the VM's settings; changed only there. -->\n\
         <domain type='kvm'>\n  <name>{name}</name>\n{uuid}  \
         <memory unit='MiB'>{memory}</memory>\n  <currentMemory unit='MiB'>{memory}</currentMemory>\n  \
         <vcpu placement='static'>{cpus}</vcpu>\n{os}  <features>\n    <acpi/>\n    <apic/>\n{smm}{hyperv}  </features>\n  \
         <cpu mode='host-passthrough' check='none'/>\n  <clock offset='{clock}'/>\n  \
         <on_poweroff>destroy</on_poweroff>\n  <on_reboot>restart</on_reboot>\n  <on_crash>destroy</on_crash>\n  \
         <pm>\n    <suspend-to-mem enabled='no'/>\n    <suspend-to-disk enabled='no'/>\n  </pm>\n  \
         <devices>\n{devices}  </devices>\n</domain>\n"
    )
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{
        MacAddress, VmCpus, VmDevices, VmDisk, VmDiskSize, VmDrive, VmMemory, VmNic, VmNicSlot, VmUsbDevice,
    };

    use super::*;

    fn tank() -> VmStorage {
        VmStorage { pool: PoolName::parse("tank").unwrap(), volume: VolumeNumber::new(1).unwrap() }
    }

    fn disk(slot: u8, gib: u32, bus: VmDiskBus) -> VmDisk {
        VmDisk {
            slot: VmDiskSlot::new(slot).unwrap(),
            size: VmDiskSize::new(gib).unwrap(),
            bus,
            location: tank(),
        }
    }

    fn nic(slot: u8, model: VmNicModel, network: VmNetwork) -> VmNic {
        VmNic {
            slot: VmNicSlot::new(slot).unwrap(),
            model,
            network,
            mac: MacAddress::parse(&format!("52:54:00:00:00:0{slot}")).unwrap(),
        }
    }

    fn drive(slot: u8, image: Option<&str>) -> VmDrive {
        VmDrive {
            slot: VmDriveSlot::new(slot).unwrap(),
            image: image.map(|id| VmImageId::parse(id).unwrap()),
        }
    }

    fn spec(os: VmOs, firmware: VmFirmware, devices: VmDevices) -> VmSpec {
        VmSpec::new(
            VmId::parse("0a1b2c3d4e5f").unwrap(),
            os,
            VmCpus::new(2).unwrap(),
            VmMemory::new(2048).unwrap(),
            firmware,
            devices,
            false,
            VolumeNumber::new(1).unwrap(),
        )
        .unwrap()
    }

    /// One virtio disk, a passt card, an ISO in the first drive, booted first.
    fn linux() -> VmDevices {
        VmDevices {
            disks: vec![disk(0, 20, VmDiskBus::Virtio)],
            nics: vec![nic(0, VmNicModel::Virtio, VmNetwork::User)],
            drives: vec![drive(0, Some("00000000000a"))],
            boot: vec![
                VmBoot::Drive(VmDriveSlot::new(0).unwrap()),
                VmBoot::Disk(VmDiskSlot::new(0).unwrap()),
            ],
            ..VmDevices::default()
        }
    }

    fn xml(spec: &VmSpec) -> String {
        let places = Places {
            zvols: PathBuf::from("/dev/zvol"),
            images: PathBuf::from("/volume1/@vms/isos"),
            console: PathBuf::from("/run/zendiloos-vms/0a1b2c3d4e5f.vnc"),
            uuid: None,
        };
        domain_xml(spec, &places)
    }

    #[test]
    fn a_domain_defined_again_keeps_its_uuid() {
        let spec = spec(VmOs::Linux, VmFirmware::Uefi, linux());
        let mut places = Places {
            zvols: PathBuf::from("/dev/zvol"),
            images: PathBuf::from("/volume1/@vms/isos"),
            console: PathBuf::from("/run/zendiloos-vms/0a1b2c3d4e5f.vnc"),
            uuid: Some("8b45cec4-586f-4885-973a-9e24c63b82fe".to_owned()),
        };
        let xml = domain_xml(&spec, &places);
        assert!(
            xml.contains(
                "<name>zendiloos-0a1b2c3d4e5f</name>\n  <uuid>8b45cec4-586f-4885-973a-9e24c63b82fe</uuid>\n"
            ),
            "{xml}"
        );
        // Anything else is left for libvirt to make up, never written.
        places.uuid = Some("8b45cec4-586f-4885-973a-9e24c63b82fe'/><x".to_owned());
        assert!(!domain_xml(&spec, &places).contains("<uuid>"));
        assert!(is_uuid("8b45cec4-586f-4885-973a-9e24c63b82fe"));
        assert!(!is_uuid("8b45cec4586f4885973a9e24c63b82fe") && !is_uuid(""));
    }

    #[test]
    fn a_linux_vm_gets_virtio_uefi_passt_and_its_iso_first() {
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, linux()));
        for part in [
            "<domain type='kvm'>",
            "<name>zendiloos-0a1b2c3d4e5f</name>",
            "<memory unit='MiB'>2048</memory>",
            "<vcpu placement='static'>2</vcpu>",
            "<os firmware='efi'>",
            "<feature enabled='no' name='secure-boot'/>",
            "<source dev='/dev/zvol/tank/zendiloos.vms/0a1b2c3d4e5f-disk0'/>\n      <target dev='vda' bus='virtio'/>\n      <boot order='2'/>",
            "<source file='/volume1/@vms/isos/00000000000a.iso'/>\n      <target dev='sdi' bus='sata'/>\n      <readonly/>\n      <boot order='1'/>",
            "<interface type='user'>\n      <mac address='52:54:00:00:00:00'/>\n      <backend type='passt'/>\n      <model type='virtio'/>",
            "<listen type='socket' socket='/run/zendiloos-vms/0a1b2c3d4e5f.vnc'/>",
            "<clock offset='utc'/>",
            "<stats period='5'/>",
            "<rng model='virtio'>",
        ] {
            assert!(xml.contains(part), "{part} missing in\n{xml}");
        }
        // No network of libvirt's, no filter, nothing listening on TCP.
        for absent in ["type='network'", "filterref", "port=", "listen type='address'", "hyperv"] {
            assert!(!xml.contains(absent), "{absent} in\n{xml}");
        }
        assert_eq!(xml.matches("<disk ").count(), 2);
    }

    /// A card on an internal network is on its bridge, as any other; one
    /// directly on a card is macvtap in bridge mode, without a bridge.
    #[test]
    fn internal_and_direct_cards() {
        let devices = VmDevices {
            nics: vec![
                nic(
                    0,
                    VmNicModel::Virtio,
                    VmNetwork::Internal { bridge: InterfaceName::parse("vmnet0").unwrap() },
                ),
                nic(
                    1,
                    VmNicModel::E1000e,
                    VmNetwork::Direct { card: InterfaceName::parse("enp2s0").unwrap() },
                ),
            ],
            ..linux()
        };
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, devices));
        assert!(xml.contains(
            "<interface type='bridge'>\n      <mac address='52:54:00:00:00:00'/>\n      <source bridge='vmnet0'/>\n      \
             <model type='virtio'/>\n    </interface>"
        ), "{xml}");
        assert!(
            xml.contains(
                "<interface type='direct'>\n      <mac address='52:54:00:00:00:01'/>\n      \
             <source dev='enp2s0' mode='bridge'/>\n      <model type='e1000e'/>\n    </interface>"
            ),
            "{xml}"
        );
        for absent in
            ["type='network'", "filterref", "mode='vepa'", "mode='passthrough'", "<target dev='macvtap"]
        {
            assert!(!xml.contains(absent), "{absent} in\n{xml}");
        }
    }

    /// Each disk is on its location: its zvol below that pool's dataset.
    #[test]
    fn disks_on_two_locations() {
        let cold =
            VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() };
        let devices = VmDevices {
            disks: vec![
                disk(0, 20, VmDiskBus::Virtio),
                VmDisk { location: cold, ..disk(1, 100, VmDiskBus::Sata) },
            ],
            ..linux()
        };
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, devices));
        assert!(
            xml.contains(
                "<source dev='/dev/zvol/tank/zendiloos.vms/0a1b2c3d4e5f-disk0'/>\n      <target dev='vda'"
            ),
            "{xml}"
        );
        assert!(
            xml.contains(
                "<source dev='/dev/zvol/cold/zendiloos.vms/0a1b2c3d4e5f-disk1'/>\n      <target dev='sdb'"
            ),
            "{xml}"
        );
    }

    #[test]
    fn several_disks_cards_and_drives_keep_their_places_and_boot_in_order() {
        let bridge = VmNetwork::Bridge { bridge: InterfaceName::parse("br0").unwrap() };
        let devices = VmDevices {
            disks: vec![disk(2, 100, VmDiskBus::Sata), disk(0, 20, VmDiskBus::Virtio)],
            nics: vec![nic(1, VmNicModel::E1000e, bridge), nic(0, VmNicModel::Virtio, VmNetwork::User)],
            drives: vec![drive(2, None), drive(0, Some("00000000000a")), drive(1, Some("00000000000b"))],
            boot: vec![
                VmBoot::Disk(VmDiskSlot::new(2).unwrap()),
                VmBoot::Drive(VmDriveSlot::new(1).unwrap()),
            ],
            ..VmDevices::default()
        };
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, devices));
        let at = |part: &str| xml.find(part).unwrap_or_else(|| panic!("{part} missing in\n{xml}"));
        // Disks by slot, then drives, each on its own target.
        assert!(
            at("0a1b2c3d4e5f-disk0'/>\n      <target dev='vda' bus='virtio'/>\n    </disk>")
                < at("-disk2'/>")
        );
        assert!(xml.contains(
            "0a1b2c3d4e5f-disk2'/>\n      <target dev='sdc' bus='sata'/>\n      <boot order='1'/>"
        ));
        assert!(xml.contains("00000000000b.iso'/>\n      <target dev='sdj' bus='sata'/>\n      <readonly/>\n      <boot order='2'/>"));
        assert!(xml.contains(
            "00000000000a.iso'/>\n      <target dev='sdi' bus='sata'/>\n      <readonly/>\n    </disk>"
        ));
        // An empty drive has no source: an image goes in while it runs.
        assert!(xml.contains("<driver name='qemu' type='raw'/>\n      <target dev='sdk' bus='sata'/>\n      <readonly/>\n    </disk>"), "{xml}");
        assert_eq!(xml.matches("<boot order=").count(), 2, "only what is in the boot order boots");
        assert!(at("52:54:00:00:00:00") < at("52:54:00:00:00:01"), "cards by slot");
        assert!(xml.contains(
            "<interface type='bridge'>\n      <mac address='52:54:00:00:00:01'/>\n      <source bridge='br0'/>\n      <model type='e1000e'/>"
        ));
        assert_eq!(xml.matches("<interface ").count(), 2);
        assert_eq!(xml.matches("<disk ").count(), 5);
    }

    #[test]
    fn windows_gets_local_time_hyperv_and_vga() {
        let devices = VmDevices {
            disks: vec![disk(0, 64, VmDiskBus::Sata)],
            nics: vec![nic(0, VmNicModel::E1000e, VmNetwork::User)],
            drives: vec![],
            boot: vec![VmBoot::Disk(VmDiskSlot::new(0).unwrap())],
            ..VmDevices::default()
        };
        let xml = xml(&spec(VmOs::Windows, VmFirmware::Uefi, devices));
        assert!(xml.contains("<target dev='sda' bus='sata'/>\n      <boot order='1'/>"), "{xml}");
        assert!(xml.contains("<clock offset='localtime'/>") && xml.contains("<hyperv mode='custom'>"));
        assert!(xml.contains("<model type='vga'/>") && xml.contains("<memballoon model='none'/>"));
        assert!(!xml.contains("device='cdrom'"), "no drive, no drive");
    }

    /// Windows 11: a TPM 2.0 through swtpm, and Secure Boot with
    /// Microsoft's keys, which libvirt picks from the firmware QEMU
    /// describes, with SMM as it needs.
    #[test]
    fn a_tpm_and_secure_boot_with_microsofts_keys() {
        let devices = VmDevices { tpm: true, secure_boot: true, ..linux() };
        let xml = xml(&spec(VmOs::Windows, VmFirmware::Uefi, devices));
        for part in [
            "<os firmware='efi'>\n    <type arch='x86_64' machine='q35'>hvm</type>\n    <firmware>\n      \
             <feature enabled='yes' name='enrolled-keys'/>\n      <feature enabled='yes' name='secure-boot'/>\n    \
             </firmware>\n    <loader secure='yes'/>\n  </os>",
            "<features>\n    <acpi/>\n    <apic/>\n    <smm state='on'/>\n",
            "<tpm model='tpm-crb'>\n      <backend type='emulator' version='2.0'/>\n    </tpm>",
        ] {
            assert!(xml.contains(part), "{part} missing in\n{xml}");
        }
        // Without them: neither, and Secure Boot said to be off.
        let plain = self::xml(&spec(VmOs::Windows, VmFirmware::Uefi, linux()));
        assert!(plain.contains("<feature enabled='no' name='secure-boot'/>"), "{plain}");
        for absent in ["<tpm", "<smm", "enrolled-keys", "secure='yes'"] {
            assert!(!plain.contains(absent), "{absent} in\n{plain}");
        }
        // A TPM with BIOS is fine; Secure Boot is not.
        let bios = self::xml(&spec(VmOs::Other, VmFirmware::Bios, VmDevices { tpm: true, ..linux() }));
        assert!(bios.contains("<tpm model='tpm-crb'>") && !bios.contains("firmware="), "{bios}");
        let refused = VmSpec::new(
            VmId::parse("0a1b2c3d4e5f").unwrap(),
            VmOs::Other,
            VmCpus::new(1).unwrap(),
            VmMemory::new(512).unwrap(),
            VmFirmware::Bios,
            VmDevices { secure_boot: true, ..linux() },
            false,
            VolumeNumber::new(1).unwrap(),
        );
        assert!(refused.unwrap_err().to_string().contains("Secure Boot needs UEFI"));
    }

    /// The host's devices: USB by vendor and product, PCI by address and
    /// managed by libvirt (bound to vfio-pci only while the VM runs); every
    /// VM has the guest agent's channel.
    #[test]
    fn the_hosts_devices_and_the_guest_agents_channel() {
        let devices = VmDevices {
            usb: vec![VmUsbDevice { vendor: 0x046d, product: 0xc31c }],
            pci: vec![zendiloos_agent_proto::PciAddress::parse("0000:0a:1f.3").unwrap()],
            ..linux()
        };
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, devices));
        for part in [
            "<hostdev mode='subsystem' type='usb' managed='yes'>\n      <source>\n        <vendor id='0x046d'/>\n        \
             <product id='0xc31c'/>\n      </source>\n    </hostdev>",
            "<hostdev mode='subsystem' type='pci' managed='yes'>\n      <source>\n        \
             <address domain='0x0000' bus='0x0a' slot='0x1f' function='0x3'/>\n      </source>\n    </hostdev>",
            "<channel type='unix'>\n      <target type='virtio' name='org.qemu.guest_agent.0'/>\n    </channel>",
        ] {
            assert!(xml.contains(part), "{part} missing in\n{xml}");
        }
        let (usb, pci) = devices::handed_over(&xml);
        assert_eq!(usb, [VmUsbDevice { vendor: 0x046d, product: 0xc31c }]);
        assert_eq!(pci[0].as_str(), "0000:0a:1f.3");
        assert!(!self::xml(&spec(VmOs::Linux, VmFirmware::Uefi, linux())).contains("<hostdev"));
    }

    #[test]
    fn another_firmware_is_another_os_section() {
        let uefi = xml(&spec(VmOs::Linux, VmFirmware::Uefi, linux()));
        let secure = xml(&spec(VmOs::Linux, VmFirmware::Uefi, VmDevices { secure_boot: true, ..linux() }));
        let more = xml(&spec(VmOs::Linux, VmFirmware::Uefi, VmDevices { tpm: true, ..linux() }));
        assert_ne!(os_section(&uefi), os_section(&secure));
        assert_eq!(os_section(&uefi), os_section(&more), "a TPM is no other firmware");
        assert!(os_section(&uefi).starts_with("<os firmware='efi'>") && os_section(&uefi).ends_with("</os>"));
    }

    #[test]
    fn bios_has_no_firmware_element() {
        let xml = xml(&spec(VmOs::Other, VmFirmware::Bios, linux()));
        assert!(xml.contains("<os>\n    <type arch='x86_64' machine='q35'>hvm</type>\n  </os>"), "{xml}");
        assert!(!xml.contains("firmware="));
    }

    #[test]
    fn values_are_escaped_should_one_ever_hold_markup() {
        assert_eq!(escape("a'<b>&\""), "a&apos;&lt;b&gt;&amp;&quot;");
    }

    #[test]
    fn only_zendiloos_domains_leave_the_agent() {
        let text = "Domain: 'win10'\n  cpu.time=1\n\nDomain: 'zendiloos-0a1b2c3d4e5f'\n  cpu.time=2\n  \
                    balloon.rss=3\n\nDomain: 'zendiloos-win10'\n  cpu.time=4\n";
        assert_eq!(own_domains(text), "Domain: 'zendiloos-0a1b2c3d4e5f'\n  cpu.time=2\n  balloon.rss=3\n\n");
        assert_eq!(own_domains(""), "");
    }

    #[test]
    fn a_units_end_says_what_it_stored() {
        let done = Some(UnitLine::Done(serde_json::json!({ "bytes": 7 })));
        assert_eq!(unit_end(done.clone(), true, "").unwrap(), 7);
        assert!(unit_end(done, false, "").is_err(), "a unit that failed stored nothing");
        let refused = Some(UnitLine::Failed { message: "short".into(), refused: true });
        assert!(matches!(unit_end(refused, false, ""), Err(OperationError::Refused(m)) if m == "short"));
        assert!(
            matches!(unit_end(None, false, "x\nKilled\n"), Err(OperationError::Failed(m)) if m.ends_with("Killed"))
        );
    }

    #[test]
    fn the_machine_is_read_from_proc_and_sys() {
        let cpuinfo =
            "processor\t: 0\nflags\t\t: fpu vme sse2 vmx ept\n\nprocessor\t: 1\nflags\t\t: fpu vmx\n";
        assert_eq!(cpu_facts(cpuinfo), (2, Some("vmx".into())));
        assert_eq!(cpu_facts("processor : 0\nflags : fpu svm\n").1.as_deref(), Some("svm"));
        assert_eq!(cpu_facts("processor : 0\nflags : fpu sse2\n").1, None);
        assert_eq!(memory_bytes("MemTotal:        2027168 kB\nMemFree: 1 kB\n"), 2_027_168 * 1024);
        assert_eq!(memory_bytes(""), 0);
    }

    #[test]
    fn the_locations_are_where_their_mount_units_say() {
        let units = tempfile::tempdir().unwrap();
        let host = HostPaths { units: units.path().to_owned(), ..HostPaths::default() };
        assert!(vms_locations(&host).is_empty());
        let cold =
            VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() };
        fs::write(
            units.path().join("volume2-\\x40vms.mount"),
            "[Mount]\nWhat=cold/zendiloos.vms\nWhere=/volume2/@vms\n",
        )
        .unwrap();
        assert_eq!(vms_locations(&host), std::slice::from_ref(&cold));
        fs::write(units.path().join("volume1-\\x40vms.mount"), "[Mount]\nWhat=tank/zendiloos.vms\n").unwrap();
        assert_eq!(vms_locations(&host), [tank(), cold.clone()], "by volume");
        // A unit naming a pool another names already is a hand's work: neither counts.
        fs::write(units.path().join("volume3-\\x40vms.mount"), "[Mount]\nWhat=cold/zendiloos.vms\n").unwrap();
        // Nor does one naming anything else, or a volume out of bounds.
        fs::write(units.path().join("volume4-\\x40vms.mount"), "[Mount]\nWhat=tank/other\n").unwrap();
        fs::write(units.path().join("volume0-\\x40vms.mount"), "[Mount]\nWhat=zero/zendiloos.vms\n").unwrap();
        assert_eq!(vms_locations(&host), [tank()]);
    }

    #[test]
    fn files_an_administrator_wrote_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local/libvirt-qemu");
        write_own(&path, &format!("{MARKER}: one\n")).unwrap();
        write_own(&path, &format!("{MARKER}: two\n")).unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("two\n"));
        fs::write(&path, "/srv/isos/** r,\n").unwrap();
        assert!(write_own(&path, &format!("{MARKER}: three\n")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "/srv/isos/** r,\n");
    }
}
