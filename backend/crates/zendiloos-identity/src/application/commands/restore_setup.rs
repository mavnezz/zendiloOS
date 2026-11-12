use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::IdentityError;
use crate::application::context::Context;
use crate::domain::{AuditAction, is_own_network};

/// Command: whether a configuration backup may be restored instead of
/// creating the first administrator. The same gate as the setup: no
/// administrator yet, the setup code shown on the console or, for an open
/// setup, a browser in the NAS's own network; tries throttled as the
/// setup's. Once restored, [`RestoreSetup::close`] ends the setup: the
/// restored accounts sign in.
#[derive(Clone)]
pub struct RestoreSetup {
    context: Arc<Context>,
}

impl RestoreSetup {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn allow(
        &self,
        setup_code: Option<Secret>,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        let now = c.now();
        let audit = |outcome| AuditEntry::new(now, AuditAction::Setup.as_str(), outcome, request);
        let keys = Context::throttle_keys(request, None);
        let attempt = c.begin_attempt(keys, request, now).await?;
        if c.ports.accounts.any_admin().await? {
            return Err(IdentityError::SetupCompleted);
        }
        if c.ports.setup_codes.is_open().await? {
            if !request.client_ip.is_some_and(is_own_network) {
                attempt.failed(request).await?;
                c.audit(audit(Outcome::Failure).reason("open_setup_from_elsewhere")).await?;
                return Err(IdentityError::SetupNotLocal);
            }
        } else {
            let code = c.ports.setup_codes.current().await?;
            let given = setup_code.as_ref().map(Secret::expose);
            if !code.zip(given).is_some_and(|(code, given)| code.matches(given)) {
                attempt.failed(request).await?;
                c.audit(audit(Outcome::Failure).reason("wrong_setup_code")).await?;
                return Err(IdentityError::InvalidSetupCode);
            }
        }
        drop(attempt);
        Ok(())
    }

    /// The setup is over: its code and the marker of an open setup go.
    pub async fn close(&self) -> Result<(), IdentityError> {
        self.context.ports.setup_codes.consume().await?;
        self.context.ports.setup_codes.close().await?;
        Ok(())
    }
}
