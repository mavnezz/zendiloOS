//! A fixed network, for tests here and in the API crate (feature `testing`).

use async_trait::async_trait;
use zendiloos_kernel::PortError;

use crate::application::NetworkSource;
use crate::domain::{Address, AddressScope, DefaultRoute, Interface, InterfaceKind, LinkState, Network};

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

/// A host that takes changes and keeps one waiting, without a clock.
#[derive(Default)]
pub struct FakeConfigurator {
    pub pending: std::sync::Mutex<Option<crate::application::PendingChange>>,
    pub applied: std::sync::Mutex<Vec<crate::domain::InterfaceSettings>>,
}

#[async_trait]
impl crate::application::NetworkConfigurator for FakeConfigurator {
    async fn setup(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<crate::application::NetworkSetup, PortError> {
        Ok(crate::application::NetworkSetup { managed: true, pending: self.pending.lock().unwrap().clone() })
    }

    async fn apply(
        &self,
        settings: &crate::domain::InterfaceSettings,
        confirm_within_seconds: u16,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ConfigureError> {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_some() {
            return Err(crate::application::ConfigureError::Pending);
        }
        *pending = Some(crate::application::PendingChange {
            interface: settings.interface.clone(),
            revert_at: zendiloos_kernel::Timestamp::from_unix_seconds(i64::from(confirm_within_seconds)),
        });
        self.applied.lock().unwrap().push(settings.clone());
        Ok(())
    }

    async fn confirm(
        &self,
        _: &zendiloos_kernel::RequestContext,
    ) -> Result<(), crate::application::ConfigureError> {
        match self.pending.lock().unwrap().take() {
            Some(_) => Ok(()),
            None => {
                Err(crate::application::ConfigureError::Refused("no change waits for confirmation".into()))
            }
        }
    }
}
