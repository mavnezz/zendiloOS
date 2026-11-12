//! Bounded context Recovery: zendiloOS's configuration (its database, the
//! files it wrote that the database does not say again, the accounts'
//! Linux users) as one encrypted archive, downloaded by hand or written on
//! a schedule into a folder of a share or a removable drive, as many kept
//! as asked; and restored from one, on this installation or a fresh one
//! (docs/architecture.md, 4.15).

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
