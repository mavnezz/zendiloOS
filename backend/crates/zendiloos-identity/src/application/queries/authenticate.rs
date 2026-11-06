use std::sync::Arc;

use zendiloos_kernel::Secret;

use crate::application::context::{Context, summary};
use crate::application::{AuthContext, IdentityError};

/// Query: resolves a session token to the signed-in account.
///
/// Not entirely free of side effects: it keeps an active session alive and
/// removes one that has ended.
#[derive(Clone)]
pub struct Authenticate {
    context: Arc<Context>,
}

impl Authenticate {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    /// For a request: the caller counts as active, which keeps the session
    /// alive.
    pub async fn execute(&self, token: &Secret) -> Result<AuthContext, IdentityError> {
        self.resolve(token, true).await
    }

    /// For long-lived connections that only check the session is still
    /// valid: does not count as activity, so an open page alone does not
    /// keep a session alive.
    pub async fn verify(&self, token: &Secret) -> Result<AuthContext, IdentityError> {
        self.resolve(token, false).await
    }

    async fn resolve(&self, token: &Secret, touch: bool) -> Result<AuthContext, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let policy = &c.session_policy().await?;
        let sessions = &c.ports.sessions;

        let hash = c.ports.tokens.digest(token);
        let session = sessions.find_by_token_hash(&hash).await?.ok_or(IdentityError::Unauthenticated)?;
        if !session.is_active(now, policy) {
            sessions.delete(session.id).await?;
            return Err(IdentityError::Unauthenticated);
        }
        let account = match c.ports.accounts.find_by_id(session.account_id).await? {
            Some(account) if !account.disabled => account,
            _ => {
                sessions.delete(session.id).await?;
                return Err(IdentityError::Unauthenticated);
            }
        };
        if touch && session.needs_touch(now, policy) {
            sessions.touch(session.id, now).await?;
        }

        Ok(AuthContext {
            session_id: session.id,
            csrf_token: session.csrf_token,
            expires_at: session.expires_at,
            account: summary(&account),
        })
    }
}
