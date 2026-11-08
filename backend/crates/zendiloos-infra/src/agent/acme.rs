use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AcmeCa, AcmeChallenge, AcmeDomain, AcmeDomains, AcmeEmail, AcmeKey, AcmeOrder, AgentTransport,
    ConfirmWithin, DnsApi, DnsCredentials, DnsValue, DnsVariableName, ErrorCode, PackageRunState,
    PackageTask, Report, Request, Response, WebPort, WebServerConfig,
};
use zendiloos_certificates::application::{AcmeHost, HostError, Lines, Order, WebServerState};
use zendiloos_certificates::domain::{AcmeServer, ChallengeMethod, KeyType, WebServer};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use super::{WHAT, agent_error, call, call_raw, call_raw_reporting, expect_done, unexpected};

const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);

/// [`AcmeHost`] through the system agent.
pub struct AgentAcme {
    transport: Arc<dyn AgentTransport>,
}

impl AgentAcme {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    fn transport(&self) -> &dyn AgentTransport {
        self.transport.as_ref()
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn number(id: i64, what: &str) -> Result<u32, HostError> {
    u32::try_from(id).map_err(|_| invalid(what))
}

/// The order in the agent's terms: every value checked once more.
fn order(order: &Order) -> Result<AcmeOrder, HostError> {
    let domains = order
        .domains
        .iter()
        .map(|name| AcmeDomain::parse(name).map_err(|_| invalid("host name")))
        .collect::<Result<Vec<_>, _>>()?;
    let challenge = match &order.method {
        ChallengeMethod::Dns { provider, values } => {
            let credentials = values
                .iter()
                .map(|(name, value)| {
                    let name = DnsVariableName::parse(name).map_err(|_| invalid("DNS API variable"))?;
                    let value = DnsValue::new(value).map_err(|_| invalid("DNS API value"))?;
                    Ok((name, value))
                })
                .collect::<Result<BTreeMap<_, _>, HostError>>()?;
            AcmeChallenge::Dns {
                api: DnsApi::parse(provider).map_err(|_| invalid("DNS API"))?,
                credentials: DnsCredentials::new(credentials)
                    .map_err(|_| invalid("set of DNS API values"))?,
            }
        }
        ChallengeMethod::Http => AcmeChallenge::Http,
    };
    Ok(AcmeOrder {
        certificate: number(order.certificate_id, "certificate number")?,
        account: number(order.account_id, "account number")?,
        ca: match order.server {
            AcmeServer::LetsEncrypt => AcmeCa::LetsEncrypt,
            AcmeServer::LetsEncryptStaging => AcmeCa::LetsEncryptStaging,
        },
        email: AcmeEmail::parse(&order.email).map_err(|_| invalid("e-mail address"))?,
        domains: AcmeDomains::new(domains).map_err(|_| invalid("set of host names"))?,
        key: match order.key {
            KeyType::Ec256 => AcmeKey::Ec256,
            KeyType::Rsa2048 => AcmeKey::Rsa2048,
            KeyType::Rsa4096 => AcmeKey::Rsa4096,
        },
        challenge,
    })
}

fn web_config(web: &WebServer) -> Result<WebServerConfig, HostError> {
    let port = |port: u16| WebPort::new(port).map_err(|_| invalid("port"));
    Ok(WebServerConfig {
        http: web.http_port.map(port).transpose()?,
        https: port(web.https_port)?,
        certificate: web.certificate_id.map(|id| number(id, "certificate number")).transpose()?,
    })
}

/// The agent's refusal or failure, in its words.
fn host_error(response: Response) -> HostError {
    match response {
        Response::Error { code: ErrorCode::Forbidden, message } => HostError::Refused(message),
        Response::Error { code: ErrorCode::Conflict, .. } => HostError::Conflict,
        Response::Error { code: ErrorCode::Failed, message } => HostError::Failed(message),
        Response::Error { code, message } => HostError::Port(agent_error(code, &message)),
        other => HostError::Port(unexpected(other)),
    }
}

/// acme.sh's lines, passed on as they come.
struct Forward<'a>(&'a dyn Lines);

impl Report for Forward<'_> {
    fn step(&self, _: &str, _: Option<u8>) {}

    fn line(&self, line: &str) {
        self.0.line(line);
    }
}

#[async_trait]
impl AcmeHost for AgentAcme {
    async fn installed(&self, context: &RequestContext) -> Result<bool, PortError> {
        match call(self.transport(), Request::AcmeState {}, context).await? {
            Response::AcmeState { installed } => Ok(installed),
            other => Err(unexpected(other)),
        }
    }

