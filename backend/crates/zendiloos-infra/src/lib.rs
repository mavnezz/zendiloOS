//! Secondary (driven) adapters: the implementations behind the ports that
//! the bounded contexts declare.
//!
//! - [`agent`]: gateways to the privileged agent.
//! - [`host`]: unprivileged reads from the running Linux host.
//! - [`identity`]: password hashing, session tokens, the setup code file.
//! - [`sqlite`]: the database, its migrations, backups and repositories.

pub mod agent;
mod blocking;
mod clock;
pub mod host;
pub mod identity;
pub mod sqlite;

pub use clock::SystemClock;
