//! Use cases: the ACME plugin installed, accounts, challenges and
//! certificates kept, certificates issued and renewed in jobs, and the web
//! interface's ports and certificate changed with confirmation.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_jobs::{Job, JobError, Jobs, Progress};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::domain::{
    AcmeAccount, AcmeServer, Attempt, Certificate, Challenge, ChallengeMethod, Invalid, KeyType, NewAccount,
    NewCertificate, WebServer, check_challenge_name,
};

/// Accounts, challenges, certificates and the web server's settings, kept.
#[async_trait]
pub trait CertificateRepository: Send + Sync {
    async fn accounts(&self) -> Result<Vec<AcmeAccount>, PortError>;
    async fn create_account(&self, account: &NewAccount, now: Timestamp) -> Result<AcmeAccount, PortError>;
    /// `false`: no such account.
    async fn update_account(&self, id: i64, account: &NewAccount) -> Result<bool, PortError>;
    async fn delete_account(&self, id: i64) -> Result<bool, PortError>;

    async fn challenges(&self) -> Result<Vec<Challenge>, PortError>;
    async fn create_challenge(
        &self,
        name: &str,
        method: &ChallengeMethod,
        now: Timestamp,
    ) -> Result<Challenge, PortError>;
    async fn update_challenge(
        &self,
        id: i64,
        name: &str,
        method: &ChallengeMethod,
    ) -> Result<bool, PortError>;
    async fn delete_challenge(&self, id: i64) -> Result<bool, PortError>;

    async fn certificates(&self) -> Result<Vec<Certificate>, PortError>;
    async fn create_certificate(
        &self,
        certificate: &NewCertificate,
        now: Timestamp,
    ) -> Result<Certificate, PortError>;
    async fn update_certificate(&self, id: i64, certificate: &NewCertificate) -> Result<bool, PortError>;
    async fn delete_certificate(&self, id: i64) -> Result<bool, PortError>;
    /// Records a try to issue or renew it.
    async fn record_attempt(&self, id: i64, attempt: &Attempt) -> Result<bool, PortError>;

    async fn web_server(&self) -> Result<WebServer, PortError>;
    async fn save_web_server(&self, web: &WebServer) -> Result<(), PortError>;
}

/// Where the lines acme.sh writes go while it runs.
pub trait Lines: Send + Sync {
    fn line(&self, line: &str);
}

/// What the host was asked to make, with the challenge's secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub certificate_id: i64,
    pub account_id: i64,
    pub server: AcmeServer,
    pub email: String,
    pub domains: Vec<String>,
    pub key: KeyType,
    pub method: ChallengeMethod,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host would not, in its words.
    #[error("{0}")]
    Refused(String),
    /// It tried and failed, in its words.
    #[error("{0}")]
    Failed(String),
    /// Another change of the web server waits for confirmation.
    #[error("another change waits for confirmation")]
    Conflict,
    #[error(transparent)]
    Port(#[from] PortError),
}

/// The web server's change waiting for confirmation, if one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebServerState {
    pub pending_until: Option<Timestamp>,
}

