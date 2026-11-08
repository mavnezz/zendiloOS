//! State and helpers shared by the use cases.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zendiloos_kernel::{AuditEntry, Outcome, PortError, RequestContext, Secret, Timestamp, constant_time_eq};

use super::ports::Creation;
use super::{AccountSummary, AuthContext, IdentityConfig, IdentityError, IdentityPorts, SignedIn};
use crate::domain::{
    Account, AccountId, AccountName, AddressRange, AuditAction, BlockedAddress, FailureLimit, LoginThrottle,
    NewSession, PasswordHash, Permission, Role, SecuritySettings, SessionId, SessionPolicy, ThrottleKey,
    ThrottlePolicy, TokenHash, TotpSecret, TwoFactor, TypedCode, is_blocked, matching_step, time_step,
};

/// How long a sign-in waits for its second factor.
pub(crate) const PENDING_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Sign-ins waiting at most at a time; the oldest go first.
const MAX_PENDING: usize = 1_000;
/// Sign-ins waiting at most for one account, and from one network (an
/// IPv4 address, an IPv6 /64): another one makes room by ending the oldest
/// of the same, so nobody ends others' waiting sign-ins by starting many.
pub(crate) const MAX_PENDING_PER_ACCOUNT: usize = 5;
pub(crate) const MAX_PENDING_PER_NETWORK: usize = 20;
/// Wrong codes a waiting sign-in takes before it ends: then the password
/// again (and the throttles count each).
pub(crate) const MAX_CODE_FAILURES: u8 = 3;
/// How long a second factor being set up waits for its first code.
pub(crate) const SETUP_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Wrong second codes an account takes at most within each window, from
/// wherever they come and whatever the account lock is set to. A sign-in
/// that succeeds does not reset them: the owner signing in meanwhile gives
/// someone who knows the password and guesses codes no new tries.
pub(crate) const CODE_LIMITS: [(Duration, usize); 2] =
    [(Duration::from_secs(60 * 60), 10), (Duration::from_secs(24 * 60 * 60), 30)];

/// A sign-in whose password checked out, waiting for its second factor.
#[derive(Debug, Clone)]
pub(crate) struct PendingSignIn {
    pub(crate) account_id: AccountId,
    /// What the password and the second factor were when the password was
    /// checked: changed in between, the sign-in ends.
    pub(crate) password_hash: PasswordHash,
    pub(crate) enabled_at: Option<Timestamp>,
    /// The address the password came from; the code must come from it too.
    pub(crate) client_ip: Option<IpAddr>,
    pub(crate) expires_at: Timestamp,
    pub(crate) failures: u8,
}

/// A second factor being set up, waiting for its first code; its secret is
/// kept with the account's second factor (`pending`).
#[derive(Debug, Clone)]
pub(crate) struct PendingSetup {
    /// The session that began it: no other confirms it.
    pub(crate) session_id: SessionId,
    /// The password it was begun with: changed or reset, the setup ends.
    pub(crate) password_hash: PasswordHash,
    pub(crate) expires_at: Timestamp,
    pub(crate) failures: u8,
}

/// Which of the account's codes a code that checked out was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeUsed {
    /// From the app, for this time step.
    OneTime(u64),
    /// A recovery code: the one at this index.
    Recovery(usize),
}

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
        // The attempts under way in a throttle replaced are forgotten; they
        // are only recorded in the new one if they fail.
        if self.accounts.as_ref().map(|(limit, _)| *limit) != settings.account_lock {
            self.accounts = settings.account_lock.map(|limit| (limit, per(limit, true)));
        }
        let blocking = settings.auto_block.map(|block| block.limit);
        if self.blocking.as_ref().map(|(limit, _)| *limit) != blocking {
            self.blocking = blocking.map(|limit| (limit, per(limit, false)));
        }
    }

    /// The throttles an attempt for `key` counts against.
    fn throttles(&self, key: &ThrottleKey) -> [Option<&LoginThrottle>; 2] {
        match key {
            ThrottleKey::Address(_) => [Some(&self.addresses), self.blocking.as_ref().map(|(_, t)| t)],
            ThrottleKey::Account(_) => [self.accounts.as_ref().map(|(_, t)| t), None],
        }
    }

    fn throttles_mut(&mut self, key: &ThrottleKey) -> [Option<&mut LoginThrottle>; 2] {
        match key {
            ThrottleKey::Address(_) => [Some(&mut self.addresses), self.blocking.as_mut().map(|(_, t)| t)],
            ThrottleKey::Account(_) => [self.accounts.as_mut().map(|(_, t)| t), None],
        }
    }

    /// Ends an attempt for `keys`; whether, failed, it reached the limit for
    /// blocking an address.
    fn end(&mut self, keys: &[ThrottleKey], failed: bool, now: Timestamp) -> bool {
        let mut reached = false;
        for key in keys {
            for throttle in self.throttles_mut(key).into_iter().flatten() {
                throttle.end(key, failed, now);
            }
            if let (true, ThrottleKey::Address(_), Some((_, blocking))) =
                (failed, key, self.blocking.as_mut())
                && blocking.reached(key, now)
            {
                blocking.clear(key);
                reached = true;
            }
        }
        reached
    }
}

