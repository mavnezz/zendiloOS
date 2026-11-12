//! The configuration backup's share of the host: the files zendiloOS wrote
//! that its database does not say again (the web interface's certificates,
//! Samba's passwords, ZFS's host id, the IDs handed out) and the accounts'
//! Linux users and groups with their IDs. Read whole into a backup, and
//! written back from one on another installation. Only these places, each
//! named by what it is, never by a path zendiloosd sends; settings the
//! agent writes from typed values (SSH, the clock, TLS, the host name) are
//! read here for zendiloosd to understand and go back through their own
//! requests, and the network's files are read only.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::{GroupName, UserName};

/// Where a file of the configuration lives. On the wire its absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ConfigurationPath {
    /// `/etc/zendiloos/tls/zendiloos.crt`: the machine's own certificate.
    OwnCertificate,
    /// `/etc/zendiloos/tls/zendiloos.key`.
    OwnKey,
    /// `/etc/zendiloos/web/certificates/<n>/fullchain.pem`: an issued one.
    IssuedCertificate(u32),
    /// `/etc/zendiloos/web/certificates/<n>/key.pem`.
    IssuedKey(u32),
    /// `/etc/zendiloos/nginx/tls.conf`: the oldest TLS version; read only.
    TlsSettings,
    /// `/etc/ssh/sshd_config.d/00-zendiloos.conf`; read only.
    SshSettings,
    /// `/etc/systemd/timesyncd.conf.d/zendiloos.conf`; read only.
    TimeServers,
    /// `/var/lib/samba/private/passdb.tdb`: the accounts' SMB passwords.
    SambaPasswords,
    /// `/etc/hostid`: ZFS's host id, by which a pool knows its machine.
    HostId,
    /// `/etc/zendiloos/id-high-water`: the highest UID and GID handed out.
    IdHighWater,
    /// `/etc/systemd/network/05-zendiloos-<name>.network` or `.netdev`;
    /// read only: a network from elsewhere is applied with confirmation.
    Network(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a file of zendiloOS's configuration")]
pub struct InvalidConfigurationPath;

const CERTIFICATES: &str = "/etc/zendiloos/web/certificates/";
const NETWORK: &str = "/etc/systemd/network/";

impl ConfigurationPath {
    /// Whether a restore writes it back as the backup has it.
    pub fn restorable(&self) -> bool {
        !matches!(self, Self::TlsSettings | Self::SshSettings | Self::TimeServers | Self::Network(_))
    }

    /// The most a file of it may hold.
    pub fn max_bytes(&self) -> usize {
        match self {
            Self::SambaPasswords => 8 * 1024 * 1024,
            Self::HostId => 4,
            Self::OwnCertificate | Self::IssuedCertificate(_) => 64 * 1024,
            _ => 16 * 1024,
        }
    }

    pub fn parse(path: &str) -> Result<Self, InvalidConfigurationPath> {
        Ok(match path {
            "/etc/zendiloos/tls/zendiloos.crt" => Self::OwnCertificate,
            "/etc/zendiloos/tls/zendiloos.key" => Self::OwnKey,
            "/etc/zendiloos/nginx/tls.conf" => Self::TlsSettings,
            "/etc/ssh/sshd_config.d/00-zendiloos.conf" => Self::SshSettings,
            "/etc/systemd/timesyncd.conf.d/zendiloos.conf" => Self::TimeServers,
            "/var/lib/samba/private/passdb.tdb" => Self::SambaPasswords,
            "/etc/hostid" => Self::HostId,
            "/etc/zendiloos/id-high-water" => Self::IdHighWater,
            _ => {
                if let Some(rest) = path.strip_prefix(CERTIFICATES) {
                    let (number, file) = rest.split_once('/').ok_or(InvalidConfigurationPath)?;
                    let valid = !number.is_empty()
                        && number.len() <= 9
                        && number.bytes().all(|b| b.is_ascii_digit())
                        && !number.starts_with('0');
                    let number: u32 =
                        if valid { number.parse().map_err(|_| InvalidConfigurationPath)? } else { 0 };
                    match file {
                        "fullchain.pem" if valid => Self::IssuedCertificate(number),
                        "key.pem" if valid => Self::IssuedKey(number),
                        _ => return Err(InvalidConfigurationPath),
                    }
                } else if let Some(name) = path.strip_prefix(NETWORK) {
                    if network_file_name(name) {
                        Self::Network(name.to_owned())
                    } else {
                        return Err(InvalidConfigurationPath);
                    }
                } else {
                    return Err(InvalidConfigurationPath);
                }
            }
        })
    }

    pub fn as_path(&self) -> String {
        match self {
            Self::OwnCertificate => "/etc/zendiloos/tls/zendiloos.crt".into(),
            Self::OwnKey => "/etc/zendiloos/tls/zendiloos.key".into(),
            Self::IssuedCertificate(n) => format!("{CERTIFICATES}{n}/fullchain.pem"),
            Self::IssuedKey(n) => format!("{CERTIFICATES}{n}/key.pem"),
            Self::TlsSettings => "/etc/zendiloos/nginx/tls.conf".into(),
            Self::SshSettings => "/etc/ssh/sshd_config.d/00-zendiloos.conf".into(),
            Self::TimeServers => "/etc/systemd/timesyncd.conf.d/zendiloos.conf".into(),
            Self::SambaPasswords => "/var/lib/samba/private/passdb.tdb".into(),
            Self::HostId => "/etc/hostid".into(),
            Self::IdHighWater => "/etc/zendiloos/id-high-water".into(),
            Self::Network(name) => format!("{NETWORK}{name}"),
        }
    }
}

/// `05-zendiloos-<interface>.network` or `.netdev`, as the agent names its
/// systemd-networkd files.
pub fn network_file_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("05-zendiloos-") else { return false };
    let Some(interface) = rest.strip_suffix(".network").or_else(|| rest.strip_suffix(".netdev")) else {
        return false;
    };
    (1..=15).contains(&interface.len())
        && interface.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !interface.starts_with(['.', '-'])
}

