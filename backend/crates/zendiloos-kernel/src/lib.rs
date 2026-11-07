//! Shared kernel of Zendiloos.
//!
//! Types that every bounded context and every adapter agrees on. Nothing in
//! here performs I/O.

mod audit;
mod error;
mod request;
mod secret;
mod time;
mod work;

pub use audit::{Actor, AuditEntry, AuditLog, Outcome};
pub use error::PortError;
pub use request::RequestContext;
pub use secret::{Secret, constant_time_eq};
pub use time::{Clock, Timestamp, UtcTime};
pub use work::{NoWork, RunningWork};

/// The version of zendiloOS that runs: the package's, which its build takes
/// from `debian/changelog` (`ZENDILOOS_VERSION`), or the crates' in a build
/// without one.
pub const VERSION: &str = match option_env!("ZENDILOOS_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
