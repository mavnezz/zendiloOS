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
