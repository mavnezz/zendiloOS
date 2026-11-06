use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::context::Context;

/// Command, run periodically: removes sessions that have ended, and blocks
/// that are over. Both are already ignored; this keeps the tables small.
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
        let now = c.now();
        if c.ports.security.delete_expired(now).await? > 0 {
            c.reload_defences().await?;
        }
        let idle_timeout = c.session_policy().await?.idle_timeout;
        c.ports.sessions.delete_inactive(now, idle_timeout).await
    }
}
