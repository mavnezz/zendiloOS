use std::net::IpAddr;
use std::sync::Arc;

use zendiloos_kernel::{PortError, Timestamp};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{AccountName, BlockedAddress, Permission, SecuritySettings, SessionId};

/// Query: the security settings. Needs `system.manage`.
#[derive(Clone)]
pub struct GetSecurity {
    context: Arc<Context>,
}

impl GetSecurity {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<SecuritySettings, IdentityError> {
        Context::require(auth, Permission::SystemManage)?;
        Ok(self.context.defences().await?.settings.clone())
    }
}

/// Query: the blocks in force, the newest first.
#[derive(Clone)]
pub struct ListBlockedAddresses {
    context: Arc<Context>,
}

impl ListBlockedAddresses {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Vec<BlockedAddress>, IdentityError> {
        Context::require(auth, Permission::SystemManage)?;
        let c = &self.context;
        let now = c.now();
        let defences = c.defences().await?;
        Ok(defences.blocked.iter().filter(|block| !block.is_over(now)).cloned().collect())
    }
}

/// Query, for every request: whether the web interface refuses the
/// address it comes from. Answered from memory.
#[derive(Clone)]
pub struct CheckAddress {
    context: Arc<Context>,
}

impl CheckAddress {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn is_blocked(&self, ip: Option<IpAddr>) -> Result<bool, PortError> {
        let c = &self.context;
        Ok(c.defences().await?.blocks(ip, c.now()))
    }
}

/// A session that has not ended, as administrators see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSession {
    pub id: SessionId,
    pub account: AccountName,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    /// The caller's own.
    pub current: bool,
}

/// Query: the sessions of every account that have not ended, the most
/// recently active first.
#[derive(Clone)]
pub struct ListSessions {
    context: Arc<Context>,
}

impl ListSessions {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Vec<ActiveSession>, IdentityError> {
        Context::require(auth, Permission::SystemManage)?;
        let c = &self.context;
        let now = c.now();
        let idle_timeout = c.session_policy().await?.idle_timeout;
        let accounts = c.ports.accounts.list().await?;
        let mut sessions: Vec<ActiveSession> = c
            .ports
            .sessions
            .list()
            .await?
            .into_iter()
            .filter(|session| session.is_active(now, idle_timeout))
            .filter_map(|session| {
                let account = accounts.iter().find(|a| a.id == session.account_id)?;
                Some(ActiveSession {
                    id: session.id,
                    account: account.name.clone(),
                    client_ip: session.client_ip,
                    user_agent: session.user_agent,
                    created_at: session.created_at,
                    last_seen_at: session.last_seen_at,
                    current: session.id == auth.session_id,
                })
            })
            .collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_seen_at));
        Ok(sessions)
    }
}
