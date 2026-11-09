use std::net::IpAddr;

use zendiloos_kernel::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShareId(i64);

impl ShareId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Name of a share: also its dataset, its directory below the volume and
/// its SMB name. A lowercase letter, then up to 31 lowercase letters,
/// digits, `_` or `-`; not a name Samba reserves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShareName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidShareName {
    #[error("the name must be 1 to 32 characters long")]
    Length,
    #[error(
        "the name must start with a lowercase letter and contain only lowercase letters, digits, '_' and '-'"
    )]
    Characters,
    #[error("the name {0:?} is reserved")]
    Reserved(String),
}

impl ShareName {
    /// Samba's own sections, and `home`, the share of the personal folders.
    const RESERVED: [&str; 6] = ["global", "homes", "home", "printers", "print", "ipc"];

    pub fn parse(name: &str) -> Result<Self, InvalidShareName> {
        if name.is_empty() || name.len() > 32 {
            return Err(InvalidShareName::Length);
        }
        let valid = name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !valid {
            return Err(InvalidShareName::Characters);
        }
        if Self::RESERVED.contains(&name) {
            return Err(InvalidShareName::Reserved(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShareName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What is granted: reading, or reading and writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    ReadWrite,
}

impl Access {
    /// The broader of the two.
    pub fn max(self, other: Access) -> Access {
        if self == Access::ReadWrite || other == Access::ReadWrite { Access::ReadWrite } else { Access::Read }
    }
}

/// What a rule says: no access at all, which wins over every other rule,
/// or a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Deny,
    Read,
    ReadWrite,
}

impl Permission {
    /// What it grants; nothing for a denial.
    pub fn grant(self) -> Option<Access> {
        match self {
            Permission::Deny => None,
            Permission::Read => Some(Access::Read),
            Permission::ReadWrite => Some(Access::ReadWrite),
        }
    }
}

impl From<Access> for Permission {
    fn from(access: Access) -> Self {
        match access {
            Access::Read => Permission::Read,
            Access::ReadWrite => Permission::ReadWrite,
        }
    }
}

/// Who a rule is for: an account or a group, by ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subject {
    Account(i64),
    Group(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRule {
    pub subject: Subject,
    pub permission: Permission,
}

/// What `account`, a member of `groups`, may do where `rules` apply.
///
/// A denial wins, whether for the account or one of its groups; then the
/// account's own rule, more specific than its groups'; then the broadest
/// grant of its groups. The file manager decides with it; Samba's sections
/// say the same (`invalid users` wins over `valid users` and `write list`).
pub fn effective(rules: &[AccessRule], account: i64, groups: &[i64]) -> Option<Access> {
    let applies = |rule: &&AccessRule| match rule.subject {
        Subject::Account(id) => id == account,
        Subject::Group(id) => groups.contains(&id),
    };
    if rules.iter().filter(applies).any(|rule| rule.permission == Permission::Deny) {
        return None;
    }
    let own = rules.iter().find(|rule| rule.subject == Subject::Account(account));
    if let Some(own) = own {
        return own.permission.grant();
    }
    rules.iter().filter(applies).filter_map(|rule| rule.permission.grant()).reduce(Access::max)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not an IP address or network such as 192.168.1.0/24")]
pub struct InvalidNetwork(pub String);

/// Validates and normalizes an IP address or CIDR network.
pub fn network(value: &str) -> Result<String, InvalidNetwork> {
    let invalid = || InvalidNetwork(value.to_owned());
    let (address, prefix) = match value.trim().split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (value.trim(), None),
    };
    let address: IpAddr = address.parse().map_err(|_| invalid())?;
    let maximum = if address.is_ipv4() { 32 } else { 128 };
    match prefix {
        None => Ok(address.to_string()),
        Some(prefix) => {
            let prefix: u8 = prefix.parse().map_err(|_| invalid())?;
            if prefix > maximum {
                return Err(invalid());
            }
            Ok(if prefix == maximum { address.to_string() } else { format!("{address}/{prefix}") })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is no IP address, network, host name or pattern such as *.example.com")]
pub struct InvalidNfsHost(pub String);

/// Longest host name or pattern an NFS client may be given by.
pub const MAX_NFS_HOST: usize = 253;

/// Validates and normalizes who an NFS rule is for: an IP address or CIDR
/// network ([`network`]), or a host name, possibly with the wildcards `*`
/// and `?` as `exportfs` takes them (`nas.local`, `*.example.com`),
/// lowercase. Nothing else reaches the exports, whose syntax would read
/// spaces, parentheses or commas as more.
pub fn nfs_host(value: &str) -> Result<String, InvalidNfsHost> {
    let trimmed = value.trim();
    if let Ok(network) = network(trimmed) {
        return Ok(network);
    }
    let host = trimmed.to_ascii_lowercase();
    let label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-*?".contains(&b))
    };
    // A name, not an address in disguise: some letter or wildcard.
    let named = host.bytes().any(|b| b.is_ascii_lowercase() || b == b'*' || b == b'?');
    if host.len() <= MAX_NFS_HOST && named && host.split('.').all(label) {
        Ok(host)
    } else {
        Err(InvalidNfsHost(value.to_owned()))
    }
}

/// A host or network that may mount a share over NFS, and how. NFS with
/// `sec=sys` believes whatever user a client says it acts for, so access
/// over NFS is the client's, not a person's: whoever is on a client a rule
/// names has the share's read, or read and write, access, whoever they
/// claim to be. Every request is squashed to the group `zendiloos-files`,
/// whose access every share's folder gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NfsClient {
    /// Normalized by [`nfs_host`].
    pub network: String,
    pub write: bool,
    /// Answers writes before they are on disk: faster, but what a crash
    /// catches is lost although the client was told it was written.
    pub asynchronous: bool,
    /// Also from ports above 1024, which any user of the client may open.
    pub insecure: bool,
}

impl NfsClient {
    /// A client with writes on disk before they are answered, from
    /// privileged ports only.
    pub fn new(network: impl Into<String>, write: bool) -> Self {
        Self { network: network.into(), write, asynchronous: false, insecure: false }
    }
}

/// How a share's dataset compresses what is written to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Compression {
    Off,
    /// Fast, and what ZFS recommends by default.
    #[default]
    Lz4,
    /// Smaller, costs more CPU; for archives and documents.
    Zstd,
}

impl Compression {
    pub const ALL: [Compression; 3] = [Self::Off, Self::Lz4, Self::Zstd];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == value)
    }
}

/// The largest block ZFS writes for a file: small for databases and VM
/// images, large for big media files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecordSize {
    K16,
    /// ZFS's default; right for most files.
    #[default]
    K128,
    M1,
}

impl RecordSize {
    pub const ALL: [RecordSize; 3] = [Self::K16, Self::K128, Self::M1];

