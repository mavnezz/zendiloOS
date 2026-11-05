//! What this context needs from the outside world. All of it comes from
//! the privileged agent (or its simulation), so every call carries the
//! request context for correlation.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext};

use crate::domain::{Array, BlockDevice, SmartReport};

/// The host's block device tree.
#[async_trait]
pub trait BlockDeviceSource: Send + Sync {
    async fn block_devices(&self, context: &RequestContext) -> Result<Vec<BlockDevice>, PortError>;
}

/// SMART data of one disk. Callers pass only names of existing disks.
#[async_trait]
pub trait SmartSource: Send + Sync {
    async fn smart_report(&self, disk: &str, context: &RequestContext) -> Result<SmartReport, PortError>;
}

/// The software RAID arrays the kernel knows.
#[async_trait]
pub trait ArraySource: Send + Sync {
    async fn arrays(&self, context: &RequestContext) -> Result<Vec<Array>, PortError>;
}
