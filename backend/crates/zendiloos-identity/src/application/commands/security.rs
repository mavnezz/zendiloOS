use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{AddressRange, AuditAction, BlockedAddress, Permission, SecuritySettings, SessionId};

/// Command: an administrator sets when sessions end, when accounts lock and
/// when addresses are blocked. Counts of failed attempts start anew where a
/// limit changed; blocks already in place stay.
#[derive(Clone)]
pub struct SetSecurity {
    context: Arc<Context>,
}

impl SetSecurity {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        settings: SecuritySettings,
        request: &RequestContext,
    ) -> Result<SecuritySettings, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::SystemManage)?;
        let settings = settings.checked()?;
        c.ports.security.save_settings(&settings).await?;
        let stored = c.reload_defences().await?;
        let entry = AuditEntry::new(c.now(), AuditAction::SecurityUpdate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor())).await?;
        Ok(stored.settings.clone())
    }
}

/// Command: an administrator blocks an address or a network, until the
/// block is lifted. Not the one the administrator is using, and not one
/// that is allowed.
#[derive(Clone)]
pub struct BlockAddress {
    context: Arc<Context>,
}

impl BlockAddress {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        range: &str,
        request: &RequestContext,
    ) -> Result<BlockedAddress, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::SystemManage)?;
        let range = AddressRange::parse(range)?;
        if request.client_ip.is_some_and(|ip| range.contains(ip)) {
            return Err(IdentityError::OwnAddress);
        }
        let defences = c.defences().await?;
        let allowed = range.is_single() && defences.settings.is_allowed(range.network());
        if allowed || defences.settings.allowed.contains(&range) {
            return Err(IdentityError::AddressAllowed);
        }
        let now = c.now();
        let block = BlockedAddress { range, blocked_at: now, until: None, automatic: false };
        c.ports.security.block(&block).await?;
        c.reload_defences().await?;
        let entry = AuditEntry::new(now, AuditAction::AddressBlock.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(range.to_string()).reason("manual")).await?;
        Ok(block)
    }
}

/// Command: an administrator lifts a block.
#[derive(Clone)]
pub struct UnblockAddress {
    context: Arc<Context>,
}

impl UnblockAddress {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        range: &str,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::SystemManage)?;
        let range = AddressRange::parse(range)?;
        if !c.ports.security.unblock(&range).await? {
            return Err(IdentityError::BlockNotFound);
        }
        c.reload_defences().await?;
        let entry = AuditEntry::new(c.now(), AuditAction::AddressUnblock.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(range.to_string())).await?;
        Ok(())
    }
}

/// Command: an administrator ends a session, of any account; the browser
/// that used it is signed out with its next request.
#[derive(Clone)]
pub struct EndSession {
    context: Arc<Context>,
}

impl EndSession {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: SessionId,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::SystemManage)?;
        let sessions = c.ports.sessions.list().await?;
        let session = sessions.iter().find(|s| s.id == id).ok_or(IdentityError::SessionNotFound)?;
        let account = c.ports.accounts.find_by_id(session.account_id).await?;
        c.ports.sessions.delete(id).await?;
        let entry = AuditEntry::new(c.now(), AuditAction::SessionEnd.as_str(), Outcome::Success, request);
        let target = account.map_or_else(|| session.account_id.get().to_string(), |a| a.name.to_string());
        c.audit(entry.by(&auth.actor()).target(target)).await?;
        Ok(())
    }
}
