//! [`VmHost`] through the system agent: libvirt's facts and states read
//! here, unprivileged; the VMs' settings checked once more as protocol
//! types; a console's socket relayed to channels.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, InterfaceName, PackageRunState, PackageTask, PoolName, Report, Request,
    RequestId, Response, ShareLocation, ShareName, SharePath, VmCpus, VmDiskSize, VmFirmware, VmId,
    VmIsoSource, VmMemory, VmNetwork, VmOs, VmSpec, VolumeNumber,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_storage::application::ports::ZfsSource;
use zendiloos_vms::application::{
    ConsoleSession, HostError, HostFacts, IsoSource, Steps, StoragePlace, VmHost, VmIds,
};
use zendiloos_vms::domain::{Firmware, HostLimits, Network, OsPreset, PowerState, Vm, VmAction};

use super::{AgentStorage, WHAT, agent_error, call, call_raw, call_raw_reporting, unexpected};

const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);
/// What is read from a console at once.
const CONSOLE_CHUNK: usize = 64 * 1024;

pub struct AgentVms {
    transport: Arc<dyn AgentTransport>,
    storage: AgentStorage,
}

impl AgentVms {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { storage: AgentStorage::new(transport.clone()), transport }
    }

    fn transport(&self) -> &dyn AgentTransport {
        self.transport.as_ref()
    }

    /// `Done`, or the agent's refusal or failure in its words.
    async fn done(&self, request: Request, context: &RequestContext) -> Result<(), HostError> {
        host_answer(call_raw(self.transport(), request, context).await?)
    }
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

