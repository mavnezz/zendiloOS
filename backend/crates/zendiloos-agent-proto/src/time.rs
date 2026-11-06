//! The machine's clock: its time zone, the time servers that keep it, and
//! the time itself when nobody does. Every value is checked while it is
//! read, so nothing else reaches `timedatectl` or the file timesyncd reads.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// A time zone as the tz database names it: `Europe/Berlin`, `UTC`,
/// `America/Argentina/Buenos_Aires`, `Etc/GMT+1`. No path tricks: no empty
/// part, none starting with a dot or a dash. Whether the system has it is
/// for systemd-timedated to say.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TimeZone(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the name of a time zone")]
pub struct InvalidTimeZone;

impl TimeZone {
    pub fn parse(name: &str) -> Result<Self, InvalidTimeZone> {
        let part = |part: &str| {
            !part.is_empty()
                && !part.starts_with(['.', '-'])
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
        };
        if (1..=64).contains(&name.len()) && name.split('/').all(part) {
            Ok(Self(name.to_owned()))
        } else {
            Err(InvalidTimeZone)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TimeZone {
    type Error = InvalidTimeZone;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TimeZone> for String {
    fn from(zone: TimeZone) -> Self {
        zone.0
    }
}

/// A time server: a host name (`ptb.de`, `0.de.pool.ntp.org`) or an IP
/// address. Nothing with a space, so it cannot add to the line it is
/// written on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TimeServer(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a host name or IP address")]
pub struct InvalidTimeServer;

impl TimeServer {
    pub fn parse(host: &str) -> Result<Self, InvalidTimeServer> {
        if host.parse::<IpAddr>().is_ok() {
            return Ok(Self(host.to_owned()));
        }
        let label = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        let host_name = host.strip_suffix('.').unwrap_or(host);
        if (1..=253).contains(&host_name.len()) && host_name.split('.').all(label) {
            Ok(Self(host.to_ascii_lowercase()))
        } else {
            Err(InvalidTimeServer)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TimeServer {
    type Error = InvalidTimeServer;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TimeServer> for String {
    fn from(server: TimeServer) -> Self {
        server.0
    }
}

/// The time servers to ask, each once; none means Debian's pool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<TimeServer>", into = "Vec<TimeServer>")]
pub struct TimeServers(Vec<TimeServer>);

/// The most time servers; timesyncd asks one at a time anyway.
pub const MAX_TIME_SERVERS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most {MAX_TIME_SERVERS} time servers, each once")]
pub struct InvalidTimeServers;

impl TimeServers {
    pub fn new(servers: Vec<TimeServer>) -> Result<Self, InvalidTimeServers> {
        let unique = servers.iter().enumerate().all(|(at, server)| !servers[..at].contains(server));
        if servers.len() <= MAX_TIME_SERVERS && unique { Ok(Self(servers)) } else { Err(InvalidTimeServers) }
    }

    pub fn as_slice(&self) -> &[TimeServer] {
        &self.0
    }
}

impl TryFrom<Vec<TimeServer>> for TimeServers {
    type Error = InvalidTimeServers;

    fn try_from(value: Vec<TimeServer>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TimeServers> for Vec<TimeServer> {
    fn from(servers: TimeServers) -> Self {
        servers.0
    }
}

/// A moment to set the clock to, in seconds since 1970, from 2000 to
/// 2099: a clock set outside that is a typing error, and certificates
/// would stop working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct ClockTime(i64);

/// 2000-01-01 00:00:00 UTC.
const EARLIEST: i64 = 946_684_800;
/// 2100-01-01 00:00:00 UTC.
const LATEST: i64 = 4_102_444_800;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the clock can be set to a time from 2000 to 2099")]
pub struct InvalidClockTime;

impl ClockTime {
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, InvalidClockTime> {
        if (EARLIEST..LATEST).contains(&seconds) { Ok(Self(seconds)) } else { Err(InvalidClockTime) }
    }

    pub fn unix_seconds(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for ClockTime {
    type Error = InvalidClockTime;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::from_unix_seconds(value)
    }
}

impl From<ClockTime> for i64 {
    fn from(time: ClockTime) -> Self {
        time.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zones_are_names_not_paths() {
        for zone in ["UTC", "Europe/Berlin", "America/Argentina/Buenos_Aires", "Etc/GMT+1", "Etc/GMT-14"] {
            assert!(TimeZone::parse(zone).is_ok(), "{zone}");
        }
        for zone in
            ["", "/etc/passwd", "Europe/../../etc", "Europe//Berlin", "Europe/", ".hidden", "-u", "a b"]
        {
            assert!(TimeZone::parse(zone).is_err(), "{zone}");
        }
        assert!(TimeZone::parse(&"A".repeat(65)).is_err());
    }

    #[test]
    fn time_servers_are_host_names_or_addresses() {
        for host in ["ptb.de", "0.de.pool.ntp.org", "time.example.", "192.168.1.1", "2001:db8::1", "ntp"] {
            assert!(TimeServer::parse(host).is_ok(), "{host}");
        }
        for host in ["", "a b", "-x.de", "x-.de", "a..b", "a_b.de", "x\nNTP=evil", "#comment"] {
            assert!(TimeServer::parse(host).is_err(), "{host}");
        }
        assert_eq!(TimeServer::parse("PTB.de").unwrap().as_str(), "ptb.de");
    }

    #[test]
    fn a_server_list_is_short_and_without_repeats() {
        let server = |host: &str| TimeServer::parse(host).unwrap();
        assert!(TimeServers::new(vec![server("a.de"), server("b.de")]).is_ok());
        assert!(TimeServers::new(vec![server("a.de"), server("a.de")]).is_err());
        assert!(TimeServers::new((0..5).map(|i| server(&format!("s{i}.de"))).collect()).is_err());
        assert!(serde_json::from_str::<TimeServers>(r#"["a.de","a.de"]"#).is_err());
    }

    #[test]
    fn the_clock_is_set_within_this_century() {
        assert!(ClockTime::from_unix_seconds(1_800_000_000).is_ok());
        assert!(ClockTime::from_unix_seconds(0).is_err());
        assert!(ClockTime::from_unix_seconds(LATEST).is_err());
        assert!(serde_json::from_str::<ClockTime>("-1").is_err());
    }
}
