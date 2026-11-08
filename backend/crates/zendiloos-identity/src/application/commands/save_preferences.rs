use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{Permission, Preferences, StarPrompt};

/// Command: keeps the caller's interface choices with their account.
#[derive(Clone)]
pub struct SavePreferences {
    context: Arc<Context>,
}

impl SavePreferences {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// Replaces the choices, except that the reminder to star zendiloOS
    /// never goes back to an earlier state ([`StarPrompt::further`]).
    pub async fn execute(
        &self,
        auth: &AuthContext,
        mut preferences: Preferences,
    ) -> Result<(), IdentityError> {
        Context::require(auth, Permission::AccountSelf)?;
        let repository = &self.context.ports.preferences;
        // Read, merged and saved as one: two tabs saving at once cannot
        // take the reminder back by saving over each other.
        let _saving = self.context.preferences.lock().await;
        let kept = repository.load(auth.account.id).await?;
        preferences.star_prompt = StarPrompt::further(kept.star_prompt, preferences.star_prompt);
        Ok(repository.save(auth.account.id, preferences).await?)
    }
}
