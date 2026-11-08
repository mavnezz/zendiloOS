//! Certificates kept in memory and a host that pretends to run acme.sh and
//! nginx, for tests here and in the API crate (feature `testing`).

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use crate::application::{
    AcmeHost, CertificateRepository, HostError, Lines, Order, PortsInUse, WebServerState,
};
use crate::domain::{
    AcmeAccount, Attempt, Certificate, Challenge, ChallengeMethod, NewAccount, NewCertificate, WebServer,
};

#[derive(Default)]
struct Stored {
    accounts: Vec<AcmeAccount>,
    challenges: Vec<Challenge>,
    certificates: Vec<Certificate>,
    web: WebServer,
    last_id: i64,
}

impl Stored {
    fn next_id(&mut self) -> i64 {
        self.last_id += 1;
        self.last_id
    }
}

#[derive(Default)]
pub struct MemoryCertificates {
    stored: Mutex<Stored>,
}

#[async_trait]
impl CertificateRepository for MemoryCertificates {
    async fn accounts(&self) -> Result<Vec<AcmeAccount>, PortError> {
        Ok(self.stored.lock().unwrap().accounts.clone())
    }

    async fn create_account(&self, account: &NewAccount, now: Timestamp) -> Result<AcmeAccount, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let made = AcmeAccount {
            id: stored.next_id(),
            name: account.name.clone(),
            server: account.server,
            email: account.email.clone(),
            created_at: now,
        };
        stored.accounts.push(made.clone());
        Ok(made)
    }

    async fn update_account(&self, id: i64, account: &NewAccount) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let Some(found) = stored.accounts.iter_mut().find(|a| a.id == id) else { return Ok(false) };
        found.name.clone_from(&account.name);
        found.server = account.server;
        found.email.clone_from(&account.email);
        Ok(true)
    }

    async fn delete_account(&self, id: i64) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let before = stored.accounts.len();
        stored.accounts.retain(|a| a.id != id);
        Ok(stored.accounts.len() < before)
    }

    async fn challenges(&self) -> Result<Vec<Challenge>, PortError> {
        Ok(self.stored.lock().unwrap().challenges.clone())
    }

    async fn create_challenge(
        &self,
        name: &str,
        method: &ChallengeMethod,
        now: Timestamp,
    ) -> Result<Challenge, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let made = Challenge {
            id: stored.next_id(),
            name: name.to_owned(),
            method: method.clone(),
            created_at: now,
        };
        stored.challenges.push(made.clone());
        Ok(made)
    }

    async fn update_challenge(
        &self,
        id: i64,
        name: &str,
        method: &ChallengeMethod,
    ) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let Some(found) = stored.challenges.iter_mut().find(|c| c.id == id) else { return Ok(false) };
        name.clone_into(&mut found.name);
        found.method = method.clone();
        Ok(true)
    }

    async fn delete_challenge(&self, id: i64) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let before = stored.challenges.len();
        stored.challenges.retain(|c| c.id != id);
        Ok(stored.challenges.len() < before)
    }

    async fn certificates(&self) -> Result<Vec<Certificate>, PortError> {
        Ok(self.stored.lock().unwrap().certificates.clone())
    }

    async fn create_certificate(
        &self,
        certificate: &NewCertificate,
        now: Timestamp,
    ) -> Result<Certificate, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let made = Certificate {
            id: stored.next_id(),
            domains: certificate.domains.clone(),
            key: certificate.key,
            account_id: certificate.account_id,
            challenge_id: certificate.challenge_id,
            auto_renew: certificate.auto_renew,
            created_at: now,
            issued_at: None,
            not_after: None,
            last_attempt_at: None,
            last_error: None,
            last_output: Vec::new(),
        };
        stored.certificates.push(made.clone());
        Ok(made)
    }

    async fn update_certificate(&self, id: i64, certificate: &NewCertificate) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let Some(found) = stored.certificates.iter_mut().find(|c| c.id == id) else { return Ok(false) };
        found.domains.clone_from(&certificate.domains);
        found.key = certificate.key;
        found.account_id = certificate.account_id;
        found.challenge_id = certificate.challenge_id;
        found.auto_renew = certificate.auto_renew;
        Ok(true)
    }

    async fn delete_certificate(&self, id: i64) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let before = stored.certificates.len();
        stored.certificates.retain(|c| c.id != id);
        Ok(stored.certificates.len() < before)
    }

    async fn record_attempt(&self, id: i64, attempt: &Attempt) -> Result<bool, PortError> {
        let mut stored = self.stored.lock().unwrap();
        let Some(found) = stored.certificates.iter_mut().find(|c| c.id == id) else { return Ok(false) };
        found.last_attempt_at = Some(attempt.at);
        found.last_output.clone_from(&attempt.output);
        match &attempt.outcome {
            Ok(not_after) => {
                found.issued_at = Some(attempt.at);
                found.not_after = Some(*not_after);
                found.last_error = None;
            }
            Err(reason) => found.last_error = Some(reason.clone()),
        }
        Ok(true)
    }

    async fn web_server(&self) -> Result<WebServer, PortError> {
        Ok(self.stored.lock().unwrap().web)
    }

    async fn save_web_server(&self, web: &WebServer) -> Result<(), PortError> {
        self.stored.lock().unwrap().web = *web;
        Ok(())
    }
}

/// How long a certificate the fake issues holds.
pub const ISSUED_FOR: Duration = Duration::from_secs(90 * 24 * 3600);

