use std::net::IpAddr;

use async_trait::async_trait;

use crate::{PortError, RequestContext, Timestamp};

/// The signed-in account on whose behalf something happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub account_id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// The attempt was made and failed, e.g. a wrong password.
    Failure,
    /// The attempt was not even made, e.g. throttled or not permitted.
    Denied,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
            Outcome::Denied => "denied",
        }
    }
}

/// One line in the audit log, written by any context. Never contains
/// secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub at: Timestamp,
    pub account_id: Option<i64>,
    /// Account name at the time; for failed logins the name that was tried.
    pub actor: Option<String>,
    pub client_ip: Option<IpAddr>,
    /// Dotted, stable name of what happened, e.g. `identity.login`.
    pub action: &'static str,
    pub outcome: Outcome,
    /// Short machine-readable reason, e.g. `wrong_password`.
    pub reason: Option<&'static str>,
    pub request_id: Option<String>,
}

impl AuditEntry {
    /// Names typed into a login form are attacker-controlled; keep them short.
    const MAX_ACTOR_CHARS: usize = 64;

    pub fn new(at: Timestamp, action: &'static str, outcome: Outcome, request: &RequestContext) -> Self {
        Self {
            at,
            account_id: None,
            actor: None,
            client_ip: request.client_ip,
            action,
            outcome,
            reason: None,
            request_id: request.request_id.clone(),
        }
    }

    pub fn by(mut self, actor: &Actor) -> Self {
        self.account_id = Some(actor.account_id);
        self.actor = Some(actor.name.clone());
        self
    }

    /// A name that is not (yet) a known account, such as one typed into the
    /// login form.
    pub fn actor(mut self, name: &str) -> Self {
        self.actor = Some(name.chars().take(Self::MAX_ACTOR_CHARS).collect());
        self
    }

    pub fn reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }
}

/// Where audit entries are kept. A port shared by all contexts.
#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> AuditEntry {
        AuditEntry::new(
            Timestamp::from_unix_seconds(0),
            "test.action",
            Outcome::Failure,
            &RequestContext::default(),
        )
    }

    #[test]
    fn truncates_untrusted_actor_names() {
        assert_eq!(entry().actor(&"x".repeat(1000)).actor.unwrap().len(), 64);
    }

    #[test]
    fn records_the_acting_account() {
        let entry = entry().by(&Actor { account_id: 7, name: "alice".to_owned() });
        assert_eq!((entry.account_id, entry.actor.as_deref()), (Some(7), Some("alice")));
    }
}
