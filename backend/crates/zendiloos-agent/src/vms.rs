//! Virtual machines through libvirt (docs/architecture.md, 4.13).
//!
//! libvirt's daemon runs outside this sandbox and does the work: it starts
//! QEMU as `libvirt-qemu`, confined by AppArmor, and passt for user-mode
//! networking. The agent talks to it with `virsh` over its Unix socket and
//! only ever names domains `zendiloos-<id>`; it never lists, changes or
//! removes another. Each domain's XML is built here from the checked
//! [`VmSpec`], never from text a client sent, and kept as
//! `/etc/zendiloos/vms/zendiloos-<id>.xml`.
//!
//! A VM's disk is a sparse zvol `<pool>/zendiloos.vms/<id>-disk0`
//! (`volmode=dev`: no partitions of it show up on the host). An ISO is a
//! copy in `/volume<N>/@vms/isos/<id>.iso`, made by a unit of its own from
//! the share (`copy-vm-iso`, see [`crate::vm_iso`]): QEMU never sees a share,
//! and libvirt, which hands the files it attaches to QEMU's account, never
//! changes a file in one.
//!
//! The console is QEMU's VNC server on a Unix socket in
//! `/run/zendiloos-vms` (`libvirt-qemu`'s, made by systemd-tmpfiles). The
//! agent opens it without following a link, connects, and hands the socket
//! to zendiloosd, as it hands a terminal's master.

use std::fs;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zendiloos_agent_proto::{
    InterfaceName, PoolName, Response, VM_CONSOLES, VMS_DATASET, VMS_DIRECTORY, VmAction, VmFirmware, VmId,
    VmIsoSource, VmNetwork, VmOs, VmSpec, VmStorage, VolumeNumber,
};
use zendiloos_exec::{ExecError, Limits, Program, Surroundings};

use crate::operations::OperationError;
use crate::pools::HostPaths;
use crate::vm_iso::IsoJob;
use crate::volume_archive::{UnitLine, encode};

/// libvirt's system instance; `virsh` is always told so, whatever its
/// environment would say.
const CONNECT: [&str; 2] = ["--connect", "qemu:///system"];
const QUICK: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 1024 * 1024 };
/// Starting a VM waits for QEMU; destroying a large zvol takes a while.
const SLOW: Limits = Limits { timeout: Duration::from_secs(5 * 60), max_stdout: 1024 * 1024 };
/// What an ISO's copy may take; zendiloosd waits two hours for a long request.
const COPY_FOR: Duration = Duration::from_secs(100 * 60);
/// Most of `/proc/cpuinfo` read: a line of flags per thread.
const MAX_CPUINFO: u64 = 8 * 1024 * 1024;
/// The unit an ISO is copied in reads the share, whoever owns the file.
const COPY_CAPABILITIES: &str = "CAP_DAC_READ_SEARCH";
/// Marks the files zendiloOS writes for libvirt, so that one an
/// administrator wrote is never replaced.
const MARKER: &str = "# Written by zendiloOS";

/// Where the agent looks at the machine and keeps the domains' XML; other
/// than the real places only in tests.
#[derive(Debug, Clone)]
pub struct VmPaths {
    pub kvm: PathBuf,
    pub cpuinfo: PathBuf,
    pub meminfo: PathBuf,
    pub iommu: PathBuf,
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
}

