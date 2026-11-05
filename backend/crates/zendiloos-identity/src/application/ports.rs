//! What this context needs from the outside world.

use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Secret, Timestamp};

use crate::domain::{
    Account, AccountId, AccountName, NewAccount, NewSession, PasswordHash, Session, SessionId, SetupCode,
    TokenHash,
};

#[async_trait]
pub trait AccountRepository: Send + Sync {
    async fn any_admin(&self) -> Result<bool, PortError>;

    async fn find_by_id(&self, id: AccountId) -> Result<Option<Account>, PortError>;

    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, PortError>;

    /// Stores `account` only if no administrator exists yet, atomically, so
    /// two concurrent setups cannot both succeed. `None` if one existed.
    async fn create_first_admin(&self, account: NewAccount) -> Result<Option<Account>, PortError>;

    /// Replaces the password hash and clears `password_change_required`.
    async fn update_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError>;
}

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, session: NewSession) -> Result<SessionId, PortError>;

    async fn find_by_token_hash(&self, hash: &TokenHash) -> Result<Option<Session>, PortError>;

    async fn touch(&self, id: SessionId, at: Timestamp) -> Result<(), PortError>;

    async fn delete(&self, id: SessionId) -> Result<(), PortError>;

    /// Ends every session of `account` except `keep`. Returns how many ended.
    async fn delete_others(&self, account: AccountId, keep: SessionId) -> Result<u64, PortError>;

    /// Removes sessions past their lifetime or idle for `idle_timeout`.
    async fn delete_inactive(&self, now: Timestamp, idle_timeout: Duration) -> Result<u64, PortError>;
}

#[async_trait]
pub trait PasswordHasher: Send + Sync {
    async fn hash(&self, password: &Secret) -> Result<PasswordHash, PortError>;

    /// Checks `password` against `hash`. With `None` it takes as long as a
    /// real check and returns `false`, so whether an account exists cannot
    /// be told from the response time.
    async fn verify(&self, password: &Secret, hash: Option<&PasswordHash>) -> Result<bool, PortError>;
}

/// A new session token: the value for the cookie and the digest to store.
pub struct IssuedToken {
    pub token: Secret,
    pub hash: TokenHash,
}

/// Random tokens. Implementations must use a cryptographically secure
/// random source.
pub trait SessionTokens: Send + Sync {
    fn issue(&self) -> IssuedToken;

    /// The digest under which [`SessionTokens::issue`] stored `token`.
    fn digest(&self, token: &Secret) -> TokenHash;

    fn csrf_token(&self) -> Secret;
}

/// Where the setup code lives while the initial setup is pending.
#[async_trait]
pub trait SetupCodes: Send + Sync {
    async fn current(&self) -> Result<Option<SetupCode>, PortError>;

    /// Returns the current code, creating a random one if there is none.
    async fn issue(&self) -> Result<SetupCode, PortError>;

    /// Removes the code; a no-op if there is none.
    async fn consume(&self) -> Result<(), PortError>;
}
