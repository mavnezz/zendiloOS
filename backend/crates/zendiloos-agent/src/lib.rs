//! The privileged agent: performs the operations of
//! [`zendiloos_agent_proto::Request`] for exactly one client, the
//! unprivileged `zendiloosd`.
//!
//! "The agent executes, zendiloosd interprets": tool output goes back raw;
//! parsing happens unprivileged, so parser bugs never run as root.

pub mod config;
mod operations;
pub mod passwd;
mod server;

pub use operations::Operations;
pub use server::{Server, serve};
