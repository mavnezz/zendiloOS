//! Use cases of the Storage context and the ports they need.

mod error;
pub mod ports;
pub mod queries;

use std::sync::Arc;

pub use error::StorageError;
use ports::{ArraySource, BlockDeviceSource, SmartSource};

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct StoragePorts {
    pub devices: Arc<dyn BlockDeviceSource>,
    pub smart: Arc<dyn SmartSource>,
    pub arrays: Arc<dyn ArraySource>,
}

/// All use cases of this context.
#[derive(Clone)]
pub struct StorageUseCases {
    pub list_disks: queries::ListDisks,
    pub get_smart_report: queries::GetSmartReport,
    pub list_arrays: queries::ListArrays,
    pub list_volumes: queries::ListVolumes,
}

impl StorageUseCases {
    pub fn new(ports: StoragePorts) -> Self {
        Self {
            list_disks: queries::ListDisks::new(ports.devices.clone()),
            get_smart_report: queries::GetSmartReport::new(ports.devices.clone(), ports.smart),
            list_arrays: queries::ListArrays::new(ports.arrays),
            list_volumes: queries::ListVolumes::new(ports.devices),
        }
    }
}
