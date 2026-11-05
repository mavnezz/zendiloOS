use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::AuditAction;

/// Command: ends the caller's session.
#[derive(Clone)]
pub struct Logout {
    context: Arc<Context>,
}

impl Logout {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext, request: &RequestContext) -> Result<(), IdentityError> {
        let c = &self.context;
        c.ports.sessions.delete(auth.session_id).await?;
        let entry = AuditEntry::new(c.now(), AuditAction::Logout.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor())).await?;
        Ok(())
    }
}