    pub fn bytes(self) -> u64 {
        match self {
            Self::K16 => 16 * 1024,
            Self::K128 => 128 * 1024,
            Self::M1 => 1024 * 1024,
        }
    }

    pub fn from_bytes(bytes: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.bytes() == bytes)
    }
}

/// The smallest quota: below it, a share fills up with its first photos.
pub const MIN_QUOTA_BYTES: u64 = 1024 * 1024 * 1024;

/// How a share's dataset stores data. The quota limits the share's own
/// data (`refquota`): snapshots do not count against it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShareSettings {
    pub quota_bytes: Option<u64>,
    pub compression: Compression,
    pub record_size: RecordSize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSettings {
    #[error("a quota is at least 1 GiB")]
    QuotaTooSmall,
    #[error("the quota is below what the share holds ({used} bytes)")]
    QuotaBelowUsage { used: u64 },
}

impl ShareSettings {
    pub fn check(&self) -> Result<(), InvalidSettings> {
        match self.quota_bytes {
            Some(quota) if quota < MIN_QUOTA_BYTES => Err(InvalidSettings::QuotaTooSmall),
            _ => Ok(()),
        }
    }

    /// A quota below what the share holds already would stop every write
    /// at once; that is refused instead.
    pub fn check_against(&self, used: u64) -> Result<(), InvalidSettings> {
        self.check()?;
        match self.quota_bytes {
            Some(quota) if quota < used => Err(InvalidSettings::QuotaBelowUsage { used }),
            _ => Ok(()),
        }
    }
}

