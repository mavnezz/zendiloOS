use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ErrorCode, PackageRunState, PackageTask, Request, Response,
};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_ups::application::{HostError, HostFacts, Stored, UpsHost};
use zendiloos_ups::domain::{Found, Settings, ShutdownWhen, SnmpVersion, Source};

use super::{WHAT, call, call_raw, refusal, unexpected};

const PACKAGES_POLL: Duration = if cfg!(test) { Duration::from_millis(5) } else { Duration::from_secs(2) };
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);

/// [`UpsHost`] through the system agent: NUT's packages, files and units.
pub struct AgentUps {
    transport: Arc<dyn AgentTransport>,
}

impl AgentUps {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

fn invalid(what: &str) -> PortError {
    PortError::new(WHAT, format!("not a valid {what} for the agent"))
}

fn word(text: &str, what: &str) -> Result<proto::UpsWord, PortError> {
    proto::UpsWord::parse(text).map_err(|_| invalid(what))
}

fn host(text: &str) -> Result<proto::UpsHost, PortError> {
    proto::UpsHost::parse(text).map_err(|_| invalid("host"))
}

fn usb_id(id: Option<&String>) -> Result<Option<proto::UsbId>, PortError> {
    id.map(|id| proto::UsbId::parse(id).map_err(|_| invalid("USB id"))).transpose()
}

fn source(source: &Source) -> Result<proto::UpsSource, PortError> {
    Ok(match source {
        Source::Usb { driver, vendor_id, product_id, serial } => proto::UpsSource::Usb {
            driver: proto::UsbDriver::parse(driver).ok_or_else(|| invalid("driver"))?,
            vendor_id: usb_id(vendor_id.as_ref())?,
            product_id: usb_id(product_id.as_ref())?,
            serial: serial.as_deref().map(|serial| word(serial, "serial number")).transpose()?,
        },
        Source::Snmp { address, community, version } => proto::UpsSource::Snmp {
            address: host(address)?,
            community: word(community, "community")?,
            version: match version {
                SnmpVersion::V1 => proto::SnmpVersion::V1,
                SnmpVersion::V2c => proto::SnmpVersion::V2c,
            },
        },
        Source::Server { host: name, port, ups, user, password } => proto::UpsSource::Server {
            host: host(name)?,
            port: proto::UpsServerPort::new(*port).map_err(|_| invalid("port"))?,
            ups: word(ups, "UPS name")?,
            user: word(user, "account")?,
            password: word(password, "password")?,
        },
        Source::Simulated => proto::UpsSource::Simulated,
    })
}

/// What the agent is given for `settings`; none without a UPS.
pub(crate) fn config(
    settings: &Settings,
    local_password: &str,
) -> Result<Option<proto::UpsConfig>, PortError> {
    let Some(from) = &settings.source else {
        return Ok(None);
    };
    Ok(Some(proto::UpsConfig {
        source: source(from)?,
        shutdown: match settings.shutdown {
            ShutdownWhen::LowBattery => proto::UpsShutdown::LowBattery,
            ShutdownWhen::ChargeBelow(percent) => proto::UpsShutdown::ChargeBelow { percent },
            ShutdownWhen::AfterMinutes(minutes) => proto::UpsShutdown::AfterMinutes { minutes },
        },
        cut_power: settings.cut_power,
        serve: settings
            .serve
            .as_ref()
            .map(|account| {
                Ok::<_, PortError>(proto::UpsAccount {
                    user: word(&account.user, "account")?,
                    password: word(&account.password, "password")?,
                })
            })
            .transpose()?,
        local_password: word(local_password, "password")?,
    }))
}

/// The agent's answer, with what it refused or could not do as an error.
fn answer(response: Response) -> Result<Response, HostError> {
    match response {
        Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
            Err(HostError::Refused(refusal(&message)))
        }
        Response::Error { message, .. } => Err(HostError::Failed(message)),
        response => Ok(response),
    }
}

#[async_trait]
impl UpsHost for AgentUps {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError> {
        match call(self.transport.as_ref(), Request::UpsFacts {}, context).await? {
            Response::UpsFacts { installed, snmp } => Ok(HostFacts { installed, snmp }),
            other => Err(unexpected(other)),
        }
    }

