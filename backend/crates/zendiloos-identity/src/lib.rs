//! Bounded context *Identity*: who may use Zendiloos and what they may do.
//!
//! One account per person, valid for the web interface, SMB and the file
//! manager; roles that grant permissions; login sessions; the one-time
//! initial setup; and the audit log of all of it.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
