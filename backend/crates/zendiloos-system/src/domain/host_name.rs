//! The machine's name: what the network calls it (`<name>.local` through
//! Bonjour, the name Windows lists), what Samba answers to, what the web
//! interface's own certificate names.

/// The most characters a name has: NetBIOS's limit, so the one name holds
/// everywhere. Samba's NetBIOS name, which Windows shows, is the host
/// name's in uppercase, and a longer one would be cut there.
pub const HOST_NAME_MAX: usize = 15;

/// A name for the machine: one label as RFC 1123 allows (letters, digits
/// and `-`, neither first nor last a `-`), at most [`HOST_NAME_MAX`]
/// characters, not digits only (it would read as an address) and not
/// `localhost`; kept in lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidHostName {
    #[error("the name is empty")]
    Empty,
    #[error("the name has more than {HOST_NAME_MAX} characters")]
    TooLong,
    #[error("the name may hold only letters (a–z), digits and -")]
    Characters,
    #[error("the name may neither begin nor end with -")]
    Hyphen,
    #[error("the name may not be digits only")]
    DigitsOnly,
    #[error("localhost is the name every machine has for itself")]
    Reserved,
}

impl HostName {
    /// `input` without the spaces around it, in lowercase.
    pub fn parse(input: &str) -> Result<Self, InvalidHostName> {
        let name = input.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(InvalidHostName::Empty);
        }
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(InvalidHostName::Characters);
        }
        if name.len() > HOST_NAME_MAX {
            return Err(InvalidHostName::TooLong);
        }
        if name.starts_with('-') || name.ends_with('-') {
            return Err(InvalidHostName::Hyphen);
        }
        if name.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidHostName::DigitsOnly);
        }
        if name == "localhost" {
            return Err(InvalidHostName::Reserved);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// As Samba and Windows show it.
    pub fn netbios_name(&self) -> String {
        self.0.to_ascii_uppercase()
    }
}

/// What came of a rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    pub name: String,
    /// The web interface's own certificate names the new name.
    pub certificate_renewed: bool,
    /// The web interface serves that certificate: browsers meet a new one
    /// and ask to trust it again.
    pub own_certificate_served: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_short_label() {
        assert_eq!(HostName::parse("  Office-NAS ").unwrap().as_str(), "office-nas");
        assert_eq!(HostName::parse("nas2").unwrap().netbios_name(), "NAS2");
        assert!(HostName::parse(&"a".repeat(HOST_NAME_MAX)).is_ok());
        for (input, error) in [
            ("", InvalidHostName::Empty),
            ("   ", InvalidHostName::Empty),
            ("a-very-long-name", InvalidHostName::TooLong),
            ("nas.local", InvalidHostName::Characters),
            ("my nas", InvalidHostName::Characters),
            ("küche", InvalidHostName::Characters),
            ("nas_2", InvalidHostName::Characters),
            ("-nas", InvalidHostName::Hyphen),
            ("nas-", InvalidHostName::Hyphen),
            ("1234", InvalidHostName::DigitsOnly),
            ("LocalHost", InvalidHostName::Reserved),
        ] {
            assert_eq!(HostName::parse(input), Err(error), "{input:?}");
        }
    }
}
