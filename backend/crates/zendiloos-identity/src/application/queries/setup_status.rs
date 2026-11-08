use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupStatus {
    /// No administrator exists yet; the interface must show the setup.
    pub required: bool,
    /// The setup asks for the code shown on the console; not when it is
    /// open (a machine installed from the ISO).
    pub code_required: bool,
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
        let required = !self.context.ports.accounts.any_admin().await?;
        let open = required && self.context.ports.setup_codes.is_open().await?;
        Ok(SetupStatus { required, code_required: required && !open })
    }
}