/// acme.sh and nginx on the host, through the system agent.
#[async_trait]
pub trait AcmeHost: Send + Sync {
    /// Whether acme.sh is installed.
    async fn installed(&self, context: &RequestContext) -> Result<bool, PortError>;
    /// Installs it from Debian, waiting until apt is done.
    async fn install(&self, context: &RequestContext) -> Result<(), HostError>;
    /// Issues (or renews) a certificate; until when it holds.
    async fn issue(
        &self,
        order: &Order,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<Timestamp, HostError>;
    /// Removes what acme.sh and nginx kept of a certificate.
    async fn remove(&self, certificate_id: i64, context: &RequestContext) -> Result<(), PortError>;
    /// Removes what acme.sh kept of an account: its key and settings.
    async fn remove_account(&self, account_id: i64, context: &RequestContext) -> Result<(), PortError>;
    async fn web_state(&self, context: &RequestContext) -> Result<WebServerState, PortError>;
    /// Applies the settings; they go back unless confirmed in time.
    async fn apply_web(
        &self,
        web: &WebServer,
        confirm_within: u16,
        context: &RequestContext,
    ) -> Result<(), HostError>;
    async fn confirm_web(&self, context: &RequestContext) -> Result<(), HostError>;
    /// The certificate the web interface shows (or a change waiting would)
    /// was renewed: nginx takes the new one.
    async fn reload_certificate(&self, context: &RequestContext) -> Result<(), HostError>;
}

/// Whether something listens on a port now.
#[async_trait]
pub trait PortsInUse: Send + Sync {
    async fn is_listening(&self, port: u16) -> Result<bool, PortError>;
}

#[derive(Debug, thiserror::Error)]
pub enum CertificatesError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Invalid(#[from] Invalid),
    /// Still used by a certificate, or by the web interface.
    #[error("still used by {0}")]
    InUse(&'static str),
    #[error("acme.sh is not installed")]
    NotInstalled,
    #[error("acme.sh is installed already")]
    Installed,
    #[error("port {0} is in use")]
    PortBusy(u16),
    #[error("another change waits for confirmation")]
    Pending,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    HostFailed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for CertificatesError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => Self::Refused(reason),
            HostError::Failed(reason) => Self::HostFailed(reason),
            HostError::Conflict => Self::Pending,
            HostError::Port(error) => Self::Port(error),
        }
    }
}

/// How long a change of the web server waits to be confirmed from the
/// browser at its new address before it goes back.
pub const CONFIRM_WITHIN_SECONDS: u16 = 90;

/// Lines acme.sh wrote kept with a certificate.
pub const OUTPUT_KEPT: usize = 200;

/// The plugin: whether acme.sh is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcmePlugin {
    pub installed: bool,
}

/// The web server's settings, and a change waiting for confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebServerSettings {
    pub current: WebServer,
    pub pending: Option<(WebServer, Timestamp)>,
}

pub struct CertificatesPorts {
    pub repository: Arc<dyn CertificateRepository>,
    pub host: Arc<dyn AcmeHost>,
    pub listening: Arc<dyn PortsInUse>,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct Certificates {
    ports: Arc<CertificatesPorts>,
    /// Settings applied and waiting for confirmation; saved once confirmed.
    pending: Arc<Mutex<Option<WebServer>>>,
    /// Certificates being issued now: not issued twice at once, nor deleted.
    issuing: Arc<Mutex<BTreeSet<i64>>>,
}

/// Takes a certificate out of [`Certificates::issuing`] when its issuance
/// ends, however it ends.
struct Issuing {
    set: Arc<Mutex<BTreeSet<i64>>>,
    id: i64,
}

impl Drop for Issuing {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.id);
    }
}

/// A job's output, forwarded and kept for the certificate.
struct JobLines {
    progress: Progress,
    kept: Mutex<Vec<String>>,
}

impl Lines for JobLines {
    fn line(&self, line: &str) {
        self.progress.line(line);
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.push(line.to_owned());
        let excess = kept.len().saturating_sub(OUTPUT_KEPT);
        kept.drain(..excess);
    }
}

