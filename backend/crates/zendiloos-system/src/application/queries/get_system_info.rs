use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::ports::SystemInfoProvider;
use crate::domain::SystemInfo;

/// Query: facts about the host. A pure read, so it goes straight to the port.
#[derive(Clone)]
pub struct GetSystemInfo {
    provider: Arc<dyn SystemInfoProvider>,
}

impl GetSystemInfo {
    pub fn new(provider: Arc<dyn SystemInfoProvider>) -> Self {
        Self { provider }
    }

    pub async fn execute(&self) -> Result<SystemInfo, PortError> {
        self.provider.system_info().await
    }
}
