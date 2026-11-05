use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::context::Context;
use crate::application::{IdentityError, SignedIn};
use crate::domain::{AccountName, AuditAction, NewAccount, NewPassword, Role};

pub struct SetupInput {
    pub setup_code: Secret,
    pub name: String,
    pub password: Secret,
}

/// Command: creates the first administrator, which completes the initial
/// setup, and signs them in. Requires the setup code shown on the console.
#[derive(Clone)]
pub struct CompleteSetup {
    context: Arc<Context>,
}

impl CompleteSetup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        input: SetupInput,
        request: &RequestContext,
    ) -> Result<SignedIn, IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| AuditEntry::new(now, AuditAction::Setup.as_str(), outcome, request);

        let keys = Context::throttle_keys(request, None);
        c.check_throttle(&keys, now).map_err(|retry_after| IdentityError::TooManyAttempts { retry_after })?;
        if c.ports.accounts.any_admin().await? {
            return Err(IdentityError::SetupCompleted);
        }

        let code = c.ports.setup_codes.current().await?;
        if !code.is_some_and(|code| code.matches(input.setup_code.expose())) {
            c.record_failure(keys, now);
            c.audit(audit(Outcome::Failure).reason("wrong_setup_code")).await?;
            return Err(IdentityError::InvalidSetupCode);
        }

        let name = AccountName::parse(input.name.trim())?;
        let password = NewPassword::parse(input.password, &name)?;
        c.provision_user(&name, password.secret(), request).await?;
        let created = async {
            let password_hash = c.ports.hasher.hash(password.secret()).await?;
            let new_account = NewAccount {
                name: name.clone(),
                full_name: None,
                role: Role::Admin,
                password_hash,
                password_change_required: false,
                created_at: now,
            };
            // `None`: another setup won the race.
            c.ports.accounts.create_first_admin(new_account).await?.ok_or(IdentityError::SetupCompleted)
        }
        .await;
        let account = match created {
            Ok(account) => account,
            Err(error) => {
                c.unprovision_user(&name, request).await;
                return Err(error);
            }
        };

        c.ports.setup_codes.consume().await?;
        c.audit(audit(Outcome::Success).by(&account.actor())).await?;
        c.start_session(&account, request, now).await
    }
}
