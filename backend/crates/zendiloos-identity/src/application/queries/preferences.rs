use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{Permission, Preferences};

/// Query: the caller's interface choices.
#[derive(Clone)]
pub struct GetPreferences {
    context: Arc<Context>,
}

impl GetPreferences {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Preferences, IdentityError> {
        Context::require(auth, Permission::AccountSelf)?;
        Ok(self.context.ports.preferences.load(auth.account.id).await?)
    }
}
