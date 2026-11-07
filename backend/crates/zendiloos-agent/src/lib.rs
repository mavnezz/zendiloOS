//! The privileged agent: performs the operations of
//! [`zendiloos_agent_proto::Request`] for exactly one client, the
//! unprivileged `zendiloosd`.
//!
//! "The agent executes, zendiloosd interprets": tool output goes back raw;
//! parsing happens unprivileged, so parser bugs never run as root.

mod accounts;
mod acl;
mod app_shares;
mod apps;
pub mod config;
mod console;
mod docker;
mod durable;
mod files;
mod network;
mod operations;
mod packages;
pub mod passwd;
mod pools;
mod progress;
mod projects;
mod removable;
mod server;
mod sharing;
mod ssh;
mod time;
mod waiting;
mod web;

pub use accounts::AccountFiles;
pub use app_shares::{pin_project, pin_shares, unpin_project, unpin_shares};
pub use network::NetworkPaths;
pub use operations::{Operations, Paths};
pub use packages::run_packages;
pub use pools::{Erasable, ErasePolicy, HostPaths};
pub use removable::power_off_drive;
pub use server::{Server, serve};
pub use ssh::SshPaths;
pub use time::TimePaths;
pub use web::WebPaths;
