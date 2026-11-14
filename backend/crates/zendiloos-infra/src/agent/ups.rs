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

const PACKAGES_POLL: Duration = Duration::from_secs(2);
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
    use zendiloos_ups::domain::Account;

    use super::*;

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
