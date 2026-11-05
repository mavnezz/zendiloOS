use zendiloos_kernel::PortError;

/// Runs blocking work (hashing, small file I/O) off the async workers.
pub(crate) async fn run<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, PortError> + Send + 'static,
) -> Result<T, PortError> {
    tokio::task::spawn_blocking(work).await.map_err(|error| PortError::new(what, error))?
}
