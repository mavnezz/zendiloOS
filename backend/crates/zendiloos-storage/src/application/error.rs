use zendiloos_kernel::PortError;

use crate::application::ports::AdminError;
use crate::domain::{InvalidEnclosure, InvalidInventory, InvalidSelfTestPlan, Occupant, PlanError};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("there is no disk with this name")]
    DiskNotFound,
    #[error("there is no pool with this name")]
    PoolNotFound,
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// Unknown, used, expired, or made by someone else.
    #[error("the plan is no longer valid")]
    PlanExpired,
    #[error("the confirmation does not match the pool's name")]
    ConfirmationMismatch,
    /// The disks changed since the plan was made.
    #[error("the disks changed since the plan was made")]
    PlanChanged,
    #[error("refused by the host: {0}")]
    Refused(String),
    #[error("the name or volume number is taken")]
    Conflict,
    /// Only a running scrub can be stopped; a resilver must finish.
    #[error("the pool is not being scrubbed")]
    NotScrubbing,
    #[error("the disk cannot test itself")]
    SelfTestUnsupported,
    #[error("the disk is testing itself already")]
    SelfTestRunning,
    #[error("the disk is not testing itself")]
    NotTesting,
    #[error(transparent)]
    InvalidSelfTestPlan(#[from] InvalidSelfTestPlan),
    #[error("there is no removable drive with this id")]
    RemovableNotFound,
    /// Files on the drive are open: it cannot be ejected now.
    #[error("the drive is in use")]
    DriveBusy,
    /// Shares, apps, the personal folders or Docker's data live on the pool.
    #[error("the pool holds data of shares, apps or Docker")]
    PoolInUse(Vec<Occupant>),
    #[error(transparent)]
    InvalidEnclosure(#[from] InvalidEnclosure),
    #[error(transparent)]
    InvalidInventory(#[from] InvalidInventory),
    #[error("a disk blinks for 1 to {} seconds", super::enclosure::LOCATE_MAX_SECONDS)]
    LocateTime,
    #[error("the disk is not blinking")]
    NotLocating,
    #[error("no disk with this serial number was seen")]
    InventoryNotFound,
    /// Only disks no longer present can be removed from the inventory.
    #[error("the disk is present")]
    DiskPresent,
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<AdminError> for StorageError {
    fn from(error: AdminError) -> Self {
        match error {
            AdminError::Refused(reason) => StorageError::Refused(reason),
            AdminError::Conflict => StorageError::Conflict,
            AdminError::Port(error) => StorageError::Port(error),
        }
    }
}
