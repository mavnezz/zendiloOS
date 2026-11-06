//! The machine's clock: which time zone it shows, and whether time servers
//! keep it or it is set by hand.

use std::net::IpAddr;

use zendiloos_kernel::Timestamp;

/// The most time servers to list.
pub const MAX_TIME_SERVERS: usize = 4;

/// The clock can be set from 2000 up to 2099; anything else is a typing
/// error, and would break certificates.
const EARLIEST: i64 = 946_684_800;
const LATEST: i64 = 4_102_444_800;

/// The clock's settings as someone wants them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeSettings {
    /// A zone of the tz database: `Europe/Berlin`.
    pub zone: String,
    /// Time servers keep the clock.
    pub network_time: bool,
    /// Servers to ask; empty: Debian's pool.
    pub servers: Vec<String>,
    /// Without time servers: the time to set the clock to.
    pub clock: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTimeSettings {
    #[error("{0} is not a time zone of this system")]
    Zone(String),
    #[error("{0} is not a host name or IP address")]
    Server(String),
    #[error("at most {MAX_TIME_SERVERS} time servers, each once")]
    TooManyServers,
    #[error("the time servers set the clock; turn them off to set it by hand")]
    ClockWithServers,
    #[error("the clock can be set to a time from 2000 to 2099")]
    Clock,
}

impl TimeSettings {
    /// The settings with servers trimmed and lowercase, blank rows left
    /// out, if they hold up. Whether the zone exists is for the caller to
    /// check against [`TimeStatus::zones`].
    pub fn checked(mut self) -> Result<Self, InvalidTimeSettings> {
        if !is_zone_name(&self.zone) {
            return Err(InvalidTimeSettings::Zone(self.zone));
        }
        self.servers =
            self.servers.iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
        if let Some(bad) = self.servers.iter().find(|server| !is_host(server)) {
            return Err(InvalidTimeSettings::Server(bad.clone()));
        }
        let repeated = self.servers.iter().enumerate().any(|(at, s)| self.servers[..at].contains(s));
        if self.servers.len() > MAX_TIME_SERVERS || repeated {
            return Err(InvalidTimeSettings::TooManyServers);
        }
        if let Some(clock) = self.clock {
            if self.network_time {
                return Err(InvalidTimeSettings::ClockWithServers);
            }
            if !(EARLIEST..LATEST).contains(&clock.unix_seconds()) {
                return Err(InvalidTimeSettings::Clock);
            }
        }
        Ok(self)
    }
}

/// Names of the tz database, not paths: `America/Argentina/Buenos_Aires`,
/// `Etc/GMT+1`.
pub fn is_zone_name(name: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '-'])
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
    };
    (1..=64).contains(&name.len()) && name.split('/').all(part)
}

/// A host name or an IP address.
pub fn is_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let name = host.strip_suffix('.').unwrap_or(host);
    (1..=253).contains(&name.len()) && name.split('.').all(label)
}

/// How the clock is kept now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeStatus {
    pub zone: String,
    pub network_time: bool,
    /// A time service is installed: network time can be turned on.
    pub can_network_time: bool,
    /// The clock agrees with a time server.
    pub synchronized: bool,
    /// The servers set here; empty: Debian's pool.
    pub servers: Vec<String>,
    /// The server the clock comes from now, if any.
    pub server_in_use: Option<String>,
    /// Every zone the system has, sorted.
    pub zones: Vec<String>,
    /// The host's time when this was read.
    pub now: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(network_time: bool, servers: &[&str], clock: Option<i64>) -> TimeSettings {
        TimeSettings {
            zone: "Europe/Berlin".into(),
            network_time,
            servers: servers.iter().map(|s| (*s).to_owned()).collect(),
            clock: clock.map(Timestamp::from_unix_seconds),
        }
    }

    #[test]
    fn servers_are_tidied_and_checked() {
        let checked = settings(true, &[" PTB.de ", "", "192.168.1.1"], None).checked().unwrap();
        assert_eq!(checked.servers, ["ptb.de", "192.168.1.1"]);
        assert_eq!(settings(true, &["a b"], None).checked(), Err(InvalidTimeSettings::Server("a b".into())));
        assert_eq!(
            settings(true, &["a.de", "A.de"], None).checked(),
            Err(InvalidTimeSettings::TooManyServers)
        );
        assert_eq!(
            settings(true, &["a.de", "b.de", "c.de", "d.de", "e.de"], None).checked(),
            Err(InvalidTimeSettings::TooManyServers)
        );
    }

    #[test]
    fn the_clock_is_set_by_hand_only_without_servers() {
        assert!(settings(false, &[], Some(1_800_000_000)).checked().is_ok());
        assert_eq!(
            settings(true, &[], Some(1_800_000_000)).checked(),
            Err(InvalidTimeSettings::ClockWithServers)
        );
        assert_eq!(settings(false, &[], Some(0)).checked(), Err(InvalidTimeSettings::Clock));
    }

    #[test]
    fn zones_are_names_not_paths() {
        for zone in ["UTC", "Europe/Berlin", "America/Argentina/Buenos_Aires", "Etc/GMT+1"] {
            assert!(is_zone_name(zone), "{zone}");
        }
        for zone in ["", "/etc/passwd", "Europe/../x", "Europe//Berlin", "-x", "a b"] {
            assert!(!is_zone_name(zone), "{zone}");
        }
    }
}
