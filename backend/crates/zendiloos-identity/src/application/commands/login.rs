use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::context::Context;
use crate::application::{IdentityError, SignedIn};
use crate::domain::{AccountName, AuditAction, ThrottleKey};

pub struct LoginInput {
    pub name: String,
    pub password: Secret,
}

/// Command: checks name and password and starts a session.
#[derive(Clone)]
pub struct Login {
    context: Arc<Context>,
}

impl Login {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        input: LoginInput,
        request: &RequestContext,
    ) -> Result<SignedIn, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| AuditEntry::new(now, AuditAction::Login.as_str(), outcome, request);

        let typed = input.name.trim();
        // Account names are lowercase; accept them as people type them.
        let account = match AccountName::parse(&typed.to_lowercase()) {
            Ok(name) => c.ports.accounts.find_by_name(&name).await?,
            Err(_) => None,
        };
        // Against the account only when it exists: made-up names must not
        // fill the throttle with fake accounts.
        let keys = Context::throttle_keys(request, account.as_ref().map(|a| &a.name));
        let attempt = c.begin_attempt(keys, request, now).await?;
        // Runs for unknown names too, so the response time does not reveal
        // which accounts exist.
        let verified =
            c.ports.hasher.verify(&input.password, account.as_ref().map(|a| &a.password_hash)).await?;

        let checked = match account {
            Some(account) if verified => account,
            Some(account) => {
                attempt.failed(request).await?;
                let entry = audit(Outcome::Failure).by(&account.actor());
                c.audit(entry.reason("wrong_password")).await?;
                return Err(IdentityError::InvalidCredentials);
            }
            None => {
                attempt.failed(request).await?;
                c.audit(audit(Outcome::Failure).actor(typed).reason("unknown_account")).await?;
                return Err(IdentityError::InvalidCredentials);
            }
        };

        // From here one with changes to accounts: the password was checked
        // against the hash read before, outside, as Argon2 takes a while. A
        // change or reset of the password in between ended the account's
        // sessions; this one must not start after it with the old password.
        let _changes = c.changes.lock().await;
        let account = c.ports.accounts.find_by_id(checked.id).await?;
        let Some(mut account) = account.filter(|now| now.password_hash == checked.password_hash) else {
            // The password given is not the account's (or there is no
            // account) any more.
            attempt.failed(request).await?;
            let entry = audit(Outcome::Failure).by(&checked.actor());
            c.audit(entry.reason("wrong_password")).await?;
            return Err(IdentityError::InvalidCredentials);
        };

        if account.disabled {
            let entry = audit(Outcome::Denied).by(&account.actor());
            c.audit(entry.reason("disabled")).await?;
            return Err(IdentityError::AccountDisabled);
        }

        drop(attempt);
        c.clear_throttle(&ThrottleKey::Account(account.name.clone()));
        if !account.password_locked && !account.password_change_required {
            let policy = c.ports.accounts.password_policy().await?;
            let expired = policy.max_age_days.is_some_and(|days| {
                now.duration_since(account.password_changed_at)
                    > std::time::Duration::from_secs(u64::from(days) * 86_400)
            });
            if expired {
                c.ports.accounts.require_password_change(account.id).await?;
                account.password_change_required = true;
            }
        }
        let signed_in = c.start_session(&account, request, now).await?;
        c.audit(audit(Outcome::Success).by(&account.actor())).await?;
        Ok(signed_in)
    }
}
