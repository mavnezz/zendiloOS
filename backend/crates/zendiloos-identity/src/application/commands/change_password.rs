use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{AuditAction, NewPassword, ThrottleKey};

pub struct ChangePasswordInput {
    pub current_password: Secret,
    pub new_password: Secret,
}

/// Command: the caller changes their own password, for the web interface and
/// SMB. Ends all their other sessions, since one of them might belong to
/// whoever knew the old one.
#[derive(Clone)]
pub struct ChangePassword {
    context: Arc<Context>,
}

impl ChangePassword {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        input: ChangePasswordInput,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        let now = c.now();
        let keys = Context::throttle_keys(request, Some(&auth.account.name));
        c.check_throttle(&keys, now).map_err(|retry_after| IdentityError::TooManyAttempts { retry_after })?;

        let account =
            c.ports.accounts.find_by_id(auth.account.id).await?.ok_or(IdentityError::Unauthenticated)?;
        let audit = |outcome| {
            AuditEntry::new(now, AuditAction::PasswordChange.as_str(), outcome, request).by(&account.actor())
        };

        if !c.ports.hasher.verify(&input.current_password, Some(&account.password_hash)).await? {
            c.record_failure(keys, now);
            c.audit(audit(Outcome::Failure).reason("wrong_password")).await?;
            return Err(IdentityError::WrongPassword);
        }

        let password = NewPassword::parse(input.new_password, &account.name)?;
        // SMB first: if the agent fails, the old password still works everywhere.
        c.ports.system.set_password(&account.name, password.secret(), request).await?;
        let hash = c.ports.hasher.hash(password.secret()).await?;
        c.ports.accounts.update_password(account.id, &hash, now).await?;
        c.ports.sessions.delete_others(account.id, auth.session_id).await?;
        c.clear_throttle(&ThrottleKey::Account(account.name.clone()));
        c.audit(audit(Outcome::Success)).await?;
        Ok(())
    }
}
