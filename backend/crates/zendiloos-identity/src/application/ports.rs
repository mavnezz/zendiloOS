//! What this context needs from the outside world.

use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{Actor, PortError, RequestContext, Secret, Timestamp};

use crate::domain::{
    Account, AccountId, AccountName, AddressRange, BlockedAddress, Group, GroupId, GroupName, NewAccount,
    NewGroup, NewSession, PasswordHash, PasswordPolicy, Preferences, SecuritySettings, Session,
    SessionDetails, SessionId, SetupCode, TokenHash,
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

    /// All accounts, ordered by name.
    async fn list(&self) -> Result<Vec<Account>, PortError>;

    /// `None` if the name is taken.
    async fn create(&self, account: NewAccount) -> Result<Option<Account>, PortError>;

    /// Stores full name, role, `disabled` and `updated_at` of `account`.
    async fn update(&self, account: &Account) -> Result<(), PortError>;

    /// Replaces the password hash and sets `password_change_required`.
    async fn reset_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError>;

    /// Deletes the account with its sessions and group memberships.
    async fn delete(&self, id: AccountId) -> Result<(), PortError>;

    /// The account must choose a new password at its next login.
    async fn require_password_change(&self, id: AccountId) -> Result<(), PortError>;

    /// The password policy; the defaults until an administrator sets one.
    async fn password_policy(&self) -> Result<PasswordPolicy, PortError>;

    async fn set_password_policy(&self, policy: &PasswordPolicy) -> Result<(), PortError>;
}

#[async_trait]
pub trait GroupRepository: Send + Sync {
    /// All groups, ordered by name.
    async fn list(&self) -> Result<Vec<Group>, PortError>;

    async fn find_by_id(&self, id: GroupId) -> Result<Option<Group>, PortError>;

    /// `None` if the name is taken.
    async fn create(&self, group: NewGroup) -> Result<Option<Group>, PortError>;

    /// Stores description, members and `updated_at` of `group`.
    async fn update(&self, group: &Group) -> Result<(), PortError>;

    async fn delete(&self, id: GroupId) -> Result<(), PortError>;
}

/// Whether a name could be used on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Creation {
    Created,
    /// The operating system already has a user or group of that name.
    NameTaken,
}

/// The Linux users and groups behind accounts and groups, and their Samba
/// entries; needs the privileged agent. Only users and groups created
/// through this port can be changed through it.
#[async_trait]
pub trait SystemAccounts: Send + Sync {
    async fn create_user(&self, name: &AccountName, context: &RequestContext) -> Result<Creation, PortError>;

    async fn delete_user(&self, name: &AccountName, context: &RequestContext) -> Result<(), PortError>;

    /// The password for SMB. The web login keeps its own hash.
    async fn set_password(
        &self,
        name: &AccountName,
        password: &Secret,
        context: &RequestContext,
    ) -> Result<(), PortError>;

    async fn set_enabled(
        &self,
        name: &AccountName,
        enabled: bool,
        context: &RequestContext,
    ) -> Result<(), PortError>;

    async fn create_group(&self, name: &GroupName, context: &RequestContext) -> Result<Creation, PortError>;

    async fn delete_group(&self, name: &GroupName, context: &RequestContext) -> Result<(), PortError>;

    async fn set_group_members(
        &self,
        group: &GroupName,
        members: &[AccountName],
        context: &RequestContext,
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

    /// Ends every session of `account`. Returns how many ended.
    async fn delete_all(&self, account: AccountId) -> Result<u64, PortError>;

    /// Removes sessions past their lifetime or idle for `idle_timeout`.
    async fn delete_inactive(&self, now: Timestamp, idle_timeout: Duration) -> Result<u64, PortError>;

    /// Every stored session, newest first; ended ones may be among them.
    async fn list(&self) -> Result<Vec<SessionDetails>, PortError>;
}

/// The settings of [`SecuritySettings`] and the addresses the web
/// interface refuses.
#[async_trait]
pub trait SecurityRepository: Send + Sync {
    /// The defaults until an administrator sets them.
    async fn settings(&self) -> Result<SecuritySettings, PortError>;

    async fn save_settings(&self, settings: &SecuritySettings) -> Result<(), PortError>;

    /// Every block, the newest first; those that are over may be among them.
    async fn blocked(&self) -> Result<Vec<BlockedAddress>, PortError>;

    /// Adds `block`, or replaces the one for the same range.
    async fn block(&self, block: &BlockedAddress) -> Result<(), PortError>;

    /// `false` if `range` was not blocked.
    async fn unblock(&self, range: &AddressRange) -> Result<bool, PortError>;

    /// Removes blocks that are over. Returns how many.
    async fn delete_expired(&self, now: Timestamp) -> Result<u64, PortError>;
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

/// Each account's [`Preferences`].
#[async_trait]
pub trait PreferenceRepository: Send + Sync {
    /// Nothing chosen for an account that never saved any.
    async fn load(&self, account: AccountId) -> Result<Preferences, PortError>;

    async fn save(&self, account: AccountId, preferences: Preferences) -> Result<(), PortError>;
}

/// Told about changes to groups that others keep state about. The
/// Sharing context gives members of groups denied a share an ACL entry of
/// their own, which has to follow the group's members.
#[async_trait]
pub trait GroupObserver: Send + Sync {
    /// The members of `group` changed; `before` were its members until now.
    async fn members_changed(
        &self,
        group: GroupId,
        before: &[AccountId],
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), PortError>;

    /// `group` is about to be deleted; it and its members still exist.
    async fn deleting(
        &self,
        group: GroupId,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), PortError>;
}
