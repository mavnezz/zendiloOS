use std::collections::BTreeMap;
use std::time::Duration;

use zendiloos_jobs::JobState;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_notify::testing::MemoryNotifications;

use super::*;
use crate::domain::{AcmeServer, KeyType};
use crate::testing::{FakeAcmeHost, FakePorts, MemoryCertificates};

#[derive(Default)]
struct Audit(Mutex<Vec<&'static str>>);

#[async_trait]
impl AuditLog for Audit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry.action);
        Ok(())
    }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

struct Setup {
    certificates: Certificates,
    repository: Arc<MemoryCertificates>,
    host: Arc<FakeAcmeHost>,
    ports: Arc<FakePorts>,
    notifications: Arc<MemoryNotifications>,
    audit: Arc<Audit>,
    jobs: Jobs,
}

fn setup_with(host: FakeAcmeHost) -> Setup {
    let repository = Arc::new(MemoryCertificates::default());
    let host = Arc::new(host);
    let ports = Arc::new(FakePorts::listening(&[22, 80, 443, 8080]));
    let notifications = Arc::new(MemoryNotifications::default());
    let audit = Arc::new(Audit::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let certificates = Certificates::new(CertificatesPorts {
        repository: repository.clone(),
        host: host.clone(),
        listening: ports.clone(),
        jobs: jobs.clone(),
        notifier: Notifier::new(notifications.clone(), Arc::new(FixedClock)),
        audit: audit.clone(),
        clock: Arc::new(FixedClock),
    });
    Setup { certificates, repository, host, ports, notifications, audit, jobs }
}

fn setup() -> Setup {
    setup_with(FakeAcmeHost::installed())
}

/// zendiloosd started again: what it keeps in memory is gone, the host
/// and the database stay.
fn restarted(s: &Setup) -> Certificates {
    Certificates::new(CertificatesPorts {
        repository: s.repository.clone(),
        host: s.host.clone(),
        listening: s.ports.clone(),
        jobs: s.jobs.clone(),
        notifier: Notifier::new(s.notifications.clone(), Arc::new(FixedClock)),
        audit: s.audit.clone(),
        clock: Arc::new(FixedClock),
    })
}

fn now() -> Timestamp {
    FixedClock.now()
}

async fn finished(jobs: &Jobs, job: Job) -> Job {
    for _ in 0..200 {
        let current = jobs.find(job.id).await.unwrap().unwrap();
        if current.finished_at.is_some() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the job did not finish");
}

fn ionos(prefix: &str, secret: &str) -> ChallengeMethod {
    let values =
        BTreeMap::from([("IONOS_PREFIX".into(), prefix.into()), ("IONOS_SECRET".into(), secret.into())]);
    ChallengeMethod::Dns { provider: "dns_ionos".into(), values }
}

/// An account, a DNS and an HTTP challenge, and a certificate for
/// `nas.example.org` through the challenge given.
async fn certificate(s: &Setup, dns: bool, domains: &[&str]) -> Certificate {
    let context = RequestContext::default();
    let account = NewAccount {
        name: "Let's Encrypt".into(),
        server: AcmeServer::LetsEncryptStaging,
        email: "admin@example.org".into(),
    };
    let account = s.certificates.create_account(account, &alice(), &context).await.unwrap();
    let method = if dns { ionos("prefix", "secret") } else { ChallengeMethod::Http };
    let challenge = s.certificates.create_challenge("IONOS", method, &alice(), &context).await.unwrap();
    let new = NewCertificate {
        domains: domains.iter().map(|d| (*d).to_owned()).collect(),
        key: KeyType::Ec256,
        account_id: account.id,
        challenge_id: challenge.id,
        auto_renew: true,
    };
    let made = s.certificates.create_certificate(new, &alice(), &context).await.unwrap();
    // What was kept under their numbers before is cleared first.
    let calls = s.host.calls();
    assert_eq!(
        calls[calls.len() - 2..],
        [format!("remove account {}", account.id), format!("remove {}", made.id)]
    );
    s.host.forget_calls();
    made
}

async fn stored(s: &Setup, id: i64) -> Certificate {
    s.repository.certificates().await.unwrap().into_iter().find(|c| c.id == id).unwrap()
}

#[tokio::test]
async fn the_plugin_installs_acme_sh_in_a_job() {
    let s = setup_with(FakeAcmeHost::default());
    let context = RequestContext::default();
    assert!(!s.certificates.plugin(&context).await.unwrap().installed);
    let made = certificate(&s, true, &["nas.example.org"]).await;
    let issued = s.certificates.issue(made.id, Some(&alice()), &context).await;
    assert!(matches!(issued, Err(CertificatesError::NotInstalled)), "{issued:?}");
    assert!(s.certificates.renew_due().await.unwrap().is_empty());

    let job = s.certificates.install(&alice(), &context).await.unwrap();
    assert_eq!(job.kind, "certificates.acme_install");
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
    assert!(s.certificates.plugin(&context).await.unwrap().installed);
    assert!(matches!(s.certificates.install(&alice(), &context).await, Err(CertificatesError::Installed)));
    assert_eq!(s.host.calls(), ["install"]);
    assert!(s.audit.0.lock().unwrap().contains(&"certificates.acme_install"));
}

#[tokio::test]
async fn accounts_are_checked_and_kept_while_used() {
    let s = setup();
    let context = RequestContext::default();
    let bad = NewAccount { name: "LE".into(), server: AcmeServer::LetsEncrypt, email: "not an email".into() };
    assert!(matches!(
        s.certificates.create_account(bad, &alice(), &context).await,
        Err(CertificatesError::Invalid(_))
    ));

    let made = certificate(&s, true, &["nas.example.org"]).await;
    let changed = NewAccount {
        name: "Production".into(),
        server: AcmeServer::LetsEncrypt,
        email: "ops@example.org".into(),
    };
    s.certificates.update_account(made.account_id, changed.clone(), &alice(), &context).await.unwrap();
    let accounts = s.certificates.accounts().await.unwrap();
    assert_eq!((accounts[0].name.as_str(), accounts[0].server), ("Production", AcmeServer::LetsEncrypt));
    let missing = s.certificates.update_account(99, changed, &alice(), &context).await;
    assert!(matches!(missing, Err(CertificatesError::NotFound)));

    let used = s.certificates.delete_account(made.account_id, &alice(), &context).await;
    assert!(matches!(used, Err(CertificatesError::InUse(_))), "{used:?}");
    let used = s.certificates.delete_challenge(made.challenge_id, &alice(), &context).await;
    assert!(matches!(used, Err(CertificatesError::InUse(_))), "{used:?}");
    s.certificates.delete_certificate(made.id, &alice(), &context).await.unwrap();
    assert_eq!(s.host.calls(), [format!("remove {}", made.id)]);
    s.certificates.delete_challenge(made.challenge_id, &alice(), &context).await.unwrap();
    s.certificates.delete_account(made.account_id, &alice(), &context).await.unwrap();
    assert!(matches!(
        s.certificates.delete_account(made.account_id, &alice(), &context).await,
        Err(CertificatesError::NotFound)
    ));
}

#[tokio::test]
async fn secrets_left_empty_stay_and_a_wildcard_keeps_its_dns_challenge() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, true, &["*.example.org", "example.org"]).await;
    s.certificates
        .update_challenge(made.challenge_id, "IONOS DNS", ionos("other", ""), &alice(), &context)
        .await
        .unwrap();
    let challenges = s.certificates.challenges().await.unwrap();
    assert_eq!(challenges[0].name, "IONOS DNS");
    assert_eq!(challenges[0].method, ionos("other", "secret"));

    let http = s
        .certificates
        .update_challenge(made.challenge_id, "HTTP", ChallengeMethod::Http, &alice(), &context)
        .await;
    assert!(matches!(http, Err(CertificatesError::InUse(_))), "{http:?}");
    let missing =
        s.certificates.update_challenge(99, "HTTP", ChallengeMethod::Http, &alice(), &context).await;
    assert!(matches!(missing, Err(CertificatesError::NotFound)));
}

