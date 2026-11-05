//! What this context needs from the outside world.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{PowerAction, SystemInfo};

/// Supplies facts about the host. Implemented in `zendiloos-infra`.
#[async_trait]
pub trait SystemInfoProvider: Send + Sync {
    async fn system_info(&self) -> Result<SystemInfo, PortError>;
}

/// Reboots or powers off the host; needs the privileged agent.
#[async_trait]
pub trait PowerControl: Send + Sync {
    /// Returns once the action is under way.
    async fn request(&self, action: PowerAction, context: &RequestContext) -> Result<(), PortError>;
}
