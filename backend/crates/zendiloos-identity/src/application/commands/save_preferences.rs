use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{Permission, Preferences};

/// Command: keeps the caller's interface choices with their account.
#[derive(Clone)]
pub struct SavePreferences {
    context: Arc<Context>,
}

impl SavePreferences {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext, preferences: Preferences) -> Result<(), IdentityError> {
        Context::require(auth, Permission::AccountSelf)?;
        Ok(self.context.ports.preferences.save(auth.account.id, preferences).await?)
    }
}
