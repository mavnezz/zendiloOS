//! State and helpers shared by the use cases.

use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{AuditEntry, Outcome, PortError, RequestContext, Secret, Timestamp};

use super::ports::Creation;
use super::{AccountSummary, AuthContext, IdentityConfig, IdentityError, IdentityPorts, SignedIn};
use crate::domain::{
    Account, AccountId, AccountName, AddressRange, AuditAction, BlockedAddress, FailureLimit, LoginThrottle,
    NewSession, Permission, Role, SecuritySettings, SessionPolicy, ThrottleKey, ThrottlePolicy, is_blocked,
};

/// The security settings and blocks in force: loaded once, replaced
/// whenever they change.
pub(crate) struct Defences {
    pub(crate) settings: SecuritySettings,
    pub(crate) blocked: Vec<BlockedAddress>,
}

impl Defences {
    pub(crate) fn blocks(&self, ip: Option<IpAddr>, now: Timestamp) -> bool {
        ip.is_some_and(|ip| is_blocked(ip, &self.blocked, &self.settings, now))
    }
}

/// Failed attempts, in memory: against the fixed limit per address, the
/// account lock and the count towards blocking an address, as set.
struct Attempts {
    addresses: LoginThrottle,
    /// With the limit it follows; `None`: accounts do not lock.
    accounts: Option<(FailureLimit, LoginThrottle)>,
    /// `None`: addresses are not blocked.
    blocking: Option<(FailureLimit, LoginThrottle)>,
}

impl Attempts {
    fn new(policy: ThrottlePolicy) -> Self {
        let defaults = SecuritySettings::default();
        let mut attempts = Self { addresses: LoginThrottle::new(policy), accounts: None, blocking: None };
        attempts.follow(&defaults, policy);
        attempts
    }

    /// Starts counting anew where a limit changed.
    fn follow(&mut self, settings: &SecuritySettings, base: ThrottlePolicy) {
        let per = |limit: FailureLimit, account: bool| {
            let failures = usize::from(limit.failures);
            LoginThrottle::new(ThrottlePolicy {
                window: limit.window(),
                max_failures_per_account: if account { failures } else { usize::MAX },
                max_failures_per_address: if account { usize::MAX } else { failures },
                max_tracked_addresses: base.max_tracked_addresses,
            })
        };
        if self.accounts.as_ref().map(|(limit, _)| *limit) != settings.account_lock {
            self.accounts = settings.account_lock.map(|limit| (limit, per(limit, true)));
        }
        let blocking = settings.auto_block.map(|block| block.limit);
        if self.blocking.as_ref().map(|(limit, _)| *limit) != blocking {
            self.blocking = blocking.map(|limit| (limit, per(limit, false)));
        }
    }
}

pub(crate) struct Context {
    pub(crate) ports: IdentityPorts,
    pub(crate) config: IdentityConfig,
    attempts: Mutex<Attempts>,
    defences: Mutex<Option<Arc<Defences>>>,
    /// Held while accounts and groups change, so that checks such as "an
    /// administrator remains" cannot be raced by a concurrent change.
    pub(crate) changes: tokio::sync::Mutex<()>,
}

