use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;

/// Command, run periodically: removes sessions that have ended. Ended
/// sessions are already refused when used; this only keeps the table small.
#[derive(Clone)]
pub struct PurgeExpiredSessions {
    context: Arc<Context>,
}

impl PurgeExpiredSessions {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// Returns how many sessions were removed.
    pub async fn execute(&self) -> Result<u64, PortError> {
        let c = &self.context;
        c.ports.sessions.delete_inactive(c.now(), c.config.sessions.idle_timeout).await
    }
}
