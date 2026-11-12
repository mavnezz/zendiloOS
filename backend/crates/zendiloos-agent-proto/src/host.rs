//! The machine's name. Checked while it is read, so nothing else reaches
//! `hostnamectl`, `/etc/hosts` or the certificate's subject.

use serde::{Deserialize, Serialize};

/// A host name as RFC 1123 allows one label of it: 1 to 63 letters, digits
/// and `-`, neither first nor last a `-`, not digits only (it would read as
/// an address), kept in lowercase. No dot: the machine's name is one label,
/// the domain is the network's. zendiloosd allows fewer characters (15, as
/// NetBIOS does); this is what the agent lets through.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HostName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a host name: 1 to 63 letters, digits and -, not digits only")]
pub struct InvalidHostName;

impl HostName {
    pub const MAX_LEN: usize = 63;

    pub fn parse(name: &str) -> Result<Self, InvalidHostName> {
        let valid = (1..=Self::MAX_LEN).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.bytes().all(|b| b.is_ascii_digit());
        if valid { Ok(Self(name.to_ascii_lowercase())) } else { Err(InvalidHostName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HostName {
    type Error = InvalidHostName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<HostName> for String {
    fn from(name: HostName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_are_one_label_of_rfc_1123() {
        for name in ["nas", "NAS-2", "zendiloos", "a", "2nas", "x1", &"a".repeat(63)] {
            assert!(HostName::parse(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "-nas",
            "nas-",
            "nas.local",
            "my nas",
            "nas_2",
            "1234",
            "küche",
            "nas\n127.0.0.1 evil",
            "--help",
            &"a".repeat(64),
        ] {
            assert!(HostName::parse(name).is_err(), "{name}");
        }
        assert_eq!(HostName::parse("Office-NAS").unwrap().as_str(), "office-nas");
        assert!(serde_json::from_str::<HostName>(r#""nas.example""#).is_err());
    }
}
