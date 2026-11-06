//! Use cases of the Apps context and the ports they need.

mod catalogs;
mod docker;
mod installs;
pub mod ports;

pub use catalogs::{AppsError, Catalogs, ListedApp};
pub use docker::{Docker, DockerSetup};
pub use installs::{InstallPorts, InstallRequest, InstalledView, Installs};