impl Context {
    pub(crate) fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        Self {
            ports,
            config,
            attempts: Mutex::new(Attempts::new(config.throttle)),
            defences: Mutex::new(None),
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

    fn attempts(&self) -> MutexGuard<'_, Attempts> {
        // The counts hold no invariant a panic could break halfway.
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cached_defences(&self) -> MutexGuard<'_, Option<Arc<Defences>>> {
        self.defences.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The security settings and blocks in force.
    pub(crate) async fn defences(&self) -> Result<Arc<Defences>, PortError> {
        let cached = self.cached_defences().clone();
        match cached {
            Some(defences) => Ok(defences),
            None => self.reload_defences().await,
        }
    }

    /// Reads them again, after a change.
    pub(crate) async fn reload_defences(&self) -> Result<Arc<Defences>, PortError> {
        let settings = self.ports.security.settings().await?;
        let blocked = self.ports.security.blocked().await?;
        self.attempts().follow(&settings, self.config.throttle);
        let defences = Arc::new(Defences { settings, blocked });
        *self.cached_defences() = Some(Arc::clone(&defences));
        Ok(defences)
    }

    /// How long sessions live, with the idle time as set.
    pub(crate) async fn session_policy(&self) -> Result<SessionPolicy, PortError> {
        let idle_timeout = self.defences().await?.settings.idle_timeout();
        Ok(SessionPolicy { idle_timeout, ..self.config.sessions })
    }

    /// The keys an attempt from `request` for `account` counts against.
    pub(crate) fn throttle_keys(request: &RequestContext, account: Option<&AccountName>) -> Vec<ThrottleKey> {
        let address = request.client_ip.map(ThrottleKey::Address);
        let account = account.cloned().map(ThrottleKey::Account);
        address.into_iter().chain(account).collect()
    }

    /// Refuses an attempt from a blocked address, or for one of `keys` with
    /// no attempts left.
    pub(crate) async fn check_throttle(
        &self,
        keys: &[ThrottleKey],
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<(), IdentityError> {
        if self.defences().await?.blocks(request.client_ip, now) {
            return Err(IdentityError::AddressBlocked);
        }
        let attempts = self.attempts();
        let check = |key: &ThrottleKey| match key {
            ThrottleKey::Address(_) => attempts.addresses.check(key, now),
            ThrottleKey::Account(_) => {
                attempts.accounts.as_ref().map_or(Ok(()), |(_, throttle)| throttle.check(key, now))
            }
        };
        keys.iter().try_for_each(check).map_err(|retry_after| IdentityError::TooManyAttempts { retry_after })
    }

    /// Counts a failed attempt; an address that failed too often is blocked.
    pub(crate) async fn record_failure(
        &self,
        keys: Vec<ThrottleKey>,
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<(), PortError> {
        let defences = self.defences().await?;
        let mut reached = None;
        {
            let mut attempts = self.attempts();
            for key in keys {
                match key {
                    ThrottleKey::Address(ip) => {
                        attempts.addresses.record_failure(ThrottleKey::Address(ip), now);
                        if let Some((_, blocking)) = attempts.blocking.as_mut() {
                            let key = ThrottleKey::Address(ip);
                            blocking.record_failure(key.clone(), now);
                            if blocking.check(&key, now).is_err() {
                                blocking.clear(&key);
                                reached = Some(ip);
                            }
                        }
                    }
                    ThrottleKey::Account(_) => {
                        if let Some((_, throttle)) = attempts.accounts.as_mut() {
                            throttle.record_failure(key, now);
                        }
                    }
                }
            }
        }
        let Some(ip) = reached else { return Ok(()) };
        let Some(auto) = defences.settings.auto_block else { return Ok(()) };
        if defences.settings.is_allowed(ip) || defences.blocks(Some(ip), now) {
            return Ok(());
        }
        let release = |days: u16| now.saturating_add(Duration::from_secs(u64::from(days) * 86_400));
        let block = BlockedAddress {
            range: AddressRange::single(ip),
            blocked_at: now,
            until: auto.release_after_days.map(release),
            automatic: true,
        };
        self.ports.security.block(&block).await?;
        self.reload_defences().await?;
        tracing::warn!(address = %ip, "blocked after failed logins");
        let entry = AuditEntry::new(now, AuditAction::AddressBlock.as_str(), Outcome::Success, request);
        self.audit(entry.target(block.range.to_string()).reason("automatic")).await
    }

    pub(crate) fn clear_throttle(&self, key: &ThrottleKey) {
        let mut attempts = self.attempts();
        match key {
            ThrottleKey::Address(_) => attempts.addresses.clear(key),
            ThrottleKey::Account(_) => {
                if let Some((_, throttle)) = attempts.accounts.as_mut() {
                    throttle.clear(key);
                }
            }
        }
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
        password_locked: account.password_locked,
    }
}
