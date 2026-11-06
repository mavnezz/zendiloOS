//! In-memory implementations of every port of this context, for tests here
//! and in the API crate (feature `testing`).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, AuditLog, Clock, PortError, Secret, Timestamp};

use zendiloos_kernel::RequestContext;

use crate::application::ports::{
    AccountRepository, Creation, GroupObserver, GroupRepository, IssuedToken, PasswordHasher,
    PreferenceRepository, SecurityRepository, SessionRepository, SessionTokens, SetupCodes, SystemAccounts,
};
use crate::application::{IdentityConfig, IdentityPorts, IdentityUseCases};
use crate::domain::{
    Account, AccountId, AccountName, AddressRange, BlockedAddress, Group, GroupId, GroupName, NewAccount,
    NewGroup, NewSession, PasswordHash, Preferences, Role, SecuritySettings, Session, SessionDetails,
    SessionId, SetupCode, TokenHash,
};

/// The code [`MemorySetupCodes`] issues.
pub const SETUP_CODE: &str = "ABCD-EFGH-JKLM";

#[derive(Default)]
pub struct InMemoryAccounts {
    accounts: Mutex<Vec<Account>>,
    policy: Mutex<Option<crate::domain::PasswordPolicy>>,
    /// IDs are never reused, as in the database.
    last_id: AtomicI64,
    /// Group memberships go with a deleted account, as in the database.
    groups: Mutex<Option<Arc<InMemoryGroups>>>,
}

impl InMemoryAccounts {
    pub fn all(&self) -> Vec<Account> {
        self.accounts.lock().unwrap().clone()
    }

    pub fn insert(&self, new: NewAccount) -> Account {
        let mut accounts = self.accounts.lock().unwrap();
        let account = new.stored(AccountId::new(self.last_id.fetch_add(1, Ordering::Relaxed) + 1));
        accounts.push(account.clone());
        account
    }

    pub fn update(&self, id: AccountId, change: impl FnOnce(&mut Account)) {
        let mut accounts = self.accounts.lock().unwrap();
        change(accounts.iter_mut().find(|a| a.id == id).expect("account exists"));
    }
}

#[async_trait]
impl AccountRepository for InMemoryAccounts {
    async fn any_admin(&self) -> Result<bool, PortError> {
        Ok(self.accounts.lock().unwrap().iter().any(|a| a.role == Role::Admin))
    }

    async fn find_by_id(&self, id: AccountId) -> Result<Option<Account>, PortError> {
        Ok(self.accounts.lock().unwrap().iter().find(|a| a.id == id).cloned())
    }

    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, PortError> {
        Ok(self.accounts.lock().unwrap().iter().find(|a| &a.name == name).cloned())
    }

    async fn create_first_admin(&self, account: NewAccount) -> Result<Option<Account>, PortError> {
        if self.any_admin().await? {
            return Ok(None);
        }
        Ok(Some(self.insert(account)))
    }

