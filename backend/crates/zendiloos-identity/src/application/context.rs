//! State and helpers shared by the use cases.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{AuditEntry, PortError, RequestContext, Timestamp};

use super::{AccountSummary, AuthContext, IdentityConfig, IdentityError, IdentityPorts, SignedIn};
use crate::domain::{Account, AccountName, LoginThrottle, NewSession, ThrottleKey};

pub(crate) struct Context {
    pub(crate) ports: IdentityPorts,
    pub(crate) config: IdentityConfig,
    throttle: Mutex<LoginThrottle>,
}

impl Context {
    pub(crate) fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        Self { ports, config, throttle: Mutex::new(LoginThrottle::new(config.throttle)) }
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
