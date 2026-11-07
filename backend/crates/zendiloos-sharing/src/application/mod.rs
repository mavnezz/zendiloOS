//! Use cases of the Sharing context and the ports they need.

mod files;
mod homes;
pub mod ports;
mod shares;
mod snapshots;
mod subjects;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AuditLog, Clock};
use zendiloos_notify::Notifier;

pub use files::{
    FileVolume, FilesError, FilesUseCases, Transfer, Upload, UploadTicket, UsableShare, VolumeFolder,
};
pub use homes::HomesSettings;
use ports::{Directory, FileHost, ShareHost, ShareProjects, ShareRepository, Volumes};
pub use shares::{NewShareInput, ShareChanges, SharingError, SharingUseCases};
pub use subjects::SubjectAccess;

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct SharingPorts {
    pub shares: Arc<dyn ShareRepository>,
    pub directory: Arc<dyn Directory>,
    pub volumes: Arc<dyn Volumes>,
    pub projects: Arc<dyn ShareProjects>,
    pub host: Arc<dyn ShareHost>,
    pub files: Arc<dyn FileHost>,
    pub jobs: Jobs,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    /// For what goes wrong without anybody watching, e.g. a scheduled
    /// snapshot.
    pub notifier: Notifier,
}
