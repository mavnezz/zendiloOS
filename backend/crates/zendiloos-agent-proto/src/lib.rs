//! Wire protocol between `zendiloosd` (unprivileged) and `zendiloos-agent`
//! (root). This is the privilege boundary, so the protocol is narrow:
//!
//! - one request per connection, one JSON object per line, size-limited;
//! - a closed set of operations ([`Request`]) with typed arguments that are
//!   validated while deserializing, so an invalid value never reaches the
//!   code that runs a program;
//! - a protocol version in every request; the agent refuses others.
//!
//! See docs/architecture.md, section 3.2.

mod client;
mod framing;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use client::{AgentTransport, TransportError, UnixTransport};
pub use framing::{FrameError, read_message, write_message};

/// Agent and API come from the same release; any difference is refused.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest request the agent reads.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Largest response the client reads.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// What travels from `zendiloosd` to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    #[serde(default)]
    pub request_id: Option<RequestId>,
    pub request: Request,
}

impl Envelope {
    pub fn new(request: Request, request_id: Option<RequestId>) -> Self {
        Self { version: PROTOCOL_VERSION, request_id, request }
    }
}

/// Every operation the agent performs. Nothing else can be asked of it.
///
/// Variants without arguments are written `Reboot {}` rather than `Reboot`:
/// serde ignores unknown fields next to the tag of a unit variant, and the
/// agent must refuse `{"op": "reboot", "force": true}` instead of quietly
/// rebooting without the option the caller believes it set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Version handshake; changes nothing.
    Hello {},
    Reboot {},
    PowerOff {},
    /// `lsblk` output for all block devices, columns [`LSBLK_COLUMNS`].
    ListBlockDevices {},
    /// `smartctl` output for one whole disk. Sleeping disks are not woken.
    SmartData {
        device: DiskName,
    },
    /// The kernel's software RAID status, `/proc/mdstat`.
    ArrayStatus {},
    /// Creates a Linux user in group `zendiloos-users`: UID from 2000, no
    /// login shell, no home directory, locked Linux password. `conflict` if
    /// the name is taken.
    CreateUser {
        name: UserName,
    },
    /// Deletes a managed user and its Samba entry. Its files stay.
    DeleteUser {
        name: UserName,
    },
    /// Sets the Samba password of a managed user, adding the Samba entry if
    /// needed. The password travels on the program's stdin.
    SetSmbPassword {
        name: UserName,
        password: Password,
    },
    /// Enables or disables the Samba entry of a managed user.
    SetUserEnabled {
        name: UserName,
        enabled: bool,
    },
    /// Creates a Linux group with a GID from 2000. `conflict` if the name is
    /// taken.
    CreateGroup {
        name: GroupName,
    },
    DeleteGroup {
        name: GroupName,
    },
    /// Replaces the members of a managed group; all must be managed users.
    SetGroupMembers {
        group: GroupName,
        members: Vec<UserName>,
    },
}

/// The `lsblk` columns of [`Request::ListBlockDevices`]; the parser in
/// zendiloosd relies on them.
pub const LSBLK_COLUMNS: &str = "NAME,KNAME,PATH,TYPE,SIZE,MODEL,SERIAL,WWN,TRAN,ROTA,RM,RO,\
FSTYPE,LABEL,UUID,FSSIZE,FSUSED,FSAVAIL,MOUNTPOINTS,PKNAME,ID-LINK";