    async fn update_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError> {
        self.update(id, |account| {
            account.password_hash = hash.clone();
            account.password_change_required = false;
            account.password_changed_at = at;
            account.updated_at = at;
        });
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Account>, PortError> {
        let mut accounts = self.all();
        accounts.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        Ok(accounts)
    }

    async fn create(&self, account: NewAccount) -> Result<Option<Account>, PortError> {
        if self.find_by_name(&account.name).await?.is_some() {
            return Ok(None);
        }
        Ok(Some(self.insert(account)))
    }

    async fn update(&self, account: &Account) -> Result<(), PortError> {
        self.update(account.id, |stored| {
            stored.full_name.clone_from(&account.full_name);
            stored.description.clone_from(&account.description);
            stored.email.clone_from(&account.email);
            stored.role = account.role;
            stored.disabled = account.disabled;
            stored.password_locked = account.password_locked;
            stored.password_change_required = account.password_change_required;
            stored.updated_at = account.updated_at;
        });
        Ok(())
    }

    async fn reset_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError> {
        self.update(id, |account| {
            account.password_hash = hash.clone();
            account.password_change_required = !account.password_locked;
            account.password_changed_at = at;
            account.updated_at = at;
        });
        Ok(())
    }

    async fn delete(&self, id: AccountId) -> Result<(), PortError> {
        self.accounts.lock().unwrap().retain(|a| a.id != id);
        if let Some(groups) = self.groups.lock().unwrap().as_ref() {
            groups.remove_member(id);
        }
        Ok(())
    }

    async fn require_password_change(&self, id: AccountId) -> Result<(), PortError> {
        self.update(id, |account| account.password_change_required = true);
        Ok(())
    }

    async fn password_policy(&self) -> Result<crate::domain::PasswordPolicy, PortError> {
        Ok(self.policy.lock().unwrap().unwrap_or_default())
    }

    async fn set_password_policy(&self, policy: &crate::domain::PasswordPolicy) -> Result<(), PortError> {
        *self.policy.lock().unwrap() = Some(*policy);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryGroups {
    groups: Mutex<Vec<Group>>,
    last_id: AtomicI64,
}

impl InMemoryGroups {
    pub fn all(&self) -> Vec<Group> {
        self.groups.lock().unwrap().clone()
    }

    fn remove_member(&self, id: AccountId) {
        for group in self.groups.lock().unwrap().iter_mut() {
            group.members.retain(|&member| member != id);
        }
    }
}

#[async_trait]
impl GroupRepository for InMemoryGroups {
    async fn list(&self) -> Result<Vec<Group>, PortError> {
        let mut groups = self.all();
        groups.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        Ok(groups)
    }

    async fn find_by_id(&self, id: GroupId) -> Result<Option<Group>, PortError> {
        Ok(self.groups.lock().unwrap().iter().find(|g| g.id == id).cloned())
    }

    async fn create(&self, new: NewGroup) -> Result<Option<Group>, PortError> {
        let mut groups = self.groups.lock().unwrap();
        if groups.iter().any(|g| g.name == new.name) {
            return Ok(None);
        }
        let group = Group {
            id: GroupId::new(self.last_id.fetch_add(1, Ordering::Relaxed) + 1),
            name: new.name,
            description: new.description,
            members: new.members,
            created_at: new.created_at,
            updated_at: new.created_at,
        };
        groups.push(group.clone());
        Ok(Some(group))
    }

    async fn update(&self, group: &Group) -> Result<(), PortError> {
        let mut groups = self.groups.lock().unwrap();
        let stored = groups.iter_mut().find(|g| g.id == group.id).expect("group exists");
        stored.description.clone_from(&group.description);
        stored.members.clone_from(&group.members);
        stored.updated_at = group.updated_at;
        Ok(())
    }

    async fn delete(&self, id: GroupId) -> Result<(), PortError> {
        self.groups.lock().unwrap().retain(|g| g.id != id);
        Ok(())
    }
}

/// Records what would happen on the host. Names in `taken` exist there
/// already; `fail_passwords` makes setting a password fail.
#[derive(Default)]
pub struct FakeSystemAccounts {
    calls: Mutex<Vec<String>>,
    pub taken: Mutex<Vec<String>>,
    pub fail_passwords: std::sync::atomic::AtomicBool,
}

impl FakeSystemAccounts {
    /// Calls so far, e.g. `create_user alice`; passwords are left out.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn take(&self, name: &str) -> Creation {
        let mut taken = self.taken.lock().unwrap();
        if taken.iter().any(|n| n == name) {
            return Creation::NameTaken;
        }
        taken.push(name.to_owned());
        Creation::Created
    }
}

#[async_trait]
impl SystemAccounts for FakeSystemAccounts {
    async fn create_user(&self, name: &AccountName, _: &RequestContext) -> Result<Creation, PortError> {
        self.record(format!("create_user {name}"));
        Ok(self.take(name.as_str()))
    }

    async fn delete_user(&self, name: &AccountName, _: &RequestContext) -> Result<(), PortError> {
        self.record(format!("delete_user {name}"));
        self.taken.lock().unwrap().retain(|n| n != name.as_str());
        Ok(())
    }

    async fn set_password(
        &self,
        name: &AccountName,
        _: &Secret,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        self.record(format!("set_password {name}"));
        if self.fail_passwords.load(Ordering::Relaxed) {
            return Err(PortError::new("privileged agent", "smbpasswd failed"));
        }
        Ok(())
    }

    async fn set_enabled(
        &self,
        name: &AccountName,
        enabled: bool,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        self.record(format!("set_enabled {name} {enabled}"));
        Ok(())
    }

    async fn create_group(&self, name: &GroupName, _: &RequestContext) -> Result<Creation, PortError> {
        self.record(format!("create_group {name}"));
        Ok(self.take(name.as_str()))
    }

    async fn delete_group(&self, name: &GroupName, _: &RequestContext) -> Result<(), PortError> {
        self.record(format!("delete_group {name}"));
        self.taken.lock().unwrap().retain(|n| n != name.as_str());
        Ok(())
    }

    async fn set_group_members(
        &self,
        group: &GroupName,
        members: &[AccountName],
        _: &RequestContext,
    ) -> Result<(), PortError> {
        let members: Vec<&str> = members.iter().map(AccountName::as_str).collect();
        self.record(format!("set_group_members {group} [{}]", members.join(",")));
        Ok(())
    }
}

/// A session's address and user agent, by its id.
type Client = (SessionId, Option<std::net::IpAddr>, Option<String>);

#[derive(Default)]
pub struct InMemorySessions {
    sessions: Mutex<Vec<Session>>,
    /// Address and user agent of each, by id.
    clients: Mutex<Vec<Client>>,
    last_id: AtomicI64,
}

impl InMemorySessions {
    pub fn all(&self) -> Vec<Session> {
        self.sessions.lock().unwrap().clone()
    }
}

#[async_trait]
impl SessionRepository for InMemorySessions {
    async fn create(&self, session: NewSession) -> Result<SessionId, PortError> {
        let id = SessionId::new(self.last_id.fetch_add(1, Ordering::Relaxed) + 1);
        self.clients.lock().unwrap().push((id, session.client_ip, session.user_agent.clone()));
        self.sessions.lock().unwrap().push(Session {
            id,
            account_id: session.account_id,
            token_hash: session.token_hash,
            csrf_token: session.csrf_token,
            created_at: session.created_at,
            last_seen_at: session.created_at,
            expires_at: session.expires_at,
        });
        Ok(id)
    }

    async fn find_by_token_hash(&self, hash: &TokenHash) -> Result<Option<Session>, PortError> {
        Ok(self.sessions.lock().unwrap().iter().find(|s| &s.token_hash == hash).cloned())
    }

    async fn touch(&self, id: SessionId, at: Timestamp) -> Result<(), PortError> {
        if let Some(session) = self.sessions.lock().unwrap().iter_mut().find(|s| s.id == id) {
            session.last_seen_at = at;
        }
        Ok(())
    }

    async fn delete(&self, id: SessionId) -> Result<(), PortError> {
        self.sessions.lock().unwrap().retain(|s| s.id != id);
        Ok(())
    }

    async fn delete_others(&self, account: AccountId, keep: SessionId) -> Result<u64, PortError> {
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|s| s.account_id != account || s.id == keep);
        Ok(u64::try_from(before - sessions.len()).unwrap())
    }

    async fn delete_all(&self, account: AccountId) -> Result<u64, PortError> {
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|s| s.account_id != account);
        Ok(u64::try_from(before - sessions.len()).unwrap())
    }