/// The VM in the agent's terms: every value checked once more.
fn spec(vm: &Vm) -> Result<VmSpec, HostError> {
    let settings = &vm.settings;
    let (pool, volume) = place(&vm.pool, vm.volume)?;
    Ok(VmSpec {
        id: vm_id(&vm.id)?,
        os: match settings.os {
            OsPreset::Linux => VmOs::Linux,
            OsPreset::Windows => VmOs::Windows,
            OsPreset::Other => VmOs::Other,
        },
        cpus: VmCpus::new(settings.cpus).map_err(|_| invalid("number of CPUs"))?,
        memory: VmMemory::new(settings.memory_mib).map_err(|_| invalid("memory size"))?,
        firmware: match settings.firmware {
            Firmware::Uefi => VmFirmware::Uefi,
            Firmware::Bios => VmFirmware::Bios,
        },
        disk: VmDiskSize::new(settings.disk_gib).map_err(|_| invalid("disk size"))?,
        network: match &settings.network {
            Network::User => VmNetwork::User,
            Network::Bridge(bridge) => {
                VmNetwork::Bridge { bridge: InterfaceName::parse(bridge).map_err(|_| invalid("bridge"))? }
            }
        },
        iso: vm.iso.is_some(),
        autostart: settings.autostart,
        pool,
        volume,
    })
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
                bridges,
                storage,
            } => Ok(HostFacts {
                kvm,
                cpu_virtualization: cpu_virtualization.filter(|flag| flag == "vmx" || flag == "svm"),
                iommu,
                installed,
                limits: HostLimits {
                    cores,
                    memory_bytes,
                    bridges: bridges.into_iter().map(String::from).collect(),
                },
                storage: storage.map(|storage| StoragePlace {
                    pool: storage.pool.as_str().to_owned(),
                    volume: storage.volume.get(),
                }),
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn pools(&self, context: &RequestContext) -> Result<Vec<StoragePlace>, PortError> {
        Ok(self
            .storage
            .pools(context)
            .await?
            .into_iter()
            .filter_map(|pool| Some(StoragePlace { volume: pool.volume?.get(), pool: pool.name }))
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

    async fn set_up(&self, storage: &StoragePlace, context: &RequestContext) -> Result<(), HostError> {
        let (pool, volume) = place(&storage.pool, storage.volume)?;
        self.done(Request::EnableVms { pool, volume }, context).await
    }

    async fn copy_iso(
        &self,
        id: &str,
        storage: &StoragePlace,
        iso: &IsoSource,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::CopyVmIso {
            id: vm_id(id)?,
            volume: VolumeNumber::new(storage.volume).map_err(|_| invalid("volume number"))?,
            source: VmIsoSource {
                share: ShareLocation {
                    volume: VolumeNumber::new(iso.volume).map_err(|_| invalid("volume number"))?,
                    share: ShareName::parse(&iso.share).map_err(|_| invalid("share name"))?,
                },
                path: SharePath::parse(&iso.path).map_err(|_| invalid("path in a share"))?,
            },
        };
        host_answer(call_raw_reporting(self.transport(), request, &Forward(steps), context).await?)
    }

    async fn define(&self, vm: &Vm, context: &RequestContext) -> Result<(), HostError> {
        self.done(Request::DefineVm { spec: spec(vm)? }, context).await
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
        let (pool, volume) = place(&vm.pool, vm.volume)?;
        self.done(Request::DeleteVm { id: vm_id(&vm.id)?, pool, volume }, context).await
    }

    async fn states(&self, context: &RequestContext) -> Result<BTreeMap<String, PowerState>, PortError> {
        match call(self.transport(), Request::VmStates {}, context).await? {
            Response::VmStates { virsh_list } => Ok(states(&virsh_list)),
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
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;
    use zendiloos_sim::SimulatedAgent;
    use zendiloos_vms::domain::{IsoOrigin, VmSettings};

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
        Vm {
            id: id.into(),
            settings: VmSettings {
                cpus: 1,
                memory_mib: 256,
                disk_gib: 1,
                ..VmSettings::preset("box", OsPreset::Linux)
            },
            iso: Some(IsoOrigin { share: "media".into(), path: "isos/x.iso".into() }),
            pool: "tank".into(),
            volume: 1,
            created_at: Timestamp::from_unix_seconds(0),
            ready: false,
        }
    }

    #[test]
    fn a_vm_reaches_the_agent_only_as_checked_values() {
        let spec = spec(&vm("0a1b2c3d4e5f")).unwrap();
        assert!(spec.iso && spec.network == VmNetwork::User && spec.memory.mib() == 256);
        assert!(super::spec(&vm("../../etc")).is_err());
        let mut bridge = vm("0a1b2c3d4e5f");
        bridge.settings.network = Network::Bridge("br0 extra".into());
        assert!(super::spec(&bridge).is_err());
        let mut small = vm("0a1b2c3d4e5f");
        small.settings.memory_mib = 64;
        assert!(super::spec(&small).is_err());
    }

    #[tokio::test]
    async fn vms_are_made_run_and_removed_through_the_agent() {
        let agent = Arc::new(SimulatedAgent::new());
        let host = AgentVms::new(agent.clone());
        let context = RequestContext::default();
        let facts = host.facts(&context).await.unwrap();
        assert!(facts.kvm && !facts.installed && facts.storage.is_none());
        assert_eq!(facts.limits.bridges, ["br0"]);
        let pools = host.pools(&context).await.unwrap();
        assert!(!pools.is_empty());

        host.install(&context).await.unwrap();
        host.set_up(&pools[0], &context).await.unwrap();
        assert_eq!(host.facts(&context).await.unwrap().storage, Some(pools[0].clone()));

        let mut made = vm("0a1b2c3d4e5f");
        made.pool = pools[0].pool.clone();
        made.volume = pools[0].volume;
        let iso = IsoSource { share: "media".into(), volume: pools[0].volume, path: "isos/x.iso".into() };
        struct Silent;
        impl Steps for Silent {
            fn step(&self, _: &str, _: Option<u8>) {}
        }
        host.copy_iso(&made.id, &pools[0], &iso, &Silent, &context).await.unwrap();
        host.define(&made, &context).await.unwrap();
        assert!(matches!(host.define(&made, &context).await, Err(HostError::Refused(_))));
        assert_eq!(host.states(&context).await.unwrap()["0a1b2c3d4e5f"], PowerState::ShutOff);
        assert!(matches!(host.open_console(&made.id, &context).await, Err(HostError::Refused(_))));

        host.control(&made.id, VmAction::Start, &context).await.unwrap();
        let mut console = host.open_console(&made.id, &context).await.unwrap();
        assert_eq!(console.output.recv().await.unwrap(), b"RFB 003.008\n");
        drop(console);
        host.control(&made.id, VmAction::Shutdown, &context).await.unwrap();
        host.remove(&made, &context).await.unwrap();
        assert!(host.states(&context).await.unwrap().is_empty());
    }
}
