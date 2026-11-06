use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use zendiloos_kernel::{Secret, Timestamp};

use super::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(i64);

impl SessionId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// SHA-256 digest of a session token. Only the digest is stored, so a copy
/// of the database does not contain usable tokens.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    pub const fn new(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHash(***)")
    }
}

/// How long sessions live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPolicy {
    /// A session ends after this long without requests.
    pub idle_timeout: Duration,
    /// A session ends this long after login, active or not.
    pub lifetime: Duration,
    /// `last_seen_at` is written at most this often, to spare the database.
    /// The effective idle timeout can therefore be longer by up to this much.
    pub touch_interval: Duration,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30 * 60),
            lifetime: Duration::from_secs(12 * 60 * 60),
            touch_interval: Duration::from_secs(60),
        }
    }
}

/// A login session.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,
    pub account_id: AccountId,
    pub token_hash: TokenHash,
    /// Must accompany every state-changing request of this session.
    pub csrf_token: Secret,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub expires_at: Timestamp,
}

impl Session {
    pub fn is_active(&self, now: Timestamp, policy: &SessionPolicy) -> bool {
        now < self.expires_at && now.duration_since(self.last_seen_at) < policy.idle_timeout
    }

    pub fn needs_touch(&self, now: Timestamp, policy: &SessionPolicy) -> bool {
        now.duration_since(self.last_seen_at) >= policy.touch_interval
    }
}

/// A session as administrators see it among all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetails {
    pub id: SessionId,
    pub account_id: AccountId,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub expires_at: Timestamp,
}

impl SessionDetails {
    pub fn is_active(&self, now: Timestamp, idle_timeout: Duration) -> bool {
        now < self.expires_at && now.duration_since(self.last_seen_at) < idle_timeout
    }
}

/// A session that is about to be stored.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub account_id: AccountId,
    pub token_hash: TokenHash,
    pub csrf_token: Secret,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    fn session(last_seen: i64, expires: i64) -> Session {
        Session {
            id: SessionId::new(1),
            account_id: AccountId::new(1),
            token_hash: TokenHash::new([0; 32]),
            csrf_token: Secret::new("csrf"),
            created_at: at(0),
            last_seen_at: at(last_seen),
            expires_at: at(expires),
        }
    }

    #[test]
    fn ends_after_the_idle_timeout() {
        let policy = SessionPolicy::default();
        let s = session(1000, 100_000);
        assert!(s.is_active(at(1000 + 30 * 60 - 1), &policy));
        assert!(!s.is_active(at(1000 + 30 * 60), &policy));
    }

    #[test]
    fn ends_at_the_end_of_its_lifetime_even_when_active() {
        let policy = SessionPolicy::default();
        let s = session(5000, 5001);
        assert!(s.is_active(at(5000), &policy));
        assert!(!s.is_active(at(5001), &policy));
    }

    #[test]
    fn is_touched_at_most_once_per_interval() {
        let policy = SessionPolicy::default();
        let s = session(1000, 100_000);
        assert!(!s.needs_touch(at(1059), &policy));
        assert!(s.needs_touch(at(1060), &policy));
    }
}