#[tokio::test]
async fn certificates_need_an_account_and_a_fitting_challenge() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, false, &["nas.example.org"]).await;
    let wildcard = NewCertificate { domains: vec!["*.example.org".into()], ..new_from(&made) };
    let refused = s.certificates.create_certificate(wildcard, &alice(), &context).await;
    assert!(matches!(refused, Err(CertificatesError::Invalid(_))), "{refused:?}");
    let no_account = NewCertificate { account_id: 99, ..new_from(&made) };
    assert!(matches!(
        s.certificates.create_certificate(no_account, &alice(), &context).await,
        Err(CertificatesError::Invalid(_))
    ));
    let no_challenge = NewCertificate { challenge_id: 99, ..new_from(&made) };
    assert!(matches!(
        s.certificates.create_certificate(no_challenge, &alice(), &context).await,
        Err(CertificatesError::Invalid(_))
    ));

    let renamed = NewCertificate {
        domains: vec!["NAS.example.org".into(), "files.example.org".into()],
        ..new_from(&made)
    };
    s.certificates.update_certificate(made.id, renamed, &alice(), &context).await.unwrap();
    assert_eq!(stored(&s, made.id).await.domains, ["nas.example.org", "files.example.org"]);
    let missing = s.certificates.update_certificate(99, new_from(&made), &alice(), &context).await;
    assert!(matches!(missing, Err(CertificatesError::NotFound)));
}

