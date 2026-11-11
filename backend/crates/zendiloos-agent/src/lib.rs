//! The privileged agent: performs the operations of
//! [`zendiloos_agent_proto::Request`] for exactly one client, the
//! unprivileged `zendiloosd`.
//!
//! "The agent executes, zendiloosd interprets": tool output goes back raw;
//! parsing happens unprivileged, so parser bugs never run as root.

/// The version of zendiloOS this agent belongs to: the package's, which its
/// build takes from `debian/changelog` (`ZENDILOOS_VERSION`), or the crates'
/// in a build without one; as `zendiloos_kernel::VERSION`, which the agent
/// does not depend on.
pub const VERSION: &str = match option_env!("ZENDILOOS_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

mod accounts;
mod acl;
mod acme;
mod app_shares;
mod apps;
pub mod config;
mod console;
mod docker;
pub mod docker_bridges;
mod durable;
mod emmc;
mod enclosure;
mod files;
mod firewall;
mod internal_networks;
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
mod vm_iso;
mod vm_upload;
mod vms;
mod volume_archive;
mod volumes;
mod waiting;
mod web;
mod web_server;

pub use accounts::AccountFiles;
pub use acme::AcmePaths;
pub use app_shares::{pin_project, pin_shares, unpin_project, unpin_shares};
pub use files::lower_file_overrides;
pub use firewall::FirewallPaths;
pub use network::NetworkPaths;
pub use operations::{Operations, Paths};
pub use packages::run_packages;
pub use pools::{Erasable, ErasePolicy, HostPaths};
pub use removable::power_off_drive;
pub use server::{Server, serve};
pub use ssh::SshPaths;
pub use time::TimePaths;
pub use vm_iso::run_iso_job;
pub use vm_upload::run_upload_job;
pub use vms::VmPaths;
pub use volume_archive::{VolumeJob, run_volume_job};
pub use web::WebPaths;
pub use web_server::WebServerPaths;
