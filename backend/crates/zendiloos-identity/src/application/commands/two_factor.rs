//! Two-factor sign-in: the second step of a sign-in, and an account
//! setting up, turning off or renewing its second factor; an administrator
//! resetting another's.

use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::context::{CodeUsed, Context, PendingSetup, SETUP_LIFETIME, spend};
use crate::application::{AuthContext, IdentityError, SignedIn};
use crate::domain::{
    Account, AccountId, AuditAction, Permission, ThrottleKey, TwoFactor, TypedCode, otpauth_uri,
    show_recovery_code,
};

/// The signed-in account if `password` is its password, `None` if it is
/// not. Argon2 runs outside the lock of account changes, as it takes a
/// while: [`unchanged`] checks under it that the password is still the one
/// checked.
async fn password_checked(
    c: &Context,
    auth: &AuthContext,
    password: &Secret,
) -> Result<Option<Account>, IdentityError> {
    let account =
        c.ports.accounts.find_by_id(auth.account.id).await?.ok_or(IdentityError::Unauthenticated)?;
    Ok(c.ports.hasher.verify(password, Some(&account.password_hash)).await?.then_some(account))
}

/// With the lock of account changes held: `account`'s password is the one
/// checked and the account is still enabled.
async fn unchanged(c: &Context, account: &Account) -> Result<(), IdentityError> {
    let stored = c.ports.accounts.find_by_id(account.id).await?;
    if stored.is_some_and(|now| now.password_hash == account.password_hash && !now.disabled) {
        Ok(())
    } else {
        Err(IdentityError::Unauthenticated)
    }
}

/// Command: the second step of a sign-in. The token
/// [`Login`](super::Login) gave and a code from the app or a recovery code
/// start the session.
#[derive(Clone)]
pub struct CompleteSignIn {
    context: Arc<Context>,
}

impl CompleteSignIn {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        token: &Secret,
        code: &Secret,
        request: &RequestContext,
    ) -> Result<SignedIn, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| AuditEntry::new(now, AuditAction::Login.as_str(), outcome, request);
        let key = c.ports.tokens.digest(token);
        // From the address the password came from, or not at all: a token
        // that leaked is no use elsewhere.
        let waiting = c
            .waiting_sign_in(&key, now)
            .filter(|waiting| waiting.client_ip == request.client_ip)
            .ok_or(IdentityError::SignInExpired)?;
        let checked = c.ports.accounts.find_by_id(waiting.account_id).await?.ok_or_else(|| {
            c.end_sign_in(&key);
            IdentityError::SignInExpired
        })?;
        let keys = Context::throttle_keys(request, Some(&checked.name));
        let attempt = c.begin_attempt(keys, request, now).await?;

        // With changes to accounts held: the sign-in still waits (another
        // request with it did not end it meanwhile), and the password and
        // the second factor are still those the password was checked with,
        // or the sign-in is over.
        let _changes = c.changes.lock().await;
        if c.waiting_sign_in(&key, now).is_none() {
            return Err(IdentityError::SignInExpired);
        }
        let account = c.ports.accounts.find_by_id(waiting.account_id).await?;
        let two_factor = c.ports.two_factor.find(waiting.account_id).await?;
        let (Some(mut account), Some(mut two_factor)) = (
            account.filter(|now| now.password_hash == waiting.password_hash),
            two_factor.filter(|now| now.is_on() && now.enabled_at == waiting.enabled_at),
        ) else {
            c.end_sign_in(&key);
            return Err(IdentityError::SignInExpired);
        };
        if account.disabled {
            c.end_sign_in(&key);
            c.audit(audit(Outcome::Denied).by(&account.actor()).reason("disabled")).await?;
            return Err(IdentityError::AccountDisabled);
        }
        let code_attempt = c.begin_code_attempt(&account.name, now)?;
        let Some(used) = c.check_code(&two_factor, code.expose(), now) else {
            let ended = c.wrong_code(&key);
            code_attempt.failed();
            attempt.failed(request).await?;
            c.audit(audit(Outcome::Failure).by(&account.actor()).reason("wrong_code")).await?;
            // The last wrong code a sign-in takes says it is over.
            return Err(if ended { IdentityError::SignInExpired } else { IdentityError::InvalidCode });
        };
        drop(code_attempt);

        spend(&mut two_factor, used);
        c.ports.two_factor.save(&two_factor).await?;
        c.end_sign_in(&key);
        drop(attempt);
        c.clear_throttle(&ThrottleKey::Account(account.name.clone()));
        let signed_in = c.finish_sign_in(&mut account, request, now).await?;
        let how = match used {
            CodeUsed::OneTime(_) => "second_factor",
            CodeUsed::Recovery(_) => "recovery_code",
        };
        c.audit(audit(Outcome::Success).by(&account.actor()).reason(how)).await?;
        Ok(signed_in)
    }
}