impl TryFrom<String> for ConfigurationPath {
    type Error = InvalidConfigurationPath;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConfigurationPath> for String {
    fn from(path: ConfigurationPath) -> Self {
        path.as_path()
    }
}

/// A file's content, on the wire in base64.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FileContent(Vec<u8>);

impl FileContent {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Keys and passwords go by: never in a log.
impl std::fmt::Debug for FileContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileContent({} bytes)", self.0.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not base64")]
pub struct InvalidFileContent;

impl TryFrom<String> for FileContent {
    type Error = InvalidFileContent;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        base64::engine::general_purpose::STANDARD.decode(value).map(Self).map_err(|_| InvalidFileContent)
    }
}

impl From<FileContent> for String {
    fn from(content: FileContent) -> Self {
        base64::engine::general_purpose::STANDARD.encode(content.0)
    }
}

/// One file of the configuration, no larger than its place allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ConfigurationFileFields", into = "ConfigurationFileFields")]
pub struct ConfigurationFile {
    pub path: ConfigurationPath,
    pub content: FileContent,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationFileFields {
    path: ConfigurationPath,
    content: FileContent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the file is larger than its place allows")]
pub struct InvalidConfigurationFile;

impl ConfigurationFile {
    pub fn new(path: ConfigurationPath, content: Vec<u8>) -> Result<Self, InvalidConfigurationFile> {
        Self::try_from(ConfigurationFileFields { path, content: FileContent(content) })
    }
}

impl TryFrom<ConfigurationFileFields> for ConfigurationFile {
    type Error = InvalidConfigurationFile;

    fn try_from(fields: ConfigurationFileFields) -> Result<Self, Self::Error> {
        if fields.content.0.len() > fields.path.max_bytes() {
            return Err(InvalidConfigurationFile);
        }
        Ok(Self { path: fields.path, content: fields.content })
    }
}

impl From<ConfigurationFile> for ConfigurationFileFields {
    fn from(file: ConfigurationFile) -> Self {
        Self { path: file.path, content: file.content }
    }
}

/// The IDs the agent hands out to people's users and groups.
pub const MANAGED_IDS: std::ops::RangeInclusive<u32> = 2000..=59999;

/// An account's Linux user, with the UID its files have on the volumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredUser {
    pub name: UserName,
    #[serde(deserialize_with = "managed_id")]
    pub uid: u32,
}

/// A group, with its GID and members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredGroup {
    pub name: GroupName,
    #[serde(deserialize_with = "managed_id")]
    pub gid: u32,
    pub members: Vec<UserName>,
}

