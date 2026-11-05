//! Shared kernel of Zendiloos.
//!
//! Types that every bounded context and every adapter agrees on. Nothing in
//! here performs I/O.

mod audit;
mod error;
mod request;
mod secret;
mod time;

pub use audit::{Actor, AuditEntry, AuditLog, Outcome};
pub use error::PortError;
pub use request::RequestContext;
pub use secret::{Secret, constant_time_eq};
pub use time::{Clock, Timestamp, UtcTime};
