use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, Request, Response};
use zendiloos_kernel::{Clock, PortError, RequestContext};
use zendiloos_system::application::ports::TimeControl;
use zendiloos_system::domain::{TimeSettings, TimeStatus};

use super::{WHAT, call, expect_done, unexpected};
use crate::SystemClock;

/// [`TimeControl`] through the system agent, which asks timedated.
pub struct AgentTime {
    transport: Arc<dyn AgentTransport>,
}

impl AgentTime {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

fn invalid(what: &str) -> PortError {
    PortError::new(WHAT, format!("not a valid {what} for the agent"))
}

/// `Key=value` lines as `timedatectl show` writes them.
fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

fn status(timedatectl: &str, timesync: &str, zones: &str, servers_conf: &str) -> TimeStatus {
    let yes = |key| value(timedatectl, key) == Some("yes");
    let mut zones: Vec<String> =
        zones.lines().map(str::trim).filter(|z| !z.is_empty()).map(str::to_owned).collect();
    zones.sort();
    TimeStatus {
        zone: value(timedatectl, "Timezone").filter(|z| !z.is_empty()).unwrap_or("Etc/UTC").to_owned(),
        network_time: yes("NTP"),
        can_network_time: yes("CanNTP"),
        synchronized: yes("NTPSynchronized"),
        // The drop-in zendiloOS writes: one `NTP=` line, servers apart by spaces.
        servers: value(servers_conf, "NTP")
            .map(|list| list.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
        server_in_use: value(timesync, "ServerName").filter(|s| !s.is_empty()).map(str::to_owned),
        zones,
        // zendiloosd runs on the host: its clock is the host's.
        now: SystemClock.now(),
    }
}

#[async_trait]
impl TimeControl for AgentTime {
    async fn status(&self, context: &RequestContext) -> Result<TimeStatus, PortError> {
        match call(self.transport.as_ref(), Request::TimeState {}, context).await? {
            Response::TimeState { timedatectl, timesync, zones, servers_conf } => {
                Ok(status(&timedatectl, &timesync, &zones, &servers_conf))
            }
            other => Err(unexpected(other)),
        }
    }

    async fn apply(&self, settings: &TimeSettings, context: &RequestContext) -> Result<(), PortError> {
        let servers = settings
            .servers
            .iter()
            .map(|server| proto::TimeServer::parse(server).map_err(|_| invalid("time server")))
            .collect::<Result<Vec<_>, _>>()?;
        let request = Request::SetTime {
            zone: proto::TimeZone::parse(&settings.zone).map_err(|_| invalid("time zone"))?,
            network_time: settings.network_time,
            servers: proto::TimeServers::new(servers).map_err(|_| invalid("list of time servers"))?,
            clock: settings
                .clock
                .map(|at| proto::ClockTime::from_unix_seconds(at.unix_seconds()).map_err(|_| invalid("time")))
                .transpose()?,
        };
        expect_done(self.transport.as_ref(), request, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[test]
    fn reads_what_timedatectl_says() {
        let status = status(
            "Timezone=Europe/Berlin\nLocalRTC=no\nCanNTP=yes\nNTP=yes\nNTPSynchronized=no\n",
            "FallbackNTPServers=0.debian.pool.ntp.org\nServerName=ptb.de\nServerAddress=192.53.103.108\n",
            "Europe/Berlin\nEtc/UTC\n",
            "# Written by zendiloOS\n[Time]\nNTP=ptb.de 192.168.1.1\n",
        );
        assert_eq!(status.zone, "Europe/Berlin");
        assert!(status.network_time && status.can_network_time && !status.synchronized);
        assert_eq!(status.servers, ["ptb.de", "192.168.1.1"]);
        assert_eq!(status.server_in_use.as_deref(), Some("ptb.de"));
        assert_eq!(status.zones, ["Etc/UTC", "Europe/Berlin"]);

        let idle = super::status("Timezone=Etc/UTC\nCanNTP=no\nNTP=no\n", "", "", "");
        assert!(!idle.network_time && !idle.can_network_time);
        assert!(idle.servers.is_empty() && idle.server_in_use.is_none());
    }

    #[tokio::test]
    async fn settings_go_through_the_agent() {
        let time = AgentTime::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let settings = TimeSettings {
            zone: "Europe/Vienna".into(),
            network_time: true,
            servers: vec!["ptb.de".into()],
            clock: None,
        };

        time.apply(&settings, &context).await.unwrap();

        let status = time.status(&context).await.unwrap();
        assert_eq!(status.zone, "Europe/Vienna");
        assert_eq!(status.servers, ["ptb.de"]);
        assert_eq!(status.server_in_use.as_deref(), Some("ptb.de"));
        assert!(status.zones.contains(&"Europe/Berlin".to_owned()));
    }
}
