//! [`VmHost`] through the system agent: libvirt's facts and states read
//! here, unprivileged; the VMs' settings checked once more as protocol
//! types; a console's socket relayed to channels; what the locations hold
//! read from `zfs list`. The bridges, internal networks and cards VMs may
//! go on come from the Network context's view of the host (`ip`, which
//! zendiloosd runs in the host's network namespace; the agent has one of
//! its own and sees none of the host's links) and its files.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, InterfaceName, MacAddress, PackageRunState, PackageTask, PoolName, Report,
    Request, RequestId, Response, ShareLocation, ShareName, SharePath, VmBoot, VmCpus, VmDevices, VmDisk,
    VmDiskBus, VmDiskSize, VmDiskSlot, VmDrive, VmDriveSlot, VmFirmware, VmId, VmImageId, VmIsoSource,
    VmMemory, VmNetwork, VmNic, VmNicModel, VmNicSlot, VmOs, VmSpec, VmStorage, VolumeNumber,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_network::application::NetworkUseCases;
use zendiloos_network::domain::{InterfaceKind, Network as HostNetwork};
use zendiloos_storage::application::ports::ZfsSource;
use zendiloos_storage::domain::VdevState;
use zendiloos_vms::application::{
    ConsoleSession, DatasetUsage, HostError, HostFacts, IsoSource, Steps, VmHost, VmIds, VmPool, ZvolUsage,
};
use zendiloos_vms::domain::{
    BootDevice, DiskBus, Firmware, HostLimits, Location, Network, NicModel, OsPreset, PowerState, Vm,
    VmAction, VmCounters,
};

use super::{AgentStorage, WHAT, agent_error, call, call_raw, call_raw_reporting, unexpected};

const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);
/// What is read from a console at once.
const CONSOLE_CHUNK: usize = 64 * 1024;

pub struct AgentVms {
    transport: Arc<dyn AgentTransport>,
    storage: AgentStorage,
    network: NetworkUseCases,
}

/// What VMs may go on: bridges of the host's, internal networks, and wired
/// cards and bonds for a card directly on one.
#[derive(Debug, Default, PartialEq, Eq)]
struct VmNetworks {
    bridges: Vec<String>,
    internal: Vec<String>,
    cards: Vec<String>,
}

impl AgentVms {
    /// `network` reads the host's interfaces and zendiloOS's files, for
    /// the bridges, internal networks and cards VMs may go on.
    pub fn new(transport: Arc<dyn AgentTransport>, network: NetworkUseCases) -> Self {
        Self { storage: AgentStorage::new(transport.clone()), transport, network }
    }

    /// What VMs may go on; nothing while the network cannot be read, which
    /// keeps a VM off a bridge rather than failing the page. Internal
    /// networks need zendiloOS's files too; while those cannot be read,
    /// none is offered, and none is taken for a bridge of the LAN's.
    async fn networks(&self, context: &RequestContext) -> VmNetworks {
        let network = match self.network.get_network.execute().await {
            Ok(network) => network,
            Err(error) => {
                tracing::warn!(%error, "vms: cannot read the network for its bridges and cards");
                return VmNetworks::default();
            }
        };
        let internal = match self.network.configure.setup(context).await {
            Ok(setup) => Some(setup.internal),
            Err(error) => {
                tracing::warn!(%error, "vms: cannot read which bridges are internal networks");
                None
            }
        };
        vm_networks(&network, internal)
    }

    fn transport(&self) -> &dyn AgentTransport {
        self.transport.as_ref()
    }

    /// `Done`, or the agent's refusal or failure in its words.
    async fn done(&self, request: Request, context: &RequestContext) -> Result<(), HostError> {
        host_answer(call_raw(self.transport(), request, context).await?)
    }
}

/// What VMs may go on. Bridges: every one of the machine's but Docker's
/// (`docker0`, `br-` and twelve hex digits, which the network's view
/// leaves out already) and libvirt's (`virbr…`), those zendiloOS made and
/// those made otherwise; of them, the internal networks (`internal`, none
/// while not known, which then are no bridges either) apart. Cards: the
/// wired cards and bonds in no bridge or bond, which macvtap can sit on.
fn vm_networks(network: &HostNetwork, internal: Option<Vec<String>>) -> VmNetworks {
    let all: Vec<String> = network
        .interfaces
        .iter()
        .filter(|interface| interface.kind == InterfaceKind::Bridge)
        .map(|interface| interface.name.clone())
        .filter(|name| !network.docker_bridges.contains(name) && !name.starts_with("virbr"))
        .collect();
    let (internal, bridges) = match internal {
        Some(internal) => {
            let internal: Vec<String> = all.iter().filter(|name| internal.contains(name)).cloned().collect();
            let bridges = all.into_iter().filter(|name| !internal.contains(name)).collect();
            (internal, bridges)
        }
        None => (Vec::new(), Vec::new()),
    };
    let cards = network
        .interfaces
        .iter()
        .filter(|interface| matches!(interface.kind, InterfaceKind::Ethernet | InterfaceKind::Bond))
        .filter(|interface| interface.member_of.is_none())
        .map(|interface| interface.name.clone())
        .collect();
    VmNetworks { bridges, internal, cards }
}

