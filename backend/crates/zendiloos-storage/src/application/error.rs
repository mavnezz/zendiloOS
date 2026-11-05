use zendiloos_kernel::PortError;

use crate::application::ports::AdminError;
use crate::domain::PlanError;

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
