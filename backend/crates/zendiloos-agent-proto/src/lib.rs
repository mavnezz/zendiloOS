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
}

impl Request {
    /// Name for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Request::Hello {} => "hello",
            Request::Reboot {} => "reboot",
            Request::PowerOff {} => "power_off",
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
    /// The caller is not the account the agent serves.
    Forbidden,
    /// The operation ran but failed.
    Failed,
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
    fn request_ids_are_short_and_plain() {
        assert!(RequestId::parse("d3b07384-d113-4ec6-a5b3-1f0d0a1b2c3d").is_ok());
        for bad in ["", "with space", "new\nline", "ünï", &"x".repeat(65)] {
            assert_eq!(RequestId::parse(bad), Err(InvalidRequestId), "{bad:?}");
        }
    }
}
