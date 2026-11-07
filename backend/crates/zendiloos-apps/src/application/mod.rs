//! Use cases of the Apps context and the ports they need.

mod catalogs;
mod docker;
mod installs;
pub mod ports;
mod templates;

pub use catalogs::{AppsError, Catalogs, ListedApp};
pub use docker::{Docker, DockerSetup};
pub use installs::{InstallPorts, InstalledView, Installs};
pub use templates::{AppProjectView, FillRequest, TemplatePorts, Templates};