/// An attempt [`Context::begin_attempt`] let through. Dropped without
/// [`failed`](Self::failed) it did not fail: the secret was right, or
/// something else stopped it, or the caller went away before learning
/// whether it was right.
pub(crate) struct Attempt<'a> {
    context: &'a Context,
    keys: Vec<ThrottleKey>,
    now: Timestamp,
    ended: bool,
}

impl Attempt<'_> {
    /// The secret was wrong: counted against every key, and an address that
    /// failed too often is blocked.
    pub(crate) async fn failed(mut self, request: &RequestContext) -> Result<(), PortError> {
        self.ended = true;
        let reached = self.context.attempts().end(&self.keys, true, self.now);
        self.context.block_if_reached(reached, request, self.now).await
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.ended {
            self.context.attempts().end(&self.keys, false, self.now);
        }
    }
}

/// A second code checked for an account, which [`Context::begin_code_attempt`]
/// let through; like [`Attempt`], it counts as failed until it ends.
pub(crate) struct CodeAttempt<'a> {
    context: &'a Context,
    key: ThrottleKey,
    now: Timestamp,
    ended: bool,
}

impl CodeAttempt<'_> {
    /// The code was wrong: counted against the account's limits.
    pub(crate) fn failed(mut self) {
        self.ended = true;
        self.context.end_code_attempt(&self.key, true, self.now);
    }
}