fn new_from(certificate: &Certificate) -> NewCertificate {
    NewCertificate {
        domains: certificate.domains.clone(),
        key: certificate.key,
        account_id: certificate.account_id,
        challenge_id: certificate.challenge_id,
        auto_renew: certificate.auto_renew,
    }
}

#[tokio::test]
async fn issuing_keeps_what_acme_sh_wrote_and_until_when_it_holds() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    let job = finished(&s.jobs, job).await;
    assert_eq!((job.state, job.target.as_deref()), (JobState::Succeeded, Some("nas.example.org")));
    assert!(job.output.iter().any(|line| line == "Cert success."), "{:?}", job.output);

    let issued = stored(&s, made.id).await;
    assert_eq!(issued.not_after, Some(now().saturating_add(crate::testing::ISSUED_FOR)));
    assert_eq!(issued.last_output, ["Creating domain key for nas.example.org", "Cert success."]);
    assert!(issued.is_valid(now()));
    // Issued by hand: no notification.
    assert!(s.notifications.all().is_empty());

    s.host.fail_with(Some("Invalid TXT record"));
    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    let job = finished(&s.jobs, job).await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.error.unwrap().detail.as_deref(), Some("Invalid TXT record"));
    let failed = stored(&s, made.id).await;
    assert_eq!(failed.last_error.as_deref(), Some("Invalid TXT record"));
    // The certificate issued before still holds.
    assert!(failed.is_valid(now()));
    assert!(s.audit.0.lock().unwrap().contains(&"certificates.issue"));
    assert!(matches!(
        s.certificates.issue(99, Some(&alice()), &context).await,
        Err(CertificatesError::NotFound)
    ));
}

#[tokio::test]
async fn an_http_challenge_needs_plain_http_on_port_80() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, false, &["nas.example.org"]).await;
    s.repository
        .save_web_server(&WebServer { http_port: None, https_port: 443, certificate_id: None })
        .await
        .unwrap();
    let refused = s.certificates.issue(made.id, Some(&alice()), &context).await;
    assert!(matches!(refused, Err(CertificatesError::Refused(_))), "{refused:?}");
    s.repository.save_web_server(&WebServer::default()).await.unwrap();
    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
}