    async fn install(&self, context: &RequestContext) -> Result<(), HostError> {
        let request = Request::StartPackages { task: PackageTask::InstallUps, release: None };
        let id = match answer(call_raw(self.transport.as_ref(), request, context).await?)? {
            Response::PackagesStarted { id } => id,
            other => return Err(unexpected(other).into()),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport.as_ref(), Request::PackagesState {}, context).await? {
                Response::Packages { run, .. } => run,
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

    async fn apply(&self, stored: &Stored, context: &RequestContext) -> Result<(), HostError> {
        let config = config(&stored.settings, &stored.local_password)?;
        match answer(call_raw(self.transport.as_ref(), Request::SetUps { config }, context).await?)? {
            Response::Done => Ok(()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn scan(&self, context: &RequestContext) -> Result<Vec<Found>, HostError> {
        match answer(call_raw(self.transport.as_ref(), Request::ScanUps {}, context).await?)? {
            Response::UpsFound { devices } => Ok(devices
                .into_iter()
                .map(|device| Found {
                    driver: device.driver,
                    vendor_id: device.vendor_id,
                    product_id: device.product_id,
                    vendor: device.vendor,
                    product: device.product,
                    serial: device.serial,
                })
                .collect()),
            other => Err(unexpected(other).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::PackageRun;
    use zendiloos_ups::domain::Account;

    use super::*;
    use crate::agent::scripted::Scripted;

    fn context() -> RequestContext {
        RequestContext::default()
    }

    fn usb() -> Settings {
        Settings {
            source: Some(Source::Usb {
                driver: "usbhid-ups".into(),
                vendor_id: None,
                product_id: None,
                serial: None,
            }),
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn nut_installs_through_apt_and_a_failure_is_told_in_its_words() {
        let run = |state| PackageRun {
            id: "ab01".into(),
            task: PackageTask::InstallUps,
            state,
            step: String::new(),
            started_at: 0,
            finished_at: None,
            error: Some("apt said no".into()),
            report: None,
            release: None,
        };
        let scripted = Arc::new(Scripted::answering([
            Response::PackagesStarted { id: "ab01".into() },
            // Still running, then done.
            Response::Packages { run: Some(run(PackageRunState::Running)), update: None },
            Response::Packages { run: Some(run(PackageRunState::Succeeded)), update: None },
        ]));
        AgentUps::new(scripted.clone()).install(&context()).await.unwrap();
        assert!(matches!(scripted.asked()[0], Request::StartPackages { task: PackageTask::InstallUps, .. }));
        assert_eq!(scripted.asked().len(), 3);

        let failing = Arc::new(Scripted::answering([
            Response::PackagesStarted { id: "ab01".into() },
            Response::Packages { run: Some(run(PackageRunState::Failed)), update: None },
        ]));
        let failed = AgentUps::new(failing).install(&context()).await;
        assert!(matches!(&failed, Err(HostError::Failed(why)) if why == "apt said no"), "{failed:?}");
        let busy =
            Arc::new(Scripted::refusing(ErrorCode::Forbidden, "refused: apt is busy with another run"));
        let refused = AgentUps::new(busy).install(&context()).await;
        assert!(matches!(&refused, Err(HostError::Refused(why)) if why == "apt is busy with another run"));
    }

    #[tokio::test]
    async fn the_host_is_asked_for_its_facts_its_devices_and_to_take_the_settings() {
        let scripted = Arc::new(Scripted::answering([
            Response::UpsFacts { installed: true, snmp: false },
            Response::UpsFound {
                devices: vec![proto::FoundUps {
                    driver: "usbhid-ups".into(),
                    vendor_id: Some("0463".into()),
                    product_id: Some("FFFF".into()),
                    vendor: Some("EATON".into()),
                    product: Some("Ellipse ECO".into()),
                    serial: None,
                }],
            },
            Response::Done,
            Response::Done,
        ]));
        let host = AgentUps::new(scripted.clone());
        assert_eq!(host.facts(&context()).await.unwrap(), HostFacts { installed: true, snmp: false });
        let found = host.scan(&context()).await.unwrap();
        assert_eq!((found.len(), found[0].product.as_deref()), (1, Some("Ellipse ECO")));
        host.apply(&Stored { settings: usb(), local_password: "local".into() }, &context()).await.unwrap();
        host.apply(&Stored { settings: Settings::default(), local_password: "local".into() }, &context())
            .await
            .unwrap();
        let asked = scripted.asked();
        assert!(matches!(asked[0], Request::UpsFacts {}));
        assert!(matches!(asked[1], Request::ScanUps {}));
        assert!(
            matches!(&asked[2], Request::SetUps { config: Some(config) } if config.local_password.as_str() == "local")
        );
        assert!(matches!(&asked[3], Request::SetUps { config: None }), "no UPS: NUT is switched off");

        // What the agent refuses comes back with its reason, without its word for it.
        let refusing = Arc::new(Scripted::refusing(ErrorCode::Forbidden, "refused: the driver is missing"));
        let refused = AgentUps::new(refusing)
            .apply(&Stored { settings: usb(), local_password: "local".into() }, &context())
            .await;
        assert!(
            matches!(&refused, Err(HostError::Refused(why)) if why == "the driver is missing"),
            "{refused:?}"
        );
        // Settings the agent would not read are not sent at all.
        let untouched = Arc::new(Scripted::answering([]));
        let unsent = AgentUps::new(untouched.clone())
            .apply(&Stored { settings: usb(), local_password: "not a word".into() }, &context())
            .await;
        assert!(matches!(unsent, Err(HostError::Port(_))));
        assert!(untouched.asked().is_empty());
    }

    #[test]
    fn the_settings_become_what_the_agent_checks_again() {
        let settings = Settings {
            source: Some(Source::Usb {
                driver: "usbhid-ups".into(),
                vendor_id: Some("0463".into()),
                product_id: None,
                serial: Some("P123".into()),
            }),
            shutdown: ShutdownWhen::AfterMinutes(10),
            cut_power: true,
            serve: Some(Account { user: "monuser".into(), password: "secret".into() }),
        };
        let config = config(&settings, "local").unwrap().unwrap();
        assert!(matches!(&config.source, proto::UpsSource::Usb { driver: proto::UsbDriver::UsbHid, .. }));
        assert_eq!(config.shutdown, proto::UpsShutdown::AfterMinutes { minutes: 10 });
        assert!(config.cut_power);
        assert_eq!(config.serve.as_ref().map(|account| account.user.as_str()), Some("monuser"));
        assert_eq!(config.local_password.as_str(), "local");
        assert!(config.checked().is_ok());

        assert_eq!(
            super::config(&Settings::default(), "local").unwrap(),
            None,
            "no UPS: NUT is switched off"
        );
    }

    #[test]
    fn what_the_agent_would_not_take_is_not_sent() {
        let with = |source| Settings { source: Some(source), ..Settings::default() };
        let driver =
            with(Source::Usb { driver: "dummy-ups".into(), vendor_id: None, product_id: None, serial: None });
        assert!(config(&driver, "local").is_err());
        let community = with(Source::Snmp {
            address: "10.0.0.9".into(),
            community: "two words".into(),
            version: SnmpVersion::V1,
        });
        assert!(config(&community, "local").is_err());
        assert!(config(&with(Source::Simulated), "not a word").is_err());
    }

    #[test]
    fn a_refusal_and_a_failure_are_told_apart() {
        let refused = answer(Response::Error {
            code: ErrorCode::Forbidden,
            message: "refused: only a UPS on this machine can be offered to other devices".into(),
        });
        assert!(
            matches!(&refused, Err(HostError::Refused(why)) if why.starts_with("only a UPS")),
            "{refused:?}"
        );
        let failed = answer(Response::Error { code: ErrorCode::Failed, message: "systemctl failed".into() });
        assert!(matches!(failed, Err(HostError::Failed(_))));
        assert!(matches!(answer(Response::Done), Ok(Response::Done)));
    }
}
