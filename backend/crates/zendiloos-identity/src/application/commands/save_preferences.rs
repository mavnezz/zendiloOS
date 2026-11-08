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

    /// Makes the choices `change` makes; one it leaves out (`None`) stays
    /// as it is, and the reminder to star zendiloOS never goes back to an
    /// earlier state ([`Preferences::changed_by`]).
    pub async fn execute(&self, auth: &AuthContext, change: Preferences) -> Result<(), IdentityError> {
        Context::require(auth, Permission::AccountSelf)?;
        let repository = &self.context.ports.preferences;
        // Read, merged and saved as one: two tabs saving at once cannot
        // take the reminder back by saving over each other.
        let _saving = self.context.preferences.lock().await;
        let kept = repository.load(auth.account.id).await?;
        Ok(repository.save(auth.account.id, kept.changed_by(change)).await?)
    }
}
