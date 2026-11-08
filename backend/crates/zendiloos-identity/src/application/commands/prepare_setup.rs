use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;
use crate::domain::SetupCode;

/// How the initial setup stands, for the console.
#[derive(Debug)]
pub enum PendingSetup {
    /// An administrator exists: nothing to set up.
    Done,
    /// The setup asks for this code, shown on the console.
    Code(SetupCode),
    /// Open: no code; whoever comes first from the NAS's own network
    /// becomes the first administrator.
    Open,
}

/// Command, run at startup: while no administrator exists, makes sure there
/// is a setup code and returns it so it can be shown to the owner, unless
/// the setup is open. Once one exists, removes any leftover code and ends
/// an open setup.
#[derive(Clone)]
pub struct PrepareSetup {
    context: Arc<Context>,
}

impl PrepareSetup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self) -> Result<PendingSetup, PortError> {
        let codes = &self.context.ports.setup_codes;
        if self.context.ports.accounts.any_admin().await? {
            codes.consume().await?;
            codes.close().await?;
            Ok(PendingSetup::Done)
        } else if codes.is_open().await? {
            // A code from before the machine was opened would only confuse.
            codes.consume().await?;
            Ok(PendingSetup::Open)
        } else {
            Ok(PendingSetup::Code(codes.issue().await?))
        }
    }
}