/// Issued 70 days ago, holding 20 more: due.
async fn due(s: &Setup, id: i64) {
    let day = Duration::from_secs(24 * 3600);
    let attempt = Attempt {
        at: now().saturating_sub(day * 70),
        outcome: Ok(now().saturating_add(day * 20)),
        output: vec!["Cert success.".into()],
    };
    assert!(s.repository.record_attempt(id, &attempt).await.unwrap());
}

#[tokio::test]
async fn the_daily_look_renews_what_is_due_and_tells() {
    let s = setup();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    // Never issued: not renewed by itself.
    assert!(s.certificates.renew_due().await.unwrap().is_empty());
    due(&s, made.id).await;
    s.repository
        .save_web_server(&WebServer { certificate_id: Some(made.id), ..WebServer::default() })
        .await
        .unwrap();

    let jobs = s.certificates.renew_due().await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(finished(&s.jobs, jobs[0].clone()).await.state, JobState::Succeeded);
    assert!(!stored(&s, made.id).await.renewal_due(now()));
    // nginx takes the renewed certificate it shows.
    assert_eq!(s.host.calls(), ["issue nas.example.org", "reload"]);
    let told = s.notifications.all();
    assert_eq!((told[0].code.as_str(), told[0].level), ("certificate_renewed", Level::Success));
    assert_eq!(told[0].params["name"], "nas.example.org");
    assert!(s.certificates.renew_due().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_renewal_tells_and_waits_before_trying_again() {
    let s = setup();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    due(&s, made.id).await;
    s.host.fail_with(Some("DNS API: unauthorized"));
    let jobs = s.certificates.renew_due().await.unwrap();
    assert_eq!(finished(&s.jobs, jobs[0].clone()).await.state, JobState::Failed);
    let told = s.notifications.all();
    assert_eq!((told[0].code.as_str(), told[0].level), ("certificate_renewal_failed", Level::Error));
    assert_eq!(told[0].params["reason"], "DNS API: unauthorized");
    // Tried just now: not again today.
    assert!(s.certificates.renew_due().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_renewal_that_cannot_start_tells_too() {
    let s = setup();
    let made = certificate(&s, false, &["nas.example.org"]).await;
    due(&s, made.id).await;
    s.repository
        .save_web_server(&WebServer { http_port: None, https_port: 443, certificate_id: None })
        .await
        .unwrap();
    assert!(s.certificates.renew_due().await.unwrap().is_empty());
    let told = s.notifications.all();
    assert_eq!(told[0].code, "certificate_renewal_failed");
    assert!(stored(&s, made.id).await.last_error.unwrap().contains("port 80"));
}

#[tokio::test]
async fn web_settings_are_kept_only_once_confirmed() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    let web = WebServer { http_port: None, https_port: 8443, certificate_id: Some(made.id) };
    // Not issued yet.
    let invalid = s.certificates.apply_web_server(web, &alice(), &context).await;
    assert!(matches!(invalid, Err(CertificatesError::Invalid(_))), "{invalid:?}");
    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    finished(&s.jobs, job).await;

    let busy = WebServer { https_port: 8080, ..web };
    assert!(matches!(
        s.certificates.apply_web_server(busy, &alice(), &context).await,
        Err(CertificatesError::PortBusy(8080))
    ));
    s.certificates.apply_web_server(web, &alice(), &context).await.unwrap();
    let settings = s.certificates.web_server(&context).await.unwrap();
    assert_eq!(settings.current, WebServer::default());
    assert_eq!(settings.pending.map(|(w, _)| w), Some(web));
    let again = s.certificates.apply_web_server(WebServer::default(), &alice(), &context).await;
    assert!(matches!(again, Err(CertificatesError::Pending)), "{again:?}");
    let shown = s.certificates.delete_certificate(made.id, &alice(), &context).await;
    assert!(matches!(shown, Err(CertificatesError::InUse(_))), "{shown:?}");

    s.certificates.confirm_web_server(&alice(), &context).await.unwrap();
    let settings = s.certificates.web_server(&context).await.unwrap();
    assert_eq!((settings.current, settings.pending), (web, None));
    let nothing = s.certificates.confirm_web_server(&alice(), &context).await;
    assert!(matches!(nothing, Err(CertificatesError::Refused(_))));
    let shown = s.certificates.delete_certificate(made.id, &alice(), &context).await;
    assert!(matches!(shown, Err(CertificatesError::InUse(_))), "{shown:?}");
    let audit = s.audit.0.lock().unwrap().clone();
    assert!(audit.contains(&"certificates.web_apply") && audit.contains(&"certificates.web_confirm"));
}

#[tokio::test]
async fn web_settings_not_confirmed_go_back() {
    let s = setup();
    let context = RequestContext::default();
    let web = WebServer { http_port: Some(8081), https_port: 443, certificate_id: None };
    s.certificates.apply_web_server(web, &alice(), &context).await.unwrap();
    assert_eq!(s.host.in_force(), web);
    s.host.roll_back();
    let settings = s.certificates.web_server(&context).await.unwrap();
    assert_eq!((settings.current, settings.pending), (WebServer::default(), None));
    let late = s.certificates.confirm_web_server(&alice(), &context).await;
    assert!(matches!(late, Err(CertificatesError::Refused(_))), "{late:?}");
    // Ports the web server holds now are not in the way.
    s.ports.0.lock().unwrap().insert(8081);
    let same = WebServer { http_port: Some(80), https_port: 443, certificate_id: None };
    s.certificates.apply_web_server(same, &alice(), &context).await.unwrap();
    let reserved = WebServer { http_port: Some(445), ..same };
    s.host.roll_back();
    assert!(matches!(
        s.certificates.apply_web_server(reserved, &alice(), &context).await,
        Err(CertificatesError::Invalid(_))
    ));
}

/// A certificate being issued is not deleted meanwhile, nor issued twice at
/// once.
#[tokio::test]
async fn a_certificate_being_issued_is_neither_deleted_nor_issued_again() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    let issuing = s.certificates.begin_issuing(made.id).unwrap();

    let deleted = s.certificates.delete_certificate(made.id, &alice(), &context).await;
    assert!(matches!(deleted, Err(CertificatesError::InUse(_))), "{deleted:?}");
    let again = s.certificates.issue(made.id, Some(&alice()), &context).await;
    assert!(matches!(again, Err(CertificatesError::Refused(_))), "{again:?}");
    drop(issuing);

    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
    s.certificates.delete_certificate(made.id, &alice(), &context).await.unwrap();
}

/// A renewal nginx does not take is told as that, not as renewed; the
/// certificate of a change waiting is reloaded too.
#[tokio::test]
async fn a_renewal_nginx_does_not_take_is_told_as_that() {
    let s = setup();
    let context = RequestContext::default();
    let made = certificate(&s, true, &["nas.example.org"]).await;
    let job = s.certificates.issue(made.id, Some(&alice()), &context).await.unwrap();
    finished(&s.jobs, job).await;
    let waiting = WebServer { certificate_id: Some(made.id), ..WebServer::default() };
    s.certificates.apply_web_server(waiting, &alice(), &context).await.unwrap();
    s.host.forget_calls();

    due(&s, made.id).await;
    s.host.fail_reload_with(Some("nginx: [emerg] cannot load certificate"));
    let jobs = s.certificates.renew_due().await.unwrap();
    assert_eq!(finished(&s.jobs, jobs[0].clone()).await.state, JobState::Failed);
    assert_eq!(s.host.calls(), ["issue nas.example.org", "reload"], "the change waiting shows it");
    let told = s.notifications.all();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!((told[0].code.as_str(), told[0].level), ("certificate_not_taken", Level::Error));
    assert_eq!(told[0].params["reason"], "nginx: [emerg] cannot load certificate");
}

/// Confirming saves the settings first, and takes them back if the host
/// cannot keep the change.
#[tokio::test]
async fn web_settings_the_host_cannot_keep_are_not_saved() {
    let s = setup();
    let context = RequestContext::default();
    let moved = WebServer { http_port: Some(8081), https_port: 8444, certificate_id: None };
    s.certificates.apply_web_server(moved, &alice(), &context).await.unwrap();
    s.host.fail_confirm();
    assert!(s.certificates.confirm_web_server(&alice(), &context).await.is_err());
    assert_eq!(s.repository.web_server().await.unwrap(), WebServer::default());
}

/// A browser that sends its confirmation twice at once (two tabs, a retry)
/// gets one through to the host; the other finds nothing waiting, and
/// must not take back what the first saved.
#[tokio::test]
async fn two_confirmations_at_once_leave_the_database_as_the_host_keeps_it() {
    let s = setup();
    let context = RequestContext::default();
    let moved = WebServer { http_port: Some(8081), https_port: 8444, certificate_id: None };
    s.certificates.apply_web_server(moved, &alice(), &context).await.unwrap();
    let alice = alice();
    let (first, second) = tokio::join!(
        s.certificates.confirm_web_server(&alice, &context),
        s.certificates.confirm_web_server(&alice, &context),
    );
    assert!(first.is_ok() != second.is_ok(), "one gets through: {first:?}, {second:?}");
    assert_eq!(s.host.calls().iter().filter(|call| *call == "confirm").count(), 1);
    assert_eq!(s.repository.web_server().await.unwrap(), moved, "the host keeps it");
}

/// zendiloosd saved the settings it was confirming and stopped before the
/// host heard of it: the host went back to the settings before. Started
/// again, it puts those saved in force, once the host has no change
/// waiting any more.
#[tokio::test]
async fn settings_saved_while_confirming_are_in_force_again_after_a_restart() {
    let s = setup();
    let context = RequestContext::default();
    let moved = WebServer { http_port: Some(8081), https_port: 8444, certificate_id: None };
    s.certificates.apply_web_server(moved, &alice(), &context).await.unwrap();
    // Saved, then gone before the host was told; the host waits still.
    s.repository.save_web_server(&moved).await.unwrap();
    let restarted = restarted(&s);
    let waiting = restarted.bring_web_server_in_step(&context).await.unwrap();
    assert!(matches!(waiting, InStep::WaitUntil(_)), "{waiting:?}");
    // Nobody confirmed it: it went back.
    s.host.roll_back();
    assert_eq!(s.host.in_force(), WebServer::default());
    s.host.forget_calls();

    assert_eq!(restarted.bring_web_server_in_step(&context).await.unwrap(), InStep::Done);
    assert_eq!(s.host.in_force(), moved);
    assert_eq!(s.host.calls(), ["keep Some(8081) 8444 None"]);
    let settings = restarted.web_server(&context).await.unwrap();
    assert_eq!((settings.current, settings.pending), (moved, None));
}

/// An account's key at the authority goes with it.
#[tokio::test]
async fn what_acme_sh_kept_of_an_account_goes_with_it() {
    let s = setup();
    let context = RequestContext::default();
    let account =
        NewAccount { name: "LE".into(), server: AcmeServer::LetsEncrypt, email: "Admin@Example.ORG".into() };
    let made = s.certificates.create_account(account, &alice(), &context).await.unwrap();
    assert_eq!(made.email, "Admin@example.org");
    s.host.forget_calls();
    s.certificates.delete_account(made.id, &alice(), &context).await.unwrap();
    assert_eq!(s.host.calls(), [format!("remove account {}", made.id)]);
}