/// ZFS's word for a pool's health.
fn health(state: VdevState) -> &'static str {
    match state {
        VdevState::Online => "online",
        VdevState::Degraded => "degraded",
        VdevState::Faulted => "faulted",
        VdevState::Offline => "offline",
        VdevState::Removed => "removed",
        VdevState::Unavailable => "unavailable",
        VdevState::Suspended => "suspended",
        VdevState::Unknown => "unknown",
    }
}

/// `zfs list -H -p -r -t filesystem,volume -o
/// name,type,used,usedbydataset,referenced,available,volsize` of the
/// locations' datasets: each `<pool>/zendiloos.vms` with the zvols right
/// below it. Lines of anything else are passed by.
pub(crate) fn storage_usage(text: &str) -> Vec<DatasetUsage> {
    let mut found: Vec<DatasetUsage> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, kind, used, files, referenced, available, volsize] = fields[..] else { continue };
        let number = |text: &str| text.trim().parse::<u64>().ok();
        let suffix = format!("/{}", zendiloos_agent_proto::VMS_DATASET);
        match kind {
            "filesystem" => {
                let Some(pool) = name.strip_suffix(&suffix).filter(|pool| !pool.contains('/')) else {
                    continue;
                };
                let (Some(used), Some(files), Some(available)) =
                    (number(used), number(files), number(available))
                else {
                    continue;
                };
                found.push(DatasetUsage {
                    pool: pool.to_owned(),
                    used_bytes: used,
                    files_bytes: files,
                    available_bytes: available,
                    zvols: Vec::new(),
                });
            }
            "volume" => {
                let Some((dataset, zvol)) = name.rsplit_once('/') else { continue };
                let Some(pool) = dataset.strip_suffix(&suffix) else { continue };
                let Some(usage) = found.iter_mut().find(|usage| usage.pool == pool) else { continue };
                let (Some(used), Some(referenced), Some(volsize)) =
                    (number(used), number(referenced), number(volsize))
                else {
                    continue;
                };
                usage.zvols.push(ZvolUsage {
                    name: zvol.to_owned(),
                    used_bytes: used,
                    referenced_bytes: referenced,
                    volsize_bytes: volsize,
                });
            }
            _ => {}
        }
    }
    found
}

