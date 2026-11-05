//! Use cases of the Network context and the port they need.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_kernel::PortError;

use crate::domain::Network;

/// How the machine is connected, read from the host.
#[async_trait]
pub trait NetworkSource: Send + Sync {
    async fn network(&self) -> Result<Network, PortError>;
}

/// Query: interfaces, addresses, default routes and name servers.
#[derive(Clone)]
pub struct GetNetwork {
    source: Arc<dyn NetworkSource>,
}

impl GetNetwork {
    pub fn new(source: Arc<dyn NetworkSource>) -> Self {
        Self { source }
    }

    pub async fn execute(&self) -> Result<Network, PortError> {
        self.source.network().await
    }
}

/// All use cases of this context.
#[derive(Clone)]
pub struct NetworkUseCases {
    pub get_network: GetNetwork,
}

impl NetworkUseCases {
    pub fn new(source: Arc<dyn NetworkSource>) -> Self {
        Self { get_network: GetNetwork::new(source) }
    }
}