impl Default for VmPaths {
    fn default() -> Self {
        Self {
            kvm: PathBuf::from("/dev/kvm"),
            cpuinfo: PathBuf::from("/proc/cpuinfo"),
            meminfo: PathBuf::from("/proc/meminfo"),
            iommu: PathBuf::from("/sys/class/iommu"),
            domains: PathBuf::from("/etc/zendiloos/vms"),
            consoles: PathBuf::from(VM_CONSOLES),
            tmpfiles: PathBuf::from("/etc/tmpfiles.d/zendiloos-vms.conf"),
            apparmor: PathBuf::from("/etc/apparmor.d/abstractions/libvirt-qemu.d/zendiloos-consoles"),
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
    /// Only looked for: libvirt's daemon and QEMU, which libvirt runs.
    pub libvirtd: PathBuf,
    pub qemu: PathBuf,
    /// Run in transient units.
    pub tmpfiles: PathBuf,
    pub rm: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Vms {
    tools: VmTools,
    paths: VmPaths,
    host: HostPaths,
    /// One change of a VM at a time: two deletes of one VM, or a define
    /// beside its delete, must not interleave.
    serial: tokio::sync::Mutex<()>,
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

/// Where the VMs' dataset is: the mount unit that names it. More than one
/// (a hand's work) counts as none, so nothing goes to the wrong pool.
pub(crate) fn vms_storage(host: &HostPaths) -> Option<VmStorage> {
    let entries = fs::read_dir(&host.units).ok()?;
    let mut found = Vec::new();
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
    if found.len() == 1 { found.pop() } else { None }
}

impl Vms {
    pub(crate) fn new(tools: VmTools, paths: VmPaths, host: HostPaths) -> Self {
        Self { tools, paths, host, serial: tokio::sync::Mutex::new(()) }
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

    /// What the machine offers VMs.
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

    pub(crate) fn host_facts(&self) -> Result<Response, OperationError> {
        let mut cpuinfo = String::new();
        if let Ok(file) = fs::File::open(&self.paths.cpuinfo) {
            let _ = file.take(MAX_CPUINFO).read_to_string(&mut cpuinfo);
        }
        let (cores, cpu_virtualization) = cpu_facts(&cpuinfo);
        let meminfo = fs::read_to_string(&self.paths.meminfo).unwrap_or_default();
        let iommu = fs::read_dir(&self.paths.iommu).is_ok_and(|mut entries| entries.next().is_some());
        Ok(Response::VmHost {
            kvm: self.paths.kvm.exists(),
            cpu_virtualization,
            iommu,
            installed: self.installed(),
            cores,
            memory_bytes: memory_bytes(&meminfo),
            storage: vms_storage(&self.host),
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

    /// Copies the ISO from the share for `id`, in a unit of its own that
    /// may write only the VMs' folder, reporting how far it is.
    pub(crate) async fn copy_iso(
        &self,
        id: &VmId,
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
            folder: root.join("isos"),
            name: format!("{}.iso", id.as_str()),
        };
        self.unit(id, &root, &job).await?;
        Ok(Response::Done)
    }

    /// Runs `job` in `zendiloos-vm-iso-<id>.service`; its progress becomes
    /// the request's.
    async fn unit(&self, id: &VmId, writable: &Path, job: &IsoJob) -> Result<(), OperationError> {
        let until = Instant::now() + COPY_FOR;
        let agent = self.host.agent.to_string_lossy().into_owned();
        let unit = format!("zendiloos-vm-iso-{}.service", id.as_str());
        let name = format!("--unit={unit}");
        let description = format!("--description=zendiloOS: copy the ISO of virtual machine {}", id.as_str());
        let writable = format!("--property=ReadWritePaths={}", writable.display());
        let capabilities = format!("--property=CapabilityBoundingSet={COPY_CAPABILITIES}");
        let runtime = format!("--property=RuntimeMaxSec={}", COPY_FOR.as_secs());
        let hex = encode(job);
        let mut args = vec![name.as_str(), "--wait", "--pipe", "--collect", "--quiet", description.as_str()];
        args.extend(crate::volumes::SANDBOX);
        args.extend([
            writable.as_str(),
            capabilities.as_str(),
            runtime.as_str(),
            "--property=TimeoutStopSec=60",
            agent.as_str(),
            "copy-vm-iso",
            hex.as_str(),
        ]);
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
                zendiloos_exec::run_lines(&self.tools.systemd_run, &args, &surroundings, limits, &mut heard);
            tokio::select! {
                ran = running => Some(ran),
                () = crate::waiting::given_up() => None,
            }
        };
        let output = match ran {
            Some(Ok(output)) => output,
            Some(Err(ExecError::Timeout { .. })) | None => {
                let _ = run(&self.tools.systemctl, &["stop", unit.as_str()], SLOW).await;
                return Err(OperationError::Failed(format!("{unit} was stopped before the ISO was copied")));
            }
            Some(Err(error)) => return Err(error.into()),
        };
        match said {
            Some(UnitLine::Done(_)) if output.status.success() => Ok(()),
            Some(UnitLine::Failed { message, refused: true }) => Err(OperationError::Refused(message)),
            Some(UnitLine::Failed { message, .. }) => Err(OperationError::Failed(message)),
            _ => {
                let tail =
                    output.stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
                Err(OperationError::Failed(format!("the ISO's unit ended with {}: {tail}", output.status)))
            }
        }
    }

    /// The VMs' storage, which must be on `volume`.
    fn storage_on(&self, volume: VolumeNumber) -> Result<VmStorage, OperationError> {
        match vms_storage(&self.host) {
            Some(storage) if storage.volume == volume => Ok(storage),
            Some(storage) => Err(refused(format!(
                "the virtual machines' storage is on volume {}, not {}",
                storage.volume.get(),
                volume.get()
            ))),
            None => Err(refused("the virtual machines' storage is not set up")),
        }
    }

    fn zvol(pool: &PoolName, id: &VmId) -> String {
        format!("{}/{VMS_DATASET}/{}", pool.as_str(), id.disk())
    }

    fn iso_copy(&self, volume: VolumeNumber, id: &VmId) -> PathBuf {
        vms_root(&self.host, volume).join("isos").join(format!("{}.iso", id.as_str()))
    }

    fn domain_file(&self, id: &VmId) -> PathBuf {
        self.paths.domains.join(format!("{}.xml", id.domain()))
    }

    async fn defined(&self, id: &VmId) -> Result<bool, OperationError> {
        let domain = id.domain();
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&["domstate", &domain]), QUICK).await?;
        Ok(output.status.success())
    }

    async fn exists(&self, dataset: &str) -> Result<bool, OperationError> {
        let output =
            zendiloos_exec::run(&self.tools.zfs, ["list", "-H", "-o", "name", dataset], QUICK).await?;
        Ok(output.status.success())
    }

    /// Makes the VM's disk where missing and defines its domain.
    pub(crate) async fn define(&self, spec: &VmSpec) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        self.require_installed()?;
        let storage = self.storage_on(spec.volume)?;
        if storage.pool != spec.pool {
            return Err(refused(format!(
                "the virtual machines' storage is not on pool {}",
                spec.pool.as_str()
            )));
        }
        let parent = format!("{}/{VMS_DATASET}", spec.pool.as_str());
        if !self.exists(&parent).await? {
            return Err(refused(format!("there is no dataset {parent}")));
        }
        if self.defined(&spec.id).await? {
            return Err(OperationError::Conflict);
        }
        let iso = self.iso_copy(spec.volume, &spec.id);
        if spec.iso && !fs::symlink_metadata(&iso).is_ok_and(|meta| meta.is_file()) {
            return Err(refused("the ISO was not copied for this virtual machine"));
        }
        if let VmNetwork::Bridge { bridge } = &spec.network {
            self.check_bridge(bridge).await?;
        }
        let zvol = Self::zvol(&spec.pool, &spec.id);
        let made = if self.exists(&zvol).await? {
            false
        } else {
            let size = format!("{}G", spec.disk.gib());
            run(&self.tools.zfs, &["create", "-s", "-V", &size, "-o", "volmode=dev", &zvol], SLOW).await?;
            true
        };
        let defined = self.define_domain(spec, &iso).await;
        if defined.is_err() && made {
            let _ = run(&self.tools.zfs, &["destroy", &zvol], SLOW).await;
        }
        defined?;
        Ok(Response::Done)
    }

    async fn define_domain(&self, spec: &VmSpec, iso: &Path) -> Result<(), OperationError> {
        let disk = Path::new("/dev/zvol").join(Self::zvol(&spec.pool, &spec.id));
        let console = self.paths.consoles.join(format!("{}.vnc", spec.id.as_str()));
        let xml = domain_xml(spec, &disk, spec.iso.then_some(iso), &console);
        fs::create_dir_all(&self.paths.domains)
            .map_err(|e| OperationError::Write(self.paths.domains.clone(), e))?;
        let file = self.domain_file(&spec.id);
        crate::durable::replace(&file, xml.as_bytes())?;
        let path = file.to_string_lossy().into_owned();
        if let Err(error) = run(&self.tools.virsh, &virsh(&["define", "--validate", &path]), QUICK).await {
            let _ = fs::remove_file(&file);
            return Err(error);
        }
        if spec.autostart {
            run(&self.tools.virsh, &virsh(&["autostart", &spec.id.domain()]), QUICK).await?;
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
        let args: Vec<&str> = match action {
            VmAction::Start => {
                // Gone after a restart until systemd-tmpfiles made it again.
                self.make_consoles().await?;
                vec!["start", &domain]
            }
            VmAction::Shutdown => vec!["shutdown", "--mode", "acpi", &domain],
            VmAction::ForceOff => vec!["destroy", &domain],
            VmAction::Reboot => vec!["reboot", "--mode", "acpi", &domain],
        };
        let output = zendiloos_exec::run(&self.tools.virsh, virsh(&args), SLOW).await?;
        if output.status.success() {
            return Ok(Response::Done);
        }
        // libvirt's own words: "domain is already active", "domain is not running".
        let complaint = output.stderr.trim().trim_start_matches("error: ").replace('\n', " ");
        Err(refused(complaint))
    }

    /// Powers the VM off, undefines it, destroys its disk, and removes the
    /// copy of its ISO and its XML. What is gone already is no failure, so
    /// a VM whose creation failed halfway goes too.
    pub(crate) async fn delete(
        &self,
        id: &VmId,
        pool: &PoolName,
        volume: VolumeNumber,
    ) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let storage = self.storage_on(volume)?;
        if &storage.pool != pool {
            return Err(refused(format!("the virtual machines' storage is not on pool {}", pool.as_str())));
        }
        let domain = id.domain();
        if self.installed() && self.defined(id).await? {
            // Not running is fine; anything else shows in undefine.
            let _ = zendiloos_exec::run(&self.tools.virsh, virsh(&["destroy", &domain]), SLOW).await;
            let undefine = ["undefine", "--nvram", "--managed-save", "--snapshots-metadata", domain.as_str()];
            run(&self.tools.virsh, &virsh(&undefine), QUICK).await?;
        }
        let zvol = Self::zvol(pool, id);
        if self.exists(&zvol).await? {
            run(&self.tools.zfs, &["destroy", &zvol], SLOW).await?;
        }
        let iso = self.iso_copy(volume, id);
        if fs::symlink_metadata(&iso).is_ok() {
            self.remove_iso(&iso, &vms_root(&self.host, volume)).await?;
        }
        match fs::remove_file(self.domain_file(id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(OperationError::Write(self.domain_file(id), error)),
        }
        Ok(Response::Done)
    }

    /// `rm` in a transient unit that may write only the VMs' folder; the
    /// path is made of checked parts.
    async fn remove_iso(&self, iso: &Path, writable: &Path) -> Result<(), OperationError> {
        let rm = self.tools.rm.to_string_lossy().into_owned();
        let iso = iso.to_string_lossy().into_owned();
        let writable = format!("--property=ReadWritePaths={}", writable.display());
        let mut args = vec![
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--description=zendiloOS: remove the ISO of a virtual machine",
        ];
        args.extend(crate::volumes::SANDBOX);
        args.extend([
            writable.as_str(),
            "--property=CapabilityBoundingSet=",
            rm.as_str(),
            "--force",
            "--",
            &iso,
        ]);
        run(&self.tools.systemd_run, &args, QUICK).await
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

/// The domain's XML, from the checked spec: `disk` the zvol's device,
/// `iso` the ISO's copy, `console` the VNC socket's path.
pub(crate) fn domain_xml(spec: &VmSpec, disk: &Path, iso: Option<&Path>, console: &Path) -> String {
    let windows = spec.os == VmOs::Windows;
    let virtio = spec.os == VmOs::Linux;
    let name = escape(&spec.id.domain());
    let memory = spec.memory.mib();
    let cpus = spec.cpus.get();
    let os = match spec.firmware {
        // Secure Boot off: it would need keys enrolled and SMM; Windows 11's
        // TPM is not there either (later).
        VmFirmware::Uefi => "  <os firmware='efi'>\n    <type arch='x86_64' machine='q35'>hvm</type>\n    \
             <firmware>\n      <feature enabled='no' name='secure-boot'/>\n    </firmware>\n  </os>\n"
            .to_owned(),
        VmFirmware::Bios => "  <os>\n    <type arch='x86_64' machine='q35'>hvm</type>\n  </os>\n".to_owned(),
    };
    let hyperv = if windows {
        "    <hyperv mode='custom'>\n      <relaxed state='on'/>\n      <vapic state='on'/>\n      \
         <spinlocks state='on' retries='8191'/>\n    </hyperv>\n"
    } else {
        ""
    };
    let clock = if windows { "localtime" } else { "utc" };
    let (bus, disk_target, cdrom_target) =
        if virtio { ("virtio", "vda", "sda") } else { ("sata", "sda", "sdb") };
    let (disk_boot, cdrom_boot) = if iso.is_some() { (2, 1) } else { (1, 0) };
    let mut devices = format!(
        "    <disk type='block' device='disk'>\n      \
         <driver name='qemu' type='raw' cache='none' io='native' discard='unmap'/>\n      \
         <source dev='{disk}'/>\n      <target dev='{disk_target}' bus='{bus}'/>\n      \
         <boot order='{disk_boot}'/>\n    </disk>\n",
        disk = escape(&disk.to_string_lossy()),
    );
    if let Some(iso) = iso {
        devices.push_str(&format!(
            "    <disk type='file' device='cdrom'>\n      <driver name='qemu' type='raw'/>\n      \
             <source file='{iso}'/>\n      <target dev='{cdrom_target}' bus='sata'/>\n      <readonly/>\n      \
             <boot order='{cdrom_boot}'/>\n    </disk>\n",
            iso = escape(&iso.to_string_lossy()),
        ));
    }
    devices.push_str("    <controller type='usb' model='qemu-xhci'/>\n");
    let card = if virtio { "virtio" } else { "e1000e" };
    devices.push_str(&match &spec.network {
        // passt: the VM's traffic leaves from the NAS's own address; no
        // bridge, no NAT rules, nothing reaches the VM.
        VmNetwork::User => format!(
            "    <interface type='user'>\n      <backend type='passt'/>\n      <model type='{card}'/>\n    </interface>\n"
        ),
        VmNetwork::Bridge { bridge } => format!(
            "    <interface type='bridge'>\n      <source bridge='{}'/>\n      <model type='{card}'/>\n    </interface>\n",
            escape(bridge.as_str())
        ),
    });
    devices.push_str("    <input type='tablet' bus='usb'/>\n");
    devices.push_str(&format!(
        "    <graphics type='vnc'>\n      <listen type='socket' socket='{}'/>\n    </graphics>\n",
        escape(&console.to_string_lossy())
    ));
    devices.push_str(&format!(
        "    <video>\n      <model type='{}'/>\n    </video>\n",
        if virtio { "virtio" } else { "vga" }
    ));
    if virtio {
        devices.push_str("    <memballoon model='virtio'/>\n");
        devices.push_str(
            "    <rng model='virtio'>\n      <backend model='random'>/dev/urandom</backend>\n    </rng>\n",
        );
    } else {
        devices.push_str("    <memballoon model='none'/>\n");
    }
    format!(
        "<!-- Written by zendiloOS from the VM's settings; changed only there. -->\n\
         <domain type='kvm'>\n  <name>{name}</name>\n  \
         <memory unit='MiB'>{memory}</memory>\n  <currentMemory unit='MiB'>{memory}</currentMemory>\n  \
         <vcpu placement='static'>{cpus}</vcpu>\n{os}  <features>\n    <acpi/>\n    <apic/>\n{hyperv}  </features>\n  \
         <cpu mode='host-passthrough' check='none'/>\n  <clock offset='{clock}'/>\n  \
         <on_poweroff>destroy</on_poweroff>\n  <on_reboot>restart</on_reboot>\n  <on_crash>destroy</on_crash>\n  \
         <pm>\n    <suspend-to-mem enabled='no'/>\n    <suspend-to-disk enabled='no'/>\n  </pm>\n  \
         <devices>\n{devices}  </devices>\n</domain>\n"
    )
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{VmCpus, VmDiskSize, VmMemory};

    use super::*;

    fn spec(os: VmOs, firmware: VmFirmware, network: VmNetwork) -> VmSpec {
        VmSpec {
            id: VmId::parse("0a1b2c3d4e5f").unwrap(),
            os,
            cpus: VmCpus::new(2).unwrap(),
            memory: VmMemory::new(2048).unwrap(),
            firmware,
            disk: VmDiskSize::new(20).unwrap(),
            network,
            iso: true,
            autostart: false,
            pool: PoolName::parse("tank").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
        }
    }

    const DISK: &str = "/dev/zvol/tank/zendiloos.vms/0a1b2c3d4e5f-disk0";
    const ISO: &str = "/volume1/@vms/isos/0a1b2c3d4e5f.iso";
    const CONSOLE: &str = "/run/zendiloos-vms/0a1b2c3d4e5f.vnc";

    fn xml(spec: &VmSpec, iso: bool) -> String {
        domain_xml(spec, Path::new(DISK), iso.then_some(Path::new(ISO)), Path::new(CONSOLE))
    }

    #[test]
    fn a_linux_vm_gets_virtio_uefi_passt_and_its_iso_first() {
        let xml = xml(&spec(VmOs::Linux, VmFirmware::Uefi, VmNetwork::User), true);
        for part in [
            "<domain type='kvm'>",
            "<name>zendiloos-0a1b2c3d4e5f</name>",
            "<memory unit='MiB'>2048</memory>",
            "<vcpu placement='static'>2</vcpu>",
            "<os firmware='efi'>",
            "<feature enabled='no' name='secure-boot'/>",
            "<source dev='/dev/zvol/tank/zendiloos.vms/0a1b2c3d4e5f-disk0'/>\n      <target dev='vda' bus='virtio'/>\n      <boot order='2'/>",
            "<source file='/volume1/@vms/isos/0a1b2c3d4e5f.iso'/>\n      <target dev='sda' bus='sata'/>\n      <readonly/>\n      <boot order='1'/>",
            "<interface type='user'>\n      <backend type='passt'/>\n      <model type='virtio'/>",
            "<listen type='socket' socket='/run/zendiloos-vms/0a1b2c3d4e5f.vnc'/>",
            "<clock offset='utc'/>",
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

    #[test]
    fn windows_gets_sata_e1000e_local_time_and_hyperv() {
        let bridge = VmNetwork::Bridge { bridge: InterfaceName::parse("br0").unwrap() };
        let xml = xml(&spec(VmOs::Windows, VmFirmware::Uefi, bridge), false);
        assert!(xml.contains("<target dev='sda' bus='sata'/>\n      <boot order='1'/>"), "{xml}");
        assert!(xml.contains(
            "<interface type='bridge'>\n      <source bridge='br0'/>\n      <model type='e1000e'/>"
        ));
        assert!(xml.contains("<clock offset='localtime'/>") && xml.contains("<hyperv mode='custom'>"));
        assert!(xml.contains("<model type='vga'/>") && xml.contains("<memballoon model='none'/>"));
        assert!(!xml.contains("device='cdrom'"), "no ISO, no drive");
    }

    #[test]
    fn bios_has_no_firmware_element() {
        let xml = xml(&spec(VmOs::Other, VmFirmware::Bios, VmNetwork::User), true);
        assert!(xml.contains("<os>\n    <type arch='x86_64' machine='q35'>hvm</type>\n  </os>"), "{xml}");
        assert!(!xml.contains("firmware="));
        assert!(xml.contains("bus='sata'") && xml.contains("<model type='e1000e'/>"));
    }

    #[test]
    fn values_are_escaped_should_one_ever_hold_markup() {
        assert_eq!(escape("a'<b>&\""), "a&apos;&lt;b&gt;&amp;&quot;");
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
    fn the_storage_is_where_its_one_mount_unit_says() {
        let units = tempfile::tempdir().unwrap();
        let host = HostPaths { units: units.path().to_owned(), ..HostPaths::default() };
        assert_eq!(vms_storage(&host), None);
        fs::write(
            units.path().join("volume2-\\x40vms.mount"),
            "[Mount]\nWhat=cold/zendiloos.vms\nWhere=/volume2/@vms\n",
        )
        .unwrap();
        assert_eq!(
            vms_storage(&host),
            Some(VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() })
        );
        fs::write(units.path().join("volume1-\\x40vms.mount"), "[Mount]\nWhat=tank/zendiloos.vms\n").unwrap();
        assert_eq!(vms_storage(&host), None, "two are none");
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
