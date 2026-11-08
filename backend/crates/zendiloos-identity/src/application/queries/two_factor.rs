use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{AccountId, Permission};

/// Where the signed-in account's second factor stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwoFactorStatus {
    /// Sign-ins ask for a code.
    pub on: bool,
    /// Recovery codes not used yet.
    pub recovery_codes_left: usize,
}

/// Query: the signed-in account's second factor.
#[derive(Clone)]
pub struct GetTwoFactor {
    context: Arc<Context>,
}

impl GetTwoFactor {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<TwoFactorStatus, IdentityError> {
        let stored = self.context.ports.two_factor.find(auth.account.id).await?;
        Ok(match stored.filter(|t| t.is_on()) {
            Some(two_factor) => TwoFactorStatus { on: true, recovery_codes_left: two_factor.recovery.len() },
            None => TwoFactorStatus { on: false, recovery_codes_left: 0 },
        })
    }
}

/// Query: the accounts that sign in with a second factor. Needs
/// `users.manage`.
#[derive(Clone)]
pub struct ListTwoFactorAccounts {
    context: Arc<Context>,
}

impl ListTwoFactorAccounts {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Vec<AccountId>, IdentityError> {
        Context::require(auth, Permission::UsersManage)?;
        Ok(self.context.ports.two_factor.enabled().await?)
    }
}