impl Request {
    /// Name for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Request::Hello {} => "hello",
            Request::Reboot {} => "reboot",
            Request::PowerOff {} => "power_off",
            Request::ListBlockDevices {} => "list_block_devices",
            Request::SmartData { .. } => "smart_data",
            Request::ArrayStatus {} => "array_status",
            Request::CreateUser { .. } => "create_user",
            Request::DeleteUser { .. } => "delete_user",
            Request::SetSmbPassword { .. } => "set_smb_password",
            Request::SetUserEnabled { .. } => "set_user_enabled",
            Request::CreateGroup { .. } => "create_group",
            Request::DeleteGroup { .. } => "delete_group",
            Request::SetGroupMembers { .. } => "set_group_members",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Hello {
        protocol_version: u32,
        agent_version: String,
    },
    /// The operation was carried out; it has no output.
    Done,
    BlockDevices {
        lsblk_json: String,
    },
    Smart {
        /// smartctl's exit status: a bit mask, not a plain success flag.
        exit_status: i32,
        smartctl_json: String,
    },
    ArrayStatus {
        /// Empty if the kernel has no software RAID support loaded.
        mdstat: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request could not be read or failed validation.
    InvalidRequest,
    VersionMismatch,
    /// The caller is not the account the agent serves, or the request
    /// concerns a user or group zendiloos does not manage.
    Forbidden,
    /// The name is already taken.
    Conflict,
    /// The operation ran but failed.
    Failed,
}

/// Kernel name of a whole disk: `sda`, `vdb`, `nvme0n1`, `mmcblk0`, …
///
/// Partitions and anything else (paths, `..`, option-like strings) are
/// refused while deserializing. This is the only form in which a device
/// reaches a command line in the agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DiskName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not the kernel name of a whole disk")]
pub struct InvalidDiskName;

impl DiskName {
    pub fn parse(name: &str) -> Result<Self, InvalidDiskName> {
        let letters = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_lowercase());
        let digits = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
        let valid = if let Some(rest) = name.strip_prefix("nvme") {
            // nvme<controller>n<namespace>, but not its partitions (…p1).
            rest.split_once('n').is_some_and(|(controller, ns)| digits(controller) && digits(ns))
        } else if let Some(rest) = name.strip_prefix("mmcblk") {
            digits(rest)
        } else {
            ["sd", "vd", "xvd", "hd"].iter().any(|prefix| name.strip_prefix(prefix).is_some_and(letters))
        };
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidDiskName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DiskName {
    type Error = InvalidDiskName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DiskName> for String {
    fn from(name: DiskName) -> Self {
        name.0
    }
}

/// Name of a Linux user or group zendiloos manages: a lowercase letter, then
/// up to 31 lowercase letters, digits, `_` or `-`. Valid unchanged for the
/// shadow tools and Samba, and never mistaken for a command-line option.
fn is_linux_name(name: &str) -> bool {
    name.len() <= 32
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

macro_rules! linux_name {
    ($(#[$doc:meta])* $name:ident, $error:ident, $message:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
        #[error($message)]
        pub struct $error;

        impl $name {
            pub fn parse(name: &str) -> Result<Self, $error> {
                if is_linux_name(name) { Ok(Self(name.to_owned())) } else { Err($error) }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(name: $name) -> Self {
                name.0
            }
        }
    };
}

linux_name!(
    /// A Linux user name; see `is_linux_name`.
    UserName,
    InvalidUserName,
    "not a valid user name"
);
linux_name!(
    /// A Linux group name; see `is_linux_name`.
    GroupName,
    InvalidGroupName,
    "not a valid group name"
);

/// A password on its way to Samba. Never printed. At most 1024 bytes and
/// without NUL or line breaks, because it is passed as a line on stdin.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Password(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a password that can be passed on")]
pub struct InvalidPassword;

impl Password {
    pub const MAX_BYTES: usize = 1024;

    pub fn new(password: impl Into<String>) -> Result<Self, InvalidPassword> {
        let password = password.into();
        let valid = !password.is_empty()
            && password.len() <= Self::MAX_BYTES
            && !password.contains(['\0', '\n', '\r']);
        if valid { Ok(Self(password)) } else { Err(InvalidPassword) }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

impl TryFrom<String> for Password {
    type Error = InvalidPassword;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Password> for String {
    fn from(password: Password) -> Self {
        password.0
    }
}

/// Correlates a request across the API's audit log and the agent's journal.
/// Up to 64 ASCII letters, digits, `-` and `_`, so it is safe to log as root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RequestId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid request ID")]
pub struct InvalidRequestId;

impl RequestId {
    pub fn parse(value: &str) -> Result<Self, InvalidRequestId> {
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidRequestId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = InvalidRequestId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RequestId> for String {
    fn from(id: RequestId) -> Self {
        id.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn has_a_stable_wire_format() {
        let envelope = Envelope::new(Request::PowerOff {}, Some(RequestId::parse("4f0c-91").unwrap()));
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({ "version": 1, "request_id": "4f0c-91", "request": { "op": "power_off" } })
        );
        assert_eq!(
            serde_json::to_value(Response::Error { code: ErrorCode::Forbidden, message: "no".into() })
                .unwrap(),
            json!({ "status": "error", "code": "forbidden", "message": "no" })
        );
    }

    #[test]
    fn rejects_anything_outside_the_protocol() {
        let parse = |value: serde_json::Value| serde_json::from_value::<Envelope>(value);
        assert!(parse(json!({ "version": 1, "request": { "op": "hello" } })).is_ok());
        assert!(parse(json!({ "version": 1, "request": { "op": "run", "command": "sh" } })).is_err());
        assert!(parse(json!({ "version": 1, "request": { "op": "reboot", "force": true } })).is_err());
        assert!(parse(json!({ "version": 1, "request": { "op": "hello" }, "extra": 1 })).is_err());
        assert!(parse(json!({ "version": 1, "request_id": "a\nb", "request": { "op": "hello" } })).is_err());
    }

    #[test]
    fn disk_names_are_whole_disks_only() {
        for good in ["sda", "sdab", "vdb", "xvda", "nvme0n1", "nvme12n3", "mmcblk0"] {
            assert!(DiskName::parse(good).is_ok(), "{good}");
        }
        for bad in
            ["sda1", "nvme0n1p1", "nvme0", "/dev/sda", "../sda", "-sda", "sd", "md0", "loop0", "SDA", ""]
        {
            assert_eq!(DiskName::parse(bad), Err(InvalidDiskName), "{bad:?}");
        }
        let smuggled = json!({ "version": 1, "request": { "op": "smart_data", "device": "sda;reboot" } });
        assert!(serde_json::from_value::<Envelope>(smuggled).is_err());
    }

    #[test]
    fn user_and_group_names_cannot_become_options_or_paths() {
        for good in ["alice", "a", "bob-smith", "carol_2", &"x".repeat(32)] {
            assert!(UserName::parse(good).is_ok(), "{good}");
            assert!(GroupName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-alice", "--help", "Alice", "1alice", "al ice", "a/b", "a,b", "a:b", &"x".repeat(33)]
        {
            assert_eq!(UserName::parse(bad), Err(InvalidUserName), "{bad:?}");
            assert_eq!(GroupName::parse(bad), Err(InvalidGroupName), "{bad:?}");
        }
        let smuggled = json!({ "version": 1, "request": { "op": "set_group_members", "group": "family",
            "members": ["alice", "bob,root"] } });
        assert!(serde_json::from_value::<Envelope>(smuggled).is_err());
    }

    #[test]
    fn passwords_are_single_lines_and_never_printed() {
        let password = Password::new("correct horse battery").unwrap();
        assert_eq!(format!("{password:?}"), "Password(***)");
        let request = Request::SetSmbPassword { name: UserName::parse("alice").unwrap(), password };
        assert!(!format!("{request:?}").contains("horse"));
        for bad in ["", "two\nlines", "carriage\rreturn", "nul\0", &"x".repeat(1025)] {
            assert_eq!(Password::new(bad), Err(InvalidPassword), "{bad:?}");
        }
    }

    #[test]
    fn request_ids_are_short_and_plain() {
        assert!(RequestId::parse("d3b07384-d113-4ec6-a5b3-1f0d0a1b2c3d").is_ok());
        for bad in ["", "with space", "new\nline", "ünï", &"x".repeat(65)] {
            assert_eq!(RequestId::parse(bad), Err(InvalidRequestId), "{bad:?}");
        }
    }
}