    async fn delete_inactive(&self, now: Timestamp, idle_timeout: Duration) -> Result<u64, PortError> {
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|s| now < s.expires_at && now.duration_since(s.last_seen_at) < idle_timeout);
        Ok(u64::try_from(before - sessions.len()).unwrap())
    }

    async fn list(&self) -> Result<Vec<SessionDetails>, PortError> {
        let clients = self.clients.lock().unwrap();
        let mut list: Vec<SessionDetails> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|s| {
                let client = clients.iter().find(|(id, _, _)| *id == s.id);
                SessionDetails {
                    id: s.id,
                    account_id: s.account_id,
                    client_ip: client.and_then(|(_, ip, _)| *ip),
                    user_agent: client.and_then(|(_, _, ua)| ua.clone()),
                    created_at: s.created_at,
                    last_seen_at: s.last_seen_at,
                    expires_at: s.expires_at,
                }
            })
            .collect();
        list.reverse();
        Ok(list)
    }
}

/// Security settings and blocks in memory.
#[derive(Default)]
pub struct InMemorySecurity {
    settings: Mutex<Option<SecuritySettings>>,
    blocked: Mutex<Vec<BlockedAddress>>,
}

impl InMemorySecurity {
    pub fn blocked(&self) -> Vec<BlockedAddress> {
        self.blocked.lock().unwrap().clone()
    }
}

