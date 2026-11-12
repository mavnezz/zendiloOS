//! A fixed network and a fake host, for tests here and in the API crate
//! (feature `testing`).

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::{
    Change, ConfigureError, DeviceUsers, HostNetwork, IfupdownHost, KeptSettings, NetworkConfigurator,
    NetworkSource, PendingChange, RestoredSettings, SettingsStore,
};
use crate::domain::{
    Address, AddressScope, DefaultRoute, DeviceKind, Edit, IfupdownFile, Interface, InterfaceKind,
    InterfaceSettings, Ipv4Setting, Ipv6Setting, LinkState, Network, NetworkdFile,
};

/// One card with a DHCP address, its router and one name server.
pub struct FixedNetwork;

#[async_trait]
impl NetworkSource for FixedNetwork {
    async fn network(&self) -> Result<Network, PortError> {
        let card = Interface {
            name: "enp1s0".into(),
            kind: InterfaceKind::Ethernet,
            state: LinkState::Up,
            mac: Some("52:54:00:12:34:56".into()),
            mtu: 1500,
            member_of: None,
            parent: None,
            addresses: vec![Address {
                ip: [192, 168, 1, 20].into(),
                prefix: 24,
                scope: AddressScope::Global,
                dynamic: true,
            }],
        };
        let route =
            DefaultRoute { gateway: [192, 168, 1, 1].into(), interface: "enp1s0".into(), metric: 100 };
        Ok(Network::new(vec![card], vec![route], vec![[192, 168, 1, 1].into()]))
    }
}

/// The network a test sets, [`FixedNetwork`]'s until then.
#[derive(Default)]
pub struct SettableNetwork(pub Mutex<Option<Network>>);

impl SettableNetwork {
    pub fn set(&self, network: Network) {
        *locked(&self.0) = Some(network);
    }
}

#[async_trait]
impl NetworkSource for SettableNetwork {
    async fn network(&self) -> Result<Network, PortError> {
        let set = locked(&self.0).clone();
        match set {
            Some(network) => Ok(network),
            None => FixedNetwork.network().await,
        }
    }
}

/// [`FixedNetwork`]'s card with two more without addresses, `enp2s0` and
/// `enp3s0`, for bridges and bonds.
pub fn three_cards() -> Network {
    let card = |name: &str, mac: &str, addresses: Vec<Address>| Interface {
        name: name.into(),
        kind: InterfaceKind::Ethernet,
        state: LinkState::Up,
        mac: Some(mac.into()),
        mtu: 1500,
        member_of: None,
        parent: None,
        addresses,
    };
    let lan = card(
        "enp1s0",
        "52:54:00:12:34:56",
        vec![Address {
            ip: [192, 168, 1, 20].into(),
            prefix: 24,
            scope: AddressScope::Global,
            dynamic: true,
        }],
    );
    let route = DefaultRoute { gateway: [192, 168, 1, 1].into(), interface: "enp1s0".into(), metric: 100 };
    Network::new(
        vec![lan, card("enp2s0", "52:54:00:00:00:02", vec![]), card("enp3s0", "52:54:00:00:00:03", vec![])],
        vec![route],
        vec![[192, 168, 1, 1].into()],
    )
}

/// What uses which device: VMs on bridges, by the bridge's name.
#[derive(Default)]
pub struct FakeUsers(pub Mutex<Vec<(String, String)>>);

#[async_trait]
impl DeviceUsers for FakeUsers {
    async fn users(&self, device: &str) -> Result<Vec<String>, PortError> {
        Ok(locked(&self.0).iter().filter(|(on, _)| on == device).map(|(_, user)| user.clone()).collect())
    }
}

/// zendiloOS's network settings as a database would keep them.
#[derive(Default)]
pub struct MemorySettings(pub Mutex<Option<KeptSettings>>, pub Mutex<Option<RestoredSettings>>);

#[async_trait]
impl SettingsStore for MemorySettings {
    async fn kept(&self) -> Result<Option<KeptSettings>, PortError> {
        Ok(locked(&self.0).clone())
    }

