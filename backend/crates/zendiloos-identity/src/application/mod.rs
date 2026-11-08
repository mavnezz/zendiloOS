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
use ports::{
    AccountObserver, AccountRepository, GroupObserver, GroupRepository, OneTimeCodes, PasswordHasher,
    PreferenceRepository, SecurityRepository, SessionRepository, SessionTokens, SetupCodes, SystemAccounts,
    TwoFactorRepository,
};

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct IdentityPorts {
    pub accounts: Arc<dyn AccountRepository>,
    pub groups: Arc<dyn GroupRepository>,
    /// Others following groups' members, e.g. shares that deny a group.
    pub group_observer: Arc<dyn GroupObserver>,
    /// Others following accounts disabled, enabled again or deleted, e.g.
    /// the shares' files.
    pub account_observer: Arc<dyn AccountObserver>,
    pub sessions: Arc<dyn SessionRepository>,
    pub security: Arc<dyn SecurityRepository>,
    pub preferences: Arc<dyn PreferenceRepository>,
    pub two_factor: Arc<dyn TwoFactorRepository>,
    pub codes: Arc<dyn OneTimeCodes>,
    pub system: Arc<dyn SystemAccounts>,
    pub hasher: Arc<dyn PasswordHasher>,
    pub tokens: Arc<dyn SessionTokens>,
    pub setup_codes: Arc<dyn SetupCodes>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityConfig {
    /// The idle time is a setting; this one is the default's stand-in.
    pub sessions: SessionPolicy,
    /// The fixed limit per address; accounts lock as set.
    pub throttle: ThrottlePolicy,
}

/// All use cases of this context, sharing one set of ports and one login
/// throttle.
#[derive(Clone)]
pub struct IdentityUseCases {
    pub prepare_setup: commands::PrepareSetup,
    pub complete_setup: commands::CompleteSetup,
    pub login: commands::Login,
    pub complete_sign_in: commands::CompleteSignIn,
    pub logout: commands::Logout,
    pub change_password: commands::ChangePassword,
    pub purge_expired_sessions: commands::PurgeExpiredSessions,
    pub create_account: commands::CreateAccount,
    pub update_account: commands::UpdateAccount,
    pub reset_password: commands::ResetPassword,
    pub delete_account: commands::DeleteAccount,
    pub create_group: commands::CreateGroup,
    pub update_group: commands::UpdateGroup,
    pub delete_group: commands::DeleteGroup,
    pub save_preferences: commands::SavePreferences,
    pub set_password_policy: commands::SetPasswordPolicy,
    pub password_policy: queries::GetPasswordPolicy,
    pub setup_status: queries::GetSetupStatus,
    pub authenticate: queries::Authenticate,
    pub list_accounts: queries::ListAccounts,
    pub list_groups: queries::ListGroups,
    pub preferences: queries::GetPreferences,
    pub security: queries::GetSecurity,
    pub set_security: commands::SetSecurity,
    pub blocked_addresses: queries::ListBlockedAddresses,
    pub block_address: commands::BlockAddress,
    pub unblock_address: commands::UnblockAddress,
    pub check_address: queries::CheckAddress,
    pub sessions: queries::ListSessions,
    pub end_session: commands::EndSession,
    pub two_factor: queries::GetTwoFactor,
    pub two_factor_accounts: queries::ListTwoFactorAccounts,
    pub begin_two_factor: commands::BeginTwoFactor,
    pub confirm_two_factor: commands::ConfirmTwoFactor,
    pub disable_two_factor: commands::DisableTwoFactor,
    pub renew_recovery_codes: commands::RenewRecoveryCodes,
    pub reset_two_factor: commands::ResetTwoFactor,
}

impl IdentityUseCases {
    pub fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        let context = Arc::new(Context::new(ports, config));
        Self {
            prepare_setup: commands::PrepareSetup::new(Arc::clone(&context)),
            complete_setup: commands::CompleteSetup::new(Arc::clone(&context)),
            login: commands::Login::new(Arc::clone(&context)),
            complete_sign_in: commands::CompleteSignIn::new(Arc::clone(&context)),
            logout: commands::Logout::new(Arc::clone(&context)),
            change_password: commands::ChangePassword::new(Arc::clone(&context)),
            purge_expired_sessions: commands::PurgeExpiredSessions::new(Arc::clone(&context)),
            create_account: commands::CreateAccount::new(Arc::clone(&context)),
            update_account: commands::UpdateAccount::new(Arc::clone(&context)),
            reset_password: commands::ResetPassword::new(Arc::clone(&context)),
            delete_account: commands::DeleteAccount::new(Arc::clone(&context)),
            create_group: commands::CreateGroup::new(Arc::clone(&context)),
            update_group: commands::UpdateGroup::new(Arc::clone(&context)),
            delete_group: commands::DeleteGroup::new(Arc::clone(&context)),
            save_preferences: commands::SavePreferences::new(Arc::clone(&context)),
            set_password_policy: commands::SetPasswordPolicy::new(Arc::clone(&context)),
            password_policy: queries::GetPasswordPolicy::new(Arc::clone(&context)),
            setup_status: queries::GetSetupStatus::new(Arc::clone(&context)),
            list_accounts: queries::ListAccounts::new(Arc::clone(&context)),
            list_groups: queries::ListGroups::new(Arc::clone(&context)),
            preferences: queries::GetPreferences::new(Arc::clone(&context)),
            security: queries::GetSecurity::new(Arc::clone(&context)),
            set_security: commands::SetSecurity::new(Arc::clone(&context)),
            blocked_addresses: queries::ListBlockedAddresses::new(Arc::clone(&context)),
            block_address: commands::BlockAddress::new(Arc::clone(&context)),
            unblock_address: commands::UnblockAddress::new(Arc::clone(&context)),
            check_address: queries::CheckAddress::new(Arc::clone(&context)),
            sessions: queries::ListSessions::new(Arc::clone(&context)),
            end_session: commands::EndSession::new(Arc::clone(&context)),
            two_factor: queries::GetTwoFactor::new(Arc::clone(&context)),
            two_factor_accounts: queries::ListTwoFactorAccounts::new(Arc::clone(&context)),
            begin_two_factor: commands::BeginTwoFactor::new(Arc::clone(&context)),
            confirm_two_factor: commands::ConfirmTwoFactor::new(Arc::clone(&context)),
            disable_two_factor: commands::DisableTwoFactor::new(Arc::clone(&context)),
            renew_recovery_codes: commands::RenewRecoveryCodes::new(Arc::clone(&context)),
            reset_two_factor: commands::ResetTwoFactor::new(Arc::clone(&context)),
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
    pub password_locked: bool,
    pub created_at: Timestamp,
}

/// What a password that checked out leads to.
#[derive(Debug)]
pub enum LoginOutcome {
    SignedIn(SignedIn),
    /// The account signs in with a second factor: the code goes with this
    /// token to [`commands::CompleteSignIn`].
    SecondFactor(SecondFactorChallenge),
}

/// A sign-in waiting for its second factor.
#[derive(Debug)]
pub struct SecondFactorChallenge {
    /// Given back with the code; known to this client only.
    pub token: Secret,
    pub expires_at: Timestamp,
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