#[async_trait]
impl SecurityRepository for InMemorySecurity {
    async fn settings(&self) -> Result<SecuritySettings, PortError> {
        Ok(self.settings.lock().unwrap().clone().unwrap_or_default())
    }

    async fn save_settings(&self, settings: &SecuritySettings) -> Result<(), PortError> {
        *self.settings.lock().unwrap() = Some(settings.clone());
        Ok(())
    }

    async fn blocked(&self) -> Result<Vec<BlockedAddress>, PortError> {
        Ok(self.blocked.lock().unwrap().iter().rev().cloned().collect())
    }

    async fn block(&self, block: &BlockedAddress) -> Result<(), PortError> {
        let mut blocked = self.blocked.lock().unwrap();
        blocked.retain(|b| b.range != block.range);
        blocked.push(block.clone());
        Ok(())
    }

    async fn unblock(&self, range: &AddressRange) -> Result<bool, PortError> {
        let mut blocked = self.blocked.lock().unwrap();
        let before = blocked.len();
        blocked.retain(|b| b.range != *range);
        Ok(blocked.len() < before)
    }

    async fn delete_expired(&self, now: Timestamp) -> Result<u64, PortError> {
        let mut blocked = self.blocked.lock().unwrap();
        let before = blocked.len();
        blocked.retain(|b| !b.is_over(now));
        Ok(u64::try_from(before - blocked.len()).unwrap())
    }
}

/// "Hashes" by prefixing, so tests stay fast and readable.
pub struct FakeHasher;

impl FakeHasher {
    pub fn hash_of(password: &str) -> PasswordHash {
        PasswordHash::new(format!("fake${password}"))
    }
}

#[async_trait]
impl PasswordHasher for FakeHasher {
    async fn hash(&self, password: &Secret) -> Result<PasswordHash, PortError> {
        Ok(Self::hash_of(password.expose()))
    }

    async fn verify(&self, password: &Secret, hash: Option<&PasswordHash>) -> Result<bool, PortError> {
        Ok(hash.is_some_and(|hash| *hash == Self::hash_of(password.expose())))
    }
}

/// Issues `token-1`, `token-2`, … and `csrf-1`, `csrf-2`, …
#[derive(Default)]
pub struct FakeTokens {
    issued: AtomicU64,
    csrf: AtomicU64,
}

impl SessionTokens for FakeTokens {
    fn issue(&self) -> IssuedToken {
        let token = Secret::new(format!("token-{}", self.issued.fetch_add(1, Ordering::Relaxed) + 1));
        IssuedToken { hash: self.digest(&token), token }
    }

    fn digest(&self, token: &Secret) -> TokenHash {
        let mut digest = [0; 32];
        let bytes = token.expose().as_bytes();
        let len = bytes.len().min(32);
        digest[..len].copy_from_slice(&bytes[..len]);
        TokenHash::new(digest)
    }