/// What a share's dataset holds and may still take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareUsage {
    pub pool: String,
    pub name: ShareName,
    pub used_bytes: u64,
    /// Up to the quota, or what the pool has left.
    pub available_bytes: u64,
}

/// A folder directly on a volume: its own dataset, with its rules, which
/// hold for the folder as a whole, in the file manager as over SMB. Shared,
/// it is also offered on the network, over SMB and, with clients, NFS; not
/// shared, it is used on the NAS only. Deeper folders are plain folders and
/// never shared on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub id: ShareId,
    pub name: ShareName,
    pub description: Option<String>,
    /// The pool it lives on and the pool's volume number.
    pub pool: String,
    pub volume: u16,
    /// Offered on the network: over SMB, and over NFS to its clients.
    pub shared: bool,
    pub recycle_bin: bool,
    pub settings: ShareSettings,
    pub access: Vec<AccessRule>,
    /// Empty: not exported over NFS.
    pub nfs: Vec<NfsClient>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Share {
    pub fn path(&self) -> String {
        format!("/volume{}/{}", self.volume, self.name)
    }

    /// The rule for `subject`, if there is one.
    pub fn rule_of(&self, subject: Subject) -> Option<Permission> {
        self.access.iter().find(|rule| rule.subject == subject).map(|rule| rule.permission)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewShare {
    pub name: ShareName,
    pub description: Option<String>,
    pub pool: String,
    pub volume: u16,
    pub shared: bool,
    pub recycle_bin: bool,
    pub settings: ShareSettings,
    pub access: Vec<AccessRule>,
    pub nfs: Vec<NfsClient>,
    pub created_at: Timestamp,
}

/// Access the account of an app installed the earlier way was given to a
/// share, as recorded then. Its entries on the share's files stay as they
/// were written then, so the app keeps working; the record keeps the share
/// from being deleted while the app uses it, and goes with the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAccess {
    pub share: ShareId,
    /// The Linux account the app runs as, `app-<id>`.
    pub account: String,
    pub access: Access,
}

/// A user or group by name, as the host knows them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    User(String),
    Group(String),
}

/// One Samba share as the host should offer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbSpec {
    pub name: ShareName,
    pub volume: u16,
    /// Groups as groups: Samba follows their members itself.
    pub access: Vec<(Principal, Permission)>,
    pub recycle_bin: bool,
    /// The share's description, shown by clients.
    pub comment: Option<String>,
    /// Macs back up to it; `Some(None)`: without a limit of its own.
    pub time_machine: Option<Option<u64>>,
}

/// One NFS export as the host should offer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NfsSpec {
    pub name: ShareName,
    pub volume: u16,
    pub clients: Vec<NfsClient>,
}

