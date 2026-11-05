//! Use cases of the Identity context and the ports they need.

pub mod commands;
mod context;
mod error;
pub mod ports;
pub mod queries;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditLog, Clock, Secret, Timestamp};

use crate::domain::{AccountId, AccountName, Permission, Role, SessionId, SessionPolicy, ThrottlePolicy};
use context::Context;
pub use error::IdentityError;
use ports::{AccountRepository, PasswordHasher, SessionRepository, SessionTokens, SetupCodes};

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct IdentityPorts {
    pub accounts: Arc<dyn AccountRepository>,
    pub sessions: Arc<dyn SessionRepository>,
    pub hasher: Arc<dyn PasswordHasher>,
    pub tokens: Arc<dyn SessionTokens>,
    pub setup_codes: Arc<dyn SetupCodes>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityConfig {
    pub sessions: SessionPolicy,
    pub throttle: ThrottlePolicy,
}

/// All use cases of this context, sharing one set of ports and one login
/// throttle.
#[derive(Clone)]
pub struct IdentityUseCases {
    pub prepare_setup: commands::PrepareSetup,
    pub complete_setup: commands::CompleteSetup,
    pub login: commands::Login,
    pub logout: commands::Logout,
    pub change_password: commands::ChangePassword,
    pub purge_expired_sessions: commands::PurgeExpiredSessions,
    pub setup_status: queries::GetSetupStatus,
    pub authenticate: queries::Authenticate,
}

impl IdentityUseCases {
    pub fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        let context = Arc::new(Context::new(ports, config));
        Self {
            prepare_setup: commands::PrepareSetup::new(Arc::clone(&context)),
            complete_setup: commands::CompleteSetup::new(Arc::clone(&context)),
            login: commands::Login::new(Arc::clone(&context)),
            logout: commands::Logout::new(Arc::clone(&context)),
            change_password: commands::ChangePassword::new(Arc::clone(&context)),
            purge_expired_sessions: commands::PurgeExpiredSessions::new(Arc::clone(&context)),
            setup_status: queries::GetSetupStatus::new(Arc::clone(&context)),
            authenticate: queries::Authenticate::new(context),
        }
    }
}

/// The parts of an account a signed-in client may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSummary {
    pub id: AccountId,
    pub name: AccountName,
    pub full_name: Option<String>,
    pub role: Role,
    pub password_change_required: bool,
}

/// A session that was just started. The token goes into the cookie and is
/// never shown again.
#[derive(Debug)]
pub struct SignedIn {
    pub token: Secret,
    pub session: AuthContext,
}

/// The authenticated caller of a request.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub session_id: SessionId,
    pub csrf_token: Secret,
    pub expires_at: Timestamp,
    pub account: AccountSummary,
}

impl AuthContext {
    pub fn can(&self, permission: Permission) -> bool {
        self.account.role.grants(permission)
    }

    /// The signed-in account as the actor of an audit entry.
    pub fn actor(&self) -> Actor {
        Actor { account_id: self.account.id.get(), name: self.account.name.to_string() }
    }
}
