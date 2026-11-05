//! Bounded context *Network*: how the machine is connected. For now it only
//! shows interfaces, addresses, default routes and name servers; changing
//! them follows in milestone M6 (docs/roadmap.md).

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
