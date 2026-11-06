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
        let name = AccountName::parse(&typed.to_lowercase()).ok();
        c.check_throttle(&Context::throttle_keys(request, name.as_ref()), request, now).await?;

        let account = match &name {
            Some(name) => c.ports.accounts.find_by_name(name).await?,
            None => None,
        };
        // Runs for unknown names too, so the response time does not reveal
        // which accounts exist.
        let verified =
            c.ports.hasher.verify(&input.password, account.as_ref().map(|a| &a.password_hash)).await?;

        let account = match account {
            Some(account) if verified => account,
            Some(account) => {
                c.record_failure(Context::throttle_keys(request, Some(&account.name)), request, now).await?;
                let entry = audit(Outcome::Failure).by(&account.actor());
                c.audit(entry.reason("wrong_password")).await?;
                return Err(IdentityError::InvalidCredentials);
            }
            None => {
                // Counted against the address only: made-up names must not
                // fill the throttle with fake accounts.
                c.record_failure(Context::throttle_keys(request, None), request, now).await?;
                c.audit(audit(Outcome::Failure).actor(typed).reason("unknown_account")).await?;
                return Err(IdentityError::InvalidCredentials);
            }
        };

        if account.disabled {
            let entry = audit(Outcome::Denied).by(&account.actor());
            c.audit(entry.reason("disabled")).await?;
            return Err(IdentityError::AccountDisabled);
        }

        c.clear_throttle(&ThrottleKey::Account(account.name.clone()));
        let mut account = account;
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
