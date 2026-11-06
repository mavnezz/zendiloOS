//! The privileged agent: performs the operations of
//! [`zendiloos_agent_proto::Request`] for exactly one client, the
//! unprivileged `zendiloosd`.
//!
//! "The agent executes, zendiloosd interprets": tool output goes back raw;
//! parsing happens unprivileged, so parser bugs never run as root.

mod accounts;
mod apps;
pub mod config;
mod console;
mod files;
mod network;
mod operations;
pub mod passwd;
mod pools;
mod server;
mod sharing;

pub use accounts::AccountFiles;
pub use network::NetworkPaths;
pub use operations::{Operations, Paths};
pub use pools::{Erasable, ErasePolicy, HostPaths};
pub use server::{Server, serve};