fn managed_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let id = u32::deserialize(deserializer)?;
    if MANAGED_IDS.contains(&id) {
        Ok(id)
    } else {
        Err(serde::de::Error::custom("an ID zendiloOS hands out is from 2000 to 59999"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_files_are_named_by_what_they_are() {
        for path in [
            "/etc/zendiloos/tls/zendiloos.crt",
            "/etc/zendiloos/tls/zendiloos.key",
            "/etc/zendiloos/web/certificates/3/fullchain.pem",
            "/etc/zendiloos/web/certificates/12/key.pem",
            "/etc/zendiloos/nginx/tls.conf",
            "/etc/ssh/sshd_config.d/00-zendiloos.conf",
            "/etc/systemd/timesyncd.conf.d/zendiloos.conf",
            "/var/lib/samba/private/passdb.tdb",
            "/etc/hostid",
            "/etc/zendiloos/id-high-water",
            "/etc/systemd/network/05-zendiloos-enp1s0.network",
            "/etc/systemd/network/05-zendiloos-br0.netdev",
        ] {
            let parsed = ConfigurationPath::parse(path).unwrap_or_else(|_| panic!("{path}"));
            assert_eq!(parsed.as_path(), path);
        }
        for path in [
            "/etc/shadow",
            "/etc/zendiloos/web/certificates/../../../shadow/key.pem",
            "/etc/zendiloos/web/certificates/03/key.pem",
            "/etc/zendiloos/web/certificates/3/other.pem",
            "/etc/zendiloos/web/certificates/3",
            "/etc/systemd/network/10-other.network",
            "/etc/systemd/network/05-zendiloos-../x.network",
            "/etc/systemd/network/05-zendiloos-enp1s0.link",
            "/etc/hostid/",
        ] {
            assert!(ConfigurationPath::parse(path).is_err(), "{path}");
        }
        assert!(
            ConfigurationPath::SambaPasswords.restorable() && ConfigurationPath::IssuedKey(1).restorable()
        );
        assert!(!ConfigurationPath::SshSettings.restorable());
        assert!(!ConfigurationPath::Network("05-zendiloos-eth0.network".into()).restorable());
    }

    #[test]
    fn contents_travel_as_base64_within_their_limit() {
        let file = ConfigurationFile::new(ConfigurationPath::HostId, vec![1, 2, 3, 4]).unwrap();
        let json = serde_json::to_string(&file).unwrap();
        assert_eq!(json, r#"{"path":"/etc/hostid","content":"AQIDBA=="}"#);
        assert_eq!(serde_json::from_str::<ConfigurationFile>(&json).unwrap(), file);
        assert!(ConfigurationFile::new(ConfigurationPath::HostId, vec![0; 5]).is_err());
        assert!(
            serde_json::from_str::<ConfigurationFile>(r#"{"path":"/etc/hostid","content":"AQIDBAU="}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ConfigurationFile>(r#"{"path":"/etc/hostid","content":"*"}"#).is_err()
        );
        assert_eq!(format!("{:?}", file.content), "FileContent(4 bytes)");
    }

    #[test]
    fn restored_ids_are_the_managed_ones() {
        assert!(serde_json::from_str::<RestoredUser>(r#"{"name":"anna","uid":2001}"#).is_ok());
        assert!(serde_json::from_str::<RestoredUser>(r#"{"name":"anna","uid":0}"#).is_err());
        assert!(serde_json::from_str::<RestoredUser>(r#"{"name":"anna","uid":1000}"#).is_err());
        assert!(
            serde_json::from_str::<RestoredGroup>(r#"{"name":"family","gid":60000,"members":[]}"#).is_err()
        );
    }
}