/// A second factor being set up: what the authenticator app is given.
#[derive(Debug)]
pub struct TwoFactorSetup {
    /// The secret in base32, for typing in.
    pub secret: Secret,
    /// The `otpauth://` address the QR code holds.
    pub uri: Secret,
}

/// Command: the signed-in account begins setting up its second factor,
/// with its password: a session alone (left open, taken over) cannot lock
/// its owner out with a phone of someone else's. A new secret waits for
/// its first code, from this session, for [`SETUP_LIFETIME`]; one begun
/// before is replaced. Not for an account whose password an administrator
/// sets: it may be shared, and one of those sharing it would lock the
/// others out.
#[derive(Clone)]
pub struct BeginTwoFactor {
    context: Arc<Context>,
}

impl BeginTwoFactor {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        password: &Secret,
        request: &RequestContext,
    ) -> Result<TwoFactorSetup, IdentityError> {
        let c = &self.context;
        let now = c.now();
        if c.ports.two_factor.find(auth.account.id).await?.is_some_and(|t| t.is_on()) {
            return Err(IdentityError::TwoFactorOn);
        }
        if auth.account.password_locked {
            return Err(IdentityError::PasswordLocked);
        }
        let keys = Context::throttle_keys(request, Some(&auth.account.name));
        let attempt = c.begin_attempt(keys, request, now).await?;
        let Some(account) = password_checked(c, auth, password).await? else {
            attempt.failed(request).await?;
            let entry =
                AuditEntry::new(now, AuditAction::TwoFactorEnable.as_str(), Outcome::Failure, request);
            c.audit(entry.by(&auth.actor()).reason("wrong_password")).await?;
            return Err(IdentityError::WrongPassword);
        };
        drop(attempt);

        let _changes = c.changes.lock().await;
        unchanged(c, &account).await?;
        if c.ports.accounts.find_by_id(account.id).await?.is_some_and(|now| now.password_locked) {
            return Err(IdentityError::PasswordLocked);
        }
        let mut two_factor = c.ports.two_factor.find(account.id).await?.unwrap_or(TwoFactor::new(account.id));
        if two_factor.is_on() {
            return Err(IdentityError::TwoFactorOn);
        }
        let secret = c.ports.codes.new_secret();
        two_factor.pending = Some(secret.clone());
        c.ports.two_factor.save(&two_factor).await?;
        let setup = PendingSetup {
            session_id: auth.session_id,
            password_hash: account.password_hash.clone(),
            expires_at: now.saturating_add(SETUP_LIFETIME),
            failures: 0,
        };
        c.hold_setup(account.id, setup, now);
        Ok(TwoFactorSetup {
            secret: Secret::new(secret.to_base32()),
            uri: Secret::new(otpauth_uri(&account.name, &secret)),
        })
    }
}

/// Command: the first code from the app confirms the secret begun with
/// [`BeginTwoFactor`]: sign-ins ask for a code from now on. Gives the
/// recovery codes, shown this once. Ends the account's other sessions.
/// Only the session that began the setup confirms it, while it waits, with
/// the password it was begun with; after [`MAX_CODE_FAILURES`] wrong codes
/// it is over, and is begun again with the password.
///
/// [`MAX_CODE_FAILURES`]: crate::application::context::MAX_CODE_FAILURES
#[derive(Clone)]
pub struct ConfirmTwoFactor {
    context: Arc<Context>,
}

