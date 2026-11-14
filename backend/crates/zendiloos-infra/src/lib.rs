//! Secondary (driven) adapters: the implementations behind the ports that
//! the bounded contexts declare.
//!
//! - [`agent`]: gateways to the privileged agent.
//! - [`host`]: unprivileged reads from the running Linux host.
//! - [`identity`]: password hashing, session tokens, the setup code file.
//! - [`sqlite`]: the database, its migrations, backups and repositories.
//! - `storage`: parsers for the output of storage tools.

pub mod agent;
pub mod apps;
pub mod backup;
pub mod backup_sign_in;
mod blocking;
mod clock;
pub mod containers;
pub mod enclosure;
pub mod firewall;
pub mod host;
mod https;
pub mod identity;
pub mod notify;
pub mod recovery;
pub mod sharing;
pub mod sqlite;
mod storage;
#[cfg(test)]
mod test_https;
pub mod updates;
pub mod ups;
pub mod vms;

pub use clock::SystemClock;
