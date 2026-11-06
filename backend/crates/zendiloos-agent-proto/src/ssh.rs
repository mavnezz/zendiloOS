//! The SSH server: whether it runs, on which port, and two of its rules.
//! Who may log in stays as the system has it. Checked while reading, as
//! everything that reaches a configuration file.

use serde::{Deserialize, Serialize};

/// The port SSH listens on: 22, or one from 1024 that no service of the
/// NAS uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct SshPort(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("SSH listens on 22 or a free port from 1024 to 65535")]
pub struct InvalidSshPort;

/// Ports the NAS's own services use: NFS, Avahi, WS-Discovery.
const TAKEN: [u16; 3] = [2049, 3702, 5353];

impl SshPort {
    pub fn new(port: u16) -> Result<Self, InvalidSshPort> {
        if port == 22 || (port >= 1024 && !TAKEN.contains(&port)) {
            Ok(Self(port))
        } else {
            Err(InvalidSshPort)
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for SshPort {
    type Error = InvalidSshPort;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SshPort> for u16 {
    fn from(port: SshPort) -> Self {
        port.0
    }
}

/// `PermitRootLogin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootLogin {
    No,
    /// With a key only.
    KeyOnly,
    Yes,
}

impl RootLogin {
    pub fn sshd_name(self) -> &'static str {
        match self {
            RootLogin::No => "no",
            RootLogin::KeyOnly => "prohibit-password",
            RootLogin::Yes => "yes",
        }
    }
}

/// How long SSH stays on before it turns itself off: 1 to 1440 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct OffAfter(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("SSH turns itself off after 1 to 1440 minutes")]
pub struct InvalidOffAfter;

impl OffAfter {
    pub fn minutes(minutes: u16) -> Result<Self, InvalidOffAfter> {
        if (1..=1440).contains(&minutes) { Ok(Self(minutes)) } else { Err(InvalidOffAfter) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for OffAfter {
    type Error = InvalidOffAfter;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::minutes(value)
    }
}

impl From<OffAfter> for u16 {
    fn from(off: OffAfter) -> Self {
        off.0
    }
}

/// The SSH server as it should be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshConfig {
    pub enabled: bool,
    pub port: SshPort,
    /// `None`: as the system's configuration has it.
    pub root_login: Option<RootLogin>,
    /// `PasswordAuthentication`; `None`: as the system's configuration has it.
    pub password_login: Option<bool>,
    /// Turns SSH off this long after it was turned on; `None`: never.
    pub off_after: Option<OffAfter>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_22_or_free_high_ones() {
        for port in [22, 1024, 2222, 65535] {
            assert!(SshPort::new(port).is_ok(), "{port}");
        }
        for port in [0, 21, 23, 80, 443, 445, 1023, 2049, 5353] {
            assert!(SshPort::new(port).is_err(), "{port}");
        }
        assert!(serde_json::from_str::<SshPort>("23").is_err());
    }

    #[test]
    fn ssh_turns_itself_off_within_a_day() {
        assert!(OffAfter::minutes(10).is_ok());
        assert!(OffAfter::minutes(0).is_err());
        assert!(OffAfter::minutes(1441).is_err());
        assert_eq!(RootLogin::KeyOnly.sshd_name(), "prohibit-password");
    }
}