    async fn keep(&self, files: &[NetworkdFile], at: Timestamp) -> Result<(), PortError> {
        *locked(&self.0) = Some(KeptSettings { files: files.to_vec(), kept_at: at, said: None });
        Ok(())
    }

    async fn note_said(&self, said: Option<&str>) -> Result<(), PortError> {
        if let Some(kept) = locked(&self.0).as_mut() {
            kept.said = said.map(str::to_owned);
        }
        Ok(())
    }

    async fn restored(&self) -> Result<Option<RestoredSettings>, PortError> {
        Ok(locked(&self.1).clone())
    }

    async fn forget_restored(&self) -> Result<(), PortError> {
        *locked(&self.1) = None;
        Ok(())
    }
}

/// The files the agent would write for `edit`, in the keys zendiloosd
/// reads back.
fn files_after(files: &mut Vec<NetworkdFile>, edit: &Edit) {
    let netdev = |name: &str| format!("05-zendiloos-{name}.netdev");
    let network = |name: &str| format!("05-zendiloos-{name}.network");
    for gone in &edit.removed {
        files.retain(|file| file.name != netdev(gone) && file.name != network(gone));
    }
    for gone in &edit.unset {
        files.retain(|file| file.name != network(gone));
    }
    for device in &edit.devices {
        let mut text = format!("# Members: {}\n[NetDev]\nName={}\n", device.members.join(" "), device.name);
        match &device.kind {
            DeviceKind::Bridge { stp } => text.push_str(&format!("Kind=bridge\n[Bridge]\nSTP={stp}\n")),
            DeviceKind::Bond { mode } => {
                text.push_str(&format!("Kind=bond\n[Bond]\nMode={}\n", mode.as_str()))
            }
            DeviceKind::Vlan { id, .. } => text.push_str(&format!("Kind=vlan\n[VLAN]\nId={id}\n")),
        }
        if let Some(mac) = &device.mac {
            text.push_str(&format!("[NetDev]\nMACAddress={mac}\n"));
        }
        files.retain(|file| file.name != netdev(&device.name));
        files.push(NetworkdFile { name: netdev(&device.name), text });
    }
    for link in &edit.links {
        let settings = &link.settings;
        let mut text = format!("[Match]\nName={}\n[Network]\n", settings.interface);
        if let Some(master) = &link.port_of {
            text.push_str(&format!("Bridge={master}\n"));
        }
        match settings.ipv4 {
            Ipv4Setting::Dhcp { .. } => text.push_str("DHCP=ipv4\n"),
            Ipv4Setting::Static { address, prefix, gateway } => {
                text.push_str(&format!("Address={address}/{prefix}\n"));
                if let Some(gateway) = gateway {
                    text.push_str(&format!("Gateway={gateway}\n"));
                }
            }
            Ipv4Setting::Off => {}
        }
        if settings.ipv6 == Ipv6Setting::Automatic {
            text.push_str("IPv6AcceptRA=yes\n");
        }
        for server in &settings.dns {
            text.push_str(&format!("DNS={server}\n"));
        }
        for vlan in &link.vlans {
            text.push_str(&format!("VLAN={vlan}\n"));
        }
        files.retain(|file| file.name != network(&settings.interface));
        files.push(NetworkdFile { name: network(&settings.interface), text });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
}

/// The digest [`FakeConfigurator::with_ifupdown`] gives its files.
pub const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// A host that takes changes and keeps one waiting, without a clock:
/// managed by systemd-networkd, or by ifupdown until taken over.
#[derive(Default)]
pub struct FakeConfigurator {
    pub pending: Mutex<Option<PendingChange>>,
    /// The settings of each interface changed on its own.
    pub applied: Mutex<Vec<InterfaceSettings>>,
    /// Every change, as asked for.
    pub changes: Mutex<Vec<Edit>>,
    /// zendiloOS's files, as written; a change unconfirmed leaves them so.
    pub files: Mutex<Vec<NetworkdFile>>,
    /// While there, ifupdown manages the network.
    pub ifupdown: Mutex<Option<IfupdownHost>>,
    pub network_manager: Mutex<bool>,
    /// How often systemd-resolved was installed.
    pub installs: Mutex<u32>,
    pub taken_over: Mutex<Vec<Vec<InterfaceSettings>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeConfigurator {
    /// ifupdown with `interfaces` as its only file, running;
    /// systemd-resolved not installed.
    pub fn with_ifupdown(interfaces: &str) -> Self {
        let host = Self::default();
        host.use_ifupdown(interfaces);
        host
    }

    /// As [`FakeConfigurator::with_ifupdown`], from now on.
    pub fn use_ifupdown(&self, interfaces: &str) {
        *locked(&self.ifupdown) = Some(IfupdownHost {
            files: vec![IfupdownFile { path: "interfaces".into(), text: interfaces.into(), skipped: None }],
            digest: DIGEST.into(),
            active: true,
            resolved_installed: false,
        });
    }

    fn wait(&self, change: Change, seconds: u16) -> Result<(), ConfigureError> {
        let mut pending = locked(&self.pending);
        if pending.is_some() {
            return Err(ConfigureError::Pending);
        }
        *pending =
            Some(PendingChange { change, revert_at: Timestamp::from_unix_seconds(i64::from(seconds)) });
        Ok(())
    }
}

#[async_trait]
impl NetworkConfigurator for FakeConfigurator {
    async fn state(&self, _: &RequestContext) -> Result<HostNetwork, PortError> {
        let ifupdown = locked(&self.ifupdown).clone();
        let network_manager = *locked(&self.network_manager);
        Ok(HostNetwork {
            managed: ifupdown.is_none() && !network_manager,
            network_manager,
            pending: locked(&self.pending).clone(),
            ifupdown,
            files: locked(&self.files).clone(),
        })
    }

    async fn change(
        &self,
        edit: &Edit,
        confirm_within_seconds: u16,
        _: &RequestContext,
    ) -> Result<(), ConfigureError> {
        if locked(&self.ifupdown).is_some() {
            return Err(ConfigureError::Refused("systemd-networkd does not manage the network here".into()));
        }
        self.wait(Change::of(&edit.subject), confirm_within_seconds)?;
        if let crate::domain::Subject::Interface(_) = &edit.subject {
            locked(&self.applied).extend(edit.links.first().map(|link| link.settings.clone()));
        }
        files_after(&mut locked(&self.files), edit);
        locked(&self.changes).push(edit.clone());
        Ok(())
    }

    async fn install_resolved(&self, _: &RequestContext) -> Result<(), ConfigureError> {
        *locked(&self.installs) += 1;
        if let Some(ifupdown) = locked(&self.ifupdown).as_mut() {
            ifupdown.resolved_installed = true;
        }
        Ok(())
    }

    async fn take_over(
        &self,
        interfaces: &[InterfaceSettings],
        digest: &str,
        confirm_within_seconds: u16,
        _: &RequestContext,
    ) -> Result<(), ConfigureError> {
        let mut ifupdown = locked(&self.ifupdown);
        match ifupdown.as_ref() {
            None => return Err(ConfigureError::Refused("ifupdown does not manage the network here".into())),
            Some(host) if host.digest != digest => {
                return Err(ConfigureError::Refused("ifupdown's files changed since they were read".into()));
            }
            Some(host) if !host.resolved_installed => {
                return Err(ConfigureError::Refused("systemd-resolved is not installed".into()));
            }
            Some(_) => {}
        }
        let names = interfaces.iter().map(|settings| settings.interface.clone()).collect();
        self.wait(Change::Takeover(names), confirm_within_seconds)?;
        *ifupdown = None;
        locked(&self.taken_over).push(interfaces.to_vec());
        Ok(())
    }

    async fn confirm(&self, _: &RequestContext) -> Result<(), ConfigureError> {
        match locked(&self.pending).take() {
            Some(_) => Ok(()),
            None => Err(ConfigureError::Refused("no change waits for confirmation".into())),
        }
    }
}
