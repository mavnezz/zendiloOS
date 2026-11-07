//! What this context needs from the outside world. Nearly all of it comes
//! from the privileged agent (or its simulation), so those calls carry the
//! request context for correlation; the self-test plans are in the
//! database.

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::domain::{
    Array, BlockDevice, Occupant, Pool, PoolLayout, PoolName, ScheduledSelfTest, SelfTestKind, SelfTestPlan,
    SmartReport, VolumeNumber,
};

/// The host's block device tree.
#[async_trait]
pub trait BlockDeviceSource: Send + Sync {
    async fn block_devices(&self, context: &RequestContext) -> Result<Vec<BlockDevice>, PortError>;
}

/// SMART data of one disk. Callers pass only names of existing disks.
#[async_trait]
pub trait SmartSource: Send + Sync {
    async fn smart_report(&self, disk: &str, context: &RequestContext) -> Result<SmartReport, PortError>;

    /// Has the drive start a self-test; it runs alongside its work.
    async fn start_self_test(
        &self,
        disk: &str,
        kind: SelfTestKind,
        context: &RequestContext,
    ) -> Result<(), PortError>;

    /// Stops the drive's running self-test.
    async fn abort_self_test(&self, disk: &str, context: &RequestContext) -> Result<(), PortError>;
}

/// The software RAID arrays the kernel knows.
#[async_trait]
pub trait ArraySource: Send + Sync {
    async fn arrays(&self, context: &RequestContext) -> Result<Vec<Array>, PortError>;
}

/// The ZFS pools the kernel has imported, with the space of their root
/// datasets. Disks are not resolved yet ([`crate::domain::resolve_pool_disks`]).
#[async_trait]
pub trait ZfsSource: Send + Sync {
    async fn pools(&self, context: &RequestContext) -> Result<Vec<Pool>, PortError>;
}

/// A pool to create: disks by `/dev/disk/by-id` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPool {
    pub name: PoolName,
    pub layout: PoolLayout,
    pub disks: Vec<String>,
    pub volume: VolumeNumber,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    /// A safety check on the host refused, e.g. a disk turned out to be in
    /// use.
    #[error("refused: {0}")]
    Refused(String),
    /// The pool name or volume number is taken on the host.
    #[error("the name or volume number is taken")]
    Conflict,
    #[error(transparent)]
    Port(#[from] PortError),
}

/// What other contexts keep on a volume: shares, apps, the personal
/// folders, Docker's data. Storage knows none of them itself.
#[async_trait]
pub trait VolumeOccupants: Send + Sync {
    async fn on_volume(
        &self,
        volume: VolumeNumber,
        context: &RequestContext,
    ) -> Result<Vec<Occupant>, PortError>;
}

/// Changes to pools; needs the privileged agent, which checks every disk
/// again before erasing it.
#[async_trait]
pub trait PoolAdmin: Send + Sync {
    async fn create_pool(&self, pool: &NewPool, context: &RequestContext) -> Result<(), AdminError>;

    async fn destroy_pool(
        &self,
        pool: &PoolName,
        volume: VolumeNumber,
        context: &RequestContext,
    ) -> Result<(), AdminError>;

    /// Replaces the device with GUID `device` by the disk with by-id link
    /// `disk`.
    async fn replace_disk(
        &self,
        pool: &PoolName,
        device: &str,
        disk: &str,
        context: &RequestContext,
    ) -> Result<(), AdminError>;

    async fn scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError>;

    /// Stops the pool's scrub; what it checked so far stays checked.
    async fn stop_scrub(&self, pool: &PoolName, context: &RequestContext) -> Result<(), AdminError>;
}

/// Unguessable tokens for confirming plans.
pub trait PlanTokens: Send + Sync {
    fn token(&self) -> String;
}

/// The self-test plans and when each last started its tests.
#[async_trait]
pub trait SelfTestPlanStore: Send + Sync {
    /// Ordered by kind.
    async fn plans(&self) -> Result<Vec<ScheduledSelfTest>, PortError>;

    /// Replaces the plans; a kind that stays keeps when it last ran, so
    /// changing its hour does not run it again tonight.
    async fn save(&self, plans: &[SelfTestPlan]) -> Result<(), PortError>;

    async fn ran(&self, kind: SelfTestKind, at: Timestamp) -> Result<(), PortError>;
}
