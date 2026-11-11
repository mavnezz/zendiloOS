//! Certificates from an ACME server through acme.sh, and the ports and
//! certificate of the web interface, served by nginx. Every value is
//! checked while it is read: a name reaches acme.sh's command line, and a
//! setting nginx's files, only in a form these types allow.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A host name a certificate is for, in lower case, or a wildcard
/// (`*.example.org`): letters, digits and hyphens in labels of 1 to 63,
/// at least two labels, at most 253 characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AcmeDomain(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a host name for a certificate")]
pub struct InvalidAcmeDomain;

impl AcmeDomain {
    pub fn parse(name: &str) -> Result<Self, InvalidAcmeDomain> {
        let host = name.strip_prefix("*.").unwrap_or(name);
        let label = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        let labels: Vec<&str> = host.split('.').collect();
        let valid = name.len() <= 253
            && labels.len() >= 2
            && labels.iter().all(|l| label(l))
            && !labels.last().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidAcmeDomain) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_wildcard(&self) -> bool {
        self.0.starts_with("*.")
    }
}

impl TryFrom<String> for AcmeDomain {
    type Error = InvalidAcmeDomain;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AcmeDomain> for String {
    fn from(name: AcmeDomain) -> Self {
        name.0
    }
}

/// The names of one certificate: 1 to 50, none twice. The first is its
/// common name, and the name of acme.sh's folder for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<AcmeDomain>", into = "Vec<AcmeDomain>")]
pub struct AcmeDomains(Vec<AcmeDomain>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a certificate is for 1 to 50 names, none twice")]
pub struct InvalidAcmeDomains;

impl AcmeDomains {
    pub fn new(names: Vec<AcmeDomain>) -> Result<Self, InvalidAcmeDomains> {
        let mut seen: Vec<&AcmeDomain> = names.iter().collect();
        seen.sort();
        seen.dedup();
        if (1..=50).contains(&names.len()) && seen.len() == names.len() {
            Ok(Self(names))
        } else {
            Err(InvalidAcmeDomains)
        }
    }

    pub fn as_slice(&self) -> &[AcmeDomain] {
        &self.0
    }

    pub fn main(&self) -> &AcmeDomain {
        &self.0[0]
    }
}

impl TryFrom<Vec<AcmeDomain>> for AcmeDomains {
    type Error = InvalidAcmeDomains;

    fn try_from(value: Vec<AcmeDomain>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AcmeDomains> for Vec<AcmeDomain> {
    fn from(names: AcmeDomains) -> Self {
        names.0
    }
}

/// The ACME servers acme.sh is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcmeCa {
    LetsEncrypt,
    /// For trying out: certificates no browser trusts, generous limits.
    LetsEncryptStaging,
}

impl AcmeCa {
    /// acme.sh's `--server` for it.
    pub fn server(self) -> &'static str {
        match self {
            AcmeCa::LetsEncrypt => "letsencrypt",
            AcmeCa::LetsEncryptStaging => "letsencrypt_test",
        }
    }
}

/// An account's contact address, as the ACME server is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AcmeEmail(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an e-mail address")]
pub struct InvalidAcmeEmail;

impl AcmeEmail {
    pub fn parse(address: &str) -> Result<Self, InvalidAcmeEmail> {
        let valid = address.len() <= 254
            && !address.starts_with('-')
            && address.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty()
                    && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
                    && AcmeDomain::parse(domain).is_ok_and(|d| !d.is_wildcard())
            });
        if valid { Ok(Self(address.to_owned())) } else { Err(InvalidAcmeEmail) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AcmeEmail {
    type Error = InvalidAcmeEmail;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AcmeEmail> for String {
    fn from(address: AcmeEmail) -> Self {
        address.0
    }
}

/// The key a certificate gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcmeKey {
    Ec256,
    Rsa2048,
    Rsa4096,
}

impl AcmeKey {
    /// acme.sh's `--keylength` for it.
    pub fn keylength(self) -> &'static str {
        match self {
            AcmeKey::Ec256 => "ec-256",
            AcmeKey::Rsa2048 => "2048",
            AcmeKey::Rsa4096 => "4096",
        }
    }

    pub fn is_ecc(self) -> bool {
        self == AcmeKey::Ec256
    }
}

/// One of acme.sh's DNS API scripts: `dns_ionos`, `dns_cf`. The agent
/// also requires it to be among those acme.sh brings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DnsApi(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the name of a DNS API script of acme.sh")]
pub struct InvalidDnsApi;

impl DnsApi {
    pub fn parse(name: &str) -> Result<Self, InvalidDnsApi> {
        let valid = name.len() <= 40
            && name.strip_prefix("dns_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            });
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidDnsApi) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DnsApi {
    type Error = InvalidDnsApi;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DnsApi> for String {
    fn from(name: DnsApi) -> Self {
        name.0
    }
}

/// The name of a variable a DNS API script reads (`IONOS_PREFIX`): starts
/// with a capital letter, then letters, digits and `_`. Names that change
/// how programs run (`PATH`, `LD_PRELOAD`) or acme.sh's own (`LE_…`,
/// `ACME_…`) are refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DnsVariableName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the name of a DNS API's variable")]
pub struct InvalidDnsVariableName;

const REFUSED_VARIABLES: &[&str] = &[
    "PATH", "HOME", "IFS", "ENV", "BASH_ENV", "SHELL", "USER", "LOGNAME", "TMPDIR", "LANG", "PS4", "CDPATH",
];
/// Also what curl and OpenSSL read (`SSL_CERT_FILE`, `CURL_CA_BUNDLE`), as
/// acme.sh talks to the authority and the providers through them.
const REFUSED_PREFIXES: &[&str] = &[
    "LD_", "LC_", "LE_", "ACME_", "CA_", "CERT_", "SAVED_", "DEBUG", "SYSTEMD_", "SSL_", "CURL_", "OPENSSL_",
];

impl DnsVariableName {
    /// A few providers' scripts read names that begin in lowercase
    /// (`cPanel_Username`, `pleskxml_uri`), so the lists above are compared
    /// whatever the case, and no proxy setting (`https_proxy`, `ALL_PROXY`)
    /// is taken: curl reads both spellings.
    pub fn parse(name: &str) -> Result<Self, InvalidDnsVariableName> {
        let upper = name.to_ascii_uppercase();
        let valid = (1..=64).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !REFUSED_VARIABLES.contains(&upper.as_str())
            && !REFUSED_PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
            && !upper.ends_with("_PROXY");
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidDnsVariableName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DnsVariableName {
    type Error = InvalidDnsVariableName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DnsVariableName> for String {
    fn from(name: DnsVariableName) -> Self {
        name.0
    }
}

/// A DNS API's value, often a secret: up to 4096 bytes, no control
/// characters. Never shown in logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DnsValue(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a DNS API's value is up to 4096 bytes without control characters")]
pub struct InvalidDnsValue;

impl DnsValue {
    pub fn new(value: &str) -> Result<Self, InvalidDnsValue> {
        let valid = (1..=4096).contains(&value.len()) && !value.chars().any(char::is_control);
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidDnsValue) }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DnsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DnsValue(…)")
    }
}

impl TryFrom<String> for DnsValue {
    type Error = InvalidDnsValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<DnsValue> for String {
    fn from(value: DnsValue) -> Self {
        value.0
    }
}

/// What a DNS API script is given: at most 16 variables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BTreeMap<DnsVariableName, DnsValue>", into = "BTreeMap<DnsVariableName, DnsValue>")]
pub struct DnsCredentials(BTreeMap<DnsVariableName, DnsValue>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at most 16 variables for a DNS API")]
pub struct InvalidDnsCredentials;

impl DnsCredentials {
    pub fn new(values: BTreeMap<DnsVariableName, DnsValue>) -> Result<Self, InvalidDnsCredentials> {
        if values.len() <= 16 { Ok(Self(values)) } else { Err(InvalidDnsCredentials) }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&DnsVariableName, &DnsValue)> {
        self.0.iter()
    }
}

impl TryFrom<BTreeMap<DnsVariableName, DnsValue>> for DnsCredentials {
    type Error = InvalidDnsCredentials;

    fn try_from(value: BTreeMap<DnsVariableName, DnsValue>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DnsCredentials> for BTreeMap<DnsVariableName, DnsValue> {
    fn from(credentials: DnsCredentials) -> Self {
        credentials.0
    }
}

/// How the ACME server is shown the names are ours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum AcmeChallenge {
    /// A TXT record, made through the DNS provider's API.
    Dns { api: DnsApi, credentials: DnsCredentials },
    /// A file nginx serves on port 80, under `/.well-known/acme-challenge/`.
    Http,
}

/// A certificate to issue (or issue again).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcmeOrder {
    /// zendiloosd's number for it: the name of its folders.
    pub certificate: u32,
    /// zendiloosd's number for the account: each has acme.sh settings of
    /// its own.
    pub account: u32,
    pub ca: AcmeCa,
    pub email: AcmeEmail,
    pub domains: AcmeDomains,
    pub key: AcmeKey,
    pub challenge: AcmeChallenge,
}

/// A port of the web interface: 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct WebPort(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a port is from 1 to 65535")]
pub struct InvalidWebPort;

impl WebPort {
    pub fn new(port: u16) -> Result<Self, InvalidWebPort> {
        if port == 0 { Err(InvalidWebPort) } else { Ok(Self(port)) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for WebPort {
    type Error = InvalidWebPort;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<WebPort> for u16 {
    fn from(port: WebPort) -> Self {
        port.0
    }
}

/// Where nginx serves the web interface, and with which certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebServerConfig {
    /// Plain HTTP, which points to HTTPS (and answers HTTP challenges);
    /// none: not served.
    pub http: Option<WebPort>,
    pub https: WebPort,
    /// An issued certificate by its number; none: the machine's own,
    /// self-signed one.
    pub certificate: Option<u32>,
}

/// A change of the web server that waits to be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingWebServer {
    /// When the old settings return, in Unix seconds.
    pub revert_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_while_they_are_read() {
        for good in ["nas.example.org", "*.example.org", "a-b.c0.example", "xn--bcher-kva.example"] {
            assert_eq!(AcmeDomain::parse(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "localhost",
            "NAS.example.org",
            "-a.example.org",
            "a..example",
            "*.*.example.org",
            "a.*.example.org",
            "1.2.3.4",
            "nas.example.org.",
            "nas example.org",
            "--server=x.example",
            "a_b.example.org",
        ] {
            assert!(AcmeDomain::parse(bad).is_err(), "{bad:?}");
        }
        let long = format!("{}.example.org", ["a"; 130].join("."));
        assert!(AcmeDomain::parse(&long).is_err());
        let one = AcmeDomain::parse("nas.example.org").unwrap();
        assert!(AcmeDomains::new(vec![one.clone(), one.clone()]).is_err());
        assert!(AcmeDomains::new(vec![]).is_err());
        assert_eq!(AcmeDomains::new(vec![one.clone()]).unwrap().main(), &one);
    }

    #[test]
    fn credentials_never_show_and_never_steer_a_program() {
        assert!(DnsApi::parse("dns_ionos").is_ok());
        for bad in ["ionos", "dns_", "dns_../x", "dns_IONOS", "dns_a b"] {
            assert!(DnsApi::parse(bad).is_err(), "{bad:?}");
        }
        for good in ["IONOS_PREFIX", "CF_Token", "DuckDNS_Token", "cPanel_Username", "pleskxml_uri"] {
            assert!(DnsVariableName::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "PATH",
            "path",
            "LD_PRELOAD",
            "ld_preload",
            "LE_WORKING_DIR",
            "ACME_DIRECTORY",
            "A-B",
            "",
            "SAVED_X",
            "_X",
            "1X",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "SSL_CERT_FILE",
            "CURL_CA_BUNDLE",
            "OPENSSL_CONF",
        ] {
            assert!(DnsVariableName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(DnsValue::new("line\nbreak").is_err());
        assert!(DnsValue::new("").is_err());
        let value = DnsValue::new("s3cr3t \"$x\"").unwrap();
        assert_eq!(format!("{value:?}"), "DnsValue(…)");
        let json =
            r#"{"method":"dns","api":"dns_ionos","credentials":{"IONOS_PREFIX":"p","IONOS_SECRET":"s"}}"#;
        let challenge: AcmeChallenge = serde_json::from_str(json).unwrap();
        assert!(!format!("{challenge:?}").contains("\"s\""));
        let bad = json.replace("IONOS_SECRET", "LD_PRELOAD");
        assert!(serde_json::from_str::<AcmeChallenge>(&bad).is_err());
    }

    #[test]
    fn the_rest_of_an_order_is_checked_too() {
        assert!(AcmeEmail::parse("admin@example.org").is_ok());
        for bad in ["admin", "-x@example.org", "a b@example.org", "admin@*.example.org", "a@b"] {
            assert!(AcmeEmail::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            (AcmeCa::LetsEncryptStaging.server(), AcmeKey::Rsa4096.keylength()),
            ("letsencrypt_test", "4096")
        );
        assert!(serde_json::from_str::<WebPort>("0").is_err());
        let config: WebServerConfig =
            serde_json::from_str(r#"{"http":null,"https":8443,"certificate":3}"#).unwrap();
        assert_eq!((config.http, config.https.get(), config.certificate), (None, 8443, Some(3)));
    }
}
