//! The SSH server, as a service of the machine: on or off, its port, and
//! two of its rules. Who may log in stays as the system has it.

use zendiloos_kernel::Timestamp;

/// Whether root may log in over SSH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootLogin {
    No,
    /// With a key only.
    KeyOnly,
    Yes,
}

/// Ports the NAS's own services use.
const TAKEN: [u16; 3] = [2049, 3702, 5353];

/// The SSH server as someone wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSettings {
    pub enabled: bool,
    pub port: u16,
    /// `None`: as the system's configuration has it.
    pub root_login: Option<RootLogin>,
    /// `None`: as the system's configuration has it.
    pub password_login: Option<bool>,
    /// Turned on, it turns itself off after this many minutes; `None`: never.
    pub off_after_minutes: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSshSettings {
    #[error("SSH listens on 22 or on a port from 1024 to 65535 that no service of the NAS uses")]
    Port,
    #[error("SSH turns itself off after 1 to 1440 minutes")]
    OffAfter,
}

impl SshSettings {
    /// The settings, if they hold up; turned off, there is nothing to turn
    /// off later.
    pub fn checked(mut self) -> Result<Self, InvalidSshSettings> {
        if self.port != 22 && (self.port < 1024 || TAKEN.contains(&self.port)) {
            return Err(InvalidSshSettings::Port);
        }
        if self.off_after_minutes.is_some_and(|minutes| !(1..=1440).contains(&minutes)) {
            return Err(InvalidSshSettings::OffAfter);
        }
        if !self.enabled {
            self.off_after_minutes = None;
        }
        Ok(self)
    }
}

/// The SSH server as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshStatus {
    /// The server is installed; without it, nothing can be turned on.
    pub installed: bool,
    /// Starts with the machine.
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub root_login: Option<RootLogin>,
    pub password_login: Option<bool>,
    /// When it turns itself off.
    pub off_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(
        port: u16,
        off_after_minutes: Option<u16>,
        enabled: bool,
    ) -> Result<SshSettings, InvalidSshSettings> {
        SshSettings { enabled, port, root_login: None, password_login: None, off_after_minutes }.checked()
    }

    #[test]
    fn ports_and_timers_are_checked() {
        assert!(with(22, None, true).is_ok());
        assert!(with(2222, Some(10), true).is_ok());
        assert_eq!(with(23, None, true), Err(InvalidSshSettings::Port));
        assert_eq!(with(2049, None, true), Err(InvalidSshSettings::Port));
        assert_eq!(with(22, Some(0), true), Err(InvalidSshSettings::OffAfter));
        assert_eq!(with(22, Some(10), false).unwrap().off_after_minutes, None, "off: no timer");
    }
}