impl Certificates {
    pub fn new(ports: CertificatesPorts) -> Self {
        Self {
            ports: Arc::new(ports),
            pending: Arc::new(Mutex::new(None)),
            issuing: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    fn now(&self) -> Timestamp {
        self.ports.clock.now()
    }

    async fn record(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.now(), action, outcome, context).by(actor).target(target.to_owned());
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "certificates: cannot write the audit log");
        }
    }

    // The plugin.

    pub async fn plugin(&self, context: &RequestContext) -> Result<AcmePlugin, CertificatesError> {
        Ok(AcmePlugin { installed: self.ports.host.installed(context).await? })
    }

    /// Installs acme.sh from Debian, in a job.
    pub async fn install(&self, actor: &Actor, context: &RequestContext) -> Result<Job, CertificatesError> {
        if self.ports.host.installed(context).await? {
            return Err(CertificatesError::Installed);
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start(
                "certificates.acme_install",
                Some("acme.sh".into()),
                Some(actor),
                move |progress| async move {
                    progress.step("packages", Some(0.05)).await;
                    let done = this.ports.host.install(&context).await;
                    let outcome = if done.is_ok() { Outcome::Success } else { Outcome::Failure };
                    this.record("certificates.acme_install", outcome, "acme.sh", &who, &context).await;
                    done.map_err(job_error)
                },
            )
            .await?;
        Ok(job)
    }

    async fn ensure_installed(&self, context: &RequestContext) -> Result<(), CertificatesError> {
        if self.ports.host.installed(context).await? { Ok(()) } else { Err(CertificatesError::NotInstalled) }
    }

    // Accounts.

    pub async fn accounts(&self) -> Result<Vec<AcmeAccount>, CertificatesError> {
        Ok(self.ports.repository.accounts().await?)
    }

    pub async fn create_account(
        &self,
        account: NewAccount,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<AcmeAccount, CertificatesError> {
        let account = account.check()?;
        let made = self.ports.repository.create_account(&account, self.now()).await?;
        // Its number may have been another's before: nothing acme.sh kept of
        // that one (its key at the authority) goes over to it.
        if let Err(error) = self.ports.host.remove_account(made.id, context).await {
            if let Err(error) = self.ports.repository.delete_account(made.id).await {
                tracing::error!(%error, id = made.id, "certificates: cannot take back the account made");
            }
            return Err(error.into());
        }
        self.record("certificates.account_create", Outcome::Success, &made.name, actor, context).await;
        Ok(made)
    }

    pub async fn update_account(
        &self,
        id: i64,
        account: NewAccount,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let account = account.check()?;
        if !self.ports.repository.update_account(id, &account).await? {
            return Err(CertificatesError::NotFound);
        }
        self.record("certificates.account_change", Outcome::Success, &account.name, actor, context).await;
        Ok(())
    }

    pub async fn delete_account(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        if self.ports.repository.certificates().await?.iter().any(|c| c.account_id == id) {
            return Err(CertificatesError::InUse("a certificate"));
        }
        if !self.ports.repository.delete_account(id).await? {
            return Err(CertificatesError::NotFound);
        }
        if let Err(error) = self.ports.host.remove_account(id, context).await {
            tracing::warn!(%error, id, "certificates: cannot remove what acme.sh kept of the account");
        }
        self.record("certificates.account_delete", Outcome::Success, &id.to_string(), actor, context).await;
        Ok(())
    }

    // Challenges.

    pub async fn challenges(&self) -> Result<Vec<Challenge>, CertificatesError> {
        Ok(self.ports.repository.challenges().await?)
    }

    pub async fn create_challenge(
        &self,
        name: &str,
        method: ChallengeMethod,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Challenge, CertificatesError> {
        let name = check_challenge_name(name)?;
        let method = method.check(None)?;
        let made = self.ports.repository.create_challenge(&name, &method, self.now()).await?;
        self.record("certificates.challenge_create", Outcome::Success, &name, actor, context).await;
        Ok(made)
    }

    /// Secret values left empty stay as they were.
    pub async fn update_challenge(
        &self,
        id: i64,
        name: &str,
        method: ChallengeMethod,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let name = check_challenge_name(name)?;
        let challenges = self.ports.repository.challenges().await?;
        let before = challenges.iter().find(|c| c.id == id).ok_or(CertificatesError::NotFound)?;
        let method = method.check(Some(&before.method))?;
        if !method.is_dns() {
            let certificates = self.ports.repository.certificates().await?;
            if certificates
                .iter()
                .any(|c| c.challenge_id == id && c.domains.iter().any(|d| d.starts_with("*.")))
            {
                return Err(CertificatesError::InUse("a wildcard certificate, which needs DNS"));
            }
        }
        if !self.ports.repository.update_challenge(id, &name, &method).await? {
            return Err(CertificatesError::NotFound);
        }
        self.record("certificates.challenge_change", Outcome::Success, &name, actor, context).await;
        Ok(())
    }

    pub async fn delete_challenge(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        if self.ports.repository.certificates().await?.iter().any(|c| c.challenge_id == id) {
            return Err(CertificatesError::InUse("a certificate"));
        }
        if !self.ports.repository.delete_challenge(id).await? {
            return Err(CertificatesError::NotFound);
        }
        self.record("certificates.challenge_delete", Outcome::Success, &id.to_string(), actor, context).await;
        Ok(())
    }

    // Certificates.

    pub async fn certificates(&self) -> Result<Vec<Certificate>, CertificatesError> {
        Ok(self.ports.repository.certificates().await?)
    }

    async fn checked(&self, certificate: NewCertificate) -> Result<NewCertificate, CertificatesError> {
        let repository = &self.ports.repository;
        if !repository.accounts().await?.iter().any(|a| a.id == certificate.account_id) {
            return Err(Invalid("there is no such account".into()).into());
        }
        let challenges = repository.challenges().await?;
        let challenge = challenges
            .iter()
            .find(|c| c.id == certificate.challenge_id)
            .ok_or_else(|| Invalid("there is no such challenge".into()))?;
        Ok(certificate.check(&challenge.method)?)
    }

    pub async fn create_certificate(
        &self,
        certificate: NewCertificate,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Certificate, CertificatesError> {
        let certificate = self.checked(certificate).await?;
        let made = self.ports.repository.create_certificate(&certificate, self.now()).await?;
        // Its number may have been another's before: nothing acme.sh or
        // nginx kept of that one (its key) goes over to it.
        if let Err(error) = self.ports.host.remove(made.id, context).await {
            if let Err(error) = self.ports.repository.delete_certificate(made.id).await {
                tracing::error!(%error, id = made.id, "certificates: cannot take back the certificate made");
            }
            return Err(error.into());
        }
        self.record("certificates.certificate_create", Outcome::Success, made.common_name(), actor, context)
            .await;
        Ok(made)
    }

    /// Takes effect at the next issuance or renewal.
    pub async fn update_certificate(
        &self,
        id: i64,
        certificate: NewCertificate,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let certificate = self.checked(certificate).await?;
        if !self.ports.repository.update_certificate(id, &certificate).await? {
            return Err(CertificatesError::NotFound);
        }
        let name = certificate.domains.first().cloned().unwrap_or_default();
        self.record("certificates.certificate_change", Outcome::Success, &name, actor, context).await;
        Ok(())
    }

    pub async fn delete_certificate(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let shown = self.ports.repository.web_server().await?.certificate_id == Some(id);
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|p| p.certificate_id == Some(id));
        if shown || pending {
            return Err(CertificatesError::InUse("the web interface"));
        }
        // Held until it is deleted: no issuance begins meanwhile.
        let _issuing = self.begin_issuing(id).ok_or(CertificatesError::InUse("an issuance under way"))?;
        if !self.ports.repository.delete_certificate(id).await? {
            return Err(CertificatesError::NotFound);
        }
        if let Err(error) = self.ports.host.remove(id, context).await {
            tracing::warn!(%error, id, "certificates: cannot remove what acme.sh kept");
        }
        self.record("certificates.certificate_delete", Outcome::Success, &id.to_string(), actor, context)
            .await;
        Ok(())
    }

    async fn order(&self, certificate: &Certificate) -> Result<Order, CertificatesError> {
        let repository = &self.ports.repository;
        let account = repository
            .accounts()
            .await?
            .into_iter()
            .find(|a| a.id == certificate.account_id)
            .ok_or(CertificatesError::NotFound)?;
        let challenge = repository
            .challenges()
            .await?
            .into_iter()
            .find(|c| c.id == certificate.challenge_id)
            .ok_or(CertificatesError::NotFound)?;
        Ok(Order {
            certificate_id: certificate.id,
            account_id: account.id,
            server: account.server,
            email: account.email,
            domains: certificate.domains.clone(),
            key: certificate.key,
            method: challenge.method,
        })
    }

    /// Marks certificate `id` as being issued; `None` if it is already.
    fn begin_issuing(&self, id: i64) -> Option<Issuing> {
        let fresh = self.issuing.lock().unwrap_or_else(PoisonError::into_inner).insert(id);
        fresh.then(|| Issuing { set: Arc::clone(&self.issuing), id })
    }

    /// Issues a certificate now (or again), in a job whose output is what
    /// acme.sh writes. `actor` `None`: the daily renewal, which notifies.
    /// One issuance of a certificate at a time.
    pub async fn issue(
        &self,
        id: i64,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) -> Result<Job, CertificatesError> {
        self.ensure_installed(context).await?;
        let certificate = self
            .ports
            .repository
            .certificates()
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(CertificatesError::NotFound)?;
        let order = self.order(&certificate).await?;
        if !order.method.is_dns() && !self.ports.repository.web_server().await?.answers_http_challenges() {
            return Err(CertificatesError::Refused(
                "an HTTP challenge needs the web interface to answer plain HTTP on port 80".into(),
            ));
        }
        let issuing = self
            .begin_issuing(id)
            .ok_or_else(|| CertificatesError::Refused("the certificate is being issued already".into()))?;
        let this = self.clone();
        let (who, context) = (actor.cloned(), context.clone());
        let name = certificate.common_name().to_owned();
        let job = self
            .ports
            .jobs
            .start("certificates.issue", Some(name.clone()), actor, move |progress| async move {
                let _issuing = issuing;
                progress.step("issuing", Some(0.05)).await;
                let lines = JobLines { progress: progress.clone(), kept: Mutex::new(Vec::new()) };
                let done = this.ports.host.issue(&order, &lines, &context).await;
                let output = lines.kept.into_inner().unwrap_or_else(PoisonError::into_inner);
                this.issued(&certificate, &name, done, output, who.as_ref(), &context).await
            })
            .await?;
        Ok(job)
    }

    /// Records what came of an issuance, has nginx take a renewed
    /// certificate it shows (or a change waiting would), and tells of
    /// renewals: one nginx did not take is told as that.
    async fn issued(
        &self,
        certificate: &Certificate,
        name: &str,
        done: Result<Timestamp, HostError>,
        output: Vec<String>,
        actor: Option<&Actor>,
        context: &RequestContext,
    ) -> Result<(), JobError> {
        let outcome = done.as_ref().map(|until| *until).map_err(ToString::to_string);
        let attempt = Attempt { at: self.now(), outcome: outcome.clone(), output };
        if let Err(error) = self.ports.repository.record_attempt(certificate.id, &attempt).await {
            tracing::error!(%error, "certificates: cannot record the issuance");
        }
        let automatic = actor.is_none();
        if let Some(actor) = actor {
            let result = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
            self.record("certificates.issue", result, name, actor, context).await;
        }
        match outcome {
            Ok(_) => {
                let shown = self.ports.repository.web_server().await.ok().and_then(|w| w.certificate_id);
                let waiting = self
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .and_then(|w| w.certificate_id);
                let reloaded = if shown == Some(certificate.id) || waiting == Some(certificate.id) {
                    self.ports.host.reload_certificate(context).await
                } else {
                    Ok(())
                };
                if let Err(error) = reloaded {
                    tracing::error!(%error, "certificates: nginx did not take the renewed certificate");
                    let told = NewNotification::new(Level::Error, "certificate_not_taken")
                        .with("name", name)
                        .with("reason", error.to_string());
                    self.ports.notifier.notify(told).await;
                    return Err(JobError::new(
                        "agent_failed",
                        Some(format!("the web interface did not take the new certificate: {error}")),
                    ));
                }
                if automatic {
                    let told = NewNotification::new(Level::Success, "certificate_renewed").with("name", name);
                    self.ports.notifier.notify(told).await;
                }
                Ok(())
            }
            Err(reason) => {
                if automatic {
                    let told = NewNotification::new(Level::Error, "certificate_renewal_failed")
                        .with("name", name)
                        .with("reason", reason.clone());
                    self.ports.notifier.notify(told).await;
                }
                Err(done.err().map_or_else(|| JobError::new("agent_failed", Some(reason)), job_error))
            }
        }
    }

    /// The daily look: certificates due are renewed, each in a job.
    pub async fn renew_due(&self) -> Result<Vec<Job>, CertificatesError> {
        let context = RequestContext::default();
        if !self.ports.host.installed(&context).await? {
            return Ok(Vec::new());
        }
        let now = self.now();
        let mut started = Vec::new();
        for certificate in self.ports.repository.certificates().await? {
            let issuing =
                self.issuing.lock().unwrap_or_else(PoisonError::into_inner).contains(&certificate.id);
            if !certificate.renewal_due(now) || issuing {
                continue;
            }
            match self.issue(certificate.id, None, &context).await {
                Ok(job) => started.push(job),
                Err(error) => {
                    tracing::warn!(%error, name = certificate.common_name(), "certificates: cannot renew");
                    let attempt = Attempt { at: now, outcome: Err(error.to_string()), output: Vec::new() };
                    self.ports.repository.record_attempt(certificate.id, &attempt).await?;
                    let told = NewNotification::new(Level::Error, "certificate_renewal_failed")
                        .with("name", certificate.common_name())
                        .with("reason", error.to_string());
                    self.ports.notifier.notify(told).await;
                }
            }
        }
        Ok(started)
    }

    // The web server.

    pub async fn web_server(&self, context: &RequestContext) -> Result<WebServerSettings, CertificatesError> {
        let current = self.ports.repository.web_server().await?;
        let state = self.ports.host.web_state(context).await?;
        let mut waiting = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        // Gone back on the host (not confirmed in time): nothing waits.
        if state.pending_until.is_none() {
            *waiting = None;
        }
        let pending = waiting.zip(state.pending_until);
        Ok(WebServerSettings { current, pending })
    }

    /// Applies new settings; they go back by themselves unless confirmed
    /// within [`CONFIRM_WITHIN_SECONDS`] from the browser at the new address.
    pub async fn apply_web_server(
        &self,
        web: WebServer,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let before = self.ports.repository.web_server().await?;
        web.check(&self.ports.repository.certificates().await?, self.now())?;
        for port in web.new_ports(&before) {
            if self.ports.listening.is_listening(port).await? {
                return Err(CertificatesError::PortBusy(port));
            }
        }
        // Another change waiting on the host is refused there (`Pending`).
        let applied = self.ports.host.apply_web(&web, CONFIRM_WITHIN_SECONDS, context).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.record("certificates.web_apply", outcome, &describe(&web), actor, context).await;
        applied?;
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(web);
        Ok(())
    }

    /// Keeps the settings applied: the browser reached the interface at its
    /// new address.
    pub async fn confirm_web_server(
        &self,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), CertificatesError> {
        let waiting = *self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(web) = waiting else {
            return Err(CertificatesError::Refused("no change waits for confirmation".into()));
        };
        // Saved first: once the host keeps the change, the database says so
        // too. Kept back if the host cannot keep it.
        let before = self.ports.repository.web_server().await?;
        self.ports.repository.save_web_server(&web).await?;
        let confirmed = self.ports.host.confirm_web(context).await;
        let outcome = if confirmed.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.record("certificates.web_confirm", outcome, &describe(&web), actor, context).await;
        if let Err(error) = confirmed {
            if let Err(error) = self.ports.repository.save_web_server(&before).await {
                tracing::error!(%error, "certificates: cannot keep the web server's settings from before");
            }
            return Err(error.into());
        }
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}

/// The settings in a few words, for the audit log.
fn describe(web: &WebServer) -> String {
    let http = web.http_port.map_or_else(|| "none".to_owned(), |port| port.to_string());
    let certificate =
        web.certificate_id.map_or_else(|| "self-signed".to_owned(), |id| format!("certificate {id}"));
    format!("http {http}, https {}, {certificate}", web.https_port)
}

fn job_error(error: HostError) -> JobError {
    let code = match &error {
        HostError::Refused(_) => "refused",
        HostError::Conflict => "conflict",
        HostError::Failed(_) | HostError::Port(_) => "agent_failed",
    };
    JobError::new(code, Some(error.to_string()))
}

#[cfg(test)]
mod tests;