/// Everything the SMB and NFS servers should offer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharingConfig {
    pub smb: Vec<SmbSpec>,
    pub nfs: Vec<NfsSpec>,
    pub services: super::FileServices,
    /// The share `home`, while personal folders are on.
    pub homes: Option<super::HomesSpec>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quota_is_at_least_a_gibibyte_and_never_below_what_is_there() {
        let gib = MIN_QUOTA_BYTES;
        let with = |quota| ShareSettings { quota_bytes: quota, ..ShareSettings::default() };
        assert_eq!(with(None).check_against(u64::MAX), Ok(()));
        assert_eq!(with(Some(gib - 1)).check(), Err(InvalidSettings::QuotaTooSmall));
        assert_eq!(with(Some(2 * gib)).check_against(gib), Ok(()));
        assert_eq!(
            with(Some(2 * gib)).check_against(3 * gib),
            Err(InvalidSettings::QuotaBelowUsage { used: 3 * gib })
        );
        assert_eq!(ShareSettings::default().compression, Compression::Lz4);
        assert_eq!(RecordSize::from_bytes(1 << 20), Some(RecordSize::M1));
        assert_eq!(Compression::parse("gzip-9"), None);
    }

    #[test]
    fn a_denial_wins_then_the_own_rule_then_the_broadest_group() {
        let rule = |subject, permission| AccessRule { subject, permission };
        let (alice, family, guests) = (1, 10, 20);
        let family_writes = rule(Subject::Group(family), Permission::ReadWrite);
        let guests_read = rule(Subject::Group(guests), Permission::Read);

        let rules = [family_writes.clone(), guests_read.clone()];
        assert_eq!(effective(&rules, alice, &[family, guests]), Some(Access::ReadWrite));
        assert_eq!(effective(&rules, alice, &[guests]), Some(Access::Read));
        assert_eq!(effective(&rules, alice, &[]), None);

        let own_read = [family_writes.clone(), rule(Subject::Account(alice), Permission::Read)];
        assert_eq!(
            effective(&own_read, alice, &[family]),
            Some(Access::Read),
            "the own rule is more specific"
        );

        let guests_denied = [
            rule(Subject::Account(alice), Permission::ReadWrite),
            rule(Subject::Group(guests), Permission::Deny),
        ];
        assert_eq!(
            effective(&guests_denied, alice, &[guests]),
            None,
            "a group's denial wins over the own rule"
        );
        assert_eq!(effective(&guests_denied, alice, &[]), Some(Access::ReadWrite));

        let alice_denied = [family_writes, rule(Subject::Account(alice), Permission::Deny)];
        assert_eq!(effective(&alice_denied, alice, &[family]), None);
    }

    #[test]
    fn share_names() {
        for good in ["photos", "a", "home-videos", "x_1"] {
            assert!(ShareName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "Photos", "1x", "a b", "a/b", ".recycle", &"x".repeat(33)] {
            assert!(ShareName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(matches!(ShareName::parse("homes"), Err(InvalidShareName::Reserved(_))));
        assert!(matches!(ShareName::parse("home"), Err(InvalidShareName::Reserved(_))));
    }

    #[test]
    fn nfs_clients_are_addresses_networks_or_host_names() {
        assert_eq!(nfs_host(" 192.168.1.0/24 ").unwrap(), "192.168.1.0/24");
        assert_eq!(nfs_host("10.0.0.5/32").unwrap(), "10.0.0.5");
        assert_eq!(nfs_host("NAS.local").unwrap(), "nas.local");
        assert_eq!(nfs_host("*.example.com").unwrap(), "*.example.com");
        assert_eq!(nfs_host("host?.lan").unwrap(), "host?.lan");
        assert_eq!(nfs_host("*").unwrap(), "*");
        let long = format!("{}.lan", "a".repeat(250));
        for bad in [
            "",
            "nas local",
            "nas(rw)",
            "a,b",
            "-nas.lan",
            "nas-.lan",
            "nas..lan",
            ".lan",
            "10.0.0.0/33",
            "@group",
            "/volume1",
            "1.2.3.4.5",
            long.as_str(),
        ] {
            assert!(nfs_host(bad).is_err(), "{bad:?}");
        }
        let client = NfsClient::new("10.0.0.5", true);
        assert_eq!((client.asynchronous, client.insecure), (false, false));
    }

    #[test]
    fn networks_are_normalized() {
        assert_eq!(network(" 192.168.1.0/24 ").unwrap(), "192.168.1.0/24");
        assert_eq!(network("10.0.0.5/32").unwrap(), "10.0.0.5");
        assert_eq!(network("fd00::/64").unwrap(), "fd00::/64");
        for bad in ["", "*", "nas.local", "10.0.0.0/33", "10.0.0.1(rw)"] {
            assert!(network(bad).is_err(), "{bad:?}");
        }
    }
}