fn host_answer(response: Response) -> Result<(), HostError> {
    match response {
        Response::Done => Ok(()),
        Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
            Err(HostError::Refused(message.trim_start_matches("refused: ").to_owned()))
        }
        Response::Error { code: ErrorCode::Failed, message } => Err(HostError::Failed(message)),
        Response::Error { code, message } => Err(agent_error(code, &message).into()),
        other => Err(unexpected(other).into()),
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn vm_id(id: &str) -> Result<VmId, HostError> {
    VmId::parse(id).map_err(|_| invalid("VM id"))
}

fn place(pool: &str, volume: u16) -> Result<(PoolName, VolumeNumber), HostError> {
    Ok((
        PoolName::parse(pool).map_err(|_| invalid("pool name"))?,
        VolumeNumber::new(volume).map_err(|_| invalid("volume number"))?,
    ))
}

fn location(location: &Location) -> Result<VmStorage, HostError> {
    let (pool, volume) = place(&location.pool, location.volume)?;
    Ok(VmStorage { pool, volume })
}

fn interface(name: &str, what: &str) -> Result<InterfaceName, HostError> {
    InterfaceName::parse(name).map_err(|_| invalid(what))
}

fn image_id(id: &str) -> Result<VmImageId, HostError> {
    VmImageId::parse(id).map_err(|_| invalid("image id"))
}

fn volume_number(volume: u16) -> Result<VolumeNumber, HostError> {
    VolumeNumber::new(volume).map_err(|_| invalid("volume number"))
}

fn disk_slot(slot: u8) -> Result<VmDiskSlot, HostError> {
    VmDiskSlot::new(slot).map_err(|_| invalid("disk slot"))
}

fn drive_slot(slot: u8) -> Result<VmDriveSlot, HostError> {
    VmDriveSlot::new(slot).map_err(|_| invalid("drive slot"))
}

/// The VM in the agent's terms: every value checked once more; the images
/// in its drives on the library's volume, `library`.
fn spec(vm: &Vm, library: u16) -> Result<VmSpec, HostError> {
    let settings = &vm.settings;
    let disks = settings
        .disks
        .iter()
        .map(|disk| {
            Ok(VmDisk {
                slot: disk_slot(disk.slot)?,
                size: VmDiskSize::new(disk.size_gib).map_err(|_| invalid("disk size"))?,
                bus: match disk.bus {
                    DiskBus::Virtio => VmDiskBus::Virtio,
                    DiskBus::Sata => VmDiskBus::Sata,
                },
                location: location(&disk.location)?,
            })
        })
        .collect::<Result<Vec<_>, HostError>>()?;
    let nics = settings
        .nics
        .iter()
        .map(|nic| {
            Ok(VmNic {
                slot: VmNicSlot::new(nic.slot).map_err(|_| invalid("card slot"))?,
                model: match nic.model {
                    NicModel::Virtio => VmNicModel::Virtio,
                    NicModel::E1000e => VmNicModel::E1000e,
                },
                network: match &nic.network {
                    Network::User => VmNetwork::User,
                    Network::Bridge(bridge) => VmNetwork::Bridge { bridge: interface(bridge, "bridge")? },
                    Network::Direct(card) => VmNetwork::Direct { card: interface(card, "card")? },
                    Network::Internal(bridge) => {
                        VmNetwork::Internal { bridge: interface(bridge, "internal network")? }
                    }
                },
                mac: MacAddress::parse(&nic.mac).map_err(|_| invalid("MAC address"))?,
            })
        })
        .collect::<Result<Vec<_>, HostError>>()?;
    let drives = settings
        .drives
        .iter()
        .map(|drive| {
            Ok(VmDrive {
                slot: drive_slot(drive.slot)?,
                image: drive.image.as_deref().map(image_id).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, HostError>>()?;
    let boot = settings
        .boot
        .iter()
        .map(|device| match device {
            BootDevice::Disk(slot) => disk_slot(*slot).map(VmBoot::Disk),
            BootDevice::Drive(slot) => drive_slot(*slot).map(VmBoot::Drive),
        })
        .collect::<Result<Vec<_>, HostError>>()?;
    VmSpec::new(
        vm_id(&vm.id)?,
        match settings.os {
            OsPreset::Linux => VmOs::Linux,
            OsPreset::Windows => VmOs::Windows,
            OsPreset::Other => VmOs::Other,
        },
        VmCpus::new(settings.cpus).map_err(|_| invalid("number of CPUs"))?,
        VmMemory::new(settings.memory_mib).map_err(|_| invalid("memory size"))?,
        match settings.firmware {
            Firmware::Uefi => VmFirmware::Uefi,
            Firmware::Bios => VmFirmware::Bios,
        },
        VmDevices { disks, nics, drives, boot },
        settings.autostart,
        volume_number(library)?,
    )
    .map_err(|error| HostError::Port(PortError::new(WHAT, format!("not a valid VM for the agent: {error}"))))
}

/// `Response::VmImageStored`'s bytes, or the agent's refusal or failure.
fn stored(response: Response) -> Result<u64, HostError> {
    match response {
        Response::VmImageStored { bytes } => Ok(bytes),
        other => Err(host_answer(other).err().unwrap_or_else(|| invalid("answer"))),
    }
}

/// `virsh domstats`: the counters of each domain zendiloOS made, by id.
/// What a domain does not tell (a guest without a balloon driver, a card
/// without a device on the host) is none.
pub(crate) fn counters(text: &str) -> BTreeMap<String, VmCounters> {
    let mut all = BTreeMap::new();
    let mut current: Option<(String, VmCounters)> = None;
    let add = |sum: &mut Option<u64>, value: u64| *sum = Some(sum.unwrap_or(0).saturating_add(value));
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Domain: '").and_then(|rest| rest.strip_suffix('\'')) {
            all.extend(current.take());
            current = VmId::of_domain(name).map(|id| (id.as_str().to_owned(), VmCounters::default()));
            continue;
        }
        let Some((_, counters)) = current.as_mut() else { continue };
        let Some((key, value)) = line.trim().split_once('=') else { continue };
        let Ok(value) = value.parse::<u64>() else { continue };
        match key {
            "cpu.time" => counters.cpu_time_ns = Some(value),
            "balloon.current" => counters.memory_current_kib = Some(value),
            "balloon.available" => counters.memory_available_kib = Some(value),
            "balloon.usable" => counters.memory_usable_kib = Some(value),
            "balloon.unused" => counters.memory_unused_kib = Some(value),
            "balloon.rss" => counters.memory_rss_kib = Some(value),
            _ if key.starts_with("net.") && key.ends_with(".rx.bytes") => {
                add(&mut counters.received_bytes, value)
            }
            _ if key.starts_with("net.") && key.ends_with(".tx.bytes") => {
                add(&mut counters.sent_bytes, value)
            }
            _ => {}
        }
    }
    all.extend(current);
    all
}

/// `virsh list --all`: the state of each domain zendiloOS made, by id;
/// other domains are left out.
pub(crate) fn states(text: &str) -> BTreeMap<String, PowerState> {
    text.lines()
        .skip_while(|line| !line.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let _number = words.next()?;
            let id = VmId::of_domain(words.next()?)?;
            let state = words.collect::<Vec<_>>().join(" ");
            Some((id.as_str().to_owned(), PowerState::from_virsh(&state)))
        })
        .collect()
}

/// Steps the agent reports, passed on.
struct Forward<'a>(&'a dyn Steps);

impl Report for Forward<'_> {
    fn step(&self, step: &str, percent: Option<u8>) {
        self.0.step(step, percent);
    }
}

