//! Use cases of the Apps context and the ports they need.

mod catalogs;
pub mod ports;

pub use catalogs::{AppsError, Catalogs, ListedApp};