impl ConfirmTwoFactor {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        code: &Secret,
        request: &RequestContext,
    ) -> Result<Vec<String>, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let _changes = c.changes.lock().await;
        let id = auth.account.id;
        let mut two_factor = c.ports.two_factor.find(id).await?.ok_or(IdentityError::NoTwoFactorSetup)?;
        if two_factor.is_on() {
            return Err(IdentityError::TwoFactorOn);
        }
        let pending = two_factor.pending.clone().ok_or(IdentityError::NoTwoFactorSetup)?;
        let account = c.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::Unauthenticated)?;
        let begun_here = c.waiting_setup(id, now).is_some_and(|setup| {
            setup.session_id == auth.session_id && setup.password_hash == account.password_hash
        });
        if !begun_here || account.disabled {
            return Err(IdentityError::NoTwoFactorSetup);
        }
        if account.password_locked {
            return Err(IdentityError::PasswordLocked);
        }
        // Not counted against the throttles: the secret was just shown to
        // this session, and a few wrong codes end the setup.
        let step = match TypedCode::parse(code.expose()) {
            Some(TypedCode::OneTime(digits)) => c.one_time_step(&pending, &digits, None, now),
            _ => None,
        };
        let Some(step) = step else {
            let ended = c.wrong_setup_code(id);
            if ended {
                two_factor.pending = None;
                c.ports.two_factor.save(&two_factor).await?;
            }
            let entry =
                AuditEntry::new(now, AuditAction::TwoFactorEnable.as_str(), Outcome::Failure, request);
            c.audit(entry.by(&auth.actor()).reason("wrong_code")).await?;
            return Err(if ended { IdentityError::NoTwoFactorSetup } else { IdentityError::InvalidCode });
        };

        let codes = c.ports.codes.recovery_codes();
        two_factor.secret = Some(pending);
        two_factor.pending = None;
        two_factor.last_step = Some(step);
        two_factor.recovery = codes.iter().map(|code| c.ports.codes.digest(code.expose())).collect();
        two_factor.enabled_at = Some(now);
        c.ports.two_factor.save(&two_factor).await?;
        c.ports.sessions.delete_others(id, auth.session_id).await?;
        c.end_sign_ins_of(id);
        let entry = AuditEntry::new(now, AuditAction::TwoFactorEnable.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor())).await?;
        Ok(codes.iter().map(|code| show_recovery_code(code.expose())).collect())
    }
}

/// Command: the signed-in account turns its second factor off, with its
/// password and a code (or a recovery code). Ends its other sessions.
#[derive(Clone)]
pub struct DisableTwoFactor {
    context: Arc<Context>,
}

impl DisableTwoFactor {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        password: &Secret,
        code: &Secret,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| {
            AuditEntry::new(now, AuditAction::TwoFactorDisable.as_str(), outcome, request).by(&auth.actor())
        };
        if !c.ports.two_factor.find(auth.account.id).await?.is_some_and(|t| t.is_on()) {
            return Err(IdentityError::TwoFactorOff);
        }
        let keys = Context::throttle_keys(request, Some(&auth.account.name));
        let attempt = c.begin_attempt(keys, request, now).await?;
        let Some(account) = password_checked(c, auth, password).await? else {
            attempt.failed(request).await?;
            c.audit(audit(Outcome::Failure).reason("wrong_password")).await?;
            return Err(IdentityError::WrongPassword);
        };

        let _changes = c.changes.lock().await;
        unchanged(c, &account).await?;
        let two_factor = c
            .ports
            .two_factor
            .find(account.id)
            .await?
            .filter(|t| t.is_on())
            .ok_or(IdentityError::TwoFactorOff)?;
        let code_attempt = c.begin_code_attempt(&account.name, now)?;
        if c.check_code(&two_factor, code.expose(), now).is_none() {
            code_attempt.failed();
            attempt.failed(request).await?;
            c.audit(audit(Outcome::Failure).reason("wrong_code")).await?;
            return Err(IdentityError::InvalidCode);
        }
        drop(code_attempt);
        drop(attempt);
        c.ports.two_factor.delete(account.id).await?;
        c.ports.sessions.delete_others(account.id, auth.session_id).await?;
        c.end_sign_ins_of(account.id);
        c.audit(audit(Outcome::Success)).await?;
        Ok(())
    }
}