/// acme.sh and nginx, pretended: issuing writes two lines and succeeds
/// unless told to fail.
#[derive(Default)]
pub struct FakeAcmeHost {
    installed: AtomicBool,
    failure: Mutex<Option<String>>,
    /// nginx does not take a renewed certificate, for this reason.
    reload_failure: Mutex<Option<String>>,
    /// Confirming a change of the web server fails.
    confirm_fails: AtomicBool,
    pending_until: Mutex<Option<Timestamp>>,
    applied: Mutex<Option<WebServer>>,
    calls: Mutex<Vec<String>>,
    now: Mutex<Option<Timestamp>>,
}

impl FakeAcmeHost {
    pub fn installed() -> Self {
        let host = Self::default();
        host.installed.store(true, Ordering::SeqCst);
        host
    }

    /// Issuing fails from now on, with this reason.
    pub fn fail_with(&self, reason: Option<&str>) {
        *self.failure.lock().unwrap() = reason.map(str::to_owned);
    }

    /// nginx refuses to reload from now on, with this reason.
    pub fn fail_reload_with(&self, reason: Option<&str>) {
        *self.reload_failure.lock().unwrap() = reason.map(str::to_owned);
    }

    /// Confirming a change of the web server fails from now on.
    pub fn fail_confirm(&self) {
        self.confirm_fails.store(true, Ordering::SeqCst);
    }

    /// Forgets the calls so far.
    pub fn forget_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    /// The time issuing starts from (the fake's own clock).
    pub fn set_now(&self, now: Timestamp) {
        *self.now.lock().unwrap() = Some(now);
    }

    /// The change applied went back by itself: not confirmed in time.
    pub fn roll_back(&self) {
        *self.pending_until.lock().unwrap() = None;
        *self.applied.lock().unwrap() = None;
    }

    pub fn applied(&self) -> Option<WebServer> {
        *self.applied.lock().unwrap()
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn call(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl AcmeHost for FakeAcmeHost {
    async fn installed(&self, _context: &RequestContext) -> Result<bool, PortError> {
        Ok(self.installed.load(Ordering::SeqCst))
    }

    async fn install(&self, _context: &RequestContext) -> Result<(), HostError> {
        self.call("install".into());
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn issue(
        &self,
        order: &Order,
        lines: &dyn Lines,
        _context: &RequestContext,
    ) -> Result<Timestamp, HostError> {
        self.call(format!("issue {}", order.domains.join(",")));
        lines.line(&format!("Creating domain key for {}", order.domains[0]));
        if let Some(reason) = self.failure.lock().unwrap().clone() {
            lines.line(&reason);
            return Err(HostError::Failed(reason));
        }
        lines.line("Cert success.");
        let now = self.now.lock().unwrap().unwrap_or(Timestamp::from_unix_seconds(1_793_880_000));
        Ok(now.saturating_add(ISSUED_FOR))
    }

    async fn remove(&self, certificate_id: i64, _context: &RequestContext) -> Result<(), PortError> {
        self.call(format!("remove {certificate_id}"));
        Ok(())
    }

    async fn remove_account(&self, account_id: i64, _context: &RequestContext) -> Result<(), PortError> {
        self.call(format!("remove account {account_id}"));
        Ok(())
    }

    async fn web_state(&self, _context: &RequestContext) -> Result<WebServerState, PortError> {
        Ok(WebServerState { pending_until: *self.pending_until.lock().unwrap() })
    }

    async fn apply_web(
        &self,
        web: &WebServer,
        confirm_within: u16,
        _context: &RequestContext,
    ) -> Result<(), HostError> {
        let mut pending = self.pending_until.lock().unwrap();
        if pending.is_some() {
            return Err(HostError::Conflict);
        }
        self.call(format!("apply {:?} {} {:?}", web.http_port, web.https_port, web.certificate_id));
        let now = self.now.lock().unwrap().unwrap_or(Timestamp::from_unix_seconds(1_793_880_000));
        *pending = Some(now.saturating_add(Duration::from_secs(u64::from(confirm_within))));
        *self.applied.lock().unwrap() = Some(*web);
        Ok(())
    }

    async fn confirm_web(&self, _context: &RequestContext) -> Result<(), HostError> {
        if self.confirm_fails.load(Ordering::SeqCst) {
            return Err(HostError::Failed("cannot remove the settings from before".into()));
        }
        let mut pending = self.pending_until.lock().unwrap();
        if pending.take().is_none() {
            return Err(HostError::Refused("no change waits for confirmation".into()));
        }
        self.call("confirm".into());
        Ok(())
    }

    async fn reload_certificate(&self, _context: &RequestContext) -> Result<(), HostError> {
        self.call("reload".into());
        match self.reload_failure.lock().unwrap().clone() {
            Some(reason) => Err(HostError::Failed(reason)),
            None => Ok(()),
        }
    }
}

/// Ports something listens on.
#[derive(Default)]
pub struct FakePorts(pub Mutex<BTreeSet<u16>>);

impl FakePorts {
    pub fn listening(ports: &[u16]) -> Self {
        Self(Mutex::new(ports.iter().copied().collect()))
    }
}

#[async_trait]
impl PortsInUse for FakePorts {
    async fn is_listening(&self, port: u16) -> Result<bool, PortError> {
        Ok(self.0.lock().unwrap().contains(&port))
    }
}
