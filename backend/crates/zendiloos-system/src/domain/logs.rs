//! The system journal and the audit log, as administrators read them.

use zendiloos_kernel::Timestamp;

/// Where journal entries come from. A fixed set, so a request can only
/// choose among views, never pass arbitrary filters to `journalctl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// Everything.
    System,
    /// zendiloosd and its agent.
    Zendiloos,
    /// RAID and disk monitoring.
    Storage,
    /// The SMB server.
    Sharing,
    /// The kernel's messages.
    Kernel,
}

/// syslog severity; lower is more severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogPriority {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl LogPriority {
    pub fn from_number(value: u8) -> Option<Self> {
        Some(match value {
            0 => LogPriority::Emergency,
            1 => LogPriority::Alert,
            2 => LogPriority::Critical,
            3 => LogPriority::Error,
            4 => LogPriority::Warning,
            5 => LogPriority::Notice,
            6 => LogPriority::Info,
            7 => LogPriority::Debug,
            _ => return None,
        })
    }

    pub fn number(self) -> u8 {
        self as u8
    }
}

/// Position in the journal, as the journal itself reports it. Opaque, but
/// checked to be plain ASCII so it can be passed back safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCursor(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a journal position")]
pub struct InvalidCursor;

impl JournalCursor {
    const MAX_LEN: usize = 512;

    pub fn parse(value: &str) -> Result<Self, InvalidCursor> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_LEN
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"=;-_".contains(&b));
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidCursor) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalQuery {
    pub source: LogSource,
    /// This priority and everything more severe.
    pub priority: LogPriority,
    pub limit: u16,
    /// Entries older than this position; the newest if absent.
    pub before: Option<JournalCursor>,
}

impl JournalQuery {
    pub const MAX_LIMIT: u16 = 500;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Microseconds since the Unix epoch.
    pub at_micros: u64,
    pub priority: LogPriority,
    /// The systemd unit that logged it, e.g. `smbd.service`.
    pub unit: Option<String>,
    /// The program name, e.g. `smbd` or `kernel`.
    pub identifier: Option<String>,
    pub pid: Option<u32>,
    pub message: String,
    pub cursor: JournalCursor,
}

/// Newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    pub entries: Vec<JournalEntry>,
    /// Where the next, older page starts, if there may be one.
    pub older: Option<JournalCursor>,
}

/// One line of the audit log as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub id: i64,
    pub at: Timestamp,
    pub actor: Option<String>,
    pub client_ip: Option<String>,
    pub action: String,
    pub target: Option<String>,
    /// `success`, `failure` or `denied`.
    pub outcome: String,
    pub reason: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditQuery {
    pub limit: u16,
    /// Records with a smaller ID; the newest if absent.
    pub before: Option<i64>,
    /// Only records of this action, such as `identity.login`.
    pub action: Option<String>,
    /// Only records whose action starts so, such as `containers.`.
    pub prefix: Option<String>,
}

impl AuditQuery {
    pub const MAX_LIMIT: u16 = 500;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_plain_ascii() {
        let real = "s=6d4c3e3b9a0f4d0c8f8a5e1b2c3d4e5f;i=1a2b;b=0f1e2d3c4b5a69788796a5b4c3d2e1f0;m=3c4d5e;t=61f2a3b4c5d6e;x=7a8b9c0d1e2f3a4b";
        assert_eq!(JournalCursor::parse(real).unwrap().as_str(), real);
        for bad in ["", "s=1 --since=yesterday", "a\nb", "ü", &"x".repeat(513)] {
            assert_eq!(JournalCursor::parse(bad), Err(InvalidCursor), "{bad:?}");
        }
    }

    #[test]
    fn priorities_round_trip() {
        for n in 0..=7 {
            assert_eq!(LogPriority::from_number(n).unwrap().number(), n);
        }
        assert_eq!(LogPriority::from_number(8), None);
        assert!(LogPriority::Error < LogPriority::Warning, "more severe sorts first");
    }
}