    fn csrf_token(&self) -> Secret {
        Secret::new(format!("csrf-{}", self.csrf.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

/// Issues [`SETUP_CODE`].
#[derive(Default)]
pub struct MemorySetupCodes {
    code: Mutex<Option<String>>,
}

impl MemorySetupCodes {
    pub fn is_present(&self) -> bool {
        self.code.lock().unwrap().is_some()
    }
}

#[async_trait]
impl SetupCodes for MemorySetupCodes {
    async fn current(&self) -> Result<Option<SetupCode>, PortError> {
        Ok(self.code.lock().unwrap().as_deref().and_then(SetupCode::parse))
    }

    async fn issue(&self) -> Result<SetupCode, PortError> {
        let mut code = self.code.lock().unwrap();
        let code = code.get_or_insert_with(|| SETUP_CODE.to_owned());
        Ok(SetupCode::parse(code).expect("valid test code"))
    }

    async fn consume(&self) -> Result<(), PortError> {
        self.code.lock().unwrap().take();
        Ok(())
    }
}

#[derive(Default)]
pub struct MemoryAudit {
    entries: Mutex<Vec<AuditEntry>>,
}

impl MemoryAudit {
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuditLog for MemoryAudit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryPreferences(Mutex<std::collections::HashMap<AccountId, Preferences>>);

#[async_trait]
impl PreferenceRepository for InMemoryPreferences {
    async fn load(&self, account: AccountId) -> Result<Preferences, PortError> {
        Ok(self.0.lock().unwrap().get(&account).cloned().unwrap_or_default())
    }

    async fn save(&self, account: AccountId, preferences: Preferences) -> Result<(), PortError> {
        self.0.lock().unwrap().insert(account, preferences);
        Ok(())
    }
}

/// A clock that only moves when told to.
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub const START: i64 = 1_700_000_000;

    pub fn advance(&self, duration: Duration) {
        self.0.fetch_add(i64::try_from(duration.as_secs()).unwrap(), Ordering::Relaxed);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self(AtomicI64::new(Self::START))
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(self.0.load(Ordering::Relaxed))
    }
}

/// Records what it is told about groups, as `members <group> <before>` or
/// `deleting <group>`; refuses deletions while `refuse` is set.
#[derive(Default)]
pub struct RecordingGroupObserver {
    pub events: Mutex<Vec<String>>,
    pub refuse: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl GroupObserver for RecordingGroupObserver {
    async fn members_changed(
        &self,
        group: GroupId,
        before: &[AccountId],
        _: &zendiloos_kernel::Actor,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        let before: Vec<String> = before.iter().map(|id| id.get().to_string()).collect();
        self.events.lock().unwrap().push(format!("members {} [{}]", group.get(), before.join(",")));
        Ok(())
    }

    async fn deleting(
        &self,
        group: GroupId,
        _: &zendiloos_kernel::Actor,
        _: &RequestContext,
    ) -> Result<(), PortError> {
        if self.refuse.load(Ordering::Relaxed) {
            return Err(PortError::new("sharing", "refused"));
        }
        self.events.lock().unwrap().push(format!("deleting {}", group.get()));
        Ok(())
    }
}

/// The use cases wired to in-memory ports, with handles to inspect them.
pub struct TestIdentity {
    pub use_cases: IdentityUseCases,
    pub accounts: Arc<InMemoryAccounts>,
    pub groups: Arc<InMemoryGroups>,
    pub group_observer: Arc<RecordingGroupObserver>,
    pub sessions: Arc<InMemorySessions>,
    pub security: Arc<InMemorySecurity>,
    pub system: Arc<FakeSystemAccounts>,
    pub setup_codes: Arc<MemorySetupCodes>,
    pub audit: Arc<MemoryAudit>,
    pub clock: Arc<ManualClock>,
}

impl TestIdentity {
    pub fn new() -> Self {
        Self::with_config(IdentityConfig::default())
    }

    pub fn with_config(config: IdentityConfig) -> Self {
        let accounts = Arc::new(InMemoryAccounts::default());
        let groups = Arc::new(InMemoryGroups::default());
        *accounts.groups.lock().unwrap() = Some(groups.clone());
        let sessions = Arc::new(InMemorySessions::default());
        let security = Arc::new(InMemorySecurity::default());
        let system = Arc::new(FakeSystemAccounts::default());
        let setup_codes = Arc::new(MemorySetupCodes::default());
        let audit = Arc::new(MemoryAudit::default());
        let clock = Arc::new(ManualClock::default());
        let group_observer = Arc::new(RecordingGroupObserver::default());
        let ports = IdentityPorts {
            accounts: accounts.clone(),
            groups: groups.clone(),
            group_observer: group_observer.clone(),
            sessions: sessions.clone(),
            security: security.clone(),
            preferences: Arc::new(InMemoryPreferences::default()),
            system: system.clone(),
            hasher: Arc::new(FakeHasher),
            tokens: Arc::new(FakeTokens::default()),
            setup_codes: setup_codes.clone(),
            audit: audit.clone(),
            clock: clock.clone(),
        };
        Self {
            use_cases: IdentityUseCases::new(ports, config),
            accounts,
            groups,
            group_observer,
            sessions,
            security,
            system,
            setup_codes,
            audit,
            clock,
        }
    }

    /// Stores an account directly, bypassing the use cases.
    pub fn add_account(&self, name: &str, password: &str, role: Role) -> Account {
        self.accounts.insert(NewAccount {
            name: AccountName::parse(name).unwrap(),
            full_name: None,
            description: None,
            email: None,
            role,
            password_hash: FakeHasher::hash_of(password),
            password_change_required: false,
            password_locked: false,
            created_at: self.clock.now(),
        })
    }
}

impl Default for TestIdentity {
    fn default() -> Self {
        Self::new()
    }
}
