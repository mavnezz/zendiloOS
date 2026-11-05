//! In-memory implementations of every port of this context, for tests here
//! and in the API crate (feature `testing`).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{AuditEntry, AuditLog, Clock, PortError, Secret, Timestamp};

use crate::application::ports::{
    AccountRepository, IssuedToken, PasswordHasher, SessionRepository, SessionTokens, SetupCodes,
};
use crate::application::{IdentityConfig, IdentityPorts, IdentityUseCases};
use crate::domain::{
    Account, AccountId, AccountName, NewAccount, NewSession, PasswordHash, Role, Session, SessionId,
    SetupCode, TokenHash,
};

/// The code [`MemorySetupCodes`] issues.
pub const SETUP_CODE: &str = "ABCD-EFGH-JKLM";

#[derive(Default)]
pub struct InMemoryAccounts {
    accounts: Mutex<Vec<Account>>,
}

impl InMemoryAccounts {
    pub fn all(&self) -> Vec<Account> {
        self.accounts.lock().unwrap().clone()
    }

    pub fn insert(&self, new: NewAccount) -> Account {
        let mut accounts = self.accounts.lock().unwrap();
        let account = Account {
            id: AccountId::new(i64::try_from(accounts.len()).unwrap() + 1),
            name: new.name,
            full_name: new.full_name,
            role: new.role,
            password_hash: new.password_hash,
            password_change_required: new.password_change_required,
            disabled: false,
            created_at: new.created_at,
            updated_at: new.created_at,
        };
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
            account.updated_at = at;
        });
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemorySessions {
    sessions: Mutex<Vec<Session>>,
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

    async fn delete_inactive(&self, now: Timestamp, idle_timeout: Duration) -> Result<u64, PortError> {
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|s| now < s.expires_at && now.duration_since(s.last_seen_at) < idle_timeout);
        Ok(u64::try_from(before - sessions.len()).unwrap())
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

/// The use cases wired to in-memory ports, with handles to inspect them.
pub struct TestIdentity {
    pub use_cases: IdentityUseCases,
    pub accounts: Arc<InMemoryAccounts>,
    pub sessions: Arc<InMemorySessions>,
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
        let sessions = Arc::new(InMemorySessions::default());
        let setup_codes = Arc::new(MemorySetupCodes::default());
        let audit = Arc::new(MemoryAudit::default());
        let clock = Arc::new(ManualClock::default());
        let ports = IdentityPorts {
            accounts: accounts.clone(),
            sessions: sessions.clone(),
            hasher: Arc::new(FakeHasher),
            tokens: Arc::new(FakeTokens::default()),
            setup_codes: setup_codes.clone(),
            audit: audit.clone(),
            clock: clock.clone(),
        };
        Self {
            use_cases: IdentityUseCases::new(ports, config),
            accounts,
            sessions,
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
            role,
            password_hash: FakeHasher::hash_of(password),
            password_change_required: false,
            created_at: self.clock.now(),
        })
    }
}

impl Default for TestIdentity {
    fn default() -> Self {
        Self::new()
    }
}
