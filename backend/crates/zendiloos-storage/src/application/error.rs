use zendiloos_kernel::PortError;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("there is no disk with this name")]
    DiskNotFound,
    #[error(transparent)]
    Port(#[from] PortError),
}