/// Command: new recovery codes for the signed-in account, for its password
/// and a code from the app; the old ones stop working. Gives them, shown
/// this once. Ends the account's other sessions.
#[derive(Clone)]
pub struct RenewRecoveryCodes {
    context: Arc<Context>,
}

impl RenewRecoveryCodes {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        password: &Secret,
        code: &Secret,
        request: &RequestContext,
    ) -> Result<Vec<String>, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| {
            AuditEntry::new(now, AuditAction::RecoveryCodesRenew.as_str(), outcome, request).by(&auth.actor())
        };
        if !c.ports.two_factor.find(auth.account.id).await?.is_some_and(|t| t.is_on()) {
            return Err(IdentityError::TwoFactorOff);
        }
        let keys = Context::throttle_keys(request, Some(&auth.account.name));
        let attempt = c.begin_attempt(keys, request, now).await?;
        let Some(account) = password_checked(c, auth, password).await? else {
            attempt.failed(request).await?;
            c.audit(audit(Outcome::Failure).reason("wrong_password")).await?;
            return Err(IdentityError::WrongPassword);
        };

        let _changes = c.changes.lock().await;
        unchanged(c, &account).await?;
        let mut two_factor = c
            .ports
            .two_factor
            .find(auth.account.id)
            .await?
            .filter(|t| t.is_on())
            .ok_or(IdentityError::TwoFactorOff)?;
        // The app's code only: it shows the phone is still at hand.
        let code_attempt = c.begin_code_attempt(&account.name, now)?;
        let step = match (TypedCode::parse(code.expose()), two_factor.secret.as_ref()) {
            (Some(TypedCode::OneTime(digits)), Some(secret)) => {
                c.one_time_step(secret, &digits, two_factor.last_step, now)
            }
            _ => None,
        };
        let Some(step) = step else {
            code_attempt.failed();
            attempt.failed(request).await?;
            c.audit(audit(Outcome::Failure).reason("wrong_code")).await?;
            return Err(IdentityError::InvalidCode);
        };
        drop(code_attempt);
        drop(attempt);
        let codes = c.ports.codes.recovery_codes();
        two_factor.last_step = Some(step);
        two_factor.recovery = codes.iter().map(|code| c.ports.codes.digest(code.expose())).collect();
        c.ports.two_factor.save(&two_factor).await?;
        // A session or sign-in started with an old code may be why.
        c.ports.sessions.delete_others(auth.account.id, auth.session_id).await?;
        c.end_sign_ins_of(auth.account.id);
        c.audit(audit(Outcome::Success)).await?;
        Ok(codes.iter().map(|code| show_recovery_code(code.expose())).collect())
    }
}

/// Command: an administrator turns another account's second factor off,
/// for someone who lost their phone and their recovery codes; they set it
/// up again. Ends that account's sessions.
#[derive(Clone)]
pub struct ResetTwoFactor {
    context: Arc<Context>,
}

impl ResetTwoFactor {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: AccountId,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        if id == auth.account.id {
            return Err(IdentityError::OwnAccount);
        }
        let _changes = c.changes.lock().await;
        let account = c.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::AccountNotFound)?;
        if !c.ports.two_factor.find(id).await?.is_some_and(|t| t.is_on()) {
            return Err(IdentityError::TwoFactorOff);
        }
        c.ports.two_factor.delete(id).await?;
        c.ports.sessions.delete_all(id).await?;
        c.end_sign_ins_of(id);
        let entry = AuditEntry::new(c.now(), AuditAction::TwoFactorReset.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(account.name.to_string())).await?;
        Ok(())
    }
}
