use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{PasswordPolicy, Permission};

/// Query: the rules a new password must follow. Every account may read
/// them: they apply when it changes its own.
#[derive(Clone)]
pub struct GetPasswordPolicy {
    context: Arc<Context>,
}

impl GetPasswordPolicy {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<PasswordPolicy, IdentityError> {
        Context::require(auth, Permission::AccountSelf)?;
        Ok(self.context.ports.accounts.password_policy().await?)
    }
}