#[async_trait]
impl VmHost for AgentVms {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError> {
        match call(self.transport(), Request::VmHost {}, context).await? {
            Response::VmHost {
                kvm,
                cpu_virtualization,
                iommu,
                installed,
                cores,
                memory_bytes,
                locations,
            } => Ok(HostFacts {
                kvm,
                cpu_virtualization: cpu_virtualization.filter(|flag| flag == "vmx" || flag == "svm"),
                iommu,
                installed,
                limits: {
                    let VmNetworks { bridges, internal, cards } = self.networks(context).await;
                    HostLimits { cores, memory_bytes, bridges, internal, cards, locations: Vec::new() }
                },
                locations: locations
                    .into_iter()
                    .map(|location| Location {
                        pool: location.pool.as_str().to_owned(),
                        volume: location.volume.get(),
                    })
                    .collect(),
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn pools(&self, context: &RequestContext) -> Result<Vec<VmPool>, PortError> {
        Ok(self
            .storage
            .pools(context)
            .await?
            .into_iter()
            .filter_map(|pool| {
                let space = pool.space;
                Some(VmPool {
                    location: Location { volume: pool.volume?.get(), pool: pool.name },
                    health: health(pool.state).to_owned(),
                    used_bytes: space.map_or(pool.allocated_bytes, |space| space.used_bytes),
                    available_bytes: space
                        .map_or(pool.size_bytes.saturating_sub(pool.allocated_bytes), |space| {
                            space.available_bytes
                        }),
                })
            })
            .collect())
    }

    async fn install(&self, context: &RequestContext) -> Result<(), HostError> {
        let request = Request::StartPackages { task: PackageTask::InstallVms, release: None };
        let id = match call_raw(self.transport(), request, context).await? {
            Response::PackagesStarted { id } => id,
            other => return Err(host_answer(other).err().unwrap_or_else(|| invalid("answer"))),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport(), Request::PackagesState {}, context).await? {
                Response::Packages { run, .. } => run,
                other => return Err(unexpected(other).into()),
            };
            match run {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => return Ok(()),
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    return Err(HostError::Failed(run.error.unwrap_or_else(|| "apt failed".into())));
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(HostError::Failed("apt did not finish within an hour".into()));
                }
                _ => {}
            }
        }
    }

    async fn add_location(&self, storage: &Location, context: &RequestContext) -> Result<(), HostError> {
        let (pool, volume) = place(&storage.pool, storage.volume)?;
        self.done(Request::EnableVms { pool, volume }, context).await
    }

    async fn remove_location(&self, storage: &Location, context: &RequestContext) -> Result<(), HostError> {
        let (pool, volume) = place(&storage.pool, storage.volume)?;
        self.done(Request::DisableVms { pool, volume }, context).await
    }

    async fn storage_usage(&self, context: &RequestContext) -> Result<Vec<DatasetUsage>, PortError> {
        match call(self.transport(), Request::VmStorageUsage {}, context).await? {
            Response::VmStorageUsage { zfs_list } => Ok(storage_usage(&zfs_list)),
            other => Err(unexpected(other)),
        }
    }

    async fn copy_image(
        &self,
        image: &str,
        storage: &Location,
        iso: &IsoSource,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<u64, HostError> {
        let request = Request::CopyVmImage {
            image: image_id(image)?,
            volume: volume_number(storage.volume)?,
            source: VmIsoSource {
                share: ShareLocation {
                    volume: volume_number(iso.volume)?,
                    share: ShareName::parse(&iso.share).map_err(|_| invalid("share name"))?,
                },
                path: SharePath::parse(&iso.path).map_err(|_| invalid("path in a share"))?,
            },
        };
        stored(call_raw_reporting(self.transport(), request, &Forward(steps), context).await?)
    }

    async fn image_files(
        &self,
        storage: &Location,
        context: &RequestContext,
    ) -> Result<BTreeMap<String, u64>, PortError> {
        let volume =
            VolumeNumber::new(storage.volume).map_err(|_| PortError::new(WHAT, "not a volume number"))?;
        match call(self.transport(), Request::VmImages { volume }, context).await? {
            Response::VmImages { files } => {
                Ok(files.into_iter().map(|file| (file.image.as_str().to_owned(), file.bytes)).collect())
            }
            other => Err(unexpected(other)),
        }
    }

    async fn delete_image(
        &self,
        image: &str,
        volume: u16,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        self.done(Request::DeleteVmImage { image: image_id(image)?, volume: volume_number(volume)? }, context)
            .await
    }

    async fn open_upload(
        &self,
        image: &str,
        volume: u16,
        bytes: u64,
        context: &RequestContext,
    ) -> Result<tokio::fs::File, HostError> {
        let request =
            Request::OpenVmImageUpload { image: image_id(image)?, volume: volume_number(volume)?, bytes };
        let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
        let answer =
            self.transport.call_with_file(request, request_id).await.map_err(|e| PortError::new(WHAT, e))?;
        match answer {
            (Response::VmImageUpload {}, Some(pipe)) => {
                Ok(tokio::fs::File::from_std(std::fs::File::from(pipe)))
            }
            (other, _) => Err(host_answer(other).err().unwrap_or_else(|| invalid("answer"))),
        }
    }

    async fn finish_upload(&self, image: &str, context: &RequestContext) -> Result<u64, HostError> {
        stored(
            call_raw(self.transport(), Request::FinishVmImageUpload { image: image_id(image)? }, context)
                .await?,
        )
    }

    async fn define(&self, vm: &Vm, library: u16, context: &RequestContext) -> Result<(), HostError> {
        self.done(Request::DefineVm { spec: spec(vm, library)? }, context).await
    }

    async fn update(
        &self,
        vm: &Vm,
        library: u16,
        removed: &[u8],
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let remove = removed.iter().map(|slot| disk_slot(*slot)).collect::<Result<Vec<_>, _>>()?;
        self.done(Request::UpdateVm { spec: spec(vm, library)?, remove }, context).await
    }

    async fn change_media(
        &self,
        vm: &Vm,
        library: u16,
        drive: u8,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::ChangeVmMedia { spec: spec(vm, library)?, drive: drive_slot(drive)? };
        self.done(request, context).await
    }

    async fn set_autostart(
        &self,
        id: &str,
        autostart: bool,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        self.done(Request::SetVmAutostart { id: vm_id(id)?, autostart }, context).await
    }

    async fn control(&self, id: &str, action: VmAction, context: &RequestContext) -> Result<(), HostError> {
        let action = match action {
            VmAction::Start => zendiloos_agent_proto::VmAction::Start,
            VmAction::Shutdown => zendiloos_agent_proto::VmAction::Shutdown,
            VmAction::ForceOff => zendiloos_agent_proto::VmAction::ForceOff,
            VmAction::Restart => zendiloos_agent_proto::VmAction::Reboot,
        };
        self.done(Request::ControlVm { id: vm_id(id)?, action }, context).await
    }

    async fn remove(&self, vm: &Vm, context: &RequestContext) -> Result<(), HostError> {
        self.done(Request::DeleteVm { id: vm_id(&vm.id)? }, context).await
    }

    async fn states(&self, context: &RequestContext) -> Result<BTreeMap<String, PowerState>, PortError> {
        match call(self.transport(), Request::VmStates {}, context).await? {
            Response::VmStates { virsh_list } => Ok(states(&virsh_list)),
            other => Err(unexpected(other)),
        }
    }

    async fn counters(&self, context: &RequestContext) -> Result<BTreeMap<String, VmCounters>, PortError> {
        match call(self.transport(), Request::VmStats {}, context).await? {
            Response::VmStats { domstats } => Ok(counters(&domstats)),
            other => Err(unexpected(other)),
        }
    }

    async fn open_console(&self, id: &str, context: &RequestContext) -> Result<ConsoleSession, HostError> {
        let request = Request::OpenVmConsole { id: vm_id(id)? };
        let request_id = context.request_id.as_deref().and_then(|id| RequestId::parse(id).ok());
        let answer =
            self.transport.call_with_file(request, request_id).await.map_err(|e| PortError::new(WHAT, e))?;
        match answer {
            (Response::VmConsole {}, Some(socket)) => relay(socket)
                .map_err(|e| PortError::new(WHAT, format!("cannot relay the console: {e}")).into()),
            (other, _) => Err(host_answer(other).err().unwrap_or_else(|| invalid("answer"))),
        }
    }
}

/// Relays a console's socket between the descriptor and the session's
/// channels. Dropping the session's `input` closes the socket.
fn relay(socket: OwnedFd) -> std::io::Result<ConsoleSession> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let socket = std::os::unix::net::UnixStream::from(socket);
    socket.set_nonblocking(true)?;
    let socket = tokio::net::UnixStream::from_std(socket)?;
    let (mut reader, mut writer) = socket.into_split();
    let (output, from_console) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (input, mut to_console) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(async move {
        let mut buffer = vec![0u8; CONSOLE_CHUNK];
        loop {
            tokio::select! {
                read = reader.read(&mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output.send(buffer[..n].to_vec()).await.is_err() {
                            break;
                        }
                    }
                },
                typed = to_console.recv() => match typed {
                    Some(data) => {
                        if writer.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    // Closed: the socket closes as this ends.
                    None => break,
                },
            }
        }
    });
    Ok(ConsoleSession { output: from_console, input })
}

/// Ids at random from the system's source.
pub struct RandomVmIds;

impl VmIds for RandomVmIds {
    fn new_id(&self) -> String {
        let mut bytes = [0_u8; 6];
        getrandom::fill(&mut bytes).expect("the system's random source is available");
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn new_mac(&self) -> String {
        let mut bytes = [0_u8; 3];
        getrandom::fill(&mut bytes).expect("the system's random source is available");
        format!("52:54:00:{:02x}:{:02x}:{:02x}", bytes[0], bytes[1], bytes[2])
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;
    use zendiloos_sim::SimulatedAgent;
    use zendiloos_vms::domain::{Drive, VmSettings};

    use super::*;

    #[test]
    fn states_come_only_for_zendiloos_domains() {
        let text = " Id   Name                     State\n\
                    -------------------------------------------\n \
                    1    zendiloos-0a1b2c3d4e5f   running\n \
                    -    zendiloos-00000000000a   shut off\n \
                    -    zendiloos-00000000000b   in shutdown\n \
                    3    win10                    running\n \
                    -    zendiloos-win10          paused\n\n";
        let read = states(text);
        assert_eq!(read.len(), 3);
        assert_eq!(read["0a1b2c3d4e5f"], PowerState::Running);
        assert_eq!(read["00000000000a"], PowerState::ShutOff);
        assert_eq!(read["00000000000b"], PowerState::ShuttingDown);
        assert!(states("").is_empty());
    }

    #[test]
    fn ids_are_twelve_hex_digits() {
        let id = RandomVmIds.new_id();
        assert!(zendiloos_vms::domain::is_vm_id(&id), "{id}");
        assert_ne!(id, RandomVmIds.new_id());
    }

    fn vm(id: &str) -> Vm {
        let mut settings =
            VmSettings { cpus: 1, memory_mib: 256, ..VmSettings::preset("box", OsPreset::Linux) };
        settings.disks[0].location = Location { pool: "tank".into(), volume: 1 };
        settings.nics[0].mac = "52:54:00:00:00:01".into();
        settings.drives = vec![Drive { slot: 0, image: Some("00000000000a".into()) }];
        Vm { id: id.into(), settings, created_at: Timestamp::from_unix_seconds(0), ready: false }
    }

    /// The network's use cases over `source` and zendiloOS's files making
    /// `vms0` an internal network.
    fn network_of(source: Arc<dyn zendiloos_network::application::NetworkSource>) -> NetworkUseCases {
        use zendiloos_network::application::NetworkPorts;
        use zendiloos_network::domain::NetworkdFile;
        use zendiloos_network::testing::{FakeConfigurator, FakeUsers};
        let configurator = FakeConfigurator::default();
        configurator.files.lock().unwrap().extend([
            NetworkdFile {
                name: "05-zendiloos-vms0.netdev".into(),
                text: "[NetDev]\nName=vms0\nKind=bridge\n".into(),
            },
            NetworkdFile {
                name: "05-zendiloos-vms0.network".into(),
                text: "[Match]\nName=vms0\n\n[Network]\nDHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n"
                    .into(),
            },
        ]);
        NetworkUseCases::new(NetworkPorts {
            source,
            configurator: Arc::new(configurator),
            users: Arc::new(FakeUsers::default()),
            jobs: zendiloos_jobs::Jobs::new(
                Arc::new(zendiloos_jobs::testing::MemoryJobs::default()),
                Arc::new(zendiloos_jobs::testing::FixedClock),
            ),
            audit: Arc::new(zendiloos_vms::testing::MemoryAudit::default()),
            clock: Arc::new(zendiloos_jobs::testing::FixedClock),
        })
    }

    /// The host as zendiloosd reads it: a card in the bridge br0, a bridge
    /// for VMs only without members (an internal network by zendiloOS's
    /// files), Docker's and libvirt's bridges, a free card, a bond.
    fn network_with_bridges() -> NetworkUseCases {
        use zendiloos_network::domain::{Interface, LinkState};
        use zendiloos_network::testing::SettableNetwork;
        let interface = |name: &str, kind: InterfaceKind, member_of: Option<&str>| Interface {
            name: name.into(),
            kind,
            state: LinkState::Up,
            mac: None,
            mtu: 1500,
            member_of: member_of.map(Into::into),
            parent: None,
            addresses: vec![],
        };
        let network = HostNetwork::new(
            vec![
                interface("enp1s0", InterfaceKind::Ethernet, Some("br0")),
                interface("br0", InterfaceKind::Bridge, None),
                interface("vms0", InterfaceKind::Bridge, None),
                interface("docker0", InterfaceKind::Bridge, None),
                interface("br-0123456789ab", InterfaceKind::Bridge, None),
                interface("virbr0", InterfaceKind::Bridge, None),
                interface("bond0", InterfaceKind::Bond, None),
                interface("enp2s0", InterfaceKind::Ethernet, Some("bond0")),
                interface("enp3s0", InterfaceKind::Ethernet, None),
                interface("wlp1s0", InterfaceKind::Wireless, None),
            ],
            vec![],
            vec![],
        );
        let source = Arc::new(SettableNetwork::default());
        source.set(network);
        network_of(source)
    }

    /// The wizard offers every bridge of the machine's but Docker's and
    /// libvirt's, the internal networks apart, and the wired cards and
    /// bonds in no bridge or bond; nothing while the network cannot be read.
    #[tokio::test]
    async fn vms_are_offered_the_machines_own_bridges_internal_networks_and_cards() {
        let host = AgentVms::new(Arc::new(SimulatedAgent::new()), network_with_bridges());
        let facts = host.facts(&RequestContext::default()).await.unwrap();
        assert_eq!(facts.limits.bridges, ["br0"]);
        assert_eq!(facts.limits.internal, ["vms0"]);
        assert_eq!(facts.limits.cards, ["bond0", "enp3s0"]);

        struct Unreadable;
        #[async_trait]
        impl zendiloos_network::application::NetworkSource for Unreadable {
            async fn network(&self) -> Result<HostNetwork, PortError> {
                Err(PortError::new("network", "ip not found"))
            }
        }
        let blind = AgentVms::new(Arc::new(SimulatedAgent::new()), network_of(Arc::new(Unreadable)));
        let facts = blind.facts(&RequestContext::default()).await.unwrap();
        assert!(facts.limits.bridges.is_empty() && facts.limits.cards.is_empty() && facts.kvm);
    }

    /// Without zendiloOS's files no bridge is taken for one of the LAN's:
    /// it might be an internal network.
    #[test]
    fn without_the_files_no_bridge_is_offered() {
        use zendiloos_network::domain::{Interface, LinkState};
        let bridge = Interface {
            name: "br0".into(),
            kind: InterfaceKind::Bridge,
            state: LinkState::Up,
            mac: None,
            mtu: 1500,
            member_of: None,
            parent: None,
            addresses: vec![],
        };
        let network = HostNetwork::new(vec![bridge], vec![], vec![]);
        assert_eq!(vm_networks(&network, None), VmNetworks::default());
        assert_eq!(vm_networks(&network, Some(vec![])).bridges, ["br0"]);
    }

    #[test]
    fn what_the_locations_hold_is_read_from_zfs() {
        let text = "tank/zendiloos.vms\tfilesystem\t5368709120\t1073741824\t1073741824\t100000\t-\n\
                    tank/zendiloos.vms/0a1b2c3d4e5f-disk0\tvolume\t3221225472\t3221225472\t2147483648\t100000\t21474836480\n\
                    tank/zendiloos.vms/0a1b2c3d4e5f-disk0@x\tsnapshot\t1\t1\t1\t-\t-\n\
                    cold/zendiloos.vms\tfilesystem\t0\t0\t0\t7\t-\n\
                    cold/other/zendiloos.vms\tfilesystem\t1\t1\t1\t1\t-\n\
                    elsewhere/zendiloos.vms/x-disk0\tvolume\t1\t1\t1\t1\t1\n\
                    garbage\n";
        let read = storage_usage(text);
        assert_eq!(read.len(), 2);
        assert_eq!(
            (read[0].pool.as_str(), read[0].used_bytes, read[0].files_bytes),
            ("tank", 5 << 30, 1 << 30)
        );
        assert_eq!(read[0].zvols.len(), 1);
        let zvol = &read[0].zvols[0];
        assert_eq!(zvol.name, "0a1b2c3d4e5f-disk0");
        assert_eq!(
            (zvol.used_bytes, zvol.referenced_bytes, zvol.volsize_bytes),
            (3 << 30, 2 << 30, 20 << 30)
        );
        assert_eq!((read[1].pool.as_str(), read[1].available_bytes), ("cold", 7));
        assert!(read[1].zvols.is_empty());
        assert!(storage_usage("").is_empty());
    }

    #[test]
    fn a_vm_reaches_the_agent_only_as_checked_values() {
        let spec = spec(&vm("0a1b2c3d4e5f"), 1).unwrap();
        assert!(spec.nics()[0].network == VmNetwork::User && spec.memory.mib() == 256);
        assert_eq!(spec.images().map(VmImageId::as_str).collect::<Vec<_>>(), ["00000000000a"]);
        assert_eq!(
            spec.boot(),
            [VmBoot::Drive(VmDriveSlot::new(0).unwrap()), VmBoot::Disk(VmDiskSlot::new(0).unwrap())]
        );
        assert_eq!(spec.disks()[0].location.pool.as_str(), "tank");
        assert!(super::spec(&vm("../../etc"), 1).is_err());
        assert!(super::spec(&vm("0a1b2c3d4e5f"), 0).is_err(), "no volume 0");
        let broken = |change: fn(&mut Vm)| {
            let mut vm = vm("0a1b2c3d4e5f");
            change(&mut vm);
            super::spec(&vm, 1).is_err()
        };
        assert!(broken(|vm| vm.settings.disks[0].location.pool = "tank/../x".into()));
        assert!(broken(|vm| vm.settings.disks[0].location.volume = 0));
        assert!(broken(|vm| vm.settings.nics[0].network = Network::Direct("enp1s0 mode=vepa".into())));
        assert!(broken(|vm| vm.settings.nics[0].network = Network::Internal("".into())));
        assert!(broken(|vm| vm.settings.nics[0].network = Network::Bridge("br0 extra".into())));
        assert!(broken(|vm| vm.settings.memory_mib = 64));
        assert!(broken(|vm| vm.settings.nics[0].mac = "ff:ff:ff:ff:ff:ff".into()));
        assert!(broken(|vm| vm.settings.drives[0].image = Some("../../shadow".into())));
        assert!(broken(|vm| vm.settings.disks[0].slot = 8));
        assert!(broken(|vm| vm.settings.boot = vec![BootDevice::Disk(5)]));
    }

    #[test]
    fn counters_come_only_for_zendiloos_domains_and_add_up_the_cards() {
        let text = "Domain: 'zendiloos-0a1b2c3d4e5f'\n  state.state=1\n  cpu.time=12000000000\n  \
                    balloon.current=2097152\n  balloon.available=2000000\n  balloon.usable=1500000\n  \
                    balloon.rss=900000\n  net.count=2\n  net.0.name=vnet0\n  net.0.rx.bytes=100\n  \
                    net.0.tx.bytes=10\n  net.1.name=vnet1\n  net.1.rx.bytes=50\n  net.1.tx.bytes=5\n\n\
                    Domain: 'win10'\n  cpu.time=1\n\
                    Domain: 'zendiloos-00000000000b'\n  cpu.time=7\n  balloon.current=1024\n  balloon.rss=512\n  \
                    net.count=1\n";
        let read = counters(text);
        assert_eq!(read.len(), 2);
        let web = &read["0a1b2c3d4e5f"];
        assert_eq!(web.cpu_time_ns, Some(12_000_000_000));
        assert_eq!((web.received_bytes, web.sent_bytes), (Some(150), Some(15)));
        assert_eq!(web.memory_used_bytes(), Some(500_000 * 1024));
        let windows = &read["00000000000b"];
        assert_eq!((windows.received_bytes, windows.memory_used_bytes()), (None, Some(512 * 1024)));
        assert!(counters("").is_empty());
    }

    #[tokio::test]
    async fn vms_are_made_run_and_removed_through_the_agent() {
        let agent = Arc::new(SimulatedAgent::new());
        let host = AgentVms::new(agent.clone(), network_with_bridges());
        let context = RequestContext::default();
        let facts = host.facts(&context).await.unwrap();
        assert!(facts.kvm && !facts.installed && facts.locations.is_empty());
        assert_eq!(facts.limits.bridges, ["br0"]);
        let pools = host.pools(&context).await.unwrap();
        assert!(
            pools.iter().any(|pool| pool.health == "online")
                && pools.iter().any(|pool| pool.health == "degraded"),
            "{pools:?}"
        );
        let first = pools[0].location.clone();

        host.install(&context).await.unwrap();
        host.add_location(&first, &context).await.unwrap();
        assert_eq!(host.facts(&context).await.unwrap().locations, std::slice::from_ref(&first));

        let mut made = vm("0a1b2c3d4e5f");
        made.settings.disks[0].location = first.clone();
        let pools: Vec<Location> = pools.into_iter().map(|pool| pool.location).collect();
        let iso = IsoSource { share: "media".into(), volume: pools[0].volume, path: "isos/x.iso".into() };
        struct Silent;
        impl Steps for Silent {
            fn step(&self, _: &str, _: Option<u8>) {}
        }
        assert!(
            matches!(host.define(&made, first.volume, &context).await, Err(HostError::Refused(reason)) if reason.contains("library"))
        );
        assert_eq!(host.copy_image("00000000000a", &pools[0], &iso, &Silent, &context).await.unwrap(), 4096);
        assert_eq!(host.image_files(&pools[0], &context).await.unwrap()["00000000000a"], 4096);
        host.define(&made, first.volume, &context).await.unwrap();
        assert!(matches!(host.define(&made, first.volume, &context).await, Err(HostError::Refused(_))));
        let usage = host.storage_usage(&context).await.unwrap();
        assert_eq!(usage[0].zvols[0].name, "0a1b2c3d4e5f-disk0");
        let refused = host.remove_location(&first, &context).await;
        assert!(matches!(refused, Err(HostError::Refused(reason)) if reason.contains("disk")), "in use");
        assert_eq!(host.states(&context).await.unwrap()["0a1b2c3d4e5f"], PowerState::ShutOff);
        assert!(matches!(host.open_console(&made.id, &context).await, Err(HostError::Refused(_))));
        made.settings.cpus = 2;
        host.update(&made, first.volume, &[], &context).await.unwrap();

        host.control(&made.id, VmAction::Start, &context).await.unwrap();
        assert!(
            matches!(host.update(&made, first.volume, &[], &context).await, Err(HostError::Refused(reason)) if reason.contains("not shut off"))
        );
        host.change_media(&made, first.volume, 0, &context).await.unwrap();
        let counted = host.counters(&context).await.unwrap();
        assert!(counted["0a1b2c3d4e5f"].cpu_time_ns.is_some());
        let mut console = host.open_console(&made.id, &context).await.unwrap();
        assert_eq!(console.output.recv().await.unwrap(), b"RFB 003.008\n");
        drop(console);
        host.control(&made.id, VmAction::Shutdown, &context).await.unwrap();
        host.remove(&made, &context).await.unwrap();
        assert!(host.states(&context).await.unwrap().is_empty());
        host.delete_image("00000000000a", pools[0].volume, &context).await.unwrap();
        assert!(host.image_files(&pools[0], &context).await.unwrap().is_empty());
        host.remove_location(&first, &context).await.unwrap();
        assert!(host.facts(&context).await.unwrap().locations.is_empty());
    }

    #[tokio::test]
    async fn an_upload_is_written_into_the_agents_pipe() {
        use tokio::io::AsyncWriteExt;

        let host = AgentVms::new(Arc::new(SimulatedAgent::new()), network_with_bridges());
        let context = RequestContext::default();
        host.install(&context).await.unwrap();
        let pools: Vec<Location> =
            host.pools(&context).await.unwrap().into_iter().map(|pool| pool.location).collect();
        host.add_location(&pools[0], &context).await.unwrap();
        let mut file = host.open_upload("00000000000c", pools[0].volume, 5, &context).await.unwrap();
        file.write_all(b"hello").await.unwrap();
        file.flush().await.unwrap();
        drop(file);
        assert_eq!(host.finish_upload("00000000000c", &context).await.unwrap(), 5);
        assert!(matches!(host.finish_upload("00000000000c", &context).await, Err(HostError::Refused(_))));

        let mut short = host.open_upload("00000000000d", pools[0].volume, 5, &context).await.unwrap();
        short.write_all(b"he").await.unwrap();
        short.flush().await.unwrap();
        drop(short);
        let refused = host.finish_upload("00000000000d", &context).await;
        assert!(matches!(refused, Err(HostError::Refused(reason)) if reason.contains("2 of 5")));
        assert!(matches!(host.open_upload("x", 1, 5, &context).await, Err(HostError::Port(_))));
    }
}
