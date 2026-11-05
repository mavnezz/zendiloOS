//! State and helpers shared by the use cases.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{AuditEntry, PortError, RequestContext, Secret, Timestamp};

use super::ports::Creation;
use super::{AccountSummary, AuthContext, IdentityConfig, IdentityError, IdentityPorts, SignedIn};
use crate::domain::{
    Account, AccountId, AccountName, LoginThrottle, NewSession, Permission, Role, ThrottleKey,
};

pub(crate) struct Context {
    pub(crate) ports: IdentityPorts,
    pub(crate) config: IdentityConfig,
    throttle: Mutex<LoginThrottle>,
    /// Held while accounts and groups change, so that checks such as "an
    /// administrator remains" cannot be raced by a concurrent change.
    pub(crate) changes: tokio::sync::Mutex<()>,
}

impl Context {
    pub(crate) fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        Self {
            ports,
            config,
            throttle: Mutex::new(LoginThrottle::new(config.throttle)),
            changes: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn require(auth: &AuthContext, permission: Permission) -> Result<(), IdentityError> {
        if auth.can(permission) { Ok(()) } else { Err(IdentityError::Forbidden) }
    }

    /// Creates the Linux user and its Samba password for a new account.
    /// Undoes the user if the password cannot be set.
    pub(crate) async fn provision_user(
        &self,
        name: &AccountName,
        password: &Secret,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        if self.ports.system.create_user(name, request).await? == Creation::NameTaken {
            return Err(IdentityError::NameTaken);
        }
        if let Err(error) = self.ports.system.set_password(name, password, request).await {
            self.unprovision_user(name, request).await;
            return Err(error.into());
        }
        Ok(())
    }

    /// Whether an enabled administrator other than `except` exists.
    pub(crate) async fn another_enabled_admin(&self, except: AccountId) -> Result<bool, PortError> {
        let accounts = self.ports.accounts.list().await?;
        Ok(accounts.iter().any(|a| a.id != except && a.role == Role::Admin && !a.disabled))
    }

    /// Best effort, after a later step failed: the error that made it
    /// necessary is the one to report.
    pub(crate) async fn unprovision_user(&self, name: &AccountName, request: &RequestContext) {
        if let Err(error) = self.ports.system.delete_user(name, request).await {
            tracing::error!(user = name.as_str(), %error, "cannot remove the Linux user of a failed account creation");
        }
    }

    pub(crate) fn now(&self) -> Timestamp {
        self.ports.clock.now()
    }

    fn throttle(&self) -> MutexGuard<'_, LoginThrottle> {
        // The throttle holds no invariant a panic could break halfway.
        self.throttle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The keys an attempt from `request` for `account` counts against.
    pub(crate) fn throttle_keys(request: &RequestContext, account: Option<&AccountName>) -> Vec<ThrottleKey> {
        let address = request.client_ip.map(ThrottleKey::Address);
        let account = account.cloned().map(ThrottleKey::Account);
        address.into_iter().chain(account).collect()
    }

    /// `Err(retry_after)` if any of `keys` has no attempts left.
    pub(crate) fn check_throttle(&self, keys: &[ThrottleKey], now: Timestamp) -> Result<(), Duration> {
        let throttle = self.throttle();
        keys.iter().try_for_each(|key| throttle.check(key, now))
    }

    pub(crate) fn record_failure(&self, keys: Vec<ThrottleKey>, now: Timestamp) {
        let mut throttle = self.throttle();
        for key in keys {
            throttle.record_failure(key, now);
        }
    }

    pub(crate) fn clear_throttle(&self, key: &ThrottleKey) {
        self.throttle().clear(key);
    }

    pub(crate) async fn audit(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.ports.audit.record(entry).await
    }

    pub(crate) async fn start_session(
        &self,
        account: &Account,
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<SignedIn, IdentityError> {
        let issued = self.ports.tokens.issue();
        let csrf_token = self.ports.tokens.csrf_token();
        let expires_at = now.saturating_add(self.config.sessions.lifetime);
        let session_id = self
            .ports
            .sessions
            .create(NewSession {
                account_id: account.id,
                token_hash: issued.hash,
                csrf_token: csrf_token.clone(),
                client_ip: request.client_ip,
                user_agent: request.user_agent.clone(),
                created_at: now,
                expires_at,
            })
            .await?;
        Ok(SignedIn {
            token: issued.token,
            session: AuthContext { session_id, csrf_token, expires_at, account: summary(account) },
        })
    }
}

pub(crate) fn summary(account: &Account) -> AccountSummary {
    AccountSummary {
        id: account.id,
        name: account.name.clone(),
        full_name: account.full_name.clone(),
        role: account.role,
        password_change_required: account.password_change_required,
    }
}
