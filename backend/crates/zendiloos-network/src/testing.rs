//! A fixed network and a fake host, for tests here and in the API crate
//! (feature `testing`).

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::{
    Change, ConfigureError, HostNetwork, IfupdownHost, NetworkConfigurator, NetworkSource, PendingChange,
};
use crate::domain::{
    Address, AddressScope, DefaultRoute, IfupdownFile, Interface, InterfaceKind, InterfaceSettings,
    LinkState, Network,
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

/// The digest [`FakeConfigurator::with_ifupdown`] gives its files.
pub const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// A host that takes changes and keeps one waiting, without a clock:
/// managed by systemd-networkd, or by ifupdown until taken over.
#[derive(Default)]
pub struct FakeConfigurator {
    pub pending: Mutex<Option<PendingChange>>,
    pub applied: Mutex<Vec<InterfaceSettings>>,
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
        })
    }

    async fn apply(
        &self,
        settings: &InterfaceSettings,
        confirm_within_seconds: u16,
        _: &RequestContext,
    ) -> Result<(), ConfigureError> {
        if locked(&self.ifupdown).is_some() {
            return Err(ConfigureError::Refused("systemd-networkd does not manage the network here".into()));
        }
        self.wait(Change::Interface(settings.interface.clone()), confirm_within_seconds)?;
        locked(&self.applied).push(settings.clone());
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
