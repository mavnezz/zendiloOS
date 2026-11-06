use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{AuditAction, PasswordPolicy, Permission};

/// Command: an administrator sets the rules for new passwords. Passwords
/// already set stay; a shorter maximum age applies at the next login.
#[derive(Clone)]
pub struct SetPasswordPolicy {
    context: Arc<Context>,
}

impl SetPasswordPolicy {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        policy: PasswordPolicy,
        request: &RequestContext,
    ) -> Result<PasswordPolicy, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        let policy = policy.checked()?;
        c.ports.accounts.set_password_policy(&policy).await?;
        let entry =
            AuditEntry::new(c.now(), AuditAction::PasswordPolicyUpdate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor())).await?;
        Ok(policy)
    }
}