impl Drop for CodeAttempt<'_> {
    fn drop(&mut self) {
        if !self.ended {
            self.context.end_code_attempt(&self.key, false, self.now);
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
    /// Sign-ins waiting for their second factor, by their token's digest.
    pending: Mutex<HashMap<TokenHash, PendingSignIn>>,
    /// Second factors being set up, by account. In memory only: after a
    /// restart a setup begun before is begun again.
    setups: Mutex<HashMap<AccountId, PendingSetup>>,
    /// Wrong second codes per account, against [`CODE_LIMITS`].
    codes: Mutex<Vec<LoginThrottle>>,
    /// Held while preferences are read, merged and saved.
    pub(crate) preferences: tokio::sync::Mutex<()>,
}

impl Context {
    pub(crate) fn new(ports: IdentityPorts, config: IdentityConfig) -> Self {
        Self {
            ports,
            config,
            attempts: Mutex::new(Attempts::new(config.throttle)),
            defences: Mutex::new(None),
            changes: tokio::sync::Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            setups: Mutex::new(HashMap::new()),
            codes: Mutex::new(
                CODE_LIMITS
                    .iter()
                    .map(|&(window, failures)| {
                        LoginThrottle::new(ThrottlePolicy {
                            window,
                            max_failures_per_account: failures,
                            max_failures_per_address: usize::MAX,
                            max_tracked_addresses: config.throttle.max_tracked_addresses,
                        })
                    })
                    .collect(),
            ),
            preferences: tokio::sync::Mutex::new(()),
        }
    }

    fn pending(&self) -> MutexGuard<'_, HashMap<TokenHash, PendingSignIn>> {
        // A map of plain values: nothing a panic could leave half done.
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `sign_in` waiting under `token`, making room if need be: the
    /// account's oldest, then the oldest from its network, then the oldest
    /// of all.
    pub(crate) fn hold_sign_in(&self, token: TokenHash, sign_in: PendingSignIn, now: Timestamp) {
        let network = |ip: Option<IpAddr>| ip.map(|ip| AddressRange::client(ip).network());
        let mut pending = self.pending();
        pending.retain(|_, waiting| waiting.expires_at > now);
        let from = network(sign_in.client_ip);
        make_room(&mut pending, MAX_PENDING_PER_ACCOUNT, |waiting| waiting.account_id == sign_in.account_id);
        if from.is_some() {
            make_room(&mut pending, MAX_PENDING_PER_NETWORK, |waiting| network(waiting.client_ip) == from);
        }
        make_room(&mut pending, MAX_PENDING, |_| true);
        pending.insert(token, sign_in);
    }

    /// The sign-in waiting under `token`, if it has not expired.
    pub(crate) fn waiting_sign_in(&self, token: &TokenHash, now: Timestamp) -> Option<PendingSignIn> {
        let mut pending = self.pending();
        match pending.get(token) {
            Some(waiting) if waiting.expires_at > now => Some(waiting.clone()),
            Some(_) => {
                pending.remove(token);
                None
            }
            None => None,
        }
    }

    /// One more wrong code for the sign-in under `token`; it ends after
    /// [`MAX_CODE_FAILURES`]. Whether it ended.
    pub(crate) fn wrong_code(&self, token: &TokenHash) -> bool {
        let mut pending = self.pending();
        let Some(waiting) = pending.get_mut(token) else { return true };
        waiting.failures += 1;
        let ended = waiting.failures >= MAX_CODE_FAILURES;
        if ended {
            pending.remove(token);
        }
        ended
    }

    fn setups(&self) -> MutexGuard<'_, HashMap<AccountId, PendingSetup>> {
        // Plain values, as for the sign-ins waiting.
        self.setups.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps the setup `account` began waiting, in place of one before.
    pub(crate) fn hold_setup(&self, account: AccountId, setup: PendingSetup, now: Timestamp) {
        let mut setups = self.setups();
        setups.retain(|_, waiting| waiting.expires_at > now);
        setups.insert(account, setup);
    }

    /// The setup of `account` waiting, if it has not expired.
    pub(crate) fn waiting_setup(&self, account: AccountId, now: Timestamp) -> Option<PendingSetup> {
        let mut setups = self.setups();
        match setups.get(&account) {
            Some(waiting) if waiting.expires_at > now => Some(waiting.clone()),
            Some(_) => {
                setups.remove(&account);
                None
            }
            None => None,
        }
    }

    /// One more wrong first code for the setup of `account`; it ends after
    /// [`MAX_CODE_FAILURES`]. Whether it ended.
    pub(crate) fn wrong_setup_code(&self, account: AccountId) -> bool {
        let mut setups = self.setups();
        let Some(waiting) = setups.get_mut(&account) else { return true };
        waiting.failures += 1;
        let ended = waiting.failures >= MAX_CODE_FAILURES;
        if ended {
            setups.remove(&account);
        }
        ended
    }

    pub(crate) fn end_setup_of(&self, account: AccountId) {
        self.setups().remove(&account);
    }

    fn codes(&self) -> MutexGuard<'_, Vec<LoginThrottle>> {
        // The counts hold no invariant a panic could break halfway.
        self.codes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts checking a second code of `account`: refused once it took
    /// too many wrong ones ([`CODE_LIMITS`]).
    pub(crate) fn begin_code_attempt(
        &self,
        account: &AccountName,
        now: Timestamp,
    ) -> Result<CodeAttempt<'_>, IdentityError> {
        let key = ThrottleKey::Account(account.clone());
        let mut codes = self.codes();
        for throttle in codes.iter() {
            throttle
                .check(&key, now)
                .map_err(|retry_after| IdentityError::TooManyAttempts { retry_after })?;
        }
        for throttle in codes.iter_mut() {
            throttle.start(&key);
        }
        Ok(CodeAttempt { context: self, key, now, ended: false })
    }

    fn end_code_attempt(&self, key: &ThrottleKey, failed: bool, now: Timestamp) {
        for throttle in self.codes().iter_mut() {
            throttle.end(key, failed, now);
        }
    }

    pub(crate) fn end_sign_in(&self, token: &TokenHash) {
        self.pending().remove(token);
    }

    /// Ends every sign-in of `account` waiting for its second factor, and a
    /// setup of one waiting: its password or second factor changed.
    pub(crate) fn end_sign_ins_of(&self, account: AccountId) {
        self.pending().retain(|_, waiting| waiting.account_id != account);
        self.end_setup_of(account);
    }

    /// Which code of `two_factor` (on) `code` is, if any: a one-time code of
    /// a step not taken yet, or a recovery code not used yet.
    pub(crate) fn check_code(&self, two_factor: &TwoFactor, code: &str, now: Timestamp) -> Option<CodeUsed> {
        match TypedCode::parse(code)? {
            TypedCode::OneTime(digits) => {
                let secret = two_factor.secret.as_ref()?;
                self.one_time_step(secret, &digits, two_factor.last_step, now).map(CodeUsed::OneTime)
            }
            TypedCode::Recovery(normalized) => {
                let digest = self.ports.codes.digest(&normalized);
                two_factor
                    .recovery
                    .iter()
                    .position(|kept| constant_time_eq(kept.as_str().as_bytes(), digest.as_str().as_bytes()))
                    .map(CodeUsed::Recovery)
            }
        }
    }

    /// The time step a one-time `code` of `secret` belongs to.
    pub(crate) fn one_time_step(
        &self,
        secret: &TotpSecret,
        code: &str,
        last_step: Option<u64>,
        now: Timestamp,
    ) -> Option<u64> {
        matching_step(code, time_step(now), last_step, |step| self.ports.codes.code(secret, step))
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
        let address = request.client_ip.map(|ip| ThrottleKey::Address(AddressRange::client(ip).network()));
        let account = account.cloned().map(ThrottleKey::Account);
        address.into_iter().chain(account).collect()
    }

    /// Starts an attempt to give a secret for `keys` (a password, the setup
    /// code); refused from a blocked address, or for a key with no attempts
    /// left. Until it ends, it counts as failed: attempts made at once
    /// cannot all get past the limits while none of them has failed yet.
    pub(crate) async fn begin_attempt(
        &self,
        keys: Vec<ThrottleKey>,
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<Attempt<'_>, IdentityError> {
        if self.defences().await?.blocks(request.client_ip, now) {
            return Err(IdentityError::AddressBlocked);
        }
        let mut attempts = self.attempts();
        for key in &keys {
            for throttle in attempts.throttles(key).into_iter().flatten() {
                throttle
                    .check(key, now)
                    .map_err(|retry_after| IdentityError::TooManyAttempts { retry_after })?;
            }
        }
        for key in &keys {
            for throttle in attempts.throttles_mut(key).into_iter().flatten() {
                throttle.start(key);
            }
        }
        Ok(Attempt { context: self, keys, now, ended: false })
    }

    /// Blocks the address `request` came from once its failures `reached`
    /// the limit for that.
    async fn block_if_reached(
        &self,
        reached: bool,
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<(), PortError> {
        let defences = self.defences().await?;
        let (true, Some(ip)) = (reached, request.client_ip) else { return Ok(()) };
        let Some(auto) = defences.settings.auto_block else { return Ok(()) };
        if defences.settings.is_allowed(ip) || defences.blocks(Some(ip), now) {
            return Ok(());
        }
        let release = |days: u16| now.saturating_add(Duration::from_secs(u64::from(days) * 86_400));
        let block = BlockedAddress {
            range: AddressRange::client(ip),
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

    /// Starts the session of a sign-in that checked out: an account whose
    /// password is older than the policy allows must choose a new one first.
    pub(crate) async fn finish_sign_in(
        &self,
        account: &mut Account,
        request: &RequestContext,
        now: Timestamp,
    ) -> Result<SignedIn, IdentityError> {
        if !account.password_locked && !account.password_change_required {
            let policy = self.ports.accounts.password_policy().await?;
            let expired = policy.max_age_days.is_some_and(|days| {
                now.duration_since(account.password_changed_at)
                    > Duration::from_secs(u64::from(days) * 86_400)
            });
            if expired {
                self.ports.accounts.require_password_change(account.id).await?;
                account.password_change_required = true;
            }
        }
        self.start_session(account, request, now).await
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

/// Ends the oldest of the sign-ins waiting that `belongs` holds for, until
/// fewer than `most` of them wait.
fn make_room(
    pending: &mut HashMap<TokenHash, PendingSignIn>,
    most: usize,
    belongs: impl Fn(&PendingSignIn) -> bool,
) {
    while pending.values().filter(|waiting| belongs(waiting)).count() >= most {
        let oldest = pending
            .iter()
            .filter(|(_, waiting)| belongs(waiting))
            .min_by_key(|(_, waiting)| waiting.expires_at)
            .map(|(token, _)| *token);
        match oldest {
            Some(oldest) => pending.remove(&oldest),
            None => return,
        };
    }
}

/// Takes `used` out of `two_factor`: its time step is not taken again, a
/// recovery code is gone.
pub(crate) fn spend(two_factor: &mut TwoFactor, used: CodeUsed) {
    match used {
        CodeUsed::OneTime(step) => two_factor.last_step = Some(step),
        CodeUsed::Recovery(index) => {
            two_factor.recovery.remove(index);
        }
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
        created_at: account.created_at,
    }
}
