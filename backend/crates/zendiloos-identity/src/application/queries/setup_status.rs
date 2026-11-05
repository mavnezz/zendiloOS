use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupStatus {
    /// No administrator exists yet; the interface must show the setup.
    pub required: bool,
}

/// Query: whether the initial setup is still pending.
#[derive(Clone)]
pub struct GetSetupStatus {
    context: Arc<Context>,
}

impl GetSetupStatus {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<SetupStatus, PortError> {
        Ok(SetupStatus { required: !self.context.ports.accounts.any_admin().await? })
    }
}
