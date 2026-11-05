use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;
use crate::domain::SetupCode;

/// Command, run at startup: while no administrator exists, makes sure there
/// is a setup code and returns it so it can be shown to the owner. Once one
/// exists, removes any leftover code.
#[derive(Clone)]
pub struct PrepareSetup {
    context: Arc<Context>,
}

impl PrepareSetup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<Option<SetupCode>, PortError> {
        let ports = &self.context.ports;
        if ports.accounts.any_admin().await? {
            ports.setup_codes.consume().await?;
            Ok(None)
        } else {
            Ok(Some(ports.setup_codes.issue().await?))
        }
    }
}