    async fn install(&self, context: &RequestContext) -> Result<(), HostError> {
        let request = Request::StartPackages { task: PackageTask::InstallAcme, release: None };
        let id = match call_raw(self.transport(), request, context).await? {
            Response::PackagesStarted { id } => id,
            other => return Err(host_error(other)),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport(), Request::PackagesState {}, context).await? {
                Response::Packages { run } => run,
                other => return Err(unexpected(other).into()),
            };
            match run {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => return Ok(()),
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    return Err(HostError::Failed(run.error.unwrap_or_else(|| "apt failed".into())));
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(HostError::Failed("apt did not finish within an hour".into()));
                }
                _ => {}
            }
        }
    }

    async fn issue(
        &self,
        order: &Order,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<Timestamp, HostError> {
        let request = Request::IssueCertificate { order: self::order(order)? };
        match call_raw_reporting(self.transport(), request, &Forward(lines), context).await? {
            Response::CertificateIssued { not_after } => Ok(Timestamp::from_unix_seconds(not_after)),
            other => Err(host_error(other)),
        }
    }

    async fn remove(&self, certificate_id: i64, context: &RequestContext) -> Result<(), PortError> {
        let certificate =
            u32::try_from(certificate_id).map_err(|_| PortError::new(WHAT, "certificate number"))?;
        expect_done(self.transport(), Request::RemoveCertificate { certificate }, context).await
    }

    async fn web_state(&self, context: &RequestContext) -> Result<WebServerState, PortError> {
        match call(self.transport(), Request::WebServerState {}, context).await? {
            Response::WebServerState { pending } => Ok(WebServerState {
                pending_until: pending.map(|p| Timestamp::from_unix_seconds(p.revert_at)),
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn apply_web(
        &self,
        web: &WebServer,
        confirm_within: u16,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let confirm_within =
            ConfirmWithin::seconds(confirm_within).map_err(|_| invalid("time to confirm"))?;
        let request = Request::ApplyWebServer { config: web_config(web)?, confirm_within };
        match call_raw(self.transport(), request, context).await? {
            Response::Done => Ok(()),
            other => Err(host_error(other)),
        }
    }

    async fn confirm_web(&self, context: &RequestContext) -> Result<(), HostError> {
        match call_raw(self.transport(), Request::ConfirmWebServer {}, context).await? {
            Response::Done => Ok(()),
            other => Err(host_error(other)),
        }
    }

    async fn reload_certificate(&self, context: &RequestContext) -> Result<(), HostError> {
        match call_raw(self.transport(), Request::ReloadWebCertificate {}, context).await? {
            Response::Done => Ok(()),
            other => Err(host_error(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[derive(Default)]
    struct Kept(Mutex<Vec<String>>);

    impl Lines for Kept {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_owned());
        }
    }

    fn ordered(domains: &[&str], method: ChallengeMethod) -> Order {
        Order {
            certificate_id: 3,
            account_id: 1,
            server: AcmeServer::LetsEncryptStaging,
            email: "admin@example.org".into(),
            domains: domains.iter().map(|d| (*d).to_owned()).collect(),
            key: KeyType::Ec256,
            method,
        }
    }

    #[test]
    fn values_the_agent_would_refuse_are_not_sent() {
        let values = BTreeMap::from([("LD_PRELOAD".to_owned(), "/tmp/x.so".to_owned())]);
        let sneaky = ChallengeMethod::Dns { provider: "dns_ionos".into(), values };
        assert!(order(&ordered(&["nas.example.org"], sneaky)).is_err());
        assert!(order(&ordered(&["--server=x"], ChallengeMethod::Http)).is_err());
        let web = WebServer { http_port: Some(0), https_port: 443, certificate_id: None };
        assert!(web_config(&web).is_err());
        assert!(number(-1, "certificate number").is_err());
    }

    #[tokio::test]
    async fn issues_installs_and_applies_through_the_agent() {
        let acme = AgentAcme::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert!(!acme.installed(&context).await.unwrap());
        let kept = Kept::default();
        let missing =
            acme.issue(&ordered(&["nas.example.org"], ChallengeMethod::Http), &kept, &context).await;
        assert!(matches!(missing, Err(HostError::Failed(_))), "{missing:?}");

        // The simulated apt takes three seconds.
        acme.install(&context).await.unwrap();
        assert!(acme.installed(&context).await.unwrap());
        let until =
            acme.issue(&ordered(&["nas.example.org"], ChallengeMethod::Http), &kept, &context).await.unwrap();
        assert!(until.unix_seconds() > 0);
        assert!(kept.0.lock().unwrap().iter().any(|line| line.ends_with("Cert success.")));
        let failed =
            acme.issue(&ordered(&["nas.example.invalid"], ChallengeMethod::Http), &kept, &context).await;
        assert!(matches!(failed, Err(HostError::Failed(reason)) if reason.contains("Verify error")));

        let web = WebServer { http_port: None, https_port: 8443, certificate_id: Some(3) };
        acme.apply_web(&web, 90, &context).await.unwrap();
        assert!(acme.web_state(&context).await.unwrap().pending_until.is_some());
        assert!(matches!(acme.apply_web(&web, 90, &context).await, Err(HostError::Conflict)));
        acme.confirm_web(&context).await.unwrap();
        assert!(matches!(acme.confirm_web(&context).await, Err(HostError::Refused(_))));
        acme.reload_certificate(&context).await.unwrap();
        acme.remove(3, &context).await.unwrap();
        let gone = acme.apply_web(&web, 90, &context).await;
        assert!(matches!(gone, Err(HostError::Refused(_))), "{gone:?}");
        assert!(matches!(acme.apply_web(&web, 5, &context).await, Err(HostError::Port(_))));
    }
}
